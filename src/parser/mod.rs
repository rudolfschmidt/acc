//! Journal parser.
//!
//! Pure text-to-records transformation. Takes a source string, produces a
//! flat `Vec<Located<Entry>>`. No I/O, no shared state, no alias resolution,
//! no index building — every interpretive step happens in later phases.

pub mod comment;
pub mod entry;
pub mod error;
pub mod expression;
pub mod file;
pub mod located;
pub mod posting;
pub mod transaction;

pub use comment::Comment;
pub use entry::{Entry, Price};
pub use error::ParseError;
pub use file::File;
pub use located::Located;
pub use posting::{Amount, Costs, LotCost, Posting};
pub use transaction::{State, Transaction};

use std::sync::Arc;

/// Parse a journal source string into a flat stream of entries.
///
/// The returned vec preserves source order. Each entry carries the line
/// number of its top-level directive. Sub-records (postings, aliases,
/// fx-gain/loss flags) are folded into the enclosing entry rather than
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
    // Heuristic: a typical ledger line is ~30 bytes. Reserving up front
    // prevents repeated reallocation as the vec grows on price-heavy
    // inputs (which dominate the real-world workload).
    let mut entries = Vec::with_capacity(source.len() / 30);
    for (idx, text) in source.lines().enumerate() {
        dispatch(text, idx + 1, &file, &mut entries)?;
    }
    Ok(entries)
}

fn dispatch(
    text: &str,
    line: usize,
    file: &Arc<str>,
    entries: &mut Vec<Located<Entry>>,
) -> Result<(), ParseError> {
    match text.as_bytes() {
        [] => Ok(()),
        // Indent per Ledger rule: tab OR two-plus spaces.
        [b'\t', ..] | [b' ', b' ', ..] => extend_block(text, line, file, entries),
        [b'0'..=b'9', ..] => parse_transaction(text, line, file, entries),
        [b'P', b' ', ..] => parse_price(&text[2..], line, file, entries),
        [b';' | b'#', ..] => {
            entries.push(Located {
                file: file.clone(),
                line,
                value: Entry::Comment(text.to_string()),
            });
            Ok(())
        }
        [b'=', b' ', ..] | [b'=', b'\t', ..] => {
            parse_auto_rule(&text[1..].trim_start(), line, file, entries)
        }
        _ => parse_directive(text, line, file, entries),
    }
}

/// Parse an auto-transaction header line. Body is `/<pattern>/` —
/// a ledger-cli style regex delimiter. V1 accepts only the simple
/// form (no `and expr "..."` conditional). Indented `[account]
/// multiplier` lines attach via `extend_block`.
fn parse_auto_rule(
    rest: &str,
    line: usize,
    file: &Arc<str>,
    entries: &mut Vec<Located<Entry>>,
) -> Result<(), ParseError> {
    if rest.contains("and expr") {
        return Err(ParseError::new(
            line,
            1,
            "auto-rule conditional `and expr \"...\"` is not supported yet; \
             use the simple `= /pattern/` form",
        ));
    }
    let pattern = parse_auto_pattern(rest, line)?;
    entries.push(Located {
        file: file.clone(),
        line,
        value: Entry::AutoRule(crate::parser::entry::AutoRule {
            pattern,
            postings: Vec::new(),
        }),
    });
    Ok(())
}

fn parse_auto_pattern(
    rest: &str,
    line: usize,
) -> Result<crate::parser::entry::AutoPattern, ParseError> {
    let body = rest.trim();
    let inner = body
        .strip_prefix('/')
        .and_then(|s| s.strip_suffix('/'))
        .ok_or_else(|| {
            ParseError::new(
                line,
                1,
                "auto-rule pattern must be delimited by /…/",
            )
        })?;
    if inner.is_empty() {
        return Err(ParseError::new(line, 1, "auto-rule pattern is empty"));
    }
    let anchored_start = inner.starts_with('^');
    let anchored_end = inner.ends_with('$');
    let core = match (anchored_start, anchored_end) {
        (true, true) => &inner[1..inner.len() - 1],
        (true, false) => &inner[1..],
        (false, true) => &inner[..inner.len() - 1],
        (false, false) => inner,
    };
    use crate::parser::entry::AutoPattern;
    Ok(match (anchored_start, anchored_end) {
        (true, true) => AutoPattern::Exact(core.to_string()),
        (true, false) => AutoPattern::Prefix(core.to_string()),
        (false, true) => AutoPattern::Suffix(core.to_string()),
        (false, false) => AutoPattern::Contains(core.to_string()),
    })
}

