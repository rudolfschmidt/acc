//! `format` command — validate, then re-emit aligned.
//!
//! Semantics: **print --raw but aligned**. The whole input is first run
//! through the full load pipeline (parse → resolve → book), the same
//! checks `acc reg` applies, so structurally broken input is reported
//! instead of silently reformatted — a single space between account and
//! amount that collapses into one token, an unbalanced transaction, a
//! failed assertion. The aligned *output* is still produced verbatim
//! from source (missing amounts stay missing, nothing is recomputed,
//! comments and directives are preserved, transactions keep their
//! source order unless `--sort` is given).
//!
//! All-or-nothing: if any file fails validation, nothing is written —
//! no half-formatted batches.
//!
//! Writes back in place. Atomic: each file is written to
//! `<path>.ledger.tmp` and then renamed over the target, so a crash
//! mid-write never leaves a half-written file.
//!
//! Inputs can mix files and directories; directories are walked
//! recursively for journal files (`.ledger` only), matching the same
//! collector pattern the main pipeline uses for `-f DIR`. Files named
//! explicitly are kept regardless of extension.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use colored::Colorize;

use super::util::{push_spaces, render_account};
use crate::decimal::Decimal;
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

pub fn run(paths: &[String], sort: bool, infer: bool, fill: bool) -> Result<(), Error> {
    // stdin/stdout mode: `-` reads the journal from stdin and writes
    // the formatted output to stdout without touching the filesystem.
    // Used by vim's `:%!acc format -` pipe so buffer-editing stays
    // reversible under undo.
    if paths.iter().any(|p| p == "-") {
        return run_stdin_stdout(sort, infer, fill);
    }

    let files = collect_files(paths);

    // All-or-nothing: validate the entire set through the full pipeline
    // (parse → resolve → book) — the same checks `acc reg` runs — before
    // writing anything. A plain parse accepts structurally broken input
    // that the booker rejects: a single space between account and amount
    // collapses both into one account token (leaving two amount-less
    // postings), an unbalanced transaction, a failed assertion. Catching
    // it here means no file is ever rewritten from a journal that
    // wouldn't load, and a single error aborts the whole run so there
    // are never half-formatted batches.
    crate::load(&files).map_err(|e| Error::from(e.to_string()))?;

    let total = files.len();
    for path in &files {
        format_in_place(path, sort, infer, fill)?;
        println!("{} {}", "✓".green(), path.display());
    }
    let label = if total == 1 { "file" } else { "files" };
    if total > 0 {
        println!();
    }
    println!("{} {} {} formatted", "✓".green(), total, label);
    Ok(())
}

/// Format a single file in place — read, parse, render aligned, write
/// atomically — without printing anything. The caller is responsible for
/// validation (this skips the `load` check that `run` does up front).
/// Used by `sweep` to align its generated file silently.
pub fn format_in_place(path: &Path, sort: bool, infer: bool, fill: bool) -> Result<(), Error> {
    let source = fs::read_to_string(path)
        .map_err(|e| Error::from(format!("read {}: {}", path.display(), e)))?;
    let entries = parser::parse(&source)
        .map_err(|e| Error::from(format!("parse {}: {}", path.display(), e)))?;
    let formatted = render(&entries, &source, sort, infer, fill);
    write_atomic(path, &formatted)
        .map_err(|e| Error::from(format!("write {}: {}", path.display(), e)))
}

/// Format a journal source string in memory — parse + render (align, and
/// optionally date-sort) — returning the canonical text. No filesystem
/// I/O. Used by `sweep` to emit already-aligned entries on stdout.
pub fn format_source(source: &str, sort: bool) -> Result<String, Error> {
    let entries =
        parser::parse(source).map_err(|e| Error::from(format!("parse: {}", e)))?;
    // sweep / import never infer or fill — they render exactly what they
    // generated.
    Ok(render(&entries, source, sort, false, false))
}

