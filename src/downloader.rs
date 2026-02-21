// HLS/MP4 segment downloader: fetch playlist, remux each segment to .ts, manage content-SHA1 disk cache.

use anyhow::Result;
use colored::*;
use reqwest::Client;
use reqwest::header::REFERER;
use sha1::{Digest, Sha1};
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::time::sleep;
use url::Url;

use crate::utils::http::{AdaptiveDelay, download_binary};
use crate::utils::live_log::{LiveLog, LogStyle};
use crate::utils::stat::size_display;
use crate::{exit_error, remux_to_ts};

/// Sniff mime type via headers, return m3u8 or not
pub async fn sniff_m3u8(client: &Client, url: &str, referer: &str) -> Result<bool> {
    let resp = client.head(url).header(REFERER, referer).send().await?;

    if !resp.status().is_success() {
        return Err(anyhow::anyhow!(
            "HEAD request returned non-success status: {}",
            resp.status()
        ));
    }

    if let Some(content_type) = resp.headers().get(reqwest::header::CONTENT_TYPE) {
        let ct_str = content_type.to_str().unwrap_or_default().to_lowercase();
        let is_m3u8 = vec![
            "application/vnd.apple.mpegurl",
            "application/x-mpegurl",
            "application/mpegurl",
            "m3u8",
        ]
        .iter()
        .any(|t| ct_str.contains(t));
        Ok(is_m3u8)
    } else {
        Ok(false)
    }
}

/// Resolve a segment URI (possibly relative) against the base m3u8 URL.
fn resolve_segment_url(m3u8_url: &str, line: &str) -> anyhow::Result<String> {
    if line.starts_with("http://") || line.starts_with("https://") {
        Ok(line.to_string())
    } else {
        Ok(Url::parse(m3u8_url)?.join(line)?.to_string())
    }
}

/// Compute the full 40-char hex SHA-1 of raw bytes, given a stream
fn stream_sha1_hex(mut stream: impl std::io::Read) -> anyhow::Result<String> {
    let mut hasher = Sha1::new();
    std::io::copy(&mut stream, &mut hasher)?;
    Ok(format!("{:x}", hasher.finalize()))
}

/// Scan `cache_dir` for any `seg_{idx}.{hash}.ts` file whose embedded hash
/// matches the actual SHA-1 of its content. The first verified hit is returned.
async fn find_valid_hash_cached(
    cache_dir: &Path,
    prefix: &str,
    suffix: &str,
) -> Option<std::path::PathBuf> {
    let mut rd = fs::read_dir(cache_dir).await.ok()?;
    while let Ok(Some(entry)) = rd.next_entry().await {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if !name_str.starts_with(&prefix) || !name_str.ends_with(&suffix) {
            continue;
        }
        // Filename is  seg_{idx}.{hash}.ts  — extract the hash segment.
        let hash_in_name = name_str
            .strip_prefix(&prefix)? // "{idx}.{hash}.ts"
            .strip_suffix(suffix)?; // "{idx}.{hash}"
        let path = entry.path();
        let hash = stream_sha1_hex(std::fs::File::open(&path).ok()?).ok()?;
        if hash == hash_in_name {
            return Some(path);
        }
        // Hash mismatch: file is corrupt or incomplete, ignore it.
    }
    None
}

// * HLS / m3u8

