//! Posting-level secondary projections: the `--pos` / `--neg` sign filter
//! and the `--amount EXPR` threshold filter. Both narrow which postings
//! are *shown* after transaction selection — see [`crate::filter`] for
//! where they run in the phase.

use crate::parser::posting::Posting;

/// Posting-level sign filter, driven by `--pos` / `--neg`. Applied as a
/// secondary projection *after* transaction selection: it narrows which
/// postings are shown, by the sign of their amount. Zero counts as both
/// non-negative and non-positive, so a zero posting survives either flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SignFilter {
    /// No sign constraint.
    #[default]
    Any,
    /// `--pos`: keep postings whose amount is `>= 0`.
    NonNegative,
    /// `--neg`: keep postings whose amount is `<= 0`.
    NonPositive,
}

impl SignFilter {
    /// Resolve the two CLI flags. Both set (or neither) means no
    /// constraint — `>= 0` OR `<= 0` already covers every amount, so
    /// asking for both is the same as asking for nothing.
    pub fn from_flags(pos: bool, neg: bool) -> Self {
        match (pos, neg) {
            (true, false) => SignFilter::NonNegative,
            (false, true) => SignFilter::NonPositive,
            _ => SignFilter::Any,
        }
    }

    /// Whether a posting passes the sign constraint. A posting without an
    /// amount has no sign, so it is dropped when a constraint is active.
    ///
    /// `>= 0` is `!is_negative()` (already true for zero, since
    /// `is_negative` is strictly `< 0`); `<= 0` needs the explicit
    /// `is_zero()` because `is_negative()` excludes zero. The two
    /// overlap on zero, which is exactly why it shows under both flags.
    pub(super) fn keeps(&self, p: &Posting) -> bool {
        match self {
            SignFilter::Any => true,
            SignFilter::NonNegative => p.amount.as_ref().is_some_and(|a| !a.value.is_negative()),
            SignFilter::NonPositive => {
                p.amount.as_ref().is_some_and(|a| a.value.is_negative() || a.value.is_zero())
            }
        }
    }
}

/// Comparison operator for the amount filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CmpOp {
    Eq,
    Ne,
    Gt,
    Lt,
    Ge,
    Le,
}

/// Posting-level amount filter, driven by `--amount EXPR`. Like the sign
/// filter, it is a secondary projection applied *after* transaction
/// selection: it narrows which postings survive by comparing each
/// posting's (signed) amount against a threshold. A posting without an
/// amount has nothing to compare, so it is dropped.
///
/// `EXPR` is `[op]number` with `op` one of `>`, `<`, `>=`, `<=`, `=` and
/// `<>` ("not equal"); a bare number is `=`. The comparison is signed and
/// value-based, so `-A 100` keeps `100` but not `-100`, `-A -50` keeps
/// exactly `-50`, `-A '>100'` keeps amounts strictly above `100`, and
/// `-A '<>100'` keeps everything except `100`. Under `-X` the postings are
/// already valued in the target commodity, so the threshold is read in
/// that commodity too.
#[derive(Debug, Clone)]
pub struct AmountFilter {
    op: CmpOp,
    threshold: crate::decimal::Decimal,
}

impl AmountFilter {
    /// Parse an `EXPR` (`[op]number`). The two-char `>=` / `<=` / `<>` must
    /// be tested before the one-char `>` / `<`. The number is a signed
    /// decimal parsed by `Decimal::parse`, so `-A -50` and `-A '<=1.5'`
    /// both work. `Decimal` stores every value at one fixed scale, so the
    /// threshold compares equal to `100`, `100.00`, … alike.
    pub fn parse(expr: &str) -> Result<Self, String> {
        let s = expr.trim();
        let (op, rest) = if let Some(r) = s.strip_prefix(">=") {
            (CmpOp::Ge, r)
        } else if let Some(r) = s.strip_prefix("<=") {
            (CmpOp::Le, r)
        } else if let Some(r) = s.strip_prefix("<>") {
            (CmpOp::Ne, r)
        } else if let Some(r) = s.strip_prefix('>') {
            (CmpOp::Gt, r)
        } else if let Some(r) = s.strip_prefix('<') {
            (CmpOp::Lt, r)
        } else if let Some(r) = s.strip_prefix('=') {
            (CmpOp::Eq, r)
        } else {
            (CmpOp::Eq, s)
        };
        let threshold = crate::decimal::Decimal::parse(rest.trim())
            .map_err(|e| format!("invalid --amount `{expr}`: {e}"))?;
        Ok(Self { op, threshold })
    }

    /// Whether a posting passes the amount constraint. A posting without
    /// an amount is dropped — there is nothing to compare.
    pub(super) fn keeps(&self, p: &Posting) -> bool {
        let Some(a) = p.amount.as_ref() else {
            return false;
        };
        match self.op {
            CmpOp::Eq => a.value == self.threshold,
            CmpOp::Ne => a.value != self.threshold,
            CmpOp::Gt => a.value > self.threshold,
            CmpOp::Lt => a.value < self.threshold,
            CmpOp::Ge => a.value >= self.threshold,
            CmpOp::Le => a.value <= self.threshold,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amount_parse_ops_and_precedence() {
        // Two-char ops must win over the one-char prefixes.
        assert_eq!(AmountFilter::parse(">=5").unwrap().op, CmpOp::Ge);
        assert_eq!(AmountFilter::parse("<=5").unwrap().op, CmpOp::Le);
        assert_eq!(AmountFilter::parse("<>5").unwrap().op, CmpOp::Ne);
        assert_eq!(AmountFilter::parse(">5").unwrap().op, CmpOp::Gt);
        assert_eq!(AmountFilter::parse("<5").unwrap().op, CmpOp::Lt);
        assert_eq!(AmountFilter::parse("=5").unwrap().op, CmpOp::Eq);
        assert_eq!(AmountFilter::parse("5").unwrap().op, CmpOp::Eq);
    }
}