/// Stdin → stdout pipe mode. Parse what comes in on stdin as a
/// journal, emit the aligned render on stdout. No filesystem I/O.
fn run_stdin_stdout(sort: bool, infer: bool, fill: bool) -> Result<(), Error> {
    use std::io::{Read, Write};
    let mut source = String::new();
    io::stdin()
        .read_to_string(&mut source)
        .map_err(|e| Error::from(format!("read stdin: {}", e)))?;

    // Full validation, same contract as the file path. On error, echo
    // the input back unchanged so a `:%!acc format -` pipe in vim never
    // replaces the buffer with a half-formatted or empty result, and
    // report the problem on stderr.
    if let Err(e) = validate_source(&source) {
        io::stdout().write_all(source.as_bytes()).ok();
        return Err(e);
    }

    let entries = parser::parse(&source)
        .map_err(|e| Error::from(format!("parse stdin: {}", e)))?;
    let formatted = render(&entries, &source, sort, infer, fill);
    io::stdout()
        .write_all(formatted.as_bytes())
        .map_err(|e| Error::from(format!("write stdout: {}", e)))?;
    Ok(())
}

/// Run the full pipeline (parse → resolve → book) over an in-memory
/// source and return the first error. Used for stdin, where the bytes
/// are already consumed so `load` (which reads from disk) can't be
/// reused. File inputs validate via `load` directly.
fn validate_source(source: &str) -> Result<(), Error> {
    let entries = parser::parse_with_file(source, std::sync::Arc::from("<stdin>"))
        .map_err(|e| Error::from(format!("parse stdin: {}", e)))?;
    let resolved =
        crate::resolver::resolve(entries).map_err(|e| Error::from(e.to_string()))?;
    crate::booker::book(resolved.transactions).map_err(|e| Error::from(e.to_string()))?;
    Ok(())
}

