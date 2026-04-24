//! `format` command — parse, validate syntax, re-emit aligned.
//!
//! Semantics: **print --raw but aligned**. Runs the parser only,
//! no resolve / no book / no rebalance. Missing amounts stay missing
//! (nothing is computed), comments are preserved, directives keep
//! their canonical form, transactions are stably date-sorted so
//! same-day events keep their original relative order.
//!
//! Always writes back in place. Atomic: each file is written to
//! `<path>.ledger.tmp` and then renamed over the target, so a crash
//! mid-write never leaves a half-written file.
//!
//! Inputs can mix files and directories; directories are walked
//! recursively for `.ledger` files, matching the same collector
//! pattern the main pipeline uses for `-f DIR`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use colored::Colorize;

use super::util::push_spaces;
use crate::error::Error;
use crate::parser::{
    self,
    entry::Entry,
    located::Located,
    posting::Posting,
    transaction::{State, Transaction},
};

/// Leading indent on every posting and indented sub-directive line.
/// A single tab — matches the convention most ledger configs and the
/// user's vim setup already use.
const INDENT: &str = "\t";

/// Column gap in spaces between the account-name column and the
/// amount column. Separate from `INDENT` because a tab is variable-
/// width (tabstop-dependent) and would break alignment across
/// differing account-name lengths.
const GAP: usize = 8;

pub fn run(paths: &[String], no_sort: bool) -> Result<(), Error> {
    // stdin/stdout mode: `-` reads the journal from stdin and writes
    // the formatted output to stdout without touching the filesystem.
    // Used by vim's `:%!acc format -` pipe so buffer-editing stays
    // reversible under undo.
    if paths.iter().any(|p| p == "-") {
        return run_stdin_stdout(no_sort);
    }

    let files = collect_files(paths);
    let total = files.len();
    for path in files {
        let source = fs::read_to_string(&path)
            .map_err(|e| Error::from(format!("read {}: {}", path.display(), e)))?;
        let entries = parser::parse(&source)
            .map_err(|e| Error::from(format!("parse {}: {}", path.display(), e)))?;
        let formatted = render(&entries, &source, no_sort);
        write_atomic(&path, &formatted)
            .map_err(|e| Error::from(format!("write {}: {}", path.display(), e)))?;
        println!("{} {}", "✓".green(), path.display());
    }
    let label = if total == 1 { "file" } else { "files" };
    if total > 0 {
        println!();
    }
    println!("{} {} formatted", total, label);
    Ok(())
}

/// Stdin → stdout pipe mode. Parse what comes in on stdin as a
/// journal, emit the aligned render on stdout. No filesystem I/O.
fn run_stdin_stdout(no_sort: bool) -> Result<(), Error> {
    use std::io::{Read, Write};
    let mut source = String::new();
    io::stdin()
        .read_to_string(&mut source)
        .map_err(|e| Error::from(format!("read stdin: {}", e)))?;
    let entries = parser::parse(&source)
        .map_err(|e| Error::from(format!("parse stdin: {}", e)))?;
    let formatted = render(&entries, &source, no_sort);
    io::stdout()
        .write_all(formatted.as_bytes())
        .map_err(|e| Error::from(format!("write stdout: {}", e)))?;
    Ok(())
}

/// Expand each input path: files are kept verbatim, directories are
/// walked recursively for `.ledger` files, missing paths warn to
/// stderr.
fn collect_files(paths: &[String]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for p in paths {
        let path = Path::new(p);
        if path.is_file() {
            out.push(path.to_path_buf());
        } else if path.is_dir() {
            walk_dir(path, &mut out);
        } else {
            eprintln!("warning: skipping {}: not a file or directory", path.display());
        }
    }
    out
}

fn walk_dir(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mut paths: Vec<_> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            walk_dir(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("ledger") {
            out.push(path);
        }
    }
}

/// Atomic in-place write: emit to `<path>.tmp`, rename over the target
/// so a crash mid-write never leaves a half-written file.
fn write_atomic(path: &Path, content: &str) -> io::Result<()> {
    let tmp = path.with_extension("ledger.tmp");
    fs::write(&tmp, content)?;
    fs::rename(&tmp, path)
}

