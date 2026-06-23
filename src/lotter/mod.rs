//! Lotter phase — realized capital gains via FIFO lot tracking.
//!
//! Runs after the realizer, whenever the journal declares both a
//! `capital gain` and a `capital loss` account. It walks transactions
//! chronologically and tracks a per-(account, commodity) FIFO queue of
//! lots: an exchange posting that *acquires* a commodity opens a lot at
//! its cost, a posting that *disposes* of it closes lots front-to-back
//! and realizes the gain (proceeds − cost basis).
//!
//! Two valuation modes, switched by `-X`:
//!
//! - **With `-X TARGET`** — every leg is valued at the market rate
//!   (price DB) on its transaction date. The target commodity is
//!   "money": it carries no lots. The realized gain is then the
//!   *market movement over the holding period* — the trade-day
//!   execution deviation is booked separately by the realizer (fx
//!   gain/loss). `capital` + `fx` together are the full profit.
//!
//! - **Without `-X`** — legs are valued at the booked exchange rate
//!   (an explicit `@` cost, or the implied rate of a two-commodity
//!   trade). The realized gain is then the *total* gain straight from
//!   the books (proceeds − what you paid). Only works when cost and
//!   proceeds share a commodity; a mixed-currency disposal (bought in
//!   EUR, sold in USD) is skipped — it needs `-X` to net.
//!
//! Each realized gain becomes a self-balancing bracket-virtual
//! transaction, analogous to the translator's CTA release:
//!
//! ```text
//! <date> * capital gain
//!     [<lot-account>]      +gain  CUR
//!     [<capital-account>]  -gain  CUR
//! ```
//!
//! The lot account's bracket posting drives its target balance back to
//! zero (the realized portion leaves the asset), and the gain lands on
//! the declared `capital` account (income for a gain, expense for a
//! loss). Positive `gain` → `capital gain`; negative → `capital loss`.
//!
//! `realize_capital` returns the set of (account, commodity) pairs it
//! realized a gain on, so the translator can exclude them from CTA —
//! both would otherwise book the same holding-period drift and
//! double-count.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use crate::date::Date;
use crate::decimal::Decimal;
use crate::indexer::Index;
use crate::parser::located::Located;
use crate::parser::posting::{Amount, Costs, Posting};
use crate::parser::transaction::{State, Transaction};

/// One open lot: a remaining quantity carrying its per-unit cost and
/// the date it was acquired (FIFO order is insertion order; the date
/// feeds the holding period and the title).
struct Lot {
    qty: Decimal,
    cost_per_unit: Decimal,
    cost_commodity: String,
    date: Date,
}

/// A realized gain (or loss, when negative) from closing *one* lot,
/// ready to be booked. Beyond the booked amount it carries the detail
/// the title narrates: the closed quantity and traded commodity, the
/// lot's acquisition date, and the buy/sell unit prices.
struct Gain {
    /// Index of the disposal transaction that triggered this gain, so
    /// the booked capital tx can be inserted right after it.
    trigger: usize,
    date: Date,
    file: Arc<str>,
    line: usize,
    account: String,
    amount: Decimal,
    commodity: String,
    qty: Decimal,
    traded: String,
    cost_per_unit: Decimal,
    proceeds_per_unit: Decimal,
    lot_date: Date,
}

