use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use crate::utils::live_log::LiveLog;

static CLEANUP_PATHS: OnceLock<Mutex<Vec<PathBuf>>> = OnceLock::new();

pub struct CleanupGuard;

impl Drop for CleanupGuard {
    fn drop(&mut self) {
        CleanupGuard::instant_drop();
    }
}

impl CleanupGuard {
    pub fn new() -> Self {
        let _ = Self::cleanup_for_signal();
        CleanupGuard
    }
    pub fn register_path(path: impl AsRef<Path>) {
        let path_buf = path.as_ref().to_path_buf();
        if let Ok(mut paths) = CLEANUP_PATHS.get_or_init(|| Mutex::new(Vec::new())).lock() {
            paths.push(path_buf);
        }
    }
    pub fn instant_drop() {
        LiveLog::instant_drop();
        let paths_to_remove =
            if let Ok(mut paths) = CLEANUP_PATHS.get_or_init(|| Mutex::new(Vec::new())).lock() {
                std::mem::take(&mut *paths)
            } else {
                Vec::new()
            };

        for path in paths_to_remove.into_iter().rev() {
            let _ = std::fs::remove_dir_all(&path).or_else(|_| std::fs::remove_file(&path));
        }
    }
    fn cleanup_for_signal() -> anyhow::Result<()> {
        #[cfg(unix)]
        {
            use anyhow::Context;
            use tokio::signal::unix::{SignalKind, signal};

            let mut sigint = signal(SignalKind::interrupt()).context("failed to listen SIGINT")?;
            let mut sigterm =
                signal(SignalKind::terminate()).context("failed to listen SIGTERM")?;

            tokio::spawn(async move {
                tokio::select! {
                    _ = sigint.recv() => {
                        // call drop fast
                        CleanupGuard::instant_drop();
                        std::process::exit(130);
                    }
                    _ = sigterm.recv() => {
                        CleanupGuard::instant_drop();
                        std::process::exit(143);
                    }
                }
            });
        }

        #[cfg(not(unix))]
        {
            tokio::spawn(async move {
                let _ = tokio::signal::ctrl_c().await;
                CleanupGuard::instant_drop();
                std::process::exit(130);
            });
        }
        Ok(())
    }
}

pub fn size_display(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    const RATES: [u16; 5] = [1024, 1024, 1024, 1024, 1];
    const LEVEL: [u16; 5] = [1000, 1000, 1000, 1000, u16::MAX];

    const LEN: usize = UNITS.len();
    let mut num = bytes as f64;
    for i in 0..LEN {
        if num < LEVEL[i] as f64 {
            if i == 0 {
                return format!("{} {}", num as u64, UNITS[i]);
            }
            return format!("{:.2} {}", num as f64, UNITS[i]);
        }
        if i < LEN {
            num /= RATES[i] as f64;
        }
    }
    format!("{:.2} {}", num, UNITS[LEN - 1])
}

#[macro_export]
macro_rules! try_filename {
    ($path:expr) => {
        match $path.file_name() {
            Some(name) => name.to_string_lossy().to_string(),
            None => $path.to_string_lossy().to_string(),
        }
    };
}

#[macro_export]
macro_rules! exit_error {
    (code=$code:expr, $($arg:tt)*) => {{
        use colored::Colorize;
        $crate::utils::stat::CleanupGuard::instant_drop();
        eprintln!("{}: {}", "Error".red().bold(), format!($($arg)*));
        std::process::exit($code);
    }};
    ($($arg:tt)*) => {
        exit_error!(code=1, $($arg)*);
    };
}
