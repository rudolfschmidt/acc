//! Top-level record variants emitted by the parser.

use std::sync::Arc;

use crate::date::Date;
use crate::decimal::Decimal;

use super::transaction::Transaction;

/// One raw record from a journal file.
///
/// Block directives (`commodity`, `account`) carry their indented
/// sub-directives inline: a `commodity` block folds its `alias` children
/// into `Commodity.aliases`. `Account` is a scaffold that the indented
/// `fx gain`/`fx loss` sub-directive replaces with the corresponding
/// `FxGainAccount`/`FxLossAccount` entry. The parser accumulates these
/// by mutating the last emitted entry when a new indented line arrives,
/// which lets it remain state-less between lines.
///
/// Alias resolution and price-DB construction happen in the resolve
/// phase after parsing.
#[derive(Debug, Clone)]
pub enum Entry {
    Transaction(Transaction),
    Price(Price),

    /// `commodity SYMBOL` + any number of indented children:
    /// - `alias OTHER` → adds OTHER to `aliases`
    /// - `precision N` → sets the display precision override, overriding
    ///   the precision inferred from posting amounts in reports.
    Commodity {
        symbol: String,
        aliases: Vec<String>,
        precision: Option<usize>,
    },

    /// `account NAME` without (or before) a sub-directive. Acts as a
    /// scaffold that `handle_indented` replaces with `FxGainAccount`
    /// or `FxLossAccount` when the matching sub-directive arrives. If
    /// no sub-directive follows, the entry stays and resolve drops it.
    Account(String),

    /// Produced when `account NAME` is followed by indented `fx gain`.
    FxGainAccount(String),

    /// Produced when `account NAME` is followed by indented `fx loss`.
    FxLossAccount(String),

    /// Produced when `account NAME` is followed by indented
    /// `cta gain`. Target for positive Cumulative Translation
    /// Adjustments — the drift absorbed when a transit account
    /// nets to zero in native but gained value in the `-x` target
    /// currency over its holding period.
    CtaGainAccount(String),

    /// Produced when `account NAME` is followed by indented
    /// `cta loss`. Target for negative Cumulative Translation
    /// Adjustments — symmetric counterpart to `cta gain`.
    CtaLossAccount(String),

    /// A top-level comment line (`;` or `#` at column 0).
    Comment(String),

    /// Automated-transaction rule: a pattern that matches against
    /// posting accounts, plus the extra postings to inject (scaled by
    /// the matching posting's amount) into every matching transaction.
    /// Line-leading `=` at column 0, followed by `/pattern/`; indented
    /// children provide the postings with their multipliers.
    AutoRule(AutoRule),
}

/// An auto-transaction (`= /pattern/`) block.
#[derive(Debug, Clone)]
pub struct AutoRule {
    pub pattern: AutoPattern,
    pub postings: Vec<AutoPosting>,
}

/// Pattern kinds supported in V1 — a subset of ledger-cli regex
/// semantics, matching what the filter DSL already handles: a
/// `^prefix` anchor, a `suffix$` anchor, an anchored-both `^exact$`,
/// or an unanchored substring. Full regex engine deferred until a
/// real user journal needs it.
#[derive(Debug, Clone)]
pub enum AutoPattern {
    Prefix(String),
    Suffix(String),
    Exact(String),
    Contains(String),
}

impl AutoPattern {
    pub fn matches(&self, account: &str) -> bool {
        match self {
            AutoPattern::Prefix(s) => account.starts_with(s.as_str()),
            AutoPattern::Suffix(s) => account.ends_with(s.as_str()),
            AutoPattern::Exact(s) => account == s,
            AutoPattern::Contains(s) => account.contains(s.as_str()),
        }
    }
}

/// One posting inside an auto-rule. Account + multiplier + virtual
/// flags mirror the posting syntax; the multiplier is applied to the
/// triggering posting's amount during expansion.
#[derive(Debug, Clone)]
pub struct AutoPosting {
    pub account: String,
    pub multiplier: crate::decimal::Decimal,
    pub is_virtual: bool,
    pub balanced: bool,
}

/// A `P DATE BASE QUOTE RATE` directive. Commodities are stored as
/// `Arc<str>` so the resolver (and downstream phases) can intern them
/// without cloning string buffers. Alias resolution is deferred to the
/// resolve phase.
#[derive(Debug, Clone)]
pub struct Price {
    pub date: Date,
    pub base: Arc<str>,
    pub quote: Arc<str>,
    pub rate: Decimal,
}
