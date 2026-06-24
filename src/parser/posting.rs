use crate::date::Date;
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
    /// Acquisition date of the lot, in `[YYYY-MM-DD]` form. Either
    /// written by the user (only allowed alongside a `{cost}`) or set by
    /// the lotter when it splits a disposal per lot, so `print`/`reg` can
    /// show which lot each leg closed. Display-only — no computation.
    pub lot_date: Option<Date>,
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

/// Ledger lot-cost annotation captured from `{...}` / `{{...}}`.
///
/// - per-unit `{COST}` — `amount` is the cost of one unit (`total == false`).
/// - total `{{TOTAL}}` — `amount` is the cost of the whole lot
///   (`total == true`), like `@@` is to `@`.
/// - the `=` prefix (`{=COST}` / `{{=TOTAL}}`) locks the cost in; acc
///   records it in `fixed` but treats locked and floating the same for
///   balance and gain computation (there is no `--revalued` report).
#[derive(Debug, Clone)]
pub struct LotCost {
    pub amount: Amount,
    /// `{{...}}` total-cost form (the whole lot) vs per-unit `{...}`.
    pub total: bool,
    /// `{=...}` locked vs floating `{...}`. Informational.
    pub fixed: bool,
}

impl LotCost {
    /// The lot's balance weight for a leg of `qty` units, in the cost
    /// commodity. Per-unit cost scales by quantity; a total cost is the
    /// whole-lot figure itself, carrying the sign of `qty` (a disposal
    /// removes the lot, an acquisition adds it).
    pub fn weight(&self, qty: Decimal) -> Decimal {
        if self.total {
            if qty.is_negative() {
                Decimal::zero() - self.amount.value
            } else {
                self.amount.value
            }
        } else {
            qty.mul_rounded(self.amount.value)
        }
    }
}