fn render(entries: &[Located<Entry>], source: &str, no_sort: bool) -> String {
    let source_lines: Vec<&str> = source.lines().collect();
    let (account_width, amount_width) = column_widths(entries, &source_lines);
    // Stable date-sort of transactions only. Non-transaction entries
    // (price directives, commodity blocks, top-level comments) keep
    // their original positions; the transaction slots get repopulated in
    // date order. With `no_sort`, transactions stay in source order.
    let mut sorted_transactions: Vec<&Transaction> = entries
        .iter()
        .filter_map(|e| match &e.value {
            Entry::Transaction(t) => Some(t),
            _ => None,
        })
        .collect();
    if !no_sort {
        sorted_transactions.sort_by_key(|t| t.date);
    }
    let mut transaction_iter = sorted_transactions.into_iter();

    let mut out = String::new();
    let mut prev_was_tx = false;

    for entry in entries {
        let is_tx = matches!(entry.value, Entry::Transaction(_));
        // Blank line between consecutive transactions for readability.
        if is_tx && prev_was_tx {
            out.push('\n');
        }
        match &entry.value {
            Entry::Transaction(_) => {
                let transaction = transaction_iter.next().expect(
                    "transaction slot found in entries but the sorted list \
                     was already drained — counting mismatch",
                );
                render_transaction(
                    transaction,
                    account_width,
                    amount_width,
                    &source_lines,
                    &mut out,
                );
            }
            Entry::Price(_) => {
                // Prices have no multi-line structure to align, so the
                // source line is emitted verbatim. Preserves the exact
                // rate as the user wrote it (no trailing-zero drift,
                // no precision rounding).
                if let Some(line) = source_lines.get(entry.line.saturating_sub(1)) {
                    out.push_str(line);
                    out.push('\n');
                }
            }
            Entry::Commodity { symbol, aliases, precision } => {
                out.push_str(&format!("commodity {}\n", symbol));
                for a in aliases {
                    out.push_str(INDENT);
                    out.push_str(&format!("alias {}\n", a));
                }
                if let Some(p) = precision {
                    out.push_str(INDENT);
                    out.push_str(&format!("precision {}\n", p));
                }
            }
            Entry::Account(name) => {
                out.push_str(&format!("account {}\n", name));
            }
            Entry::FxGainAccount(name) => {
                out.push_str(&format!("account {}\n", name));
                out.push_str(INDENT);
                out.push_str("fx gain\n");
            }
            Entry::FxLossAccount(name) => {
                out.push_str(&format!("account {}\n", name));
                out.push_str(INDENT);
                out.push_str("fx loss\n");
            }
            Entry::CtaGainAccount(name) => {
                out.push_str(&format!("account {}\n", name));
                out.push_str(INDENT);
                out.push_str("cta gain\n");
            }
            Entry::CtaLossAccount(name) => {
                out.push_str(&format!("account {}\n", name));
                out.push_str(INDENT);
                out.push_str("cta loss\n");
            }
            Entry::Comment(text) => {
                out.push_str(text);
                if !text.ends_with('\n') {
                    out.push('\n');
                }
            }
            Entry::AutoRule(_) => {
                // Auto-rule blocks span multiple lines (header +
                // postings) and the posting format here doesn't own
                // multipliers — simpler to emit the source lines
                // verbatim than rebuild the syntax. Find where this
                // block ends: next line that isn't indented.
                let start = entry.line.saturating_sub(1);
                let mut end = start + 1;
                while end < source_lines.len() {
                    let line = source_lines[end];
                    if line.is_empty() || !(line.starts_with('\t') || line.starts_with("  ")) {
                        break;
                    }
                    end += 1;
                }
                for i in start..end {
                    out.push_str(source_lines[i]);
                    out.push('\n');
                }
            }
        }
        prev_was_tx = is_tx;
    }
    out
}

fn render_transaction(
    tx: &Transaction,
    account_width: usize,
    amount_width: usize,
    source_lines: &[&str],
    out: &mut String,
) {
    out.push_str(&tx.date.to_string());
    match tx.state {
        State::Cleared => out.push_str(" *"),
        State::Pending => out.push_str(" !"),
        State::Uncleared => {}
    }
    if let Some(code) = &tx.code {
        out.push_str(&format!(" ({})", code));
    }
    if !tx.description.is_empty() {
        out.push(' ');
        out.push_str(&tx.description);
    }
    out.push('\n');
    for c in &tx.comments {
        out.push_str(INDENT);
        out.push_str(&format!("; {}\n", c.value.text));
    }
    for lp in &tx.postings {
        let src = source_lines.get(lp.line.saturating_sub(1)).copied();
        render_posting(&lp.value, account_width, amount_width, src, out);
    }
}

/// Emit one posting line. The account column is rendered from the
/// AST (so virtual wrappings `[...]` / `(...)` and column alignment
/// are stable), but **everything after the account separator** —
/// the amount, any `@`/`@@` cost, `{…}` lot annotation, `= ASSERT`
/// and inline `; comment` — is pulled **verbatim from the source
/// line**. This preserves expressions like `@ (€1/212)` that would
/// otherwise be re-emitted from their evaluated `Decimal` and lose
/// their source form (rendering as `€0` because the expression
/// parser records `decimals = 0`).
fn render_posting(
    p: &Posting,
    account_width: usize,
    amount_width: usize,
    source_line: Option<&str>,
    out: &mut String,
) {
    let account = render_account(p);
    let parts = source_line
        .map(extract_posting_parts)
        .unwrap_or_default();

    if parts.amount_str.is_empty() && parts.tail.is_empty() {
        // Omitted-amount posting with no tail: just the account.
        out.push_str(INDENT);
        out.push_str(&account);
        out.push('\n');
    } else {
        let account_pad = account_width.saturating_sub(account.chars().count());
        let amount_pad = amount_width.saturating_sub(parts.amount_str.chars().count());
        out.push_str(INDENT);
        out.push_str(&account);
        push_spaces(out, account_pad);
        push_spaces(out, GAP);
        push_spaces(out, amount_pad);
        out.push_str(&parts.amount_str);
        out.push_str(&parts.tail);
        out.push('\n');
    }
    for c in &p.comments {
        out.push_str(INDENT);
        out.push_str(&format!("; {}\n", c.value.text));
    }
}

