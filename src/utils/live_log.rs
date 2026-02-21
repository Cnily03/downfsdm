// Live terminal progress: sticky status line above a rolling dim log tail, erased cleanly on finish.

use colored::Colorize;
use std::collections::VecDeque;
use std::io::Write;
use terminal_size::{Height, terminal_size};

/// Controls how each tail entry is rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[allow(unused)]
pub enum LogStyle {
    /// Output the entry exactly as-is, including any embedded ANSI codes.
    Raw,
    /// Preserve only foreground color, underline, and bold-related ANSI states.
    PartialDimmed,
    /// Strip all existing ANSI codes first, then apply dimmed — the current default.
    #[default]
    OnlyDimmed,
}

pub struct LiveLog {
    tail_height: usize,
    buf: VecDeque<String>,
    /// How many lines are currently occupying the terminal (status + tail).
    rendered: usize,
    /// If Some(pad), tail_height is auto-fitted to terminal height on each update.
    fit_pad: Option<usize>,
    /// How tail entries are styled.
    log_style: LogStyle,
    /// The tail_height won't exceed the terminal height minus this status rows
    prevent_overflow: bool,
}

impl LiveLog {
    pub const DEFAULT_PAD: usize = 2;

    pub fn new(tail_height: usize) -> Self {
        print!("\x1b[?25l"); // hide cursor while rendering
        let _ = std::io::stdout().flush();
        Self {
            tail_height,
            buf: VecDeque::new(),
            rendered: 0,
            fit_pad: None,
            log_style: LogStyle::default(),
            prevent_overflow: false,
        }
    }

    /// Create a LiveLog whose tail_height is automatically computed on every
    /// `update()` call as:  `terminal_rows - status_rows - pad`.
    /// If the terminal size cannot be queried the row count defaults to 10.
    pub fn new_with_fit(padding_top: usize) -> Self {
        print!("\x1b[?25l"); // hide cursor while rendering
        let _ = std::io::stdout().flush();
        Self {
            tail_height: 10usize.saturating_sub(padding_top).max(1),
            buf: VecDeque::new(),
            rendered: 0,
            fit_pad: Some(padding_top),
            log_style: LogStyle::default(),
            prevent_overflow: false,
        }
    }

    pub fn with_log_style(mut self, style: LogStyle) -> Self {
        self.log_style = style;
        self
    }

    pub fn prevent_overflow(mut self) -> Self {
        self.prevent_overflow = true;
        self
    }

    /// Recalculate tail_height from the current terminal size and the number
    /// of lines occupied by `status`. Called automatically when `fit_pad` is set.
    fn recalc_tail_height(&mut self, status: &str) {
        if let Some(pad) = self.fit_pad {
            let term_rows = terminal_size()
                .map(|(_, Height(h))| h as usize)
                .unwrap_or(10);
            let status_rows = status.split('\n').count();
            self.tail_height = term_rows
                .saturating_sub(status_rows)
                .saturating_sub(pad)
                .max(1);
        }
        if self.prevent_overflow {
            let term_rows = terminal_size()
                .map(|(_, Height(h))| h as usize)
                .unwrap_or(10);
            let status_rows = status.split('\n').count();
            self.tail_height = self
                .tail_height
                .min(term_rows.saturating_sub(status_rows))
                .max(1);
        }
    }

    pub fn buf_lines(&self) -> usize {
        self.buf
            .iter()
            .map(|s| s.matches('\n').count())
            .sum::<usize>()
            + 1
    }

    /// Push an optional log entry into the rolling tail and redraw.
    pub fn update_pad(&mut self, status: &str, entry: Option<&str>, pad: usize) {
        self.recalc_tail_height(status);
        if let Some(entry) = entry {
            let pad = &" ".repeat(pad);
            let last_end_lf = self.buf.back().map(|s| s.ends_with('\n')).unwrap_or(true);
            // `entry` may be any string, if its a line, it ends with `\n`, if not, it append to the current line
            // we should ensure that each buf only has 0 or 1 `\n`, and ends with `\n` if it has one
            let splited = entry.split('\n').collect::<Vec<&str>>();
            for i in 0..splited.len() {
                let line = splited[i].replace("\r", &format!("\r{pad}"));
                let padding = if i > 0 || last_end_lf { pad } else { "" };
                if i < splited.len() - 1 {
                    self.buf.push_back(format!("{padding}{line}\n"));
                } else {
                    if !line.is_empty() {
                        self.buf.push_back(format!("{padding}{line}"));
                    }
                }
            }
            let mut buf_lines = self.buf_lines();
            while buf_lines > self.tail_height {
                while let Some(front) = self.buf.front() {
                    let found_newline = front.matches('\n').count() > 0;
                    self.buf.pop_front();
                    if found_newline {
                        break;
                    }
                }
                buf_lines -= 1;
            }
        }
        self.draw(status, false);
    }

    /// Push an optional log entry into the rolling tail and redraw with new line
    pub fn updateln_pad(&mut self, status: &str, entry: Option<&str>, pad: usize) {
        if let Some(e) = entry {
            self.update_pad(status, Some(&format!("{}\n", e)), pad);
        } else {
            self.update_pad(status, None, pad);
        }
    }

    pub fn update(&mut self, status: &str, entry: Option<&str>) {
        self.update_pad(status, entry, Self::DEFAULT_PAD);
    }

    pub fn updateln(&mut self, status: &str, entry: Option<&str>) {
        self.updateln_pad(status, entry, Self::DEFAULT_PAD);
    }

