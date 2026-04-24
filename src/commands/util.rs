//! Shared rendering helpers for the commander subcommands.
//!
//! `print_spaces` is the single source of column-alignment: every
//! commander that needs padding calls this helper so layouts stay
//! consistent. `format_amount` is the canonical amount renderer:
//! commodity-first, per-commodity precision, "-0.00" suppressed.

use std::collections::HashMap;

use crate::decimal::Decimal;

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
