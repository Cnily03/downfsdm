use anyhow::{Context, Result};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};

use crate::exit_error;

/// Spawn a thread that reads `$pipe` byte-by-byte, splitting on `\r` or `\n`
/// so that ffmpeg progress lines are delivered immediately via `$tx`.
macro_rules! spawn_data_reader {
    ($pipe:expr, $tx:expr) => {{
        use std::io::Read as _;
        let pipe = $pipe;
        let tx = $tx;
        std::thread::spawn(move || {
            let mut reader = std::io::BufReader::new(pipe);
            let mut buf: Vec<u8> = Vec::new();
            let mut byte = [0u8; 1];
            loop {
                match reader.read(&mut byte) {
                    Ok(0) => {
                        if !buf.is_empty() {
                            let _ = tx.send(String::from_utf8_lossy(&buf).into_owned());
                        }
                        break;
                    }
                    Ok(_) => {
                        if byte[0] == b'\n' || byte[0] == b'\r' {
                            buf.push(byte[0]);
                            if !buf.is_empty() {
                                let _ = tx.send(String::from_utf8_lossy(&buf).into_owned());
                                buf.clear();
                            }
                        } else {
                            buf.push(byte[0]);
                        }
                    }
                    Err(_) => break,
                }
            }
        })
    }};
}

pub fn run_piped(cmd: &str, args: &[&str], mut on_data: impl FnMut(&str)) -> Result<()> {
    use std::process::{Command, Stdio};
    use std::sync::mpsc;

    let mut child = Command::new(cmd)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context(format!("failed to spawn {}", cmd))?;

    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();

    let (tx, rx) = mpsc::channel::<String>();

    // Split on \r and \n so ffmpeg progress updates are delivered in real time.
    let stdout_handle = spawn_data_reader!(stdout, tx.clone());
    let stderr_handle = spawn_data_reader!(stderr, tx);
    // All senders are now owned by the reader threads; rx will close when both finish.

    for data in rx {
        if !data.is_empty() {
            on_data(&data);
        }
    }

    // Join readers first — they must finish before we wait, otherwise a full
    // pipe buffer could deadlock the child.
    let _ = stdout_handle.join();
    let _ = stderr_handle.join();

    let status = child.wait().context(format!("{} wait failed", cmd))?;
    if !status.success() {
        exit_error!("{} failed (exit {})", cmd, status);
    }
    Ok(())
}

pub fn run_in_pty(cmd: &str, args: &[&str], mut on_data: impl FnMut(&str)) -> Result<()> {
    use std::sync::mpsc;

    let pty = {
        let pty_system = native_pty_system();
        // current rows and cols
        let size = terminal_size::terminal_size()
            .map(
                |(terminal_size::Width(w), terminal_size::Height(h))| PtySize {
                    rows: h as u16,
                    cols: w as u16,
                    pixel_width: 0,
                    pixel_height: 0,
                },
            )
            .unwrap_or(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            });
        pty_system
            .openpty(size)
            .context("failed to create fake pty")?
    };

    let mut command = CommandBuilder::new(cmd);
    command.args(args);

    let mut child = pty
        .slave
        .spawn_command(command)
        .context("failed to spawn ffmpeg in fake pty")?;

    // Drop the parent's copy of the slave fd so that when the child exits,
    // the master reader will see EOF/EIO instead of blocking forever.
    drop(pty.slave);

    let reader = pty
        .master
        .try_clone_reader()
        .context("failed to clone fake pty master reader")?;

    let (tx, rx) = mpsc::channel::<String>();
    let read_handle = spawn_data_reader!(reader, tx);
    for data in rx {
        if !data.is_empty() {
            on_data(&data);
        }
    }

    let _ = read_handle.join();

    let status = child.wait().context(format!("{} wait failed", cmd))?;
    if !status.success() {
        exit_error!("{} failed (exit {})", cmd, status);
    }

    Ok(())
}

#[macro_export]
macro_rules! piped_or_inherit {
    ($cmd:expr, $args:expr, on_data = $on_data:expr, no_pty = $no_pty:expr) => {
        match $on_data {
            Some(on_data) => {
                let is_tty = std::io::IsTerminal::is_terminal(&std::io::stdout());
                let pty_supported = !cfg!(windows);
                if $no_pty || !is_tty || !pty_supported {
                    // No PTY (e.g. piped or redirected) — fall back to plain pipes.
                    crate::utils::process::run_piped($cmd, $args, on_data)
                } else {
                    // Parent has a real PTY — use a fake PTY so ffmpeg
                    // produces coloured / progress output as if interactive.
                    crate::utils::process::run_in_pty($cmd, $args, on_data)
                }
            }
            None => {
                use std::process::Command;
                let status = Command::new($cmd)
                    .args($args)
                    .status()
                    .context(format!("failed to execute {}", $cmd))?;
                if !status.success() {
                    exit_error!("{} failed (exit {})", $cmd, status);
                }
                Ok(())
            }
        }
    };
}
