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

fn dispatch(
    text: &str,
    line: usize,
    file: &Arc<str>,
    commodities: &mut HashSet<Arc<str>>,
    prices_filter: Option<&HashSet<String>>,
    entries: &mut Vec<Located<Entry>>,
) -> Result<(), ParseError> {
    match text.as_bytes() {
        [] => Ok(()),
        // Indent per Ledger rule: tab OR two-plus spaces.
        [b'\t', ..] | [b' ', b' ', ..] => extend_block(text, line, file, entries),
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
        [b'=', b' ', ..] | [b'=', b'\t', ..] => {
            parse_auto_rule(text[1..].trim_start(), line, file, entries)
        }
        _ => parse_directive(text, line, file, entries),
    }
}

/// Parse a `=` directive — four forms, told apart syntactically:
///
/// - `= /pattern/` → an anonymous auto-rule (`AutoRule`), matched as-is.
/// - `= NAME[key] :: value` → one entry of a named string→string lookup table
///   (`Lookup`); the bracket in the name is the discriminator. Referenced as
///   `NAME[key]` inside a template posting account.
/// - `= NAME :: /pattern/` → a named auto-rule *template* (`AutoTemplate`);
///   its pattern and posting accounts carry positional `$1`/`$2` placeholders
///   and `lookup[key]` calls that an instantiation fills in.
/// - `= NAME arg…` → instantiate template `NAME` with a pair (`AutoInstance`).
///
/// Indented `[account] multiplier` lines attach via `extend_block`.
fn parse_auto_rule(
    rest: &str,
    line: usize,
    file: &Arc<str>,
    entries: &mut Vec<Located<Entry>>,
) -> Result<(), ParseError> {
    let body = rest.trim();

    // Anonymous auto-rule — checked first so a `/pattern/` that happens to
    // contain `::` is never mistaken for a named template.
    if body.starts_with('/') {
        let (inner, condition) = parse_predicate(body, line)?;
        entries.push(Located {
            file: file.clone(),
            line,
            value: Entry::AutoRule(crate::parser::entry::AutoRule {
                pattern: crate::parser::entry::AutoPattern::parse_inner(&inner),
                postings: Vec::new(),
                condition,
            }),
        });
        return Ok(());
    }

    // A `NAME … :: REST` form — either a lookup-table entry or a rule template.
    if let Some((name_part, rest_part)) = body.split_once("::") {
        let name_part = name_part.trim();

        // Lookup-table entry: `NAME[key] :: value`. The bracket in the name is
        // the discriminator (a rule template's name has none); the value is a
        // bare string, not a `/pattern/`. Reuses the same `::` split.
        if let Some((table, key)) = split_table_key(name_part) {
            let value = rest_part.trim();
            if value.is_empty() {
                return Err(ParseError::new(
                    line,
                    1,
                    "lookup entry `= NAME[key] :: value` has an empty value",
                ));
            }
            entries.push(Located {
                file: file.clone(),
                line,
                value: Entry::Lookup {
                    table: table.to_string(),
                    key: key.to_string(),
                    value: value.to_string(),
                },
            });
            return Ok(());
        }

        // Named auto-rule template: `NAME :: /pattern/ [amount <op> N]`.
        if name_part.is_empty() {
            return Err(ParseError::new(line, 1, "auto-rule template needs a name before `::`"));
        }
        if name_part.split_whitespace().count() > 1 {
            return Err(ParseError::new(
                line,
                1,
                "auto-rule template takes no parameter list; use positional `$1`/`$2` in the pattern",
            ));
        }
        let (pattern, condition) = parse_predicate(rest_part, line)?;
        entries.push(Located {
            file: file.clone(),
            line,
            value: Entry::AutoTemplate {
                name: name_part.to_string(),
                pattern,
                postings: Vec::new(),
                condition,
            },
        });
        return Ok(());
    }

    // Instantiation: `NAME arg1 arg2 …`.
    let mut tokens = body.split_whitespace();
    let name = tokens
        .next()
        .ok_or_else(|| ParseError::new(line, 1, "empty `=` directive"))?
        .to_string();
    let args: Vec<String> = tokens.map(str::to_string).collect();
    if args.is_empty() {
        return Err(ParseError::new(
            line,
            1,
            format!("`= {name}` needs arguments — a template instantiation is `= NAME a b`"),
        ));
    }
    entries.push(Located {
        file: file.clone(),
        line,
        value: Entry::AutoInstance { name, args },
    });
    Ok(())
}

