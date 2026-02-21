use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;
use std::sync::{OnceLock, RwLock};
use strum::{Display, EnumString};

use crate::{exit_error, piped_or_inherit};

static FFMPEG_BIN: OnceLock<RwLock<String>> = OnceLock::new();

pub fn ffmpeg_bin() -> String {
    FFMPEG_BIN
        .get_or_init(|| RwLock::new("ffmpeg".to_string()))
        .read()
        .expect("FFMPEG_BIN read lock poisoned")
        .clone()
}

/// Call this before any ffmpeg functions.
pub fn set_ffmpeg_binary(ffmpeg_bin: &str) -> Result<(String, String)> {
    // check binary exists
    let abs_ffmpeg_bin = which::which(ffmpeg_bin)
        .with_context(|| format!("ffmpeg binary not found: {}", ffmpeg_bin))?;
    // check executable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata =
            std::fs::metadata(abs_ffmpeg_bin.clone()).context("failed to stat ffmpeg binary")?;
        let permissions = metadata.permissions();
        if permissions.mode() & 0o111 == 0 {
            return Err(anyhow::anyhow!(
                "ffmpeg binary is not executable: {}",
                ffmpeg_bin
            ));
        }
    }
    // get ffmpeg version
    let version_output = Command::new(&abs_ffmpeg_bin)
        .arg("-version")
        .output()
        .context("failed to execute ffmpeg binary")?;
    if !version_output.status.success() {
        return Err(anyhow::anyhow!(
            "ffmpeg binary returned non-zero exit code: {}",
            ffmpeg_bin
        ));
    }
    let version = String::from_utf8_lossy(&version_output.stdout)
        .lines()
        .next()
        .unwrap_or("")
        .split_whitespace()
        .nth(2)
        .unwrap_or("")
        .to_string();
    if version.is_empty() {
        return Err(anyhow::anyhow!(
            "failed to get ffmpeg version from binary: {}",
            ffmpeg_bin
        ));
    }

    let bin_lock = FFMPEG_BIN.get_or_init(|| RwLock::new("ffmpeg".to_string()));
    *bin_lock.write().expect("FFMPEG_BIN write lock poisoned") = ffmpeg_bin.to_string();

    Ok((
        abs_ffmpeg_bin.to_string_lossy().to_string(),
        version.to_string(),
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Display, EnumString)]
#[strum(serialize_all = "lowercase")]
pub enum KnownFormat {
    M3u8,
    Mp4,
    Webm,
    Flv,
    Avi,
    /// MPEG-TS container; ext = "ts", ffmpeg muxer name = "mpegts"
    #[strum(serialize = "mpegts", serialize = "ts", serialize = "m2ts")]
    MpegTs,
    Mkv,
    Mov,
    M4v,
}

impl KnownFormat {
    /// All supported file extensions that can be mapped to a KnownFormat, for inferring from URLs.
    pub fn exts() -> Vec<&'static str> {
        vec![
            "m3u8", "mp4", "webm", "flv", "avi", "ts", "mpegts", "m2ts", "mkv", "mov", "m4v",
        ]
    }

    /// File extension for this format (e.g. "mp4", "ts").
    pub fn as_ext(&self) -> &str {
        match self {
            Self::M3u8 => "m3u8",
            Self::Mp4 => "mp4",
            Self::Webm => "webm",
            Self::Flv => "flv",
            Self::Avi => "avi",
            Self::MpegTs => "ts",
            Self::Mkv => "mkv",
            Self::Mov => "mov",
            Self::M4v => "m4v",
        }
    }

    /// Human-readable stream type / ffmpeg muxer name.
    pub fn as_stream_type(&self) -> &str {
        match self {
            Self::M3u8 => "hls/m3u8",
            Self::Mp4 => "mp4",
            Self::Webm => "webm",
            Self::Flv => "flv",
            Self::Avi => "avi",
            Self::MpegTs => "mpegts",
            Self::Mkv => "matroska",
            Self::Mov => "mov",
            Self::M4v => "mp4",
        }
    }

    pub fn as_ffmpeg_format(&self) -> &str {
        match self {
            Self::M3u8 => "hls",
            Self::Mp4 => "mp4",
            Self::Webm => "webm",
            Self::Flv => "flv",
            Self::Avi => "avi",
            Self::MpegTs => "mpegts",
            Self::Mkv => "matroska",
            Self::Mov => "mov",
            Self::M4v => "mp4",
        }
    }
}

/// Remux a raw MPEG-TS segment (often disguised as .png or .raw) into a
/// proper .ts file using `-f mpegts` to force the correct demuxer.
///
/// Returns captured ffmpeg stderr lines (warnings/stats) so callers can
/// surface them in a live-log display.
pub fn remux_to_ts(
    input: &Path,
    output: &Path,
    on_data: Option<impl FnMut(&str)>,
    no_pty: bool,
) -> Result<()> {
    let cmd = ffmpeg_bin();
    let args = &[
        "-y",
        "-loglevel",
        "warning",
        "-f",
        "mpegts",
        "-i",
        input.to_str().unwrap(),
        "-c",
        "copy",
        output.to_str().unwrap(),
    ];
    piped_or_inherit!(&cmd, args, on_data = on_data, no_pty = no_pty)
}

#[macro_export]
macro_rules! remux_to_ts {
    ($input:expr, $output:expr, on_data = $on_data:expr, auto_pty = $auto_pty:expr) => {
        crate::utils::ffmpeg::remux_to_ts($input, $output, Some($on_data), !($auto_pty))
    };
    ($input:expr, $output:expr) => {
        crate::utils::ffmpeg::remux_to_ts($input, $output, None, true)
    };
}