/// Track lots FIFO and inject one capital-gain/loss transaction per
/// realized disposal. Returns the (account, commodity) pairs a gain was
/// realized on, for the translator to exclude from CTA. See module docs
/// for the valuation semantics.
pub fn realize_capital(
    txs: &mut Vec<Located<Transaction>>,
    capital_gain: &str,
    capital_loss: &str,
    target: Option<&str>,
    db: &Index,
    precisions: &HashMap<String, usize>,
) -> HashSet<(String, String)> {
    let mut lots: HashMap<(String, String), VecDeque<Lot>> = HashMap::new();
    let mut tracked: HashSet<(String, String)> = HashSet::new();
    let mut gains: Vec<Gain> = Vec::new();

    for (idx, lt) in txs.iter().enumerate() {
        // Native per-commodity sums over balance-contributing postings.
        // Drive both the "is this a trade?" test and the implied rate.
        let mut sums: HashMap<String, Decimal> = HashMap::new();
        for lp in &lt.value.postings {
            if !contributes(&lp.value) {
                continue;
            }
            if let Some(a) = &lp.value.amount {
                *sums.entry(a.commodity.clone()).or_insert(Decimal::zero()) += a.value;
            }
        }
        // Single-commodity transactions can't realize a capital gain —
        // there is no exchange, hence no cost basis vs proceeds.
        if sums.len() < 2 {
            continue;
        }
        let date = lt.value.date.to_string();

        for lp in &lt.value.postings {
            if !contributes(&lp.value) {
                continue;
            }
            let Some(a) = &lp.value.amount else { continue };
            // With `-X` the target commodity is money: it neither opens
            // lots nor realizes gains. Only non-target assets do.
            if let Some(t) = target {
                if a.commodity == t {
                    continue;
                }
            }
            let Some((unit_value, value_commodity)) =
                posting_value(&lp.value, &sums, target, db, &date)
            else {
                continue;
            };
            let key = (lp.value.account.clone(), a.commodity.clone());

            if a.value > Decimal::zero() {
                // Acquisition: open a lot at this unit cost.
                lots.entry(key).or_default().push_back(Lot {
                    qty: a.value,
                    cost_per_unit: unit_value,
                    cost_commodity: value_commodity,
                    date: lt.value.date,
                });
            } else if a.value < Decimal::zero() {
                // Disposal: close lots FIFO. Each closed lot is realized
                // as its *own* gain so its acquisition date and holding
                // period stay distinct (a single large sale spanning two
                // lots yields two capital transactions).
                let Some(queue) = lots.get_mut(&key) else { continue };
                let prec = precisions.get(&value_commodity).copied().unwrap_or(2);
                let mut remaining = a.value.abs();
                while remaining > Decimal::zero() {
                    let Some(front) = queue.front_mut() else { break };
                    let take = if remaining < front.qty {
                        remaining
                    } else {
                        front.qty
                    };
                    // A lot whose cost commodity differs from the proceeds
                    // commodity (mixed-currency, native mode) can't be
                    // netted — skip its gain but still consume the lot.
                    if front.cost_commodity == value_commodity {
                        let proceeds = take.mul_rounded(unit_value);
                        let cost = take.mul_rounded(front.cost_per_unit);
                        let gain = proceeds - cost;
                        if !gain.is_display_zero(prec) {
                            tracked.insert(key.clone());
                            gains.push(Gain {
                                trigger: idx,
                                date: lt.value.date,
                                file: lt.file.clone(),
                                line: lt.line,
                                account: key.0.clone(),
                                amount: gain,
                                commodity: value_commodity.clone(),
                                qty: take,
                                traded: a.commodity.clone(),
                                cost_per_unit: front.cost_per_unit,
                                proceeds_per_unit: unit_value,
                                lot_date: front.date,
                            });
                        }
                    }
                    front.qty = front.qty - take;
                    remaining = remaining - take;
                    if front.qty.is_zero() {
                        queue.pop_front();
                    }
                }
                // remaining > 0 here means a short/over-sell with no lot
                // to match — left unrealized (no cost basis to book).
            }
        }
    }

    // Insert each capital tx immediately after the disposal that
    // triggered it, so `print`/`reg` show the gain right below its sale
    // (a lot's gain otherwise floats to the end of the day's group).
    // Multiple lots closed by one sale keep FIFO order. The downstream
    // sorter is stable, so this relative order survives a date sort.
    let with_target = target.is_some();
    let mut by_trigger: HashMap<usize, Vec<Located<Transaction>>> = HashMap::new();
    for g in &gains {
        by_trigger
            .entry(g.trigger)
            .or_default()
            .push(build_capital_tx(g, capital_gain, capital_loss, precisions, with_target));
    }
    let original = std::mem::take(txs);
    for (idx, lt) in original.into_iter().enumerate() {
        txs.push(lt);
        if let Some(caps) = by_trigger.remove(&idx) {
            txs.extend(caps);
        }
    }

    tracked
}

