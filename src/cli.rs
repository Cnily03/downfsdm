// CLI argument definitions for downfsdm.

use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "downfsdm",
    version,
    about = "Download anime from a bytegooty.com player page into a local m3u8"
)]
pub struct Args {
    /// Player page URL (e.g. https://api.bytegooty.com/test16/?url=fsyun_...)
    pub player_url: String,

    /// Output m3u8 file path
    #[arg(short, long = "output", value_name = "FILE")]
    pub output: PathBuf,

    /// Directory for cached .ts segment files
    /// (default: system temp dir]
    #[arg(long, value_name = "DIR")]
    pub cache_dir: Option<PathBuf>,

    /// Output format for the final video file
    /// (e.g. .mp4 => mp4 muxer)
    #[arg(short, long, value_name = "FORMAT", default_value = "auto")]
    pub format: String,

    /// Cleanup intermediate files after successful conversion (default: false)
    #[arg(long, default_value_t = false)]
    pub cleanup: bool,

    /// FFmpeg binary path
    #[arg(long, value_name = "FILE", default_value = "ffmpeg")]
    pub ffmpeg_bin: String,

    /// Extra arguments passed verbatim to ffmpeg during conversion.
    #[arg(last = true)]
    pub ffmpeg_args: Vec<String>,
}
