//! Journal parser.
//!
//! Pure text-to-records transformation. Takes a source string, produces a
//! flat `Vec<Located<Entry>>`. No I/O, no shared state, no alias resolution,
//! no index building — every interpretive step happens in later phases.

pub mod comment;
pub mod entry;
pub mod error;
pub mod expression;
pub mod located;
pub mod posting;
pub mod transaction;

pub use comment::Comment;
pub use entry::{Entry, Price};
pub use error::ParseError;

pub use located::Located;
pub use posting::{Amount, Costs, LotCost, Posting};
pub use transaction::{State, Transaction};

use std::collections::HashSet;
use std::sync::Arc;

/// Parse a journal source string into a flat stream of entries.
///
/// The returned vec preserves source order. Each entry carries the line
/// number of its top-level directive. Sub-records (postings, aliases,
/// role gain/loss sub-directives) are folded into the enclosing entry rather than
/// appearing on their own — this keeps the parser state-less between
/// lines; the "current block" is simply `entries.last_mut()`.
///
/// Convenience wrapper for callers without file provenance (tests).
/// Emits `Located` records with an empty file path.
pub fn parse(source: &str) -> Result<Vec<Located<Entry>>, ParseError> {
    parse_with_file(source, Arc::from(""))
}

/// Same as [`parse`] but tags every `Located` with the given file path.
/// The loader uses this so that downstream error messages can name the
/// source file.
pub fn parse_with_file(
    source: &str,
    file: Arc<str>,
) -> Result<Vec<Located<Entry>>, ParseError> {
    parse_impl(source, file, None)
}

/// Like [`parse_with_file`], but keeps a `P` directive only when BOTH its
/// base and quote are in `needed`. Used for selective price loading: the
/// journal's held commodities + the `-X` target (with their alias forms)
/// drive `needed`, so the ~800k price DB is filtered at parse time and the
/// expensive per-price work (date/decimal parse, interning, indexing) is
/// skipped for every pair the report can't reach.
pub fn parse_with_file_filtered(
    source: &str,
    file: Arc<str>,
    needed: &HashSet<String>,
) -> Result<Vec<Located<Entry>>, ParseError> {
    parse_impl(source, file, Some(needed))
}

fn parse_impl(
    source: &str,
    file: Arc<str>,
    prices_filter: Option<&HashSet<String>>,
) -> Result<Vec<Located<Entry>>, ParseError> {
    // Heuristic: a typical ledger line is ~30 bytes. Reserving up front
    // prevents repeated reallocation as the vec grows on price-heavy
    // inputs (which dominate the real-world workload).
    let mut entries = Vec::with_capacity(source.len() / 30);
    // Intern commodity symbols while parsing. A real price-heavy journal
    // repeats ~200 symbols across 800k+ `P` directives; allocating a fresh
    // `Arc<str>` per directive (which the resolver then dedups and throws
    // away) was the load-time bottleneck. Interning here collapses that to
    // one allocation per distinct symbol.
    let mut commodities: HashSet<Arc<str>> = HashSet::new();
    for (idx, text) in source.lines().enumerate() {
        dispatch(text, idx + 1, &file, &mut commodities, prices_filter, &mut entries)?;
    }
    Ok(entries)
}

/// Intern a commodity symbol: one shared `Arc<str>` per distinct string.
fn intern(commodities: &mut HashSet<Arc<str>>, s: &str) -> Arc<str> {
    if let Some(existing) = commodities.get(s) {
        return existing.clone();
    }
    let arc: Arc<str> = Arc::from(s);
    commodities.insert(arc.clone());
    arc
}

/// **The journal's one indentation rule.** A line is *nested under* the
/// directive above it when it starts with a **tab** or **two or more spaces**
/// (Ledger's convention). This is the single definition used everywhere a line
/// continues an entry: transaction postings and periodic (`~`) postings. Keep
/// this the sole source of truth so the "tab or spaces, how deep?" question
/// never recurs per feature.
fn is_indented(bytes: &[u8]) -> bool {
    matches!(bytes, [b'\t', ..] | [b' ', b' ', ..])
}