/// Parse a `P DATE BASE QUOTE RATE` directive. The leading `P ` has
/// already been stripped by the dispatcher; `rest` is the remainder.
fn parse_price(
    rest: &str,
    line: usize,
    file: &Arc<str>,
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

    let date = crate::date::Date::parse(date_str)
        .map_err(|e| ParseError::new(line, 1, e))?;
    let rate = crate::decimal::Decimal::parse(rate_str)
        .map_err(|e| ParseError::new(line, 1, format!("invalid rate: {}", e)))?;

    entries.push(Located {
        file: file.clone(),
        line,
        value: Entry::Price(Price {
            date,
            base: Arc::from(base),
            quote: Arc::from(quote),
            rate,
        }),
    });
    Ok(())
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

/// Parse a top-level keyword directive. Recognised: `commodity`,
/// `account`. Unknown keywords raise an error — acc has no silent-skip
/// policy for directives it doesn't understand.
fn parse_directive(
    text: &str,
    line: usize,
    file: &Arc<str>,
    entries: &mut Vec<Located<Entry>>,
) -> Result<(), ParseError> {
    let (keyword, after) = text
        .split_once(|c: char| c.is_whitespace())
        .unwrap_or((text, ""));
    let arg = after.trim();

    match keyword {
        "commodity" => {
            if arg.is_empty() {
                return Err(ParseError::new(line, 1, "commodity directive missing symbol"));
            }
            entries.push(Located {
                file: file.clone(),
                line,
                value: Entry::Commodity {
                    symbol: arg.to_string(),
                    aliases: Vec::new(),
                    precision: None,
                },
            });
            Ok(())
        }
        "account" => {
            if arg.is_empty() {
                return Err(ParseError::new(line, 1, "account directive missing name"));
            }
            entries.push(Located {
                file: file.clone(),
                line,
                value: Entry::Account(arg.to_string()),
            });
            Ok(())
        }
        other => Err(ParseError::new(line, 1, format!("unknown directive: {}", other))),
    }
}

/// Attach the content of an indented line to the last emitted entry:
///
/// - under `Transaction` → a posting or an indented comment
/// - under `Commodity`   → append an `alias X` sub-directive
/// - under `Account`     → replace with `FxGainAccount`/`FxLossAccount`
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
    // Attaches to a Transaction, dropped otherwise.
    if content.starts_with(';') || content.starts_with('#') {
        if let Entry::Transaction(tx) = &mut last.value {
            tx.comments.push(Located {
                file: file.clone(),
                line,
                value: Comment { text: content[1..].trim().to_string() },
            });
        }
        return Ok(());
    }

    // Mid-line `;` is an inline comment. Split it off once, centrally —
    // every sub-parser below sees clean content.
    let (body, inline_comment) = match content.find(';') {
        Some(i) => (content[..i].trim_end(), Some(content[i + 1..].trim().to_string())),
        None => (content, None),
    };

    // In the `Account` case we need to swap the variant entirely; collect
    // the replacement here and apply it after the match ends.
    let mut upgrade: Option<Entry> = None;

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
        Entry::Commodity { aliases, precision, .. } => {
            if let Some(rest) = body.strip_prefix("alias ") {
                let alias = rest.trim();
                if alias.is_empty() {
                    return Err(ParseError::new(line, 1, "alias missing name"));
                }
                aliases.push(alias.to_string());
            } else if let Some(rest) = body.strip_prefix("precision ") {
                let digits = rest.trim();
                let n: usize = digits.parse().map_err(|_| {
                    ParseError::new(line, 1, format!("precision requires a non-negative integer, got `{}`", digits))
                })?;
                *precision = Some(n);
            } else {
                return Err(ParseError::new(line, 1, "expected `alias NAME` or `precision N`"));
            }
        }
        Entry::Account(name) => {
            let mut parts = body.split_whitespace();
            match (parts.next(), parts.next(), parts.next()) {
                (Some("fx"), Some("gain"), None) => {
                    upgrade = Some(Entry::FxGainAccount(std::mem::take(name)));
                }
                (Some("fx"), Some("loss"), None) => {
                    upgrade = Some(Entry::FxLossAccount(std::mem::take(name)));
                }
                (Some("cta"), Some("gain"), None) => {
                    upgrade = Some(Entry::CtaGainAccount(std::mem::take(name)));
                }
                (Some("cta"), Some("loss"), None) => {
                    upgrade = Some(Entry::CtaLossAccount(std::mem::take(name)));
                }
                _ => return Err(ParseError::new(
                    line,
                    1,
                    "expected `fx gain`, `fx loss`, `cta gain`, or `cta loss`",
                )),
            }
        }
        Entry::AutoRule(rule) => {
            let auto_posting = parse_auto_posting(body, line)?;
            rule.postings.push(auto_posting);
        }
        _ => return Err(ParseError::new(line, 1, "indented directive not expected here")),
    }

    if let Some(new_value) = upgrade {
        last.value = new_value;
    }
    Ok(())
}