/// Account column content: `account`, `(account)`, or `[account]`
/// depending on virtual flags.
fn render_account(p: &Posting) -> String {
    match (p.is_virtual, p.balanced) {
        (true, true) => format!("[{}]", p.account),
        (true, false) => format!("({})", p.account),
        (false, _) => p.account.clone(),
    }
}

/// Scan every posting in the file to determine the max rendered
/// width of the account column and the amount column. Widths drive
/// uniform padding so accounts left-align and amounts right-align
/// across the whole file. The amount width is measured on the
/// **source-side** amount string, not on the AST-rendered value,
/// so the alignment lines up with what actually gets emitted.
fn column_widths(entries: &[Located<Entry>], source_lines: &[&str]) -> (usize, usize) {
    let mut account_max = 0usize;
    let mut amount_max = 0usize;
    for entry in entries {
        if let Entry::Transaction(tx) = &entry.value {
            for lp in &tx.postings {
                let a = render_account(&lp.value);
                account_max = account_max.max(a.chars().count());
                if let Some(src) = source_lines.get(lp.line.saturating_sub(1)).copied() {
                    let parts = extract_posting_parts(src);
                    amount_max = amount_max.max(parts.amount_str.chars().count());
                }
            }
        }
    }
    (account_max, amount_max)
}

/// Decomposition of a posting source line into the amount string
/// (what gets right-aligned in the amount column) and the tail
/// (everything after the amount — cost annotations, balance
/// assertion, inline comment — kept verbatim).
#[derive(Default)]
struct PostingParts {
    amount_str: String,
    tail: String,
}

/// Given a raw posting source line (including its leading indent and
/// any inline `; comment`), extract the amount string and the tail
/// for re-emission. The account is ignored here — the caller
/// re-derives it from the AST for virtual wrapping.
///
/// Splitting rules mirror the parser:
/// - account / body separator is a tab or two-plus spaces,
/// - the amount ends at the first of `@`, `=`, `{`, `[` (all of
///   which introduce cost, assertion, or lot annotations).
fn extract_posting_parts(source_line: &str) -> PostingParts {
    let body = source_line.trim_start();
    let (body_main, comment) = strip_inline_comment(body);

    let Some((_acc, rest)) = split_body(body_main) else {
        // No separator: whole body is the account.
        return PostingParts {
            amount_str: String::new(),
            tail: if comment.is_empty() {
                String::new()
            } else {
                format!("  {}", comment)
            },
        };
    };

    let amount_end = rest
        .find(|c: char| matches!(c, '@' | '=' | '{' | '['))
        .unwrap_or(rest.len());
    let amount_str = normalise_commodity_glue(rest[..amount_end].trim());
    let annotation = rest[amount_end..].trim();

    let mut tail = String::new();
    if !annotation.is_empty() {
        tail.push(' ');
        tail.push_str(annotation);
    }
    if !comment.is_empty() {
        if tail.is_empty() {
            tail.push_str("  ");
        } else {
            tail.push(' ');
        }
        tail.push_str(comment);
    }

    PostingParts {
        amount_str: amount_str.to_string(),
        tail,
    }
}

/// Glue the commodity symbol directly onto the number, dropping
/// any whitespace between them. Turns `DZD -90000.00` into
/// `DZD-90000.00` and leaves `DZD-20000.00` / `$5.00` untouched.
/// Only touches leading-commodity amounts; trailing-commodity form
/// like `100 EUR` stays as-is.
fn normalise_commodity_glue(s: &str) -> String {
    let Some(idx) = s.find(|c: char| c.is_ascii_digit() || c == '-' || c == '.') else {
        return s.to_string();
    };
    let commodity = s[..idx].trim();
    if commodity.is_empty() {
        return s.to_string();
    }
    let number = &s[idx..];
    format!("{}{}", commodity, number)
}

/// Separate account column from the rest of a posting body. Matches
/// the parser's rule: tab or run of 2+ spaces. Single spaces stay
/// inside the account name (e.g. `expenses: food`).
fn split_body(body: &str) -> Option<(&str, &str)> {
    if let Some(idx) = body.find('\t') {
        return Some((body[..idx].trim_end(), body[idx..].trim_start()));
    }
    if let Some(idx) = body.find("  ") {
        return Some((body[..idx].trim_end(), body[idx..].trim_start()));
    }
    None
}

/// Split off an inline `;` comment if present, ignoring `;` inside
/// parenthesised expressions (so `@ (foo;bar)` — unlikely but
/// possible — doesn't get cut). Returns (main-body-trimmed,
/// comment-with-leading-`;`-kept).
fn strip_inline_comment(body: &str) -> (&str, &str) {
    let mut depth = 0i32;
    for (i, c) in body.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ';' if depth <= 0 => {
                return (body[..i].trim_end(), &body[i..]);
            }
            _ => {}
        }
    }
    (body.trim_end(), "")
}