fn dispatch(
    text: &str,
    line: usize,
    file: &Arc<str>,
    commodities: &mut HashSet<Arc<str>>,
    prices_filter: Option<&HashSet<String>>,
    entries: &mut Vec<Located<Entry>>,
) -> Result<(), ParseError> {
    let bytes = text.as_bytes();
    match bytes {
        [] => Ok(()),
        // A nested line (see `is_indented`) continues the entry above it.
        _ if is_indented(bytes) => extend_block(text, line, file, entries),
        [b'0'..=b'9', ..] => parse_transaction(text, line, file, entries),
        [b'P', b' ', ..] => parse_price(&text[2..], line, file, commodities, prices_filter, entries),
        [b';' | b'#', ..] => {
            entries.push(Located {
                file: file.clone(),
                line,
                value: Entry::Comment(text.to_string()),
            });
            Ok(())
        }
        [b'~', b' ', ..] | [b'~', b'\t', ..] => {
            parse_periodic(text[1..].trim_start(), line, file, entries)
        }
        // A `-f` file is a pure ledger: transactions, prices, comments and
        // periodics. Commodities / accounts / roles / labels come from the
        // `CONFIG_DIR` TOML (prepended as synthetic entries before parsing); the
        // `=` automated-transaction system was removed — an embedder injects
        // programmatically-built transactions instead.
        _ => {
            let keyword = text.split_whitespace().next().unwrap_or(text);
            Err(ParseError::new(line, 1, format!("unknown directive: {}", keyword)))
        }
    }
}

/// Parse a `P DATE BASE QUOTE RATE` directive. The leading `P ` has
/// already been stripped by the dispatcher; `rest` is the remainder.
fn parse_price(
    rest: &str,
    line: usize,
    file: &Arc<str>,
    commodities: &mut HashSet<Arc<str>>,
    prices_filter: Option<&HashSet<String>>,
    entries: &mut Vec<Located<Entry>>,
) -> Result<(), ParseError> {
    let mut tokens = rest.split_whitespace();

    let date_str = tokens
        .next()
        .ok_or_else(|| ParseError::new(line, 1, "price directive missing date"))?;
    let base = tokens
        .next()
        .ok_or_else(|| ParseError::new(line, 1, "price directive missing base commodity"))?;
    let quote = tokens
        .next()
        .ok_or_else(|| ParseError::new(line, 1, "price directive missing quote commodity"))?;
    let rate_str = tokens
        .next()
        .ok_or_else(|| ParseError::new(line, 1, "price directive missing rate"))?;
    if tokens.next().is_some() {
        return Err(ParseError::new(line, 1, "price directive has extra tokens"));
    }

    // Selective loading: a price is only useful if BOTH its commodities can
    // appear in a conversion the report needs. Drop it before the expensive
    // date/decimal parse otherwise. `needed` already carries every alias
    // form, so the raw symbols here match directly.
    if let Some(needed) = prices_filter
        && !(needed.contains(base) && needed.contains(quote))
    {
        return Ok(());
    }

    let date = crate::date::Date::parse(date_str)
        .map_err(|e| ParseError::new(line, 1, e))?;
    let rate = crate::decimal::Decimal::parse(rate_str)
        .map_err(|e| ParseError::new(line, 1, format!("invalid rate: {}", e)))?;

    entries.push(Located {
        file: file.clone(),
        line,
        value: Entry::Price(Price {
            date,
            base: intern(commodities, base),
            quote: intern(commodities, quote),
            rate,
        }),
    });
    Ok(())
}

/// Parse a `~ PERIOD` header — a periodic transaction. Only a bare year
/// (`~ 2021`, optionally `~ 2021 description`) is supported; it expands in
/// resolve into one real transaction dated `YYYY-01-01`. Indented postings
/// attach via `extend_block`, parsed like a transaction's; `$year`/`$month`/
/// `$day` in an account are filled from that date at resolve time.
fn parse_periodic(
    rest: &str,
    line: usize,
    file: &Arc<str>,
    entries: &mut Vec<Located<Entry>>,
) -> Result<(), ParseError> {
    use crate::parser::entry::Cadence;
    let rest = rest.trim();
    if rest.is_empty() {
        return Err(ParseError::new(line, 1, "periodic `~` block needs a period, e.g. `~ 2021`"));
    }
    let (period, after) = rest
        .split_once(char::is_whitespace)
        .map(|(p, d)| (p.trim(), d.trim()))
        .unwrap_or((rest, ""));
    // Optional cadence keyword right after the period; whatever follows (or the
    // whole remainder, if it isn't a cadence word) is the title.
    let (cadence, description) = match after.split_once(char::is_whitespace) {
        Some((word, tail)) => match cadence_keyword(word) {
            Some(c) => (c, tail.trim()),
            None => (Cadence::Yearly, after),
        },
        None => match cadence_keyword(after) {
            Some(c) => (c, ""),
            None => (Cadence::Yearly, after),
        },
    };
    entries.push(Located {
        file: file.clone(),
        line,
        value: Entry::Periodic {
            period: period.to_string(),
            cadence,
            description: description.to_string(),
            postings: Vec::new(),
        },
    });
    Ok(())
}

/// The optional cadence keyword after a `~ PERIOD` header (`monthly` / `daily`
/// / `yearly`); `None` for anything else, which the caller treats as the title.
fn cadence_keyword(word: &str) -> Option<crate::parser::entry::Cadence> {
    use crate::parser::entry::Cadence;
    match word {
        "yearly" => Some(Cadence::Yearly),
        "monthly" => Some(Cadence::Monthly),
        "daily" => Some(Cadence::Daily),
        _ => None,
    }
}