/// Expand each input path: files are kept verbatim (extension is not
/// checked — explicit `-f FILE` is honoured as-is), directories are
/// walked recursively for journal files (see `JOURNAL_EXTENSIONS`),
/// missing paths warn to stderr.
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
        } else if crate::is_journal_file(&path) {
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

fn render(entries: &[Located<Entry>], source: &str, sort: bool, infer: bool, fill: bool) -> String {
    let source_lines: Vec<&str> = source.lines().collect();
    let (account_width, amount_width) = column_widths(entries, &source_lines);
    // Transactions keep their source order by default. With `sort`,
    // stably date-sort them: non-transaction entries (price directives,
    // commodity blocks, top-level comments) keep their original
    // positions and the transaction slots get repopulated in date order.
    let mut sorted_transactions: Vec<&Transaction> = entries
        .iter()
        .filter_map(|e| match &e.value {
            Entry::Transaction(t) => Some(t),
            _ => None,
        })
        .collect();
    if sort {
        sorted_transactions.sort_by_key(|t| t.date);
    }
    let mut transaction_iter = sorted_transactions.into_iter();

    let mut out = String::new();
    let mut prev_was_tx = false;
    let mut prev_was_comment = false;
    let mut first = true;

    for entry in entries {
        let is_tx = matches!(entry.value, Entry::Transaction(_));
        let is_comment = matches!(entry.value, Entry::Comment(_));
        // Blank line for readability: between consecutive transactions,
        // and at the boundary between a comment block and the surrounding
        // content (so a commented-out transaction gets breathing room).
        // Never before the first entry, and not between adjacent comment
        // lines — a multi-line comment block stays together.
        if !first && ((is_tx && prev_was_tx) || (is_comment != prev_was_comment)) {
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
                    infer,
                    fill,
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
            Entry::Commodity { symbol, aliases, parities, precision } => {
                out.push_str(&format!("commodity {}\n", symbol));
                for a in aliases {
                    out.push_str(INDENT);
                    out.push_str(&format!("alias {}\n", a));
                }
                for t in parities {
                    out.push_str(INDENT);
                    out.push_str(&format!("parity {}\n", t));
                }
                if let Some(p) = precision {
                    out.push_str(INDENT);
                    out.push_str(&format!("precision {}\n", p));
                }
            }
            Entry::Account(name) => {
                out.push_str(&format!("account {}\n", name));
            }
            Entry::RoleAccount { role, account } => {
                out.push_str(&format!("account {}\n", account));
                out.push_str(INDENT);
                out.push_str(role);
                out.push('\n');
            }
            Entry::Comment(text) => {
                out.push_str(text);
                if !text.ends_with('\n') {
                    out.push('\n');
                }
            }
            // Auto-rule / template / periodic blocks span multiple lines (header
            // + indented children); their body here needs no re-derivation —
            // simpler to emit the source lines verbatim than rebuild the syntax.
            // The block ends at the next line that isn't indented.
            Entry::AutoRule(_) | Entry::AutoTemplate { .. } | Entry::Periodic { .. } => {
                let start = entry.line.saturating_sub(1);
                let mut end = start + 1;
                while end < source_lines.len() {
                    let line = source_lines[end];
                    if line.is_empty() || !(line.starts_with('\t') || line.starts_with("  ")) {
                        break;
                    }
                    end += 1;
                }
                for &line in &source_lines[start..end] {
                    out.push_str(line);
                    out.push('\n');
                }
            }
            Entry::AutoInstance { .. } | Entry::Lookup { .. } => {
                // Single-line directives (`= NAME a b`, `= NAME[key] :: value`);
                // emit verbatim.
                if let Some(line) = source_lines.get(entry.line.saturating_sub(1)) {
                    out.push_str(line);
                    out.push('\n');
                }
            }
        }
        prev_was_tx = is_tx;
        prev_was_comment = is_comment;
        first = false;
    }
    out
}

/// Whether `--infer` may drop the last amount of this transaction:
/// exactly two real postings with explicit amounts in the same commodity,
/// the last bearing no cost / lot / assertion that would make its amount
/// significant. The dropped amount then auto-balances against the first.
fn infers_last(tx: &Transaction) -> bool {
    let [a, b] = &tx.postings[..] else {
        return false;
    };
    let (a, b) = (&a.value, &b.value);
    match (&a.amount, &b.amount) {
        (Some(av), Some(bv)) => {
            av.commodity == bv.commodity
                && !a.is_virtual
                && !b.is_virtual
                && b.costs.is_none()
                && b.lot_cost.is_none()
                && b.lot_date.is_none()
                && b.balance_assertion.is_none()
        }
        _ => false,
    }
}

/// `--fill`: the inverse of `--infer`. When a transaction has more than two
/// postings, exactly one of them lacks an amount, and every other posting
/// is real (non-virtual) in the same commodity with no cost, the missing
/// amount is the negated sum of the rest — compute it so it can be written
/// out explicitly. Returns (index of the empty posting, rendered amount).
fn fills(tx: &Transaction) -> Option<(usize, String)> {
    if tx.postings.len() <= 2 {
        return None;
    }
    if tx.postings.iter().any(|lp| lp.value.is_virtual) {
        return None;
    }
    let mut empty: Option<usize> = None;
    let mut commodity: Option<&str> = None;
    let mut sum = Decimal::zero();
    let mut decimals = 0usize;
    for (i, lp) in tx.postings.iter().enumerate() {
        let p = &lp.value;
        match &p.amount {
            None => {
                if empty.is_some() {
                    return None; // two empty postings — ambiguous
                }
                empty = Some(i);
            }
            Some(a) => {
                if p.costs.is_some() || p.lot_cost.is_some() {
                    return None; // a cost changes the balance maths
                }
                match commodity {
                    None => commodity = Some(a.commodity.as_str()),
                    Some(c) if c == a.commodity.as_str() => {}
                    _ => return None, // mixed commodities
                }
                sum += a.value;
                decimals = decimals.max(a.decimals);
            }
        }
    }
    let idx = empty?;
    let commodity = commodity?;
    Some((idx, render_amount(commodity, &-sum, decimals)))
}

/// Render `commodity` + `value` at `decimals`, dropping a `-0` sign.
fn render_amount(commodity: &str, value: &Decimal, decimals: usize) -> String {
    let body = value.format_decimal(decimals);
    let body = match body.strip_prefix('-') {
        Some(rest) if rest.chars().all(|c| c == '0' || c == '.') => rest.to_string(),
        _ => body,
    };
    format!("{}{}", commodity, body)
}

fn render_transaction(
    tx: &Transaction,
    account_width: usize,
    amount_width: usize,
    source_lines: &[&str],
    infer: bool,
    fill: bool,
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
    let infer_last = infer && infers_last(tx);
    let filled = if fill { fills(tx) } else { None };
    let last = tx.postings.len().saturating_sub(1);
    for (i, lp) in tx.postings.iter().enumerate() {
        let src = source_lines.get(lp.line.saturating_sub(1)).copied();
        let infer_amount = infer_last && i == last;
        let fill_amount = filled
            .as_ref()
            .filter(|(idx, _)| *idx == i)
            .map(|(_, s)| s.as_str());
        render_posting(
            &lp.value,
            lp.line,
            account_width,
            amount_width,
            src,
            infer_amount,
            fill_amount,
            out,
        );
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
    posting_line: usize,
    account_width: usize,
    amount_width: usize,
    source_line: Option<&str>,
    infer_amount: bool,
    fill: Option<&str>,
    out: &mut String,
) {
    let account = render_account(p);
    let parts = source_line
        .map(extract_posting_parts)
        .unwrap_or_default();

    if infer_amount {
        // `--infer`: drop the redundant balancing amount, keeping
        // the account and any inline comment (the tail carries no cost /
        // assertion here — `infers_last` excluded those) so it auto-balances.
        out.push_str(INDENT);
        out.push_str(&account);
        out.push_str(&parts.tail);
        out.push('\n');
    } else if let Some(amount) = fill {
        // `--fill`: write the computed balancing amount on the otherwise
        // empty posting, aligned like any other. The source line carries no
        // amount, only an optional assertion / comment — kept as the tail.
        let account_pad = account_width.saturating_sub(account.chars().count());
        let amount_pad = amount_width.saturating_sub(amount.chars().count());
        out.push_str(INDENT);
        out.push_str(&account);
        push_spaces(out, account_pad);
        push_spaces(out, GAP);
        push_spaces(out, amount_pad);
        out.push_str(amount);
        out.push_str(&parts.tail);
        out.push('\n');
    } else if parts.amount_str.is_empty() && parts.tail.is_empty() {
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
        // Inline comments share the posting's source line and are already
        // re-emitted verbatim in the tail by `extract_posting_parts`.
        // Only own-line comments that follow the posting (a different
        // source line) render here — otherwise the inline comment is
        // duplicated, once in the tail and once as a standalone line.
        if c.line == posting_line {
            continue;
        }
        out.push_str(INDENT);
        out.push_str(&format!("; {}\n", c.value.text));
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
        .find(['@', '=', '{', '['])
        .unwrap_or(rest.len());
    let amount_str = normalise_commodity_glue(rest[..amount_end].trim());
    let annotation = rest[amount_end..].trim();

    let mut tail = String::new();
    if !annotation.is_empty() {
        tail.push(' ');
        tail.push_str(annotation);
    }
    if !comment.is_empty() {
        // Two spaces before an inline comment, whether or not a `@@`/`=`/`{`
        // annotation precedes it — the annotation branch used to emit only one.
        tail.push_str("  ");
        tail.push_str(comment);
    }

    PostingParts {
        amount_str: amount_str.to_string(),
        tail,
    }
}

/// Glue the commodity symbol directly onto the number, dropping
/// any whitespace between them. Turns `USD -1200.00` into
/// `USD-1200.00` and leaves `USD-300.00` / `$5.00` untouched.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_comment_keeps_two_spaces_with_and_without_annotation() {
        // With a `@@` cost annotation before the comment: two spaces, not one.
        let p = extract_posting_parts("\trud:11:a  XMR4.314 @@ LTC10.38  ; €1000.00");
        assert_eq!(p.amount_str, "XMR4.314");
        assert_eq!(p.tail, " @@ LTC10.38  ; €1000.00");
        // A one-space source is normalised up to two.
        let p = extract_posting_parts("\trud:11:a  XMR4.314 @@ LTC10.38 ; €1000.00");
        assert_eq!(p.tail, " @@ LTC10.38  ; €1000.00");
        // Plain amount + comment (no annotation): still two spaces.
        let p = extract_posting_parts("\trud:11:a  XMR4.314  ; note");
        assert_eq!(p.tail, "  ; note");
    }
}
