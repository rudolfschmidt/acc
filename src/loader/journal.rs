//! The `Journal` is the final product of the load pipeline. It is the
//! input every report command consumes.

use std::collections::HashMap;

use crate::indexer::Index;
use crate::parser::located::Located;
use crate::parser::transaction::Transaction;

/// A fully-processed journal: balanced, booked transactions in date
/// order, a ready-to-query price index, the role gain/loss account
/// declarations extracted during resolve, and a per-commodity
/// precision map derived from the postings.
#[derive(Debug)]
pub struct Journal {
    pub transactions: Vec<Located<Transaction>>,
    pub prices: Index,
    pub slippage_gain: Option<String>,
    pub slippage_loss: Option<String>,
    /// Account for positive Currency Translation Adjustments.
    /// Declared via `account NAME / cta gain`. Both `cta_gain` and
    /// `cta_loss` must be declared for the translator phase to run.
    pub cta_gain: Option<String>,
    /// Account for negative Currency Translation Adjustments.
    /// Declared via `account NAME / cta loss`.
    pub cta_loss: Option<String>,
    /// Realized-capital-gains accounts, declared via
    /// `account NAME / capital gain` / `capital loss`. Both must be
    /// declared for the lot/capital-gains phase to run.
    pub capital_gain: Option<String>,
    pub capital_loss: Option<String>,
    /// Unrealized mark-to-market accounts, declared via
    /// `account NAME / holding gain` / `holding loss`. Both must
    /// be declared for the `--unrealized` revaluator phase to run.
    pub holding_gain: Option<String>,
    pub holding_loss: Option<String>,
    /// Maximum fractional digits observed for each commodity across
    /// every posting amount / cost / balance-assertion. Reports use
    /// this to render all amounts of a commodity consistently.
    pub precisions: HashMap<String, usize>,
    /// `alias → canonical`. Lets the CLI resolve `-X EUR` to `€` when
    /// the journal declared `commodity € / alias EUR`, so the target
    /// symbol matches the form stored in postings and the price DB.
    pub aliases: HashMap<String, String>,
    /// Automated-transaction rules from `= /pattern/` blocks. Expanded
    /// into their matching transactions by the expander phase between
    /// booker and filter.
    pub auto_rules: Vec<crate::parser::entry::AutoRule>,
    /// Cosmetic display labels from `account NAME / label …` directives.
    /// `labels` is the shared fallback (bare `label`); the two view-specific
    /// sets (`label-balance` / `label-register`) override it per view. Each
    /// set holds exact full names and `$segment` wildcard patterns. Shown by
    /// `bal` / `reg`, never filtered or computed on. See [`Self::label_for`].
    pub labels: crate::resolver::LabelSet,
    pub labels_balance: crate::resolver::LabelSet,
    pub labels_register: crate::resolver::LabelSet,
}

/// Which view is asking for a label — selects the view-specific set that
/// overrides the shared `labels` fallback.
#[derive(Debug, Clone, Copy)]
pub enum LabelView {
    Balance,
    Register,
}

impl Journal {
    /// The display label for `account` in `view`: the view-specific set
    /// wins (exact name, then `$segment` pattern), else the shared `labels`
    /// fallback, else `None`.
    pub fn label_for(&self, account: &str, view: LabelView) -> Option<&str> {
        let specific = match view {
            LabelView::Balance => &self.labels_balance,
            LabelView::Register => &self.labels_register,
        };
        specific.get(account).or_else(|| self.labels.get(account))
    }
}
