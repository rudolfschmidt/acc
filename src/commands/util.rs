//! Shared rendering helpers for the commander subcommands.
//!
//! `print_spaces` is the single source of column-alignment: every
//! commander that needs padding calls this helper so layouts stay
//! consistent. `format_amount` is the canonical amount renderer:
//! commodity-first, per-commodity precision, "-0.00" suppressed.

use std::collections::HashMap;

use crate::decimal::Decimal;
use crate::parser::posting::Posting;

/// Account column content, matching ledger's print/reg output (verified
/// against ledger 3.4.1): a real posting prints its bare `account`, a
/// balanced-virtual one `[account]`, a paren-virtual one `(account)`.
/// Shared by `print`, `register` and `format` so all three render
/// virtual postings identically.
pub(crate) fn render_account(p: &Posting) -> String {
    match (p.is_virtual, p.balanced) {
        (true, true) => format!("[{}]", p.account),
        (true, false) => format!("({})", p.account),
        (false, _) => p.account.clone(),
    }
}

/// Print `n` space characters to stdout. No-op when `n == 0`.
pub(crate) fn print_spaces(n: usize) {
    if n > 0 {
        print!("{}", " ".repeat(n));
    }
}

/// Append `n` space characters to a `String` buffer. Same idea as
/// `print_spaces` but for commands that build up output in memory
/// (e.g. `format`) instead of streaming directly to stdout.
pub(crate) fn push_spaces(out: &mut String, n: usize) {
    out.reserve(n);
    for _ in 0..n {
        out.push(' ');
    }
}

/// True if `value` rounds to a non-zero amount at `commodity`'s display
/// precision — i.e. it would actually print. The single predicate every
/// commander uses to hide display-zero balance lines, so the "would this
/// show?" rule (and its `unwrap_or(2)` default) lives in one place.
pub(crate) fn shows_nonzero(
    commodity: &str,
    value: &Decimal,
    precisions: &HashMap<String, usize>,
) -> bool {
    let prec = precisions.get(commodity).copied().unwrap_or(2);
    !value.is_display_zero(prec)
}

/// Render an amount as `{commodity}{value}` at the display precision
/// for that commodity (falls back to 2 when unknown). Suppresses the
/// cosmetic `-0.00` that would otherwise appear for values that round
/// to zero but carry a negative mantissa.
pub(crate) fn format_amount(
    commodity: &str,
    value: &Decimal,
    precisions: &HashMap<String, usize>,
) -> String {
    let prec = precisions.get(commodity).copied().unwrap_or(2);
    let formatted = value.format_decimal(prec);
    let formatted = if let Some(without_minus) = formatted.strip_prefix('-') {
        if without_minus.chars().all(|c| c == '0' || c == '.') {
            without_minus.to_string()
        } else {
            formatted
        }
    } else {
        formatted
    };
    format!("{}{}", commodity, formatted)
}
