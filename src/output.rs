use std::io::{self, Write};

use comfy_table::{Cell, Color, ContentArrangement, Table, presets::UTF8_FULL};
use serde::Serialize;

/// Controls human vs JSON output formatting.
/// Diagnostic messages always go to `stderr` so `stdout` stays clean for JSON or piped output.
#[derive(Debug, Clone, Copy)]
pub struct Output {
    pub json: bool,
    pub quiet: bool,
    pub verbose: bool,
}

impl Output {
    pub fn new(json: bool, quiet: bool, verbose: bool) -> Self {
        Self {
            json,
            quiet,
            verbose,
        }
    }

    /// Emit a structured JSON object to stdout.
    pub fn emit_json<T: Serialize>(&self, value: &T) -> io::Result<()> {
        let mut out = io::stdout().lock();
        serde_json::to_writer_pretty(&mut out, value)
            .map_err(|e| io::Error::other(format!("JSON serialization failed: {e}")))?;
        writeln!(out)?;
        out.flush()
    }

    /// Print an informational status line to stderr (suppressed when `--quiet` or `--json` unless important).
    pub fn status(&self, msg: &str) {
        if !self.quiet && !self.json {
            let _ = writeln!(io::stderr().lock(), "{msg}");
        }
    }

    /// Always print an informational line to stderr (unless `--quiet`), even in `--json` mode for progress.
    pub fn info(&self, msg: &str) {
        if !self.quiet {
            let _ = writeln!(io::stderr().lock(), "{msg}");
        }
    }

    /// Print a warning to stderr.
    pub fn warn(&self, msg: &str) {
        let _ = writeln!(io::stderr().lock(), "warning: {msg}");
    }

    /// Print verbose/debug trace to stderr when `--verbose` or `--logtostderr` is enabled.
    pub fn debug(&self, msg: &str) {
        if self.verbose {
            let _ = writeln!(io::stderr().lock(), "[debug] {msg}");
        }
    }

    /// Print a plain line to stdout (human mode).
    pub fn println(&self, msg: &str) {
        if !self.quiet {
            println!("{msg}");
        }
    }
}

/// Build a styled UTF-8 table for human display.
pub fn make_table(headers: &[&str]) -> Table {
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic);
    let cells: Vec<Cell> = headers
        .iter()
        .map(|h| Cell::new(h).fg(Color::Cyan))
        .collect();
    table.set_header(cells);
    table
}

/// Format AutoDL milli-CNY integer values (`assets`, `payg_price`, etc. = CNY * 1000).
pub fn format_cny_milli(milli: i64) -> String {
    let yuan = milli as f64 / 1000.0;
    format!("¥{yuan:.3}")
}

/// Format byte counts in human-readable units.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut val = bytes as f64;
    let mut idx = 0usize;
    while val >= 1024.0 && idx < UNITS.len() - 1 {
        val /= 1024.0;
        idx += 1;
    }
    format!("{val:.2} {}", UNITS[idx])
}

/// Mask a secret token or password for display/logs.
pub fn mask_secret(secret: &str) -> String {
    let chars: Vec<char> = secret.chars().collect();
    if chars.len() <= 10 {
        return "****".to_string();
    }
    let prefix: String = chars.iter().take(4).collect();
    let suffix: String = chars.iter().skip(chars.len() - 4).collect();
    format!("{prefix}...{suffix}")
}