/// Parse one auto-rule posting line: `[account]  MULTIPLIER` (or the
/// paren-virtual / real-posting variant). Multiplier is a signed
/// decimal, applied to the triggering posting's amount during
/// expansion.
fn parse_auto_posting(
    body: &str,
    line: usize,
) -> Result<crate::parser::entry::AutoPosting, ParseError> {
    // Separator between account and multiplier: tab or 2+ spaces,
    // same rule as regular postings.
    let (account_raw, multiplier_raw) = split_account_and_amount(body).ok_or_else(|| {
        ParseError::new(
            line,
            1,
            "auto-rule posting needs account and multiplier separated by 2+ spaces or a tab",
        )
    })?;
    let (account, is_virtual, balanced) = if let Some(inner) =
        account_raw.strip_prefix('[').and_then(|s| s.strip_suffix(']'))
    {
        (inner.to_string(), true, true)
    } else if let Some(inner) =
        account_raw.strip_prefix('(').and_then(|s| s.strip_suffix(')'))
    {
        (inner.to_string(), true, false)
    } else {
        (account_raw.to_string(), false, true)
    };
    let multiplier = crate::decimal::Decimal::parse(multiplier_raw.trim()).map_err(|e| {
        ParseError::new(line, 1, format!("invalid auto-rule multiplier: {}", e))
    })?;
    Ok(crate::parser::entry::AutoPosting {
        account,
        multiplier,
        is_virtual,
        balanced,
    })
}

/// Split a posting/auto-posting body into (account, amount) where the
/// separator is a tab or a run of 2+ spaces. Returns `None` if no
/// such separator exists.
fn split_account_and_amount(body: &str) -> Option<(&str, &str)> {
    if let Some(idx) = body.find('\t') {
        let (a, b) = body.split_at(idx);
        return Some((a.trim_end(), b.trim_start()));
    }
    if let Some(idx) = body.find("  ") {
        let (a, b) = body.split_at(idx);
        return Some((a.trim_end(), b.trim_start()));
    }
    None
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

    // Ledger lot annotations. The first `{COST}` (or `{=FIXED}`) is
    // captured as `lot_cost` — the booker uses it as the
    // balance-effective per-unit value, overriding any `@` market
    // cost. `[DATE]`, `(NOTE)`, `{{TOTAL}}` and extra `{…}` groups
    // are consumed and dropped.
    let (lot_cost, rest) = consume_lot_annotations(rest, line)?;

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
    // valid in Ledger. Drop whatever remains; we already captured
    // the lot cost from the pre-cost slot if present.
    let (_, rest) = consume_lot_annotations(rest, line)?;

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
        balance_assertion,
        is_virtual,
        balanced,
        comments: Vec::new(),
    })
}

