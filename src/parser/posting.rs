use crate::decimal::Decimal;

use super::comment::Comment;
use super::located::Located;

/// A single posting line inside a transaction — one debit or credit.
///
/// `amount` is `None` for the "omitted amount" posting whose value is
/// inferred later by the balancer. `costs` and `balance_assertion` are
/// optional Ledger annotations (`@`/`@@` and `=`).
///
/// `lot_cost` carries the Ledger `{COST}` annotation (cost basis per
/// unit, e.g. `{BTC 0.0904}`). When present, the booker uses the lot
/// cost — not the `@` market cost — to compute the balance effective
/// amount, which matches Ledger's sell-from-lot semantics. The `@`
/// market cost still participates in rebalance/reports.
#[derive(Debug, Clone)]
pub struct Posting {
    pub account: String,
    pub amount: Option<Amount>,
    pub costs: Option<Costs>,
    pub lot_cost: Option<LotCost>,
    pub balance_assertion: Option<Amount>,
    pub is_virtual: bool,
    pub balanced: bool,
    pub comments: Vec<Located<Comment>>,
}

/// A numeric value paired with a commodity symbol (e.g. `$100.50`, `10 AAPL`).
///
/// `decimals` records how many fractional digits the user wrote in the
/// source — e.g. `5.00` has `decimals = 2` even though the `Decimal`
/// value is `5`. This drives display precision: reports maximise
/// `decimals` over all observed amounts per commodity.
///
#[derive(Debug, Clone)]
pub struct Amount {
    pub commodity: String,
    pub value: Decimal,
    pub decimals: usize,
}

/// Price annotation attached to a posting: either per unit (`@`) or
/// for the total (`@@`).
#[derive(Debug, Clone)]
pub enum Costs {
    Total(Amount),
    PerUnit(Amount),
}

/// Ledger lot-price annotation captured from `{...}`.
///
/// - `Floating` — plain `{COST}`: the lot price may be revalued
///   against a later market rate in `--revalued` reports.
/// - `Fixed` — `{=COST}`: the lot price is locked in, used as the
///   cost-basis for realized-gain tracking.
///
/// The `{{TOTAL}}` (total instead of per-unit) form is not modelled;
/// the parser consumes and discards it.
#[derive(Debug, Clone)]
pub enum LotCost {
    Floating(Amount),
    Fixed(Amount),
}

impl LotCost {
    /// Per-unit amount regardless of floating/fixed — what the booker
    /// needs for the balance-effective computation.
    pub fn amount(&self) -> &Amount {
        match self {
            LotCost::Floating(a) | LotCost::Fixed(a) => a,
        }
    }
}
