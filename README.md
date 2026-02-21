# fsdm Downloader

This program can download media files, given a fsdm player html url.

## Usage

Visit url like `https://www.fsdm02.com/vodplay/xxx-1-1.html` in browser, open dev tools and find the player url.

The player url is like `https://api.bytegooty.com/test16/?url=fsyun_...`.

Use the cli tools to download the media file:

```bash
cargo run --release -- -o output.mp4 https://api.bytegooty.com/test16/?url=fsyun_...
```

Full usage:

```bash
Usage: downfsdm [OPTIONS] --output <FILE> <PLAYER_URL> [-- <FFMPEG_ARGS>...]

Arguments:
  <PLAYER_URL>      Player page URL (e.g. https://api.bytegooty.com/test16/?url=fsyun_...)
  [FFMPEG_ARGS]...  Extra arguments passed verbatim to ffmpeg during conversion

Options:
  -o, --output <FILE>      Output m3u8 file path
      --cache-dir <DIR>    Directory for cached .ts segment files (default: system temp dir]
  -f, --format <FORMAT>    Output format for the final video file (e.g. .mp4 => mp4 muxer) [default: auto]
      --cleanup            Cleanup intermediate files after successful conversion (default: false)
      --ffmpeg-bin <FILE>  FFmpeg binary path [default: ffmpeg]
  -h, --help               Print help
  -V, --version            Print version
```

## Build

Make sure you have Rust toolchain installed, then run:

```bash
cargo build --release
```

## License

Copyright (c) Cnily03. All rights reserved.

Licensed under the [Apache 2.0](LICENSE) License.
