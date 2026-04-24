//! The `Journal` is the final product of the load pipeline. It is the
//! input every report command consumes.

use std::collections::HashMap;

use crate::indexer::Index;
use crate::parser::located::Located;
use crate::parser::transaction::Transaction;

/// A fully-processed journal: balanced, booked transactions in date
/// order, a ready-to-query price index, the FX-gain/loss account
/// declarations extracted during resolve, and a per-commodity
/// precision map derived from the postings.
#[derive(Debug)]
pub struct Journal {
    pub transactions: Vec<Located<Transaction>>,
    pub prices: Index,
    pub fx_gain: Option<String>,
    pub fx_loss: Option<String>,
    /// Account for positive Cumulative Translation Adjustments.
    /// Declared via `account NAME / cta gain`. Both `cta_gain` and
    /// `cta_loss` must be declared for the translator phase to run.
    pub cta_gain: Option<String>,
    /// Account for negative Cumulative Translation Adjustments.
    /// Declared via `account NAME / cta loss`.
    pub cta_loss: Option<String>,
    /// Maximum fractional digits observed for each commodity across
    /// every posting amount / cost / balance-assertion. Reports use
    /// this to render all amounts of a commodity consistently.
    pub precisions: HashMap<String, usize>,
    /// `alias → canonical`. Lets the CLI resolve `-x EUR` to `€` when
    /// the journal declared `commodity € / alias EUR`, so the target
    /// symbol matches the form stored in postings and the price DB.
    pub aliases: HashMap<String, String>,
    /// Automated-transaction rules from `= /pattern/` blocks. Expanded
    /// into their matching transactions by the expander phase between
    /// booker and filter.
    pub auto_rules: Vec<crate::parser::entry::AutoRule>,
}