/// Balance-contributing postings: real and bracket-virtual `[account]`.
/// Paren-virtual `(account)` (the realizer's fx labels) stays out.
fn contributes(p: &Posting) -> bool {
    !p.is_virtual || p.balanced
}

/// Per-unit value of a posting in its cost/proceeds commodity.
///
/// - With `-X TARGET`: the market rate to target on `date` (price DB).
/// - Without `-X`: an explicit `@` cost if present, else the implied
///   rate of a two-commodity exchange (the other leg's sum over this
///   leg's sum). `None` when no rate is derivable.
fn posting_value(
    p: &Posting,
    sums: &HashMap<String, Decimal>,
    target: Option<&str>,
    db: &Index,
    date: &str,
) -> Option<(Decimal, String)> {
    let a = p.amount.as_ref()?;
    if let Some(t) = target {
        let rate = db.find(&a.commodity, t, date)?;
        return Some((rate, t.to_string()));
    }
    // Native: an explicit cost annotation wins.
    if let Some(costs) = &p.costs {
        return Some(match costs {
            Costs::PerUnit(c) => (c.value, c.commodity.clone()),
            Costs::Total(c) => (c.value.div_rounded(a.value.abs()), c.commodity.clone()),
        });
    }
    // Native: implied rate of a clean two-commodity exchange.
    if sums.len() != 2 {
        return None;
    }
    let other = sums.keys().find(|k| k.as_str() != a.commodity)?;
    let this_sum = sums.get(&a.commodity)?.abs();
    if this_sum.is_zero() {
        return None;
    }
    let other_sum = sums.get(other)?.abs();
    Some((other_sum.div_rounded(this_sum), other.clone()))
}