// Per-format codec selection — defaults balance smaller output size and faster conversion.
// Returns (video_args, audio_args, extra_mux_args)
fn default_codec_args(
    format: &KnownFormat,
) -> (
    &'static [&'static str],
    &'static [&'static str],
    &'static [&'static str],
) {
    let (v_args, a_args, extra): (&[&str], &[&str], &[&str]) = match format {
        // TS: already a container stream, just remux
        KnownFormat::MpegTs => (&["-c:v", "copy"], &["-c:a", "copy"], &[]),
        // WebM: VP9 + Opus — good balance of speed and compression
        KnownFormat::Webm => (
            &[
                "-c:v",
                "libvpx-vp9",
                "-crf",
                "31",
                "-b:v",
                "0",
                "-cpu-used",
                "2",
            ],
            &["-c:a", "libopus", "-b:a", "128k"],
            &[],
        ),
        // MKV: H.264 + Opus
        KnownFormat::Mkv => (
            &["-c:v", "libx264", "-crf", "23", "-preset", "medium"],
            &["-c:a", "libopus", "-b:a", "128k"],
            &[],
        ),
        // FLV: H.264 + AAC (FLV container limits codec choices)
        KnownFormat::Flv => (
            &["-c:v", "libx264", "-crf", "23", "-preset", "medium"],
            &["-c:a", "aac", "-b:a", "128k"],
            &[],
        ),
        // AVI: H.264 + MP3
        KnownFormat::Avi => (
            &["-c:v", "libx264", "-crf", "23", "-preset", "medium"],
            &["-c:a", "libmp3lame", "-b:a", "128k"],
            &[],
        ),
        // MP4 / MOV / M4V: H.264 + AAC
        _ => (
            &["-c:v", "libx264", "-crf", "23", "-preset", "medium"],
            &["-c:a", "aac", "-b:a", "128k"],
            match format {
                KnownFormat::Mp4 | KnownFormat::Mov | KnownFormat::M4v => {
                    &["-movflags", "+faststart"]
                }
                _ => &[],
            },
        ),
    };
    (v_args, a_args, extra)
}

pub fn prepare_convert<'a>(
    input: &'a Path,
    output: &'a Path,
    format: &'a KnownFormat,
    codec_args: Option<&'a [String]>,
) -> Result<(String, Vec<&'a str>)> {
    let cmd = ffmpeg_bin();

    let input_str = input.to_str().unwrap();
    let output_str = output.to_str().unwrap();

    let mut argv: Vec<&str> = vec![
        "-y",
        "-loglevel",
        "warning",
        "-stats",
        "-threads",
        "0",
        "-allowed_extensions",
        "ALL",
        "-protocol_whitelist",
        "file,http,https,tcp,tls,crypto",
        "-i",
        input_str,
    ];
    if let Some(codec_args) = codec_args
        && !codec_args.is_empty()
    {
        argv.extend_from_slice(&["-f", format.as_ffmpeg_format()]);
        argv.extend(codec_args.iter().map(|s| s.as_str()));
    } else {
        let (v_args, a_args, extra) = default_codec_args(format);
        argv.extend_from_slice(v_args);
        argv.extend_from_slice(a_args);
        argv.extend_from_slice(&["-f", format.as_ffmpeg_format()]);
        argv.extend_from_slice(extra);
    }
    argv.push(output_str);
    Ok((cmd, argv))
}
/// Convert a video file (either a single MP4 or a segmented m3u8 playlist) into a final
/// single video file.  `format` is the ffmpeg muxer name, e.g. `"mp4"` or `"mkv"`.
///
/// Each line of ffmpeg stderr output (split on `\r` or `\n`, so progress stats are
/// delivered immediately) is forwarded to `on_data` for live display.
pub fn convert_video_to(
    cmd: &str,
    argv: &[&str],
    on_data: Option<impl FnMut(&str)>,
    no_pty: bool,
) -> Result<()> {
    piped_or_inherit!(&cmd, argv, on_data = on_data, no_pty = no_pty)
}

#[macro_export]
macro_rules! convert_video_to {
    ($input:expr, $output:expr, $format:expr, codec_args = $codec_args:expr, on_data = $on_data:expr, auto_pty = $auto_pty:expr) => {{
        let (cmd, argv) =
            crate::utils::ffmpeg::prepare_convert($input, $output, &$format, Some(&$codec_args))?;
        crate::utils::ffmpeg::convert_video_to(&cmd, &argv, Some($on_data), !($auto_pty))
    }};
    ($input:expr, $output:expr, $format:expr, on_data = $on_data:expr, auto_pty = $auto_pty:expr) => {{
        let (cmd, argv) = crate::utils::ffmpeg::prepare_convert($input, $output, &$format, None)?;
        crate::utils::ffmpeg::convert_video_to(&cmd, &argv, Some($on_data), !($auto_pty))
    }};
    ($input:expr, $output:expr, $format:expr, codec_args = $codec_args:expr) => {{
        let (cmd, argv) =
            crate::utils::ffmpeg::prepare_convert($input, $output, &$format, Some(&$codec_args))?;
        crate::utils::ffmpeg::convert_video_to(&cmd, &argv, None, true)
    }};
    ($input:expr, $output:expr, $format:expr) => {{
        let (cmd, argv) = crate::utils::ffmpeg::prepare_convert($input, $output, &$format, None)?;
        crate::utils::ffmpeg::convert_video_to(&cmd, &argv, None, true)
    }};
    ($cmd:expr, $argv:expr, on_data = $on_data:expr, auto_pty = $auto_pty:expr) => {
        crate::utils::ffmpeg::convert_video_to(&$cmd, &$argv, Some($on_data), !($auto_pty))
    };
    ($cmd:expr, $argv:expr) => {
        crate::utils::ffmpeg::convert_video_to(&$cmd, &$argv, None, true)
    };
}
