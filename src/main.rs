// Entry point: parse CLI args, fetch player HTML, run WASM decryption, dispatch to downloader.

mod cli;
mod downloader;
mod html_parser;
mod utils;
mod wasm_decrypt;

use std::str::FromStr;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use clap::Parser;
use colored::*;
use reqwest::header::REFERER;
use tokio::fs;
use url::Url;
use wasmtime::Engine;

use cli::Args;
use downloader::{download_m3u8_segments, download_video};
use html_parser::{derive_code, derive_fragment, extract_encrypted_url};
use utils::http::build_client;
use utils::live_log::LiveLog;
use wasm_decrypt::WasmDecrypt;

use crate::downloader::sniff_m3u8;
use crate::utils::ffmpeg::{KnownFormat, prepare_convert};
use crate::utils::live_log::LogStyle;
use crate::utils::stat::{CleanupGuard, size_display};

macro_rules! is_auto_format {
    ($format:expr) => {
        $format.is_empty() || $format.eq_ignore_ascii_case("auto")
    };
}

#[tokio::main]
async fn main() -> Result<()> {
    // Ensure cleanup on unexpected exit
    let _cleanup_guard = CleanupGuard::new();

    // Parse CLI arguments
    let args = Args::parse();

    // Init ffmpeg path
    match utils::ffmpeg::set_ffmpeg_binary(&args.ffmpeg_bin) {
        Ok(_) => {}
        Err(e) => {
            exit_error!("{}", e);
        }
    }

    // Resolve the cache directory: build a system temp dir path if not specified, or use the provided path.
    let cache_dir = if let Some(dir) = args.cache_dir {
        dir
    } else {
        let temp_dir: std::path::PathBuf = std::env::temp_dir().join(format!(
            "downfsdm_cache_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
        ));
        let _ = fs::remove_dir_all(&temp_dir).await;
        CleanupGuard::register_path(&temp_dir);
        temp_dir
    };

    fs::create_dir_all(&cache_dir)
        .await
        .with_context(|| format!("cannot create cache dir {:?}", cache_dir))?;

    // Resolve cache_dir to absolute path; output may not exist yet so resolve via cwd.
    let cache_dir_abs = fs::canonicalize(&cache_dir)
        .await
        .unwrap_or_else(|_| cache_dir.clone());
    let output_abs = std::env::current_dir()
        .map(|cwd| cwd.join(&args.output))
        .unwrap_or_else(|_| args.output.clone());

    // Print startup banner with key parameters
    println!(
        "{} {}",
        "downfsdm".cyan().bold(),
        env!("CARGO_PKG_VERSION").dimmed()
    );
    println!();

    // Display the resolved parameters
    println!("{}", "Parameters".blue());
    println!("  {}  {}", "player url".magenta(), args.player_url);
    println!(
        "  {}  {}",
        "output    ".magenta(),
        output_abs.display().to_string()
    );
    println!(
        "  {}  {}",
        "cache dir ".magenta(),
        cache_dir_abs.display().to_string()
    );

    // try infer types by extension
    let mut inferred_format = if is_auto_format!(args.format) {
        args.output
            .extension()
            .map(|ext| ext.to_string_lossy().to_ascii_lowercase())
            .and_then(|ext| KnownFormat::from_str(&ext).ok())
    } else {
        KnownFormat::from_str(args.format.to_ascii_lowercase().as_str()).ok()
    };

    println!(
        "  {}  {}",
        "format    ".magenta(),
        match inferred_format {
            Some(fmt) => format!("{} {}", fmt, "(auto inferred)".dimmed()).normal(),
            None => "auto".normal(),
        }
    );
    println!();

    let client = build_client()?;

    // Fetch the player HTML page
    println!("{}", "Fetching player page".blue());
    println!("  url  {}", args.player_url.dimmed());
    let html = client
        .get(&args.player_url)
        .header(REFERER, "https://www.fsdm02.com/")
        .send()
        .await
        .context("Failed to fetch player page")?
        .bytes()
        .await?;
    println!("  received  {}", size_display(html.len() as u64).green());
    let html = String::from_utf8_lossy(&html);
    println!();

    // Extract per-session parameters from the page HTML
    println!("{}", "Extracting session parameters".blue());
    let encrypted_url = extract_encrypted_url(&html)?;
    let code = derive_code(&html)?;
    let fragment = derive_fragment(&html)?;
    let config_secret = "3G7Fh9Dp6R2QsE8w";
    println!("  code     {}", code.cyan());
    println!("  fragment {}", fragment.cyan());

    // Derive the base URL for fetching player assets (e.g. decrypt.wasm)
    let parsed = Url::parse(&args.player_url).context("Invalid player URL")?;
    let base_url = format!(
        "{}://{}{}/",
        parsed.scheme(),
        parsed.host_str().unwrap_or_default(),
        parsed.path().rsplit_once('/').map(|(p, _)| p).unwrap_or(""),
    );
    println!();

    // Fetch and instantiate the WASM decryption module
    let wasm_url = format!("{}js/decrypt.wasm", base_url);
    println!("{}", "Fetching decrypt.wasm".blue());
    println!("  wasm url  {}", wasm_url.dimmed());
    let wasm_bytes = client
        .get(&wasm_url)
        .send()
        .await
        .context("Failed to fetch decrypt.wasm")?
        .bytes()
        .await?;
    println!(
        "  {}, instantiating module",
        size_display(wasm_bytes.len() as u64).cyan()
    );

    let engine = Engine::default();
    let mut wasm =
        WasmDecrypt::new(&engine, &wasm_bytes).context("Failed to instantiate decrypt.wasm")?;
    println!();

    // Verify the per-session HMAC before attempting decryption
    println!("{}", "Verifying HMAC signature".blue());
    println!("  code     {}", code.cyan());
    println!("  fragment {}", fragment.cyan());
    if !wasm.hmac_verify(&code, config_secret, &fragment)? {
        println!("  HMAC  {}", "FAILED".red());
        exit_error!("  HMAC verification failed — page may have changed or expired");
    }
    println!("  HMAC  {}", "OK".green());
    println!();

    // Decrypt the obfuscated video URL
    let now_ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as i32;
    println!("{}", "Decrypting video URL".blue());
    println!("  timestamp {}", now_ts.to_string().cyan());
    let video_url = wasm.decrypt(&encrypted_url, &code, config_secret, now_ts)?;
    if video_url.is_empty() {
        exit_error!(
            "decryption returned empty — timestamp may be expired, please retry immediately"
        );
    }
    println!("  video url  {}", video_url.dimmed());

    // Download segments and remux to local TS files
    let is_m3u8 = sniff_m3u8(&client, &video_url, &args.player_url)
        .await
        .unwrap_or(video_url.contains(".m3u8") || video_url.contains("m3u8"));
    let upstream_fmt = if is_m3u8 {
        Some(KnownFormat::M3u8)
    } else {
        let mut fmt_found = None;
        for fmt in KnownFormat::exts() {
            if video_url.contains(&format!(".{}", fmt)) {
                fmt_found = KnownFormat::from_str(fmt).ok();
                break;
            }
        }
        fmt_found
    };
    let stream_type =
        upstream_fmt.map_or("unknown".to_string(), |f| f.as_stream_type().to_string());
    println!("  stream type  {}", stream_type.to_string().cyan().bold());
    println!();

    if inferred_format.is_none() {
        inferred_format = upstream_fmt;
    }

    // inferred_format is Some for sure if skip_convert is false
    let skip_convert = match &inferred_format {
        Some(fmt) => {
            if let Some(upstream_fmt) = &upstream_fmt {
                fmt == upstream_fmt
            } else {
                // we don't know the upstream format, let ffmpeg try to infer from content
                false
            }
        }
        None => true,
    };

    let down_path_abs = if skip_convert {
        output_abs.clone()
    } else {
        output_abs.with_extension(upstream_fmt.as_ref().map_or("tmp", |f| f.as_ext()))
    };

    let mut clean_paths = if down_path_abs != output_abs {
        vec![down_path_abs.clone()]
    } else {
        vec![]
    };

    if is_m3u8 {
        let paths = download_m3u8_segments(
            &client,
            &video_url,
            &args.player_url,
            &cache_dir,
            &down_path_abs,
        )
        .await?;
        clean_paths.extend(paths);
    } else {
        download_video(
            &client,
            &video_url,
            &args.player_url,
            &down_path_abs,
            // if this step is not final step, use cached file if exists
            if !skip_convert {
                Some(&cache_dir)
            } else {
                None
            },
        )
        .await?;
    }
    println!();

    // Convert to final output format if needed
    if !skip_convert {
        assert!(
            inferred_format.is_some(),
            "Output format must be inferred if conversion is needed"
        );
        let inferred_format = inferred_format.unwrap();
        // convert to m3u8 is not supported
        if inferred_format == KnownFormat::M3u8 {
            exit_error!("conversion to m3u8 is not supported");
        }

        let live = Arc::new(Mutex::new(
            LiveLog::new_with_fit(0).with_log_style(LogStyle::PartialDimmed),
        ));

        println!("{}", "Converting to final format".blue().bold());
        println!("  input    {}", down_path_abs.display().to_string().cyan());
        println!("  output   {}", output_abs.display().to_string().cyan());
        println!(
            "  format   {} {}",
            inferred_format.to_string().cyan(),
            format!("[{}]", inferred_format.as_ffmpeg_format()).yellow()
        );

        let (cmd, argv) = prepare_convert(
            &down_path_abs,
            &output_abs,
            &inferred_format,
            Some(&args.ffmpeg_args),
        )?;

        println!(
            "  execute  {} {}",
            cmd.cyan(),
            argv.iter()
                .map(|a| a.to_string())
                .collect::<Vec<_>>()
                .join(" ")
                .cyan()
                .dimmed()
        );

        let instant = std::time::Instant::now();
        let ffmpeg_format = format!("({})", inferred_format.as_ffmpeg_format());
        let make_status = move || {
            format!(
                "{}",
                format!(
                    "  [{:.1}s] FFmpeg converting {ffmpeg_format}",
                    instant.elapsed().as_secs_f64()
                )
                .blue()
            )
        };

        live.lock().unwrap().updateln(&make_status(), None);

        let status_task = tokio::spawn({
            let make_status = make_status.clone();
            let live = Arc::clone(&live);
            async move {
                loop {
                    live.lock().unwrap().update(&make_status(), None);
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            }
        });

        convert_video_to!(
            cmd,
            argv,
            on_data = |data: &str| {
                live.lock().unwrap().update(&make_status(), Some(data));
            },
            auto_pty = true
        )?;

        status_task.abort();

        live.lock().unwrap().finish(&make_status());

        if args.cleanup {
            for path in clean_paths {
                let _ = fs::remove_file(path).await;
            }
        }

        println!();
    }

    // Output results
    let size_bytes = fs::metadata(&output_abs).await?.len();

    println!(
        "{}  {} {}",
        "Output written".green().bold(),
        output_abs.display().to_string().cyan(),
        format!("({})", size_display(size_bytes)).yellow(),
    );
    println!("  {}", format!("vlc \"{}\"", output_abs.display()).dimmed());
    println!("  {}", format!("mpv \"{}\"", output_abs.display()).dimmed());

    Ok(())
}