/// Fetch an m3u8 playlist, download every segment sequentially, remux each one
/// to a proper .ts file, and stream-write a rewritten m3u8 to `output` (via a .tmp rename).
/// * Returns a Vec of segment file paths (absolute).
pub async fn download_m3u8_segments(
    client: &Client,
    m3u8_url: &str,
    referer: &str,
    cache_dir: &Path,
    output: &Path,
) -> Result<Vec<PathBuf>> {
    println!("{}", "Loading m3u8 playlist".blue());
    println!("  m3u8 url  {}", m3u8_url.dimmed());

    let output_basename = output
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let (m3u8_abs_path, m3u8_cached) = if let Some(valid_m3u8) =
        find_valid_hash_cached(cache_dir, &format!("_{}.", output_basename), ".m3u8").await
    {
        (valid_m3u8, true)
    } else {
        // download
        let m3u8_temp_path = cache_dir.join(format!("_{}.m3u8.tmp", output_basename));
        download_binary(client, m3u8_url, &m3u8_temp_path, Some(referer), 3)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to download m3u8 playlist: {e}"))?;
        // rename to `{basename}.{sha1}.m3u8`
        let m3u8_hash = stream_sha1_hex(std::fs::File::open(&m3u8_temp_path)?)?;
        let m3u8_abs_path = cache_dir.join(format!("_{}.{}.m3u8", output_basename, m3u8_hash));
        fs::rename(&m3u8_temp_path, &m3u8_abs_path).await?;
        (m3u8_abs_path, false)
    };
    drop(output_basename);

    let total = {
        let file = std::fs::File::open(&m3u8_abs_path)?;
        let reader = std::io::BufReader::new(file);
        reader
            .lines()
            .filter_map(|l| l.ok())
            .filter(|l| !l.trim().is_empty() && !l.trim().starts_with('#'))
            .count()
    };

    if total == 0 {
        println!("  Loaded  {status}", status = "FAILED".red());
        exit_error!("  no segment URIs found in m3u8");
    }
    println!(
        "  Loaded  {} segments{}",
        total.to_string().green(),
        if m3u8_cached {
            " (cached)".dimmed()
        } else {
            "".normal()
        }
    );
    println!();

    // Open the tmp output file for streaming writes.
    if let Some(parent) = output.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).await?;
        }
    }
    let tmp_output = PathBuf::from(format!("{}.tmp", output.display()));
    let mut out_file = tokio::io::BufWriter::new(fs::File::create(&tmp_output).await?);

    println!("{}", "Downloading and converting segments".blue());

    // Resolve to absolute path so log messages show full filesystem paths.
    let cache_dir_abs = fs::canonicalize(cache_dir)
        .await
        .unwrap_or_else(|_| cache_dir.to_path_buf());

    let mut live = LiveLog::new(12)
        .prevent_overflow()
        .with_log_style(LogStyle::PartialDimmed);
    let mut adaptive = AdaptiveDelay::new(50);
    let mut downloaded = 0usize;
    let mut reused = 0usize;
    let mut seg_paths: Vec<PathBuf> = Vec::with_capacity(total);

    let make_status = |done: usize, dl: usize, ru: usize| -> String {
        format!(
            "  Segments  {done}/{total}  downloaded {dl}  reused {ru}",
            done = done.to_string().green(),
            total = total,
            dl = dl.to_string().cyan(),
            ru = ru.to_string().cyan(),
        )
    };

    live.updateln(&make_status(0, 0, 0), None);

    let seg_width = format!("{total}").len();
    let mut seg_idx = 0usize; // segment counter (separate from line counter)

    let mut last_fetch_done = None;
    for line in std::io::BufReader::new(std::fs::File::open(&m3u8_abs_path)?).lines() {
        let t = line?.trim().to_string();

        // Non-segment lines: write immediately and continue.
        if t.is_empty() || t.starts_with('#') {
            out_file.write_all(t.as_bytes()).await?;
            out_file.write_all(b"\n").await?;
            continue;
        }

        let i = seg_idx;
        seg_idx += 1;
        let url = resolve_segment_url(m3u8_url, &t)?;
        let raw_path = cache_dir_abs.join(format!("seg_{i}.raw"));
        let tmp_path = cache_dir_abs.join(format!("seg_{i}.tmp.ts"));
        let label = format!("[seg {:0seg_width$}]", i + 1);
        let label_width = label.bytes().len();
        let label = label.magenta();
        let done = i + 1;

        // Check for a verified cached .ts (content SHA1 matches filename hash)
        if let Some(valid_ts) =
            find_valid_hash_cached(&cache_dir_abs, &format!("seg_{i}."), ".ts").await
        {
            let fname = valid_ts.file_name().unwrap().to_string_lossy().to_string();
            let ts_path = cache_dir_abs.join(&fname);
            out_file
                .write_all(ts_path.to_string_lossy().as_bytes())
                .await?;
            out_file.write_all(b"\n").await?;
            seg_paths.push(ts_path);
            reused += 1;
            live.updateln(
                &make_status(done, downloaded, reused),
                Some(&format!(
                    "{label}  cached  {}",
                    valid_ts.display().to_string().dimmed()
                )),
            );
            continue;
        }

        // fetch
        live.updateln(
            &make_status(done - 1, downloaded, reused),
            Some(&format!("{label}  fetching  {}", url.dimmed())),
        );

        // sleep for adaptive delay, discounting the time spent since last fetch done
        {
            let duration = last_fetch_done.map_or(0, |t: Instant| t.elapsed().as_millis()) as u64;
            let delay_ms = adaptive.delay_ms();
            if duration > 0 && delay_ms > 0 && duration < delay_ms {
                sleep(std::time::Duration::from_millis(delay_ms - duration as u64)).await;
            }
        }
        let retried = download_binary(client, &url, &raw_path, Some(referer), 3)
            .await
            .map_err(|e| anyhow::anyhow!("{label}: download failed: {e}"))?;
        last_fetch_done = Some(Instant::now());

        let raw_size_bytes = fs::metadata(&raw_path)
            .await
            .map(|m| m.len())
            .unwrap_or_default();

        live.updateln(
            &make_status(done - 1, downloaded, reused),
            Some(&format!(
                "{label}  received  {}{}",
                size_display(raw_size_bytes).green(),
                if retried > 0 {
                    format!(
                        " ({} {})",
                        retried,
                        if retried == 1 { "retry" } else { "retries" }
                    )
                    .yellow()
                    .dimmed()
                } else {
                    "".normal()
                }
            )),
        );

        // remux raw -> tmp.ts, then compute content SHA1 and rename
        live.updateln(
            &make_status(done - 1, downloaded, reused),
            Some(&format!(
                "{label}  remuxing  {}",
                format!("{} -> {}", raw_path.display(), tmp_path.display()).dimmed()
            )),
        );

        remux_to_ts!(
            &raw_path,
            &tmp_path,
            on_data = |data: &str| {
                live.update_pad(
                    &make_status(done - 1, downloaded, reused),
                    Some(data),
                    LiveLog::DEFAULT_PAD + label_width + 2,
                );
            },
            auto_pty = true
        )
        .map_err(|e| anyhow::anyhow!("{label}: remux failed: {e}"))?;
        let _ = fs::remove_file(&raw_path).await;

        // Read content, compute SHA1, rename to final seg_{i}.{hash}.ts
        let hash = stream_sha1_hex(std::fs::File::open(&tmp_path)?).unwrap_or_default();
        let ts_filename = format!("seg_{i}.{hash}.ts");
        let ts_path = cache_dir_abs.join(&ts_filename);
        fs::rename(&tmp_path, &ts_path).await?;

        // Stream-write the resolved segment path to the output m3u8.
        out_file
            .write_all(ts_path.to_string_lossy().as_bytes())
            .await?;
        out_file.write_all(b"\n").await?;
        seg_paths.push(ts_path.clone());

        downloaded += 1;

        let ts_size_bytes = fs::metadata(&ts_path)
            .await
            .map(|m| m.len())
            .unwrap_or_default();

        let st = make_status(done, downloaded, reused);
        live.updateln(
            &st,
            Some(&format!(
                "{label}  {}  {} -> {}  {}",
                "DONE".green(),
                size_display(raw_size_bytes).cyan(),
                size_display(ts_size_bytes).cyan(),
                ts_path.display().to_string().dimmed(),
            )),
        );

        // record adaptive delay
        adaptive.record(retried);
    }

    let final_status = format!(
        "  Segments {status}  (total {total}, downloaded {dl}, reused {ru})",
        status = "DONE".green(),
        total = total.to_string().green(),
        dl = downloaded.to_string().cyan(),
        ru = reused.to_string().yellow(),
    );
    live.finish(&final_status);

    out_file.flush().await?;
    drop(out_file);
    fs::rename(&tmp_output, output).await?;

    Ok(seg_paths)
}