/// Split a `table[key]` string into its two parts, trimmed. Returns `None`
/// unless the string is exactly that shape (a `[`, and a trailing `]`, with a
/// non-empty table and key) — that `None` is how a rule-template name
/// (`reconcile`) is told apart from a lookup entry (`name[key]`).
fn split_table_key(s: &str) -> Option<(&str, &str)> {
    let inner = s.strip_suffix(']')?;
    let open = inner.find('[')?;
    let table = inner[..open].trim();
    let key = inner[open + 1..].trim();
    if table.is_empty() || key.is_empty() {
        return None;
    }
    Some((table, key))
}

/// Split a `= …` predicate into the `/pattern/` inner text and an optional
/// trailing `amount <op> N` clause. Accounts never contain `/`, so the first
/// `/` after the opening one closes the pattern; anything after it is the clause.
fn parse_predicate(
    body: &str,
    line: usize,
) -> Result<(String, Option<crate::parser::entry::AmountCondition>), ParseError> {
    let rest = body
        .trim()
        .strip_prefix('/')
        .ok_or_else(|| ParseError::new(line, 1, "auto-rule pattern must be delimited by /…/"))?;
    let close = rest
        .find('/')
        .ok_or_else(|| ParseError::new(line, 1, "auto-rule pattern must be delimited by /…/"))?;
    let inner = rest[..close].to_string();
    if inner.is_empty() {
        return Err(ParseError::new(line, 1, "auto-rule pattern is empty"));
    }
    let tail = rest[close + 1..].trim();
    let condition = if tail.is_empty() {
        None
    } else {
        Some(parse_amount_condition(tail, line)?)
    };
    Ok((inner, condition))
}

/// Parse an `amount <op> <number>` clause — the only condition kind, one
/// comparison against a bare number. Boolean logic is deliberately absent
/// (AND = more clauses, OR = more rules, NOT = flip the operator).
fn parse_amount_condition(
    text: &str,
    line: usize,
) -> Result<crate::parser::entry::AmountCondition, ParseError> {
    use crate::parser::entry::CompareOp;
    let rest = text.strip_prefix("amount").map(str::trim_start).ok_or_else(|| {
        ParseError::new(
            line,
            1,
            format!("only an `amount <op> N` clause may follow the pattern, got `{text}`"),
        )
    })?;
    let (op, num) = if let Some(n) = rest.strip_prefix(">=") {
        (CompareOp::Ge, n)
    } else if let Some(n) = rest.strip_prefix("<=") {
        (CompareOp::Le, n)
    } else if let Some(n) = rest.strip_prefix("==") {
        (CompareOp::Eq, n)
    } else if let Some(n) = rest.strip_prefix("!=") {
        (CompareOp::Ne, n)
    } else if let Some(n) = rest.strip_prefix('>') {
        (CompareOp::Gt, n)
    } else if let Some(n) = rest.strip_prefix('<') {
        (CompareOp::Lt, n)
    } else {
        return Err(ParseError::new(
            line,
            1,
            "amount clause needs a comparison operator: >, <, >=, <=, ==, !=",
        ));
    };
    let value = crate::decimal::Decimal::parse(num.trim())
        .map_err(|e| ParseError::new(line, 1, format!("invalid number in amount clause: {e}")))?;
    Ok(crate::parser::entry::AmountCondition { op, value })
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
                    parities: Vec::new(),
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
/// - under `Account`     → upgrade to `RoleAccount` (any role directive)
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

    // In the `Account` case we need to swap the variant entirely; collect
    // the replacement here and apply it after the match ends. A further
    // sub-directive under an already-upgraded account is collected in
    // `push_new` and appended as a sibling entry instead.
    let mut upgrade: Option<Entry> = None;
    let mut push_new: Option<Entry> = None;

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
        Entry::Commodity { aliases, parities, precision, .. } => {
            if let Some(rest) = body.strip_prefix("alias ") {
                let alias = rest.trim();
                if alias.is_empty() {
                    return Err(ParseError::new(line, 1, "alias missing name"));
                }
                aliases.push(alias.to_string());
            } else if let Some(rest) = body.strip_prefix("parity ") {
                let target = rest.trim();
                if target.is_empty() {
                    return Err(ParseError::new(line, 1, "parity missing commodity"));
                }
                parities.push(target.to_string());
            } else if let Some(rest) = body.strip_prefix("precision ") {
                let digits = rest.trim();
                let n: usize = digits.parse().map_err(|_| {
                    ParseError::new(line, 1, format!("precision requires a non-negative integer, got `{}`", digits))
                })?;
                *precision = Some(n);
            } else {
                return Err(ParseError::new(line, 1, "expected `alias NAME`, `parity COMMODITY` or `precision N`"));
            }
        }
        Entry::Account(name) => {
            // A role sub-directive is any indented two-or-more-word line
            // under `account`. The words verbatim (e.g. `capital gain`)
            // become the role key; the resolver and `$role:slot`
            // references match on it, so no role names are baked in here.
            let role = parse_role_directive(body, line)?;
            upgrade = Some(Entry::RoleAccount { role, account: std::mem::take(name) });
        }
        Entry::RoleAccount { account, .. } => {
            // A further sub-directive under the same `account` (e.g. a
            // shared `label` plus a view-specific `label-register`): emit
            // another RoleAccount that shares the account name. The first
            // sub-directive already upgraded `Account` → `RoleAccount`.
            let role = parse_role_directive(body, line)?;
            push_new = Some(Entry::RoleAccount { role, account: account.clone() });
        }
        Entry::AutoRule(rule) => {
            let auto_posting = parse_auto_posting(body, line)?;
            rule.postings.push(auto_posting);
        }
        Entry::AutoTemplate { postings, .. } => {
            // Template postings parse like auto-rule postings — the account
            // just carries `${N}` / `lookup(key)` placeholders resolved later.
            let auto_posting = parse_auto_posting(body, line)?;
            postings.push(auto_posting);
        }
        _ => return Err(ParseError::new(line, 1, "indented directive not expected here")),
    }

    if let Some(new_value) = upgrade {
        last.value = new_value;
    }
    if let Some(entry) = push_new {
        entries.push(Located { file: file.clone(), line, value: entry });
    }
    Ok(())
}

