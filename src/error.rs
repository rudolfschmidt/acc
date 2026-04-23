//! Central error type and source-excerpt rendering.
//!
//! Every pipeline-phase error (parser, resolver, booker) displays in
//! the same format:
//!
//! ```text
//! While parsing file "path/to/file.ledger" at line N:
//! >> headline
//!
//! N | source line
//! N | source line
//! ```
//!
//! The opening line mirrors ledger-cli's `While parsing file "..." at
//! line N:` style so users transitioning from ledger see familiar
//! phrasing. The `N | ` gutter on each source line mirrors the rustc
//! / hledger style — the line number of the offending line is visible
//! at a glance inside the excerpt.
//!
//! - **Transaction-scoped** errors (balance, assertion failure): caller
//!   knows the full line range and passes it to [`render_range`].
//! - **Line-scoped** errors (parse, resolve on a single line):
//!   [`render_at_line`] scans backward for the enclosing transaction
//!   header and shows that block up to the error line.

use std::fmt;

use colored::Colorize;

#[derive(Debug)]
pub struct Error(String);

impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Error(message.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<String> for Error {
    fn from(s: String) -> Self {
        Error(s)
    }
}

impl From<&str> for Error {
    fn from(s: &str) -> Self {
        Error(s.to_owned())
    }
}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Error(err.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(err: serde_json::Error) -> Self {
        Error(err.to_string())
    }
}

impl From<ureq::Error> for Error {
    fn from(err: ureq::Error) -> Self {
        Error(err.to_string())
    }
}

// ==================== Source-excerpt rendering ====================

/// Render with an explicit line range. Used by transaction-scoped
/// errors whose span is known up front.
pub(crate) fn render_range(
    f: &mut fmt::Formatter<'_>,
    file: &str,
    start: usize,
    end: usize,
    headline: &str,
) -> fmt::Result {
    write!(
        f,
        "{}\n>> {}",
        format!("While parsing file \"{}\" at line {}:", file, start).cyan(),
        headline.red().bold(),
    )?;
    write_excerpt(f, file, start, end)
}

/// Render a line-scoped error. Scans backward from `error_line` for
/// the enclosing transaction header so the shown excerpt covers as
/// much context as is useful.
pub(crate) fn render_at_line(
    f: &mut fmt::Formatter<'_>,
    file: &str,
    error_line: usize,
    headline: &str,
) -> fmt::Result {
    let start = tx_start(file, error_line);
    write!(
        f,
        "{}\n>> {}",
        format!("While parsing file \"{}\" at line {}:", file, error_line).cyan(),
        headline.red().bold(),
    )?;
    write_excerpt(f, file, start, error_line)
}

/// Find the start line of the transaction surrounding `error_line`
/// by scanning backward for a line that begins with an ASCII digit
/// (the date column of a tx header). Returns `error_line` when no
/// such header is found above — the error is on a top-level
/// directive and the excerpt should show only that line.
fn tx_start(file: &str, error_line: usize) -> usize {
    if file.is_empty() || error_line == 0 {
        return error_line;
    }
    let Ok(source) = std::fs::read_to_string(file) else {
        return error_line;
    };
    let lines: Vec<&str> = source.lines().collect();
    let upper = error_line.min(lines.len());
    for i in (0..upper).rev() {
        if lines[i]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit())
        {
            return i + 1;
        }
    }
    error_line
}

/// Read lines `start..=end` (1-indexed, inclusive) from `file`.
/// Returns `None` when the file can't be read or the range is empty.
fn read_block(file: &str, start: usize, end: usize) -> Option<Vec<String>> {
    if file.is_empty() {
        return None;
    }
    let source = std::fs::read_to_string(file).ok()?;
    let mut out = Vec::with_capacity(end.saturating_sub(start) + 1);
    for (idx, line) in source.lines().enumerate() {
        let lineno = idx + 1;
        if lineno < start {
            continue;
        }
        if lineno > end {
            break;
        }
        out.push(line.to_string());
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Write source lines between `start..=end` with a right-aligned
/// `N |` gutter, preceded by a blank separator. No-op when the file
/// is unreadable or the range yields no lines.
fn write_excerpt(
    f: &mut fmt::Formatter<'_>,
    file: &str,
    start: usize,
    end: usize,
) -> fmt::Result {
    let Some(block) = read_block(file, start, end) else {
        return Ok(());
    };
    let width = end.to_string().len();
    write!(f, "\n\n")?;
    let mut first = true;
    for (i, line) in block.iter().enumerate() {
        if !first {
            writeln!(f)?;
        }
        let lineno = start + i;
        write!(
            f,
            "{} {}",
            format!("{:>width$} |", lineno, width = width),
            line,
        )?;
        first = false;
    }
    Ok(())
}