/// Consume zero or more Ledger lot annotations from the start of
/// `rest`. Returns the first captured lot cost (if any) and the
/// remaining slice.
///
/// Recognised forms:
///
/// - `{COST}`   → `LotCost::Floating(COST)` — only the first one is
///   kept; later `{…}` groups are discarded.
/// - `{=COST}`  → `LotCost::Fixed(COST)` — ditto.
/// - `{{TOTAL}}` / `{{=TOTAL}}` → total-cost form, consumed and
///   discarded (acc doesn't model total lot costs).
/// - `[DATE]`   → lot acquisition date, consumed and discarded.
fn consume_lot_annotations<'a>(
    mut rest: &'a str,
    line: usize,
) -> Result<(Option<LotCost>, &'a str), ParseError> {
    let mut lot_cost: Option<LotCost> = None;
    loop {
        rest = rest.trim_start();
        let bytes = rest.as_bytes();
        match bytes.first() {
            Some(b'{') => {
                // `{{...}}` — total cost. Find the `}}` sequence and drop.
                if bytes.get(1) == Some(&b'{') {
                    let close = find_double_close(bytes).ok_or_else(|| {
                        ParseError::new(line, 1, "unclosed `{{` in lot annotation")
                    })?;
                    rest = &rest[close + 2..];
                    continue;
                }
                // `{COST}` / `{=COST}` — per-unit.
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
                    lot_cost = Some(if fixed {
                        LotCost::Fixed(amt)
                    } else {
                        LotCost::Floating(amt)
                    });
                }
                rest = &rest[end + 1..];
            }
            Some(b'[') => {
                let end = find_matching_brace(bytes, b'[', b']').ok_or_else(|| {
                    ParseError::new(line, 1, "unclosed `[` in lot annotation")
                })?;
                rest = &rest[end + 1..];
            }
            _ => return Ok((lot_cost, rest)),
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
        // `{€58.11}` is a Ledger lot cost-basis tag — acc parses and
        // discards it, keeping the amount and the @market price.
        let src = "2024-06-15 * Sell\n    assets:btc  -4 BSV {€58.11} @ €250.00\n    income:cap  4 BSV\n";
        let got = parse(src).unwrap();
        if let Entry::Transaction(tx) = &got[0].value {
            let p = &tx.postings[0].value;
            let amt = p.amount.as_ref().unwrap();
            assert_eq!(amt.commodity, "BSV");
            assert!(matches!(p.costs, Some(Costs::PerUnit(_))));
        }
    }

    #[test]
    fn parse_posting_with_lot_date_annotation() {
        // `[2017-12-31]` is a lot-date tag. Like cost basis it's
        // parsed and dropped.
        let src = "2024-06-15 * Move\n    assets:pln  PLN 20 {=€0.2395} [2017-12-31]\n    income:x  -20 PLN\n";
        let got = parse(src).unwrap();
        if let Entry::Transaction(tx) = &got[0].value {
            let p = &tx.postings[0].value;
            let amt = p.amount.as_ref().unwrap();
            assert_eq!(amt.commodity, "PLN");
            assert!(p.costs.is_none());
        }
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
    fn parse_commodity_with_aliases() {
        let src = "commodity USD\n    alias $\n    alias USdollar\n";
        let got = parse(src).unwrap();
        match &got[0].value {
            Entry::Commodity { symbol, aliases, precision } => {
                assert_eq!(symbol, "USD");
                assert_eq!(aliases, &vec!["$".to_string(), "USdollar".to_string()]);
                assert_eq!(*precision, None);
            }
            _ => panic!("expected Commodity"),
        }
    }

    #[test]
    fn parse_commodity_with_precision() {
        let src = "commodity EUR\n    alias €\n    precision 2\n";
        let got = parse(src).unwrap();
        match &got[0].value {
            Entry::Commodity { symbol, aliases, precision } => {
                assert_eq!(symbol, "EUR");
                assert_eq!(aliases, &vec!["€".to_string()]);
                assert_eq!(*precision, Some(2));
            }
            _ => panic!("expected Commodity"),
        }
    }

    #[test]
    fn parse_account_with_fx_gain() {
        let src = "account Equity:FxGain\n    fx gain\n";
        let got = parse(src).unwrap();
        assert!(matches!(got[0].value, Entry::FxGainAccount(ref n) if n == "Equity:FxGain"));
    }

    #[test]
    fn parse_account_with_fx_loss() {
        let src = "account Equity:FxLoss\n    fx loss\n";
        let got = parse(src).unwrap();
        assert!(matches!(got[0].value, Entry::FxLossAccount(ref n) if n == "Equity:FxLoss"));
    }

    #[test]
    fn parse_account_without_sub_directive() {
        let src = "account Assets:Bank\n";
        let got = parse(src).unwrap();
        assert!(matches!(got[0].value, Entry::Account(ref n) if n == "Assets:Bank"));
    }

    #[test]
    fn unknown_directive_errors() {
        assert!(parse("year 2024\n").is_err());
        assert!(parse("something weird\n").is_err());
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
