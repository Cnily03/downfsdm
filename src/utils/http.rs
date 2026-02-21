// HTTP client, binary file download with retry/back-off, and adaptive inter-request delay.

use anyhow::{Result, anyhow};
use colored::*;
use reqwest::Client;
use reqwest::header::{ACCEPT_LANGUAGE, HeaderMap, HeaderValue, REFERER, USER_AGENT};
use std::collections::VecDeque;
use std::path::Path;
use std::time::Duration;
use tokio::fs;
use tokio::time::sleep;

// * client

/// Build a pre-configured reqwest client that mimics a browser.
pub fn build_client() -> Result<Client> {
    let mut headers = HeaderMap::new();
    headers.insert(
        USER_AGENT,
        HeaderValue::from_static(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
        ),
    );
    headers.insert(
        ACCEPT_LANGUAGE,
        HeaderValue::from_static("zh-CN,zh;q=0.9,en;q=0.8"),
    );
    Ok(Client::builder()
        .default_headers(headers)
        .timeout(Duration::from_secs(10))
        .build()?)
}

// * download_binary

/// Download a URL as bytes and write to `dest`.
/// Retries up to `max_retries` times with linear back-off (3 s, 6 s, 9 s, …).
/// If the server returns 403/401 the request is retried without the Referer.
/// Returns the number of extra attempts used (0 = succeeded on first try).
pub async fn download_binary(
    client: &Client,
    url: &str,
    dest: &Path,
    referer: Option<&str>,
    max_retries: u32,
) -> Result<u32> {
    let mut attempt = 0u32;
    loop {
        let result: Result<()> = async {
            let mut req = client.get(url);
            if let Some(r) = referer {
                req = req.header(REFERER, r);
            }
            let res = req.send().await?;

            if res.status().is_success() {
                fs::write(dest, res.bytes().await?).await?;
                return Ok(());
            }

            // 403/401: retry once without Referer
            if (res.status().as_u16() == 403 || res.status().as_u16() == 401) && referer.is_some() {
                let res2 = client.get(url).send().await?;
                if res2.status().is_success() {
                    fs::write(dest, res2.bytes().await?).await?;
                    return Ok(());
                }
            }

            Err(anyhow!("HTTP {} — {}", res.status(), url))
        }
        .await;

        match result {
            Ok(()) => return Ok(attempt),
            Err(e) => {
                if attempt >= max_retries {
                    return Err(e);
                }
                let wait_ms = 3000u64 * (attempt as u64 + 1);
                eprintln!(
                    "retry {}/{} in {}s: {}",
                    attempt + 1,
                    max_retries,
                    wait_ms / 1000,
                    e.to_string().dimmed(),
                );
                sleep(Duration::from_millis(wait_ms)).await;
                attempt += 1;
            }
        }
    }
}

// * adaptive delay

/// Sliding-window retry counter that maps cumulative retry pressure to a
/// per-segment sleep duration, backing off automatically under CDN throttling.
pub struct AdaptiveDelay {
    window: VecDeque<u32>,
    window_size: usize,
    sum: u32,
}

impl AdaptiveDelay {
    pub fn new(window_size: usize) -> Self {
        Self {
            window: VecDeque::new(),
            window_size,
            sum: 0,
        }
    }

    /// Record the retry count for one completed download.
    pub fn record(&mut self, retries: u32) {
        self.window.push_back(retries);
        self.sum += retries;
        if self.window.len() > self.window_size {
            self.sum -= self.window.pop_front().unwrap();
        }
    }

    /// Current recommended delay in milliseconds.
    pub fn delay_ms(&self) -> u64 {
        match self.sum {
            s if s >= 15 => 5_000,
            s if s >= 10 => 2_000,
            s if s >= 7 => 1_000,
            s if s >= 4 => 500,
            s if s >= 1 => 250,
            _ => 150,
        }
    }
}
