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
    /// Maximum fractional digits observed for each commodity across
    /// every posting amount / cost / balance-assertion. Reports use
    /// this to render all amounts of a commodity consistently.
    pub precisions: HashMap<String, usize>,
}
