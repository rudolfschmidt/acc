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

    /// A top-level comment line (`;` or `#` at column 0).
    Comment(String),
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