/// Parse a transaction header. Format:
///   DATE [=AUXDATE] [* | !] [(CODE)] [DESCRIPTION]
///
/// The aux-date after `=` is consumed but discarded — acc does not use
/// it. The body (postings, comments) is attached later by
/// `extend_block` as subsequent indented lines arrive.
fn parse_transaction(
    text: &str,
    line: usize,
    file: &Arc<str>,
    entries: &mut Vec<Located<Entry>>,
) -> Result<(), ParseError> {
    let (date_field, after_date) = text
        .split_once(char::is_whitespace)
        .unwrap_or((text, ""));
    // Primary date only; strip `=AUXDATE` if present.
    let date_str = date_field.split('=').next().unwrap();
    let date = crate::date::Date::parse(date_str)
        .map_err(|e| ParseError::new(line, 1, e))?;

    let rest = after_date.trim_start();
    let (state, rest) = parse_state(rest);
    let rest = rest.trim_start();
    let (code, rest) = parse_code(rest, line)?;
    let description = rest.trim().to_string();

    entries.push(Located {
        file: file.clone(),
        line,
        value: Entry::Transaction(Transaction {
            date,
            state,
            code,
            description,
            // Minimum balanced transaction has 2 postings; pre-allocate
            // to avoid the first regrow in the common case.
            postings: Vec::with_capacity(2),
            comments: Vec::new(),
        }),
    });
    Ok(())
}

fn parse_state(rest: &str) -> (State, &str) {
    match rest.as_bytes().first() {
        Some(b'*') => (State::Cleared, &rest[1..]),
        Some(b'!') => (State::Pending, &rest[1..]),
        _ => (State::Uncleared, rest),
    }
}

fn parse_code(rest: &str, line: usize) -> Result<(Option<String>, &str), ParseError> {
    if !rest.starts_with('(') {
        return Ok((None, rest));
    }
    let close = rest
        .find(')')
        .ok_or_else(|| ParseError::new(line, 1, "unclosed transaction code"))?;
    let code = &rest[1..close];
    let after = &rest[close + 1..];
    Ok((if code.is_empty() { None } else { Some(code.to_string()) }, after))
}

/// Attach the content of an indented line to the last emitted entry:
///
/// - under `Transaction` → a posting or an indented comment
/// - under `Periodic` → a posting
/// - anything else       → error (indented line with no valid parent)
fn extend_block(
    text: &str,
    line: usize,
    file: &Arc<str>,
    entries: &mut Vec<Located<Entry>>,
) -> Result<(), ParseError> {
    let Some(last) = entries.last_mut() else {
        return Err(ParseError::new(line, 1, "indented directive not expected here"));
    };
    let content = text.trim_start();

    // Whole-line comment under the block (e.g. `    ; note on the tx`).
    // For Transactions: if a posting already exists, the comment
    // attaches to that last posting (ledger-cli convention — a
    // comment that follows a posting belongs to it). Otherwise it
    // is a transaction-level comment that renders before any
    // posting. Comments under non-Transaction entries are dropped.
    if content.starts_with(';') || content.starts_with('#') {
        if let Entry::Transaction(tx) = &mut last.value {
            let comment = Located {
                file: file.clone(),
                line,
                value: Comment { text: content[1..].trim().to_string() },
            };
            if let Some(last_posting) = tx.postings.last_mut() {
                last_posting.value.comments.push(comment);
            } else {
                tx.comments.push(comment);
            }
        }
        return Ok(());
    }

    // Mid-line `;` is an inline comment. Split it off once, centrally —
    // every sub-parser below sees clean content.
    let (body, inline_comment) = match content.find(';') {
        Some(i) => (content[..i].trim_end(), Some(content[i + 1..].trim().to_string())),
        None => (content, None),
    };

    match &mut last.value {
        Entry::Transaction(tx) => {
            let mut posting = parse_posting(body, line)?;
            if let Some(text) = inline_comment {
                posting.comments.push(Located {
                    file: file.clone(),
                    line,
                    value: Comment { text },
                });
            }
            tx.postings.push(Located {
                file: file.clone(),
                line,
                value: posting,
            });
        }
        Entry::Periodic { postings, .. } => {
            // Periodic postings are ordinary postings (real, auto-filling),
            // parsed exactly like a transaction's; `$year`/`$month`/`$day` in
            // the account are filled at resolve time, not here.
            let mut posting = parse_posting(body, line)?;
            if let Some(text) = inline_comment {
                posting.comments.push(Located {
                    file: file.clone(),
                    line,
                    value: Comment { text },
                });
            }
            postings.push(Located { file: file.clone(), line, value: posting });
        }
        _ => return Err(ParseError::new(line, 1, "indented directive not expected here")),
    }

    Ok(())
}

