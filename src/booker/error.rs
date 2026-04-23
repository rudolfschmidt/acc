use std::fmt;
use std::sync::Arc;

use crate::decimal::Decimal;

/// Booker-phase error. Carries enough source context (file, start+end
/// line, plus a structured kind) for the loader to render a
/// ledger-cli-style message with the offending transaction quoted back.
#[derive(Debug, Clone)]
pub struct BookError {
    pub file: Arc<str>,
    pub start_line: usize,
    pub end_line: usize,
    pub kind: BookErrorKind,
}

/// One residual commodity in an unbalanced transaction.
#[derive(Debug, Clone)]
pub struct Residual {
    pub commodity: String,
    pub value: Decimal,
    pub decimals: usize,
}

#[derive(Debug, Clone)]
pub enum BookErrorKind {
    /// Postings sum is non-zero in at least one commodity. Holds one
    /// entry per commodity that failed to net to zero.
    Unbalanced {
        residuals: Vec<Residual>,
    },
    /// More than one posting had no explicit amount.
    MultipleMissing,
    /// All postings lacked amounts — nothing to infer from.
    NoAmountsToInfer,
    /// `= TARGET` assertion did not hold after applying the posting.
    AssertionFailed {
        account: String,
        expected: Decimal,
        got: Decimal,
        commodity: String,
        decimals: usize,
    },
}

impl BookError {
    pub fn new(file: Arc<str>, start_line: usize, end_line: usize, kind: BookErrorKind) -> Self {
        Self { file, start_line, end_line, kind }
    }

    /// Single-line headline (with residual/assertion detail inlined in
    /// parentheses). Used as the `path:line: MESSAGE` header.
    pub fn headline(&self) -> String {
        match &self.kind {
            BookErrorKind::Unbalanced { residuals } => {
                let joined = residuals
                    .iter()
                    .map(|r| format!("{}{}", r.commodity, r.value.format_decimal(r.decimals)))
                    .collect::<Vec<_>>()
                    .join(", ");
                let label = if residuals.len() == 1 { "residual" } else { "residuals" };
                format!("transaction does not balance ({} {})", label, joined)
            }
            BookErrorKind::MultipleMissing => {
                "only one posting may omit its amount".to_string()
            }
            BookErrorKind::NoAmountsToInfer => {
                "transaction has no explicit amounts to infer from".to_string()
            }
            BookErrorKind::AssertionFailed {
                account, expected, got, commodity, decimals,
            } => {
                format!(
                    "balance assertion on `{}` failed (expected {}{}, got {}{})",
                    account,
                    commodity, expected.format_decimal(*decimals),
                    commodity, got.format_decimal(*decimals),
                )
            }
        }
    }
}

impl fmt::Display for BookError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        crate::error::render_range(
            f,
            &self.file,
            self.start_line,
            self.end_line,
            &self.headline(),
        )
    }
}

impl std::error::Error for BookError {}