// * Direct download

/// Download a file directly to `output`.
/// If `cache_dir` is Some, it will output to the cache directory.
/// * Returns the final output absolute path.
pub async fn download_video(
    client: &Client,
    url: &str,
    referer: &str,
    output: &Path,
    cache_dir: Option<&Path>,
) -> Result<PathBuf> {
    println!("{}", "Downloading".blue());
    println!("  url  {}", url.dimmed());

    let (output_abs_path, is_cached) = if let Some(cache_dir) = cache_dir {
        // `_{output_base_name}.{hash}.{output_ext}`
        let basename = output
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let ext = output
            .extension()
            .unwrap_or(std::ffi::OsStr::new("bin"))
            .to_string_lossy()
            .to_string();
        if let Some(abs_path) =
            find_valid_hash_cached(cache_dir, &format!("_{}.", basename), &format!(".{}", ext))
                .await
        {
            (abs_path, true)
        } else {
            let abs_path = cache_dir.join(format!("_{}.{}.{}", basename, ext, "tmp"));
            download_binary(client, url, &abs_path, Some(referer), 3).await?;
            (abs_path, false)
        }
    } else {
        // Ensure parent directory exists.
        if let Some(parent) = output.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).await?;
            }
        }
        download_binary(client, url, output, Some(referer), 3).await?;
        (output.to_path_buf(), false)
    };

    let size_bytes = std::fs::metadata(output_abs_path.clone())?.len();
    println!(
        "  {msg}  {size}",
        msg = if is_cached {
            "cached".green()
        } else {
            "received".green()
        },
        size = size_display(size_bytes).green()
    );

    Ok(output_abs_path)
}