    /// Erase the tail and leave only the final status line on screen.
    pub fn finish(&mut self, status: &str) {
        self.draw(status, true);
    }

    // * internal renderer

    fn draw(&mut self, status: &str, finishing: bool) {
        let mut out = String::new();

        // Move cursor up to re-paint previous render.
        if self.rendered > 0 {
            out.push_str(&format!(
                "{}\x1b[{}A\x1b[0G",
                "".normal(),
                self.rendered - 1
            ));
        }

        // Status lines (multi-line status supported, each gets clear-to-EOL).
        let mut lines: usize = 0;
        let mut status_lines = 0;
        for status_line in status.split('\n') {
            out.push_str(status_line);
            out.push_str("\x1b[K\n");
            lines += 1;
            status_lines += 1;
        }

        if finishing {
            // Erase everything below the status line to end of screen.
            out.push_str("\x1b[J");
            out.push_str("\x1b[?25h"); // restore cursor
        } else {
            // Print filled log entries styled according to log_style.
            let mut last_end_lf = true;
            for entry in &self.buf {
                let styled = match self.log_style {
                    // Raw: emit as-is.
                    LogStyle::Raw => entry.clone(),
                    // PartialDimmed preserves only selected ANSI states, then applies dim.
                    LogStyle::PartialDimmed => preserve_partial(entry).dimmed().to_string(),
                    // OnlyDimmed: strip embedded ANSI first, then apply dimmed.
                    LogStyle::OnlyDimmed => strip_ansi(entry).dimmed().to_string(),
                };
                if last_end_lf {
                    lines += 1;
                    out.push_str("\x1b[K");
                }
                out.push_str(&styled);
                if entry.ends_with('\n') {
                    last_end_lf = true;
                } else {
                    last_end_lf = false;
                }
            }
            if last_end_lf {
                lines += 1;
                out.push_str("\x1b[K");
            }
            let rest_lines = self.tail_height + status_lines - lines;
            // Pad with blank lines so the block height stays stable.
            for _ in 0..rest_lines {
                out.push_str("\n\x1b[K");
                lines += 1;
            }
        }

        print!("{}", out);
        let _ = std::io::stdout().flush();
        self.rendered = lines;
    }

    pub fn instant_drop() {
        // Always restore cursor visibility even on panic / early return.
        print!("\x1b[?25h"); // restore cursor
        let _ = std::io::stdout().flush();
    }
}

impl Drop for LiveLog {
    fn drop(&mut self) {
        LiveLog::instant_drop();
    }
}

/// Remove all ANSI escape sequences from `s`.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            chars.next(); // consume '['
            // consume until a letter (the final byte of the CSI sequence)
            for ch in chars.by_ref() {
                if ch.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Preserve only foreground color, underline, bold-related ANSI SGR states, plus `\x1b[K`.
/// All other non-SGR escape sequences (cursor movement, etc.) are dropped.
fn preserve_partial(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 16);
    let mut chars = s.chars().peekable();
    let mut plain = String::new();

    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            if !plain.is_empty() {
                out.push_str(&plain);
                plain.clear();
            }

            let mut seq = String::new();
            seq.push(c);
            seq.push(chars.next().unwrap_or('['));

            for ch in chars.by_ref() {
                seq.push(ch);
                if ch.is_ascii_alphabetic() {
                    break;
                }
            }

            if seq == "\x1b[K" {
                // explicitly preserve erase-to-EOL
                out.push_str(&seq);
            } else if !seq.starts_with("\x1b[") || !seq.ends_with('m') {
                // drop all other non-SGR sequences
            } else {
                // Keep only foreground/bold/underline SGR params; drop everything else.
                let params = &seq[2..seq.len() - 1];
                let tokens: Vec<&str> = if params.is_empty() {
                    vec![""]
                } else {
                    params.split(';').collect()
                };

                let mut kept: Vec<&str> = Vec::new();
                let mut i = 0usize;
                while i < tokens.len() {
                    let token = tokens[i];
                    let n = token.parse::<u16>().ok();

                    match n {
                        Some(0) // reset
                        | Some(1) // bold on
                        | Some(22) // bold off
                        | Some(4) // underline on
                        | Some(24) // underline off
                        | Some(39) // default foreground
                        | Some(30..=37) // foreground colors
                        | Some(90..=97) // bright foreground colors
                        => {
                            kept.push(token);
                            i += 1;
                        }
                        Some(38) => {
                            if i + 1 < tokens.len() {
                                match tokens[i + 1] {
                                    "5" if i + 2 < tokens.len() => {
                                        kept.push("38");
                                        kept.push("5");
                                        kept.push(tokens[i + 2]);
                                        i += 3;
                                    }
                                    "2" if i + 4 < tokens.len() => {
                                        kept.push("38");
                                        kept.push("2");
                                        kept.push(tokens[i + 2]);
                                        kept.push(tokens[i + 3]);
                                        kept.push(tokens[i + 4]);
                                        i += 5;
                                    }
                                    _ => {
                                        i += 1;
                                    }
                                }
                            } else {
                                i += 1;
                            }
                        }
                        _ => {
                            i += 1;
                        }
                    }
                }

                if !kept.is_empty() {
                    out.push_str("\x1b[");
                    out.push_str(&kept.join(";"));
                    out.push('m');
                }
            }
        } else {
            plain.push(c);
        }
    }

    if !plain.is_empty() {
        out.push_str(&plain);
    }

    out
}