/// Parse the body of a posting line. `body` has already had its indent
/// and any inline `;` comment stripped by the caller.
///
/// Format: `[(|[]ACCOUNT[)|]]  AMOUNT [@ COST | @@ TOTAL] [= ASSERTION]`
///
/// Account and amount are separated by **tab or two-plus spaces** — a
/// single space stays part of the account name.
fn parse_posting(body: &str, line: usize) -> Result<Posting, ParseError> {
    // Virtual-posting wrapping: `(account)` is virtual unbalanced,
    // `[account]` is virtual balanced. Plain account is real balanced.
    let (is_virtual, balanced, account, rest) = extract_account(body, line)?;
    let rest = rest.trim_start();

    // Balance-assertion-only posting: `= AMOUNT` with no posting amount.
    let (amount, rest) = if rest.is_empty() || rest.starts_with('=') {
        (None, rest)
    } else {
        let (amt, after) = parse_amount(rest, line)?;
        (Some(amt), after.trim_start())
    };

    // Ledger lot annotations. The first `{COST}` / `{{TOTAL}}` is
    // captured as `lot_cost` (the booker's balance-effective value,
    // overriding any `@` market cost); the first `[DATE]` as `lot_date`
    // (display only). `(NOTE)` and extra groups are dropped.
    let (lot_cost, lot_date, rest) = consume_lot_annotations(rest, line)?;

    // `@@` total-cost, `@` per-unit-cost.
    let (costs, rest) = if let Some(after) = rest.strip_prefix("@@") {
        let (amt, tail) = parse_amount(after.trim_start(), line)?;
        (Some(Costs::Total(amt)), tail.trim_start())
    } else if let Some(after) = rest.strip_prefix('@') {
        let (amt, tail) = parse_amount(after.trim_start(), line)?;
        (Some(Costs::PerUnit(amt)), tail.trim_start())
    } else {
        (None, rest)
    };

    // Lot annotations can also trail the cost clause — rare but
    // valid in Ledger. Capture a lot date here too if the pre-cost
    // slot had none; the lot cost was already taken above.
    let (_, lot_date_post, rest) = consume_lot_annotations(rest, line)?;
    let lot_date = lot_date.or(lot_date_post);

    // A written `[date]` is only meaningful pinned to a written `{cost}`
    // (a specific lot). On its own, the lotter would overwrite it with the
    // FIFO acquisition date the moment it realizes a gain on this posting,
    // silently discarding what the user wrote. Reject the bare form so the
    // surprise can't happen — a lot date must accompany a lot cost.
    if lot_date.is_some() && lot_cost.is_none() {
        return Err(ParseError::new(
            line,
            1,
            "a lot date `[…]` requires a lot cost `{…}` or `{{…}}`",
        ));
    }

    // `= AMOUNT` balance assertion.
    let balance_assertion = if let Some(after) = rest.strip_prefix('=') {
        let (amt, _) = parse_amount(after.trim_start(), line)?;
        Some(amt)
    } else {
        None
    };

    // Virtual postings must carry either an explicit amount or a
    // balance assertion — they cannot be the "missing amount"
    // inference target (unbalanced virtuals are skipped by the
    // balancer, balanced virtuals still need a concrete value).
    if is_virtual && amount.is_none() && balance_assertion.is_none() {
        return Err(ParseError::new(
            line,
            1,
            "virtual posting must have an amount or balance assertion",
        ));
    }

    Ok(Posting {
        account,
        amount,
        costs,
        lot_cost,
        lot_date,
        balance_assertion,
        is_virtual,
        balanced,
        comments: Vec::new(),
    })
}