/// Parse an indented `account` sub-directive into its role key: the
/// whitespace-normalised words (e.g. `capital gain`, `label-register X`).
/// At least two words are required.
fn parse_role_directive(body: &str, line: usize) -> Result<String, ParseError> {
    let role = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if role.split(' ').count() < 2 {
        return Err(ParseError::new(
            line,
            1,
            "expected a role sub-directive of two or more words, \
             e.g. `slippage gain`, `cta loss`, or `capital gain`",
        ));
    }
    Ok(role)
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

    // --- Auto-rule patterns ---

    #[test]
    fn parse_auto_rule_segment_placeholder() {
        use crate::parser::entry::AutoPattern;
        let got = parse("= /^$segment:bar:baz/\n    [foo]  1\n").unwrap();
        match &got[0].value {
            Entry::AutoRule(rule) => match &rule.pattern {
                AutoPattern::Segmented {
                    parts,
                    anchored_start,
                    anchored_end,
                } => {
                    assert_eq!(parts, &["".to_string(), ":bar:baz".to_string()]);
                    assert!(*anchored_start);
                    assert!(!*anchored_end);
                }
                other => panic!("expected Segmented, got {:?}", other),
            },
            other => panic!("expected AutoRule, got {:?}", other),
        }
    }

    #[test]
    fn parse_lookup_template_and_instance() {
        // Two lookup entries (one table, two keys), a template referencing the
        // table as `long[$N]`, and an instantiation.
        let src = "= long[foo] :: foo-long\n= long[bar] :: bar-long\n\
                   = mirror :: /^x:$1:$segment:$2:$segment$/\n\
                   \t[$1:z:long[$2]]  -1\n\t[$2:z:long[$1]]  1\n\
                   = mirror foo bar\n";
        let got = parse(src).unwrap();
        match &got[0].value {
            Entry::Lookup { table, key, value } => {
                assert_eq!(table, "long");
                assert_eq!(key, "foo");
                assert_eq!(value, "foo-long");
            }
            other => panic!("expected Lookup, got {:?}", other),
        }
        match &got[1].value {
            Entry::Lookup { table, key, value } => {
                assert_eq!(table, "long");
                assert_eq!(key, "bar");
                assert_eq!(value, "bar-long");
            }
            other => panic!("expected Lookup, got {:?}", other),
        }
        match &got[2].value {
            Entry::AutoTemplate { name, pattern, postings, condition } => {
                assert_eq!(name, "mirror");
                assert!(condition.is_none());
                assert_eq!(pattern, "^x:$1:$segment:$2:$segment$");
                assert_eq!(postings.len(), 2);
                assert_eq!(postings[0].account, "$1:z:long[$2]");
                assert!(postings[0].is_virtual && postings[0].balanced);
            }
            other => panic!("expected AutoTemplate, got {:?}", other),
        }
        match &got[3].value {
            Entry::AutoInstance { name, args } => {
                assert_eq!(name, "mirror");
                assert_eq!(args, &vec!["foo".to_string(), "bar".to_string()]);
            }
            other => panic!("expected AutoInstance, got {:?}", other),
        }
    }

    #[test]
    fn lookup_entry_is_told_apart_from_template_by_bracket() {
        // `NAME[key] :: value` → a lookup entry; the bracket is the discriminator.
        let got = parse("= tbl[k] :: some-value\n").unwrap();
        assert!(matches!(&got[0].value,
            Entry::Lookup { table, key, value }
                if table == "tbl" && key == "k" && value == "some-value"));
        // An empty value is rejected.
        assert!(parse("= tbl[k] ::\n").is_err());
    }

    #[test]
    fn parse_template_with_amount_clause_and_unbalanced_posting() {
        use crate::parser::entry::CompareOp;
        // `amount > 0` clause after the pattern; a lone `(...)` unbalanced posting.
        let got = parse("= mirror :: /^x:$1-$segment:$2-$segment$/ amount > 0\n\t($1:z:$2)  1\n")
            .unwrap();
        match &got[0].value {
            Entry::AutoTemplate { pattern, condition, postings, .. } => {
                assert_eq!(pattern, "^x:$1-$segment:$2-$segment$");
                let c = condition.as_ref().expect("amount clause parsed");
                assert_eq!(c.op, CompareOp::Gt);
                assert!(c.value.is_zero());
                assert!(postings[0].is_virtual && !postings[0].balanced);
            }
            other => panic!("expected AutoTemplate, got {:?}", other),
        }
    }

    #[test]
    fn anonymous_auto_rule_still_parses_with_slash() {
        // `= /pattern/` must stay an anonymous auto-rule, not an instantiation.
        let got = parse("= /^assets:cash/\n\t[assets:cash]  -1\n\t[x]  1\n").unwrap();
        assert!(matches!(got[0].value, Entry::AutoRule(_)));
    }

    #[test]
    fn define_keyword_is_no_longer_a_directive() {
        // The `define` block was replaced by `= NAME[key] :: value` lookup
        // entries; `define` is now just an unknown directive.
        assert!(parse("define foo\n").is_err());
        assert!(parse("define foo = bar\n").is_err());
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
    fn parse_commodity_with_aliases() {
        let src = "commodity USD\n    alias $\n    alias USdollar\n";
        let got = parse(src).unwrap();
        match &got[0].value {
            Entry::Commodity { symbol, aliases, parities, precision } => {
                assert_eq!(symbol, "USD");
                assert_eq!(aliases, &vec!["$".to_string(), "USdollar".to_string()]);
                assert!(parities.is_empty());
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
            Entry::Commodity { symbol, aliases, parities, precision } => {
                assert_eq!(symbol, "EUR");
                assert_eq!(aliases, &vec!["€".to_string()]);
                assert!(parities.is_empty());
                assert_eq!(*precision, Some(2));
            }
            _ => panic!("expected Commodity"),
        }
    }

    #[test]
    fn parse_commodity_with_parity() {
        // `parity $` under a commodity records a fixed 1:1 target, distinct
        // from an alias — the commodity keeps its own symbol.
        let src = "commodity USDC\n    parity $\n    precision 2\n";
        let got = parse(src).unwrap();
        match &got[0].value {
            Entry::Commodity { symbol, aliases, parities, precision } => {
                assert_eq!(symbol, "USDC");
                assert!(aliases.is_empty());
                assert_eq!(parities, &vec!["$".to_string()]);
                assert_eq!(*precision, Some(2));
            }
            _ => panic!("expected Commodity"),
        }
    }

    #[test]
    fn parse_account_with_slippage_gain() {
        let src = "account Equity:SlippageGain\n    slippage gain\n";
        let got = parse(src).unwrap();
        assert!(matches!(&got[0].value,
            Entry::RoleAccount { role, account } if role == "slippage gain" && account == "Equity:SlippageGain"));
    }

    #[test]
    fn parse_account_with_multiword_role() {
        // Roles are matched generically — a three-word role parses too.
        let src = "account Equity:Foo\n    capital gain longterm\n";
        let got = parse(src).unwrap();
        assert!(matches!(&got[0].value,
            Entry::RoleAccount { role, account }
                if role == "capital gain longterm" && account == "Equity:Foo"));
    }

    #[test]
    fn parse_account_role_needs_two_words() {
        let src = "account Equity:Foo\n    capital\n";
        assert!(parse(src).is_err());
    }

    #[test]
    fn parse_account_with_multiple_sub_directives() {
        // An account may carry more than one sub-directive; each becomes
        // its own RoleAccount sharing the account name.
        let src = "account foo:1\n    label base\n    label-register reg\n";
        let got = parse(src).unwrap();
        let roles: Vec<_> = got
            .iter()
            .filter_map(|e| match &e.value {
                Entry::RoleAccount { role, account } => Some((role.as_str(), account.as_str())),
                _ => None,
            })
            .collect();
        assert_eq!(
            roles,
            vec![("label base", "foo:1"), ("label-register reg", "foo:1")]
        );
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