fn build_capital_tx(
    g: &Gain,
    capital_gain: &str,
    capital_loss: &str,
    precisions: &HashMap<String, usize>,
    with_target: bool,
) -> Located<Transaction> {
    let (capital_account, kind) = if g.amount.is_negative() {
        (capital_loss, "capital loss")
    } else {
        (capital_gain, "capital gain")
    };
    // Title narrates the closed lot, separated once by `|`:
    //   capital gain | BTC0.0005, 2018-01-18 @ USD9351.31 → USD10228.30, 1d
    // Amounts use acc's canonical commodity-first rendering (quantity in
    // the traded commodity, prices in the gain commodity). Holding period
    // is whole days, lot date → sell date.
    use crate::commands::util::format_amount;
    let held = g.lot_date.days_until(g.date);
    let description = format!(
        "{} | {}, {} @ {} → {}, {}d",
        kind,
        format_amount(&g.traded, &g.qty, precisions),
        g.lot_date,
        format_amount(&g.commodity, &g.cost_per_unit, precisions),
        format_amount(&g.commodity, &g.proceeds_per_unit, precisions),
        held,
    );
    let precision = precisions.get(&g.commodity).copied().unwrap_or(2);
    let mk = |account: String, value: Decimal, balanced: bool| Located {
        file: g.file.clone(),
        line: g.line,
        value: Posting {
            account,
            amount: Some(Amount {
                commodity: g.commodity.clone(),
                value,
                decimals: precision,
            }),
            costs: None,
            lot_cost: None,
            balance_assertion: None,
            is_virtual: true,
            balanced,
            comments: Vec::new(),
        },
    };
    let postings = if with_target {
        // With `-X` the asset carries a target-valued balance (cost at
        // buy-rate, proceeds at sell-rate); the realized gain is its
        // holding-period drift. Bracket-virtual + self-balancing drives
        // that drift to zero on the asset and lands the gain on the
        // capital account — the same mechanic as the CTA translator,
        // booked as income/expense. `+gain` on the asset, `-gain` on
        // capital (income → negative; loss → positive).
        vec![
            mk(g.account.clone(), g.amount, true),
            mk(capital_account.to_string(), -g.amount, true),
        ]
    } else {
        // Without `-X` the asset is already flat natively (bought and
        // sold), and the realized gain already sits in the cash leg of
        // the disposal. There is no target drift to neutralize, so a
        // bracket leg on the asset would inflate it with a phantom
        // balance. A single paren-virtual posting reclassifies the gain
        // as capital without touching the asset.
        vec![mk(capital_account.to_string(), -g.amount, false)]
    };
    Located {
        file: g.file.clone(),
        line: g.line,
        value: Transaction {
            date: g.date,
            state: State::Cleared,
            code: None,
            description,
            postings,
            comments: Vec::new(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;
    use crate::resolver;

    fn setup(src: &str) -> (Vec<Located<Transaction>>, Index, HashMap<String, usize>) {
        let entries = parser::parse(src).unwrap();
        let resolved = resolver::resolve(entries).unwrap();
        let prices = crate::indexer::index(resolved.prices);
        let txs = crate::booker::book(resolved.transactions).unwrap();
        let mut precisions: HashMap<String, usize> = HashMap::new();
        for lt in &txs {
            for lp in &lt.value.postings {
                if let Some(a) = &lp.value.amount {
                    let e = precisions.entry(a.commodity.clone()).or_insert(0);
                    if a.decimals > *e {
                        *e = a.decimals;
                    }
                }
            }
        }
        (txs, prices, precisions)
    }

    fn find_capital(txs: &[Located<Transaction>]) -> Vec<&Transaction> {
        txs.iter()
            .map(|lt| &lt.value)
            .filter(|t| t.description.starts_with("capital"))
            .collect()
    }

    #[test]
    fn native_total_gain_without_x() {
        // BTC bought for 30000 USD, sold for 50000 USD. No -X: the
        // native total gain straight from the books = 20000 USD.
        let src = "\
            2024-01-01 buy\n\
            \tassets:btc    1 BTC\n\
            \tassets:cash  -30000 USD\n\
            2024-06-01 sell\n\
            \tassets:btc   -1 BTC\n\
            \tassets:cash   50000 USD\n";
        let (mut txs, db, prec) = setup(src);
        let tracked = realize_capital(&mut txs, "in:capital", "ex:capital", None, &db, &prec);
        let caps = find_capital(&txs);
        assert_eq!(caps.len(), 1);
        let cap = caps[0];
        assert!(cap.description.starts_with("capital gain"));
        // capital account posting carries -gain (income, negative).
        let cap_posting = cap
            .postings
            .iter()
            .find(|lp| lp.value.account == "in:capital")
            .unwrap();
        assert_eq!(
            cap_posting.value.amount.as_ref().unwrap().value,
            Decimal::parse("-20000").unwrap()
        );
        assert_eq!(
            cap_posting.value.amount.as_ref().unwrap().commodity,
            "USD"
        );
        assert!(tracked.contains(&("assets:btc".to_string(), "BTC".to_string())));
    }

    #[test]
    fn market_movement_with_x() {
        // Bought 1 BTC paying 29000 USD on a day the market was 30000;
        // sold paying 51000 USD on a day the market was 48000. With -X
        // capital sees only the *market* movement: 48000 − 30000 = 18000.
        let src = "\
            P 2024-01-01 BTC USD 30000\n\
            P 2024-06-01 BTC USD 48000\n\
            2024-01-01 buy\n\
            \tassets:btc    1 BTC\n\
            \tassets:cash  -29000 USD\n\
            2024-06-01 sell\n\
            \tassets:btc   -1 BTC\n\
            \tassets:cash   51000 USD\n";
        let (mut txs, db, prec) = setup(src);
        realize_capital(&mut txs, "in:capital", "ex:capital", Some("USD"), &db, &prec);
        let caps = find_capital(&txs);
        assert_eq!(caps.len(), 1);
        let cap_posting = caps[0]
            .postings
            .iter()
            .find(|lp| lp.value.account == "in:capital")
            .unwrap();
        assert_eq!(
            cap_posting.value.amount.as_ref().unwrap().value,
            Decimal::parse("-18000").unwrap()
        );
    }

    #[test]
    fn fifo_partial_sale() {
        // Buy 1 BTC @30000, buy 1 BTC @40000, sell 1 BTC @50000. FIFO
        // takes the first lot: gain = 50000 − 30000 = 20000.
        let src = "\
            2024-01-01 buy a\n\
            \tassets:btc    1 BTC\n\
            \tassets:cash  -30000 USD\n\
            2024-02-01 buy b\n\
            \tassets:btc    1 BTC\n\
            \tassets:cash  -40000 USD\n\
            2024-06-01 sell\n\
            \tassets:btc   -1 BTC\n\
            \tassets:cash   50000 USD\n";
        let (mut txs, db, prec) = setup(src);
        realize_capital(&mut txs, "in:capital", "ex:capital", None, &db, &prec);
        let caps = find_capital(&txs);
        assert_eq!(caps.len(), 1);
        let cap_posting = caps[0]
            .postings
            .iter()
            .find(|lp| lp.value.account == "in:capital")
            .unwrap();
        assert_eq!(
            cap_posting.value.amount.as_ref().unwrap().value,
            Decimal::parse("-20000").unwrap()
        );
    }

    #[test]
    fn loss_routes_to_loss_account() {
        // Bought for 50000, sold for 30000: a 20000 loss → ex:capital,
        // positive (expense).
        let src = "\
            2024-01-01 buy\n\
            \tassets:btc    1 BTC\n\
            \tassets:cash  -50000 USD\n\
            2024-06-01 sell\n\
            \tassets:btc   -1 BTC\n\
            \tassets:cash   30000 USD\n";
        let (mut txs, db, prec) = setup(src);
        realize_capital(&mut txs, "in:capital", "ex:capital", None, &db, &prec);
        let caps = find_capital(&txs);
        assert_eq!(caps.len(), 1);
        assert!(caps[0].description.starts_with("capital loss"));
        let cap_posting = caps[0]
            .postings
            .iter()
            .find(|lp| lp.value.account == "ex:capital")
            .unwrap();
        assert_eq!(
            cap_posting.value.amount.as_ref().unwrap().value,
            Decimal::parse("20000").unwrap()
        );
    }

    #[test]
    fn no_gain_no_transaction() {
        // Buy and sell at the same price: no realized gain, nothing booked.
        let src = "\
            2024-01-01 buy\n\
            \tassets:btc    1 BTC\n\
            \tassets:cash  -30000 USD\n\
            2024-06-01 sell\n\
            \tassets:btc   -1 BTC\n\
            \tassets:cash   30000 USD\n";
        let (mut txs, db, prec) = setup(src);
        let tracked = realize_capital(&mut txs, "in:capital", "ex:capital", None, &db, &prec);
        assert!(find_capital(&txs).is_empty());
        assert!(tracked.is_empty());
    }

    #[test]
    fn without_x_leaves_asset_untouched() {
        // Native mode: the gain is booked single-sided (paren-virtual)
        // on the capital account only — no phantom leg on the asset.
        let src = "\
            2024-01-01 buy\n\
            \tassets:btc    1 BTC\n\
            \tassets:cash  -100 USD\n\
            2024-06-01 sell\n\
            \tassets:btc   -1 BTC\n\
            \tassets:cash   150 USD\n";
        let (mut txs, db, prec) = setup(src);
        realize_capital(&mut txs, "in:capital", "ex:capital", None, &db, &prec);
        let caps = find_capital(&txs);
        assert_eq!(caps.len(), 1);
        // Exactly one posting, paren-virtual, on the capital account.
        assert_eq!(caps[0].postings.len(), 1);
        let p = &caps[0].postings[0].value;
        assert_eq!(p.account, "in:capital");
        assert!(p.is_virtual && !p.balanced);
    }

    #[test]
    fn with_x_balances_asset_to_zero() {
        // -X mode: two bracket-virtual legs (asset + capital) so the
        // asset's target balance is driven to zero.
        let src = "\
            P 2024-01-01 BTC USD 100\n\
            P 2024-06-01 BTC USD 150\n\
            2024-01-01 buy\n\
            \tassets:btc    1 BTC\n\
            \tassets:cash  -100 USD\n\
            2024-06-01 sell\n\
            \tassets:btc   -1 BTC\n\
            \tassets:cash   150 USD\n";
        let (mut txs, db, prec) = setup(src);
        realize_capital(&mut txs, "in:capital", "ex:capital", Some("USD"), &db, &prec);
        let caps = find_capital(&txs);
        assert_eq!(caps.len(), 1);
        assert_eq!(caps[0].postings.len(), 2);
        assert!(caps[0].postings.iter().all(|lp| lp.value.is_virtual && lp.value.balanced));
        assert!(caps[0].postings.iter().any(|lp| lp.value.account == "assets:btc"));
    }

    #[test]
    fn disposal_across_two_lots_yields_two_txs() {
        // A single 10-BTC sale closes lot 1 (4 BTC) and lot 2 (6 BTC):
        // two capital transactions, each with its own holding period.
        let src = "\
            2024-01-01 buy a\n\
            \tassets:btc    4 BTC\n\
            \tassets:cash  -400 USD\n\
            2024-02-01 buy b\n\
            \tassets:btc    6 BTC\n\
            \tassets:cash  -660 USD\n\
            2024-06-01 sell\n\
            \tassets:btc  -10 BTC\n\
            \tassets:cash  1200 USD\n";
        let (mut txs, db, prec) = setup(src);
        realize_capital(&mut txs, "in:capital", "ex:capital", None, &db, &prec);
        let caps = find_capital(&txs);
        assert_eq!(caps.len(), 2);
        // lot 1: 4×(120−100)=80, lot 2: 6×(120−110)=60.
        let total: i64 = caps
            .iter()
            .map(|t| {
                t.postings
                    .iter()
                    .find(|lp| lp.value.account == "in:capital")
                    .unwrap()
                    .value
                    .amount
                    .as_ref()
                    .unwrap()
                    .value
                    .to_f64() as i64
            })
            .sum();
        assert_eq!(total, -140); // −80 + −60 (income negative)
    }

    #[test]
    fn title_carries_lot_date_prices_and_holding_period() {
        let src = "\
            2024-01-01 buy\n\
            \tassets:btc    1 BTC\n\
            \tassets:cash  -100 USD\n\
            2024-06-01 sell\n\
            \tassets:btc   -1 BTC\n\
            \tassets:cash   150 USD\n";
        let (mut txs, db, prec) = setup(src);
        realize_capital(&mut txs, "in:capital", "ex:capital", None, &db, &prec);
        let caps = find_capital(&txs);
        assert_eq!(caps.len(), 1);
        let d = &caps[0].description;
        // capital gain | BTC1, 2024-01-01 @ USD100 → USD150, 152d
        assert!(d.starts_with("capital gain | "));
        assert!(d.contains("BTC1"));
        assert!(d.contains("2024-01-01"));
        assert!(d.contains("USD100"));
        assert!(d.contains("USD150"));
        assert!(d.contains("152d"));
    }

    #[test]
    fn open_position_never_realizes() {
        // Only a buy, no sell: no disposal, no gain.
        let src = "\
            2024-01-01 buy\n\
            \tassets:btc    1 BTC\n\
            \tassets:cash  -30000 USD\n";
        let (mut txs, db, prec) = setup(src);
        realize_capital(&mut txs, "in:capital", "ex:capital", None, &db, &prec);
        assert!(find_capital(&txs).is_empty());
    }
}