/// Consume zero or more Ledger lot annotations from the start of `rest`.
/// Returns the first lot cost and first lot date captured (if any) and
/// the remaining slice.
///
/// Recognised forms:
///
/// - `{COST}` / `{=COST}` → per-unit lot cost (`total: false`); only the
///   first lot group is kept, later `{…}` groups are discarded.
/// - `{{TOTAL}}` / `{{=TOTAL}}` → whole-lot total cost (`total: true`).
/// - `[DATE]`   → lot acquisition date, kept for display (the caller
///   rejects it unless a lot cost accompanies it).
fn consume_lot_annotations(
    mut rest: &str,
    line: usize,
) -> Result<(Option<LotCost>, Option<crate::date::Date>, &str), ParseError> {
    let mut lot_cost: Option<LotCost> = None;
    let mut lot_date: Option<crate::date::Date> = None;
    loop {
        rest = rest.trim_start();
        let bytes = rest.as_bytes();
        match bytes.first() {
            Some(b'{') => {
                // `{{TOTAL}}` — total (whole-lot) cost.
                if bytes.get(1) == Some(&b'{') {
                    let close = find_double_close(bytes).ok_or_else(|| {
                        ParseError::new(line, 1, "unclosed `{{` in lot annotation")
                    })?;
                    if lot_cost.is_none() {
                        let inner = rest[2..close].trim();
                        let (cost_text, fixed) = match inner.strip_prefix('=') {
                            Some(s) => (s.trim(), true),
                            None => (inner, false),
                        };
                        let (amt, _) = parse_amount(cost_text, line)?;
                        lot_cost = Some(LotCost { amount: amt, total: true, fixed });
                    }
                    rest = &rest[close + 2..];
                    continue;
                }
                // `{COST}` / `{=COST}` — per-unit cost.
                let end = find_matching_brace(bytes, b'{', b'}').ok_or_else(|| {
                    ParseError::new(line, 1, "unclosed `{` in lot annotation")
                })?;
                if lot_cost.is_none() {
                    let inner = rest[1..end].trim();
                    let (cost_text, fixed) = match inner.strip_prefix('=') {
                        Some(s) => (s.trim(), true),
                        None => (inner, false),
                    };
                    let (amt, _) = parse_amount(cost_text, line)?;
                    lot_cost = Some(LotCost { amount: amt, total: false, fixed });
                }
                rest = &rest[end + 1..];
            }
            Some(b'[') => {
                let end = find_matching_brace(bytes, b'[', b']').ok_or_else(|| {
                    ParseError::new(line, 1, "unclosed `[` in lot annotation")
                })?;
                // Keep the first written `[DATE]` for display round-trip;
                // it carries no computation (the lotter derives its own
                // lot dates by FIFO). Later `[…]` groups are dropped.
                if lot_date.is_none() {
                    let inner = rest[1..end].trim();
                    lot_date = Some(crate::date::Date::parse(inner).map_err(|e| {
                        ParseError::new(line, 1, format!("invalid lot date `{}`: {}", inner, e))
                    })?);
                }
                rest = &rest[end + 1..];
            }
            _ => return Ok((lot_cost, lot_date, rest)),
        }
    }
}

/// Byte-index of the balancing `close` for an `open` at byte 0.
/// Tracks nesting depth. Returns `None` if unbalanced.
fn find_matching_brace(bytes: &[u8], open: u8, close: u8) -> Option<usize> {
    let mut depth = 0usize;
    for (i, &b) in bytes.iter().enumerate() {
        if b == open {
            depth += 1;
        } else if b == close {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}

/// Byte-index of the first `}}` sequence starting at byte 2
/// (skipping the leading `{{`).
fn find_double_close(bytes: &[u8]) -> Option<usize> {
    let mut i = 2;
    while i + 1 < bytes.len() {
        if bytes[i] == b'}' && bytes[i + 1] == b'}' {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Split off the account name from the rest of the posting body.
/// Handles plain, `(virtual)` and `[balanced virtual]` forms.
fn extract_account(text: &str, line: usize) -> Result<(bool, bool, String, &str), ParseError> {
    if let Some(rest) = text.strip_prefix('(') {
        let end = rest
            .find(')')
            .ok_or_else(|| ParseError::new(line, 1, "virtual posting missing `)`"))?;
        Ok((true, false, rest[..end].trim().to_string(), &rest[end + 1..]))
    } else if let Some(rest) = text.strip_prefix('[') {
        let end = rest
            .find(']')
            .ok_or_else(|| ParseError::new(line, 1, "virtual posting missing `]`"))?;
        Ok((true, true, rest[..end].trim().to_string(), &rest[end + 1..]))
    } else {
        let bytes = text.as_bytes();
        let sep = find_account_separator(bytes);
        let account = text[..sep].trim().to_string();
        if account.is_empty() {
            return Err(ParseError::new(line, 1, "posting missing account"));
        }
        Ok((false, true, account, &text[sep..]))
    }
}

/// Find the first position of either `\t` or two consecutive spaces.
/// Returns `bytes.len()` if no separator is found.
fn find_account_separator(bytes: &[u8]) -> usize {
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\t' {
            return i;
        }
        if bytes[i] == b' ' && bytes.get(i + 1) == Some(&b' ') {
            return i;
        }
        i += 1;
    }
    bytes.len()
}

/// Parse a single amount like `100 USD`, `$5.00`, `-5 EUR`, or `€-5.00`.
/// Returns the parsed `Amount` and the trailing text after it.
///
/// Amount ends at `@`, `=`, or end-of-string — those markers belong to
/// cost/assertion clauses.
fn parse_amount(text: &str, line: usize) -> Result<(Amount, &str), ParseError> {
    // `{` and `[` terminate the amount — they open Ledger lot
    // annotations (`{€58.11}` cost basis, `[2017-12-31]` lot date)
    // that sit between the amount and `@`. `parse_posting` discards
    // those blocks; here we just stop the amount scan.
    let end = text.find(['@', '=', '{', '[']).unwrap_or(text.len());
    let amt_text = text[..end].trim_end();
    let after = &text[end..];

    let bytes = amt_text.as_bytes();
    let first = bytes
        .first()
        .ok_or_else(|| ParseError::new(line, 1, "empty amount"))?;

    // Ledger-style valuation expression: `(COMMODITY EXPR)` or
    // `(EXPR COMMODITY)`. Evaluated at parse time with `+ - * /`.
    // The result's `decimals` is 0 — expressions produce derived
    // values, not user-chosen display precisions. Explicit postings
    // in the same commodity (e.g. a direct `€5.00`) set the display
    // precision; if the commodity only ever appears via expressions,
    // the default fallback (2) kicks in at render time.
    if *first == b'(' {
        let (commodity, value, consumed) = expression::parse(amt_text)
            .map_err(|e| ParseError::new(line, 1, e))?;
        let trailing = amt_text[consumed..].trim_start();
        if !trailing.is_empty() {
            return Err(ParseError::new(
                line,
                1,
                format!("unexpected text after expression: `{}`", trailing),
            ));
        }
        return Ok((Amount { commodity, value, decimals: 0 }, after));
    }

    // Two forms: `[-]NUMBER [COMMODITY]` or `COMMODITY [-]NUMBER`.
    let (commodity, value_str) = if first.is_ascii_digit() || *first == b'-' || *first == b'.' {
        // Number-first. Number ends at first non-[digit,.]-byte after
        // the optional leading `-`.
        let mut i = if *first == b'-' { 1 } else { 0 };
        while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
            i += 1;
        }
        (amt_text[i..].trim().to_string(), &amt_text[..i])
    } else {
        // Commodity-first. Commodity ends at a sign or digit.
        let mut i = 0;
        while i < bytes.len() && !bytes[i].is_ascii_digit() && bytes[i] != b'-' {
            i += 1;
        }
        let commodity = amt_text[..i].trim().to_string();
        (commodity, amt_text[i..].trim())
    };

    let value = crate::decimal::Decimal::parse(value_str)
        .map_err(|e| ParseError::new(line, 1, format!("invalid number: {}", e)))?;

    // Count fractional digits the user wrote so reports can display with
    // at least that precision — `5.00` gives 2 even though the Decimal
    // value is `5`.
    let decimals = value_str
        .rfind('.')
        .map(|i| value_str.len() - i - 1)
        .unwrap_or(0);

    Ok((Amount { commodity, value, decimals }, after))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decimal::Decimal;

    // --- Price directive ---

    #[test]
    fn parse_price_basic() {
        let got = parse("P 2024-06-15 USD EUR 0.92\n").unwrap();
        assert_eq!(got.len(), 1);
        match &got[0].value {
            Entry::Price(p) => {
                assert_eq!(p.date.to_string(), "2024-06-15");
                assert_eq!(&*p.base, "USD");
                assert_eq!(&*p.quote, "EUR");
                assert_eq!(p.rate, Decimal::parse("0.92").unwrap());
            }
            other => panic!("expected Price, got {:?}", other),
        }
        assert_eq!(got[0].line, 1);
    }

    #[test]
    fn parse_price_missing_rate_errors() {
        assert!(parse("P 2024-06-15 USD EUR\n").is_err());
    }

    // --- Periodic transactions ---

    #[test]
    fn parse_periodic_cadence_and_title() {
        use crate::parser::entry::Cadence;
        // `monthly` is peeled as the cadence; the rest is the title.
        let got = parse("~ 2021 monthly some title\n\ta  1\n\tb\n").unwrap();
        match &got[0].value {
            Entry::Periodic { period, cadence, description, .. } => {
                assert_eq!(period, "2021");
                assert_eq!(*cadence, Cadence::Monthly);
                assert_eq!(description, "some title");
            }
            other => panic!("expected Periodic, got {:?}", other),
        }
        // No cadence keyword → Yearly, and the whole remainder is the title.
        let got = parse("~ 2021 just a title\n\ta  1\n\tb\n").unwrap();
        let Entry::Periodic { cadence, description, .. } = &got[0].value else {
            panic!("expected Periodic")
        };
        assert_eq!(*cadence, Cadence::Yearly);
        assert_eq!(description, "just a title");
    }

    // --- Comments ---

    #[test]
    fn parse_top_level_comment_semicolon() {
        let got = parse("; a comment\n").unwrap();
        assert!(matches!(got[0].value, Entry::Comment(_)));
    }

    #[test]
    fn parse_top_level_comment_hash() {
        let got = parse("# another comment\n").unwrap();
        assert!(matches!(got[0].value, Entry::Comment(_)));
    }

    // --- Transaction header ---

    #[test]
    fn parse_transaction_header_minimal() {
        let got = parse("2024-06-15\n").unwrap();
        match &got[0].value {
            Entry::Transaction(tx) => {
                assert_eq!(tx.date.to_string(), "2024-06-15");
                assert_eq!(tx.state, State::Uncleared);
                assert_eq!(tx.code, None);
                assert_eq!(tx.description, "");
            }
            _ => panic!("expected Transaction"),
        }
    }

    #[test]
    fn parse_transaction_header_full() {
        let got = parse("2024-06-15 * (ABC) Grocery Store\n").unwrap();
        match &got[0].value {
            Entry::Transaction(tx) => {
                assert_eq!(tx.date.to_string(), "2024-06-15");
                assert_eq!(tx.state, State::Cleared);
                assert_eq!(tx.code, Some("ABC".to_string()));
                assert_eq!(tx.description, "Grocery Store");
            }
            _ => panic!("expected Transaction"),
        }
    }

    #[test]
    fn parse_transaction_with_aux_date() {
        let got = parse("2024-06-15=2024-06-16 * Thing\n").unwrap();
        match &got[0].value {
            Entry::Transaction(tx) => assert_eq!(tx.date.to_string(), "2024-06-15"),
            _ => panic!("expected Transaction"),
        }
    }

    // --- Transaction with postings ---

    #[test]
    fn parse_transaction_with_postings() {
        let src = "2024-06-15 * Coffee\n    expenses:food  5 USD\n    assets:cash\n";
        let got = parse(src).unwrap();
        assert_eq!(got.len(), 1);
        match &got[0].value {
            Entry::Transaction(tx) => {
                assert_eq!(tx.postings.len(), 2);
                assert_eq!(tx.postings[0].value.account, "expenses:food");
                assert!(tx.postings[0].value.amount.is_some());
                assert_eq!(tx.postings[1].value.account, "assets:cash");
                assert!(tx.postings[1].value.amount.is_none());
            }
            _ => panic!("expected Transaction"),
        }
    }

    #[test]
    fn parse_posting_with_cost() {
        let src = "2024-06-15 * Coffee\n    expenses:food  5 USD @ 0.92 EUR\n";
        let got = parse(src).unwrap();
        if let Entry::Transaction(tx) = &got[0].value {
            let p = &tx.postings[0].value;
            assert!(matches!(p.costs, Some(Costs::PerUnit(_))));
        }
    }

    #[test]
    fn parse_posting_with_total_cost() {
        let src = "2024-06-15 * Coffee\n    expenses:food  5 USD @@ 4.60 EUR\n";
        let got = parse(src).unwrap();
        if let Entry::Transaction(tx) = &got[0].value {
            assert!(matches!(tx.postings[0].value.costs, Some(Costs::Total(_))));
        }
    }

    #[test]
    fn parse_posting_with_balance_assertion() {
        let src = "2024-06-15 * Check\n    assets:bank  100 USD = 1000 USD\n";
        let got = parse(src).unwrap();
        if let Entry::Transaction(tx) = &got[0].value {
            assert!(tx.postings[0].value.balance_assertion.is_some());
        }
    }

    #[test]
    fn parse_posting_with_lot_cost_annotation() {
        // `{€58.11}` is a Ledger per-unit lot cost-basis tag, kept
        // alongside the amount and the @market price.
        let src = "2024-06-15 * Sell\n    assets:broker  -4 ABC {€58.11} @ €250.00\n    income:cap  4 ABC\n";
        let got = parse(src).unwrap();
        if let Entry::Transaction(tx) = &got[0].value {
            let p = &tx.postings[0].value;
            let amt = p.amount.as_ref().unwrap();
            assert_eq!(amt.commodity, "ABC");
            assert!(matches!(p.costs, Some(Costs::PerUnit(_))));
            let lot = p.lot_cost.as_ref().unwrap();
            assert!(!lot.total, "{{}} is per-unit");
            assert!(!lot.fixed);
            assert_eq!(lot.amount.value, crate::decimal::Decimal::parse("58.11").unwrap());
        }
    }

    #[test]
    fn parse_lot_cost_total_vs_per_unit_and_fixed() {
        // acc keeps the lot-cost value AND which form the user wrote:
        // `{COST}` per-unit, `{{TOTAL}}` whole-lot, `=` locked.
        let lot = |src: &str| {
            let got = parse(src).unwrap();
            let Entry::Transaction(tx) = &got[0].value else { panic!() };
            tx.postings[0].value.lot_cost.clone().unwrap()
        };
        let per_unit = lot("2024-06-15 * x\n    a  10 ABC {20 EUR}\n    b  -200 EUR\n");
        assert!(!per_unit.total);
        assert_eq!(per_unit.amount.value, crate::decimal::Decimal::from(20));

        let total = lot("2024-06-15 * x\n    a  10 ABC {{200 EUR}}\n    b  -200 EUR\n");
        assert!(total.total);
        assert_eq!(total.amount.value, crate::decimal::Decimal::from(200));

        let fixed = lot("2024-06-15 * x\n    a  10 ABC {=20 EUR}\n    b  -200 EUR\n");
        assert!(fixed.fixed && !fixed.total);

        let fixed_total = lot("2024-06-15 * x\n    a  10 ABC {{=200 EUR}}\n    b  -200 EUR\n");
        assert!(fixed_total.fixed && fixed_total.total);
    }

    #[test]
    fn parse_posting_with_lot_date_annotation() {
        // `[2017-12-31]` is a lot-date tag, kept for display when it
        // accompanies a lot cost (here `{=€0.2395}`).
        let src = "2024-06-15 * Move\n    assets:broker  ABC 20 {=€0.2395} [2017-12-31]\n    income:x  -20 ABC\n";
        let got = parse(src).unwrap();
        if let Entry::Transaction(tx) = &got[0].value {
            let p = &tx.postings[0].value;
            assert_eq!(p.amount.as_ref().unwrap().commodity, "ABC");
            assert_eq!(p.lot_date.unwrap().to_string(), "2017-12-31");
        }
    }

    #[test]
    fn lot_date_requires_a_lot_cost() {
        // A `[date]` is valid only pinned to a lot cost — `{}` or `{{}}`.
        let date = |src: &str| {
            let got = parse(src).unwrap();
            let Entry::Transaction(tx) = &got[0].value else { panic!() };
            tx.postings[0].value.lot_date.map(|d| d.to_string())
        };
        // With per-unit `{}` — accepted, date kept.
        assert_eq!(
            date("2024-06-15 * x\n    a  -1 BTC {100 USD} [2018-01-01]\n    b  100 USD\n"),
            Some("2018-01-01".to_string())
        );
        // With total `{{}}` — also accepted.
        assert_eq!(
            date("2024-06-15 * x\n    a  -1 BTC {{100 USD}} [2018-01-01]\n    b  100 USD\n"),
            Some("2018-01-01".to_string())
        );
        // Bare `[date]` with no lot cost — rejected.
        assert!(parse("2024-06-15 * x\n    a  -1 BTC [2018-01-01]\n    b  100 USD\n").is_err());
    }

    #[test]
    fn parse_posting_with_inline_comment() {
        let src = "2024-06-15 * Coffee\n    expenses:food  5 USD  ; morning coffee\n";
        let got = parse(src).unwrap();
        if let Entry::Transaction(tx) = &got[0].value {
            let p = &tx.postings[0].value;
            assert_eq!(p.comments.len(), 1);
            assert_eq!(p.comments[0].value.text, "morning coffee");
        }
    }

    #[test]
    fn parse_indented_comment_on_transaction() {
        let src = "2024-06-15 * Coffee\n    ; note about coffee\n    expenses:food  5 USD\n";
        let got = parse(src).unwrap();
        if let Entry::Transaction(tx) = &got[0].value {
            assert_eq!(tx.comments.len(), 1);
            assert_eq!(tx.comments[0].value.text, "note about coffee");
            assert_eq!(tx.postings.len(), 1);
        }
    }

    // --- Virtual postings ---

    #[test]
    fn parse_virtual_posting_parens() {
        let src = "2024-06-15 * X\n    (assets:x)  100 USD\n";
        let got = parse(src).unwrap();
        if let Entry::Transaction(tx) = &got[0].value {
            let p = &tx.postings[0].value;
            assert!(p.is_virtual);
            assert!(!p.balanced);
        }
    }

    #[test]
    fn parse_virtual_posting_brackets() {
        let src = "2024-06-15 * X\n    [assets:x]  100 USD\n";
        let got = parse(src).unwrap();
        if let Entry::Transaction(tx) = &got[0].value {
            let p = &tx.postings[0].value;
            assert!(p.is_virtual);
            assert!(p.balanced);
        }
    }

    // --- Directives ---

    #[test]
    fn unknown_directive_errors() {
        assert!(parse("year 2024\n").is_err());
        assert!(parse("something weird\n").is_err());
        // `commodity` / `account` are no longer directives — they come from
        // the `CONFIG_DIR` TOML, so in a `-f` ledger they are unknown.
        assert!(parse("commodity USD\n").is_err());
        assert!(parse("account Assets:Bank\n").is_err());
    }

    // --- Indent rules ---

    #[test]
    fn single_space_indent_errors() {
        // Single space is not a valid indent. Falls through to keyword
        // dispatch where empty keyword is rejected.
        assert!(parse(" something\n").is_err());
    }

    #[test]
    fn tab_indent_accepted() {
        let src = "2024-06-15 * X\n\texpenses:food  5 USD\n";
        let got = parse(src).unwrap();
        if let Entry::Transaction(tx) = &got[0].value {
            assert_eq!(tx.postings.len(), 1);
        }
    }
}
