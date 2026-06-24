//! Lotter phase — realized capital gains via FIFO lot tracking.
//!
//! Runs whenever the journal declares both a `capital gain` and a
//! `capital loss` account. It walks transactions chronologically and
//! tracks a per-(account, commodity) FIFO queue of lots: an exchange
//! posting that *acquires* a commodity opens a lot, a posting that
//! *disposes* of it closes lots front-to-back and realizes the gain.
//!
//! ## With `-X TARGET`: the holding-period market move
//!
//! Under conversion the lotter books exactly one thing: the **market
//! move** of the disposed commodity over its holding period, in the
//! target currency. A lot opens at the commodity's *market value on the
//! acquisition date* (price DB); a disposal realizes
//! `(market_sell − market_buy) × qty` as a `capital` gain/loss. The
//! disposal leg carries that acquisition-date market value as its `{}`
//! cost-basis, so the rebalancer values the asset at cost — the asset
//! account enters and leaves at the same value and nets to zero, no CTA
//! drift arises.
//!
//! The trade-day **execution spread** (where the booked rate diverges
//! from the market rate — slippage/fees) is *not* the lotter's job: the
//! [realizer](crate::realizer) books it as `fx` on every multi-commodity
//! transaction, buy and sell. The two compose: the lotter's `{cost}`
//! shifts the disposal leg by the market move, and its capital posting
//! offsets that shift exactly, so the realizer's fx stays valid and the
//! transaction still sums to zero.
//!
//! ## Without `-X`: the native trade gain
//!
//! Natively there is no market to compare against, so the lotter falls
//! back to the **booked** (trade) rate: a lot opens at its booked cost in
//! the counter-commodity, and a disposal realizes `proceeds − cost` as a
//! single `capital` posting. Mixed-currency native disposals (bought in
//! EUR, sold in USD) can't be netted and are skipped; they need `-X`.
//!
//! ## Injection
//!
//! The gain posting is **real** (not virtual) and lives inside the
//! disposal transaction, balancing against the `{}` cost-basis on the
//! asset legs. So `print` is 1:1 copy-pasteable and survives a reload.

use std::collections::{HashMap, VecDeque};

use crate::date::Date;
use crate::decimal::Decimal;
use crate::indexer::Index;
use crate::parser::located::Located;
use crate::parser::posting::{Amount, Costs, LotCost, Posting};
use crate::parser::transaction::Transaction;

/// One open lot: a remaining quantity carrying its per-unit cost and the
/// date it was acquired (FIFO order is insertion order; the date feeds
/// the holding period and the title).
///
/// `cost_per_unit` is the lot's cost in `cost_commodity`. Under `-X` that
/// is the commodity's *market value on the acquisition date*, in the
/// target currency; natively it is the *booked* (trade) rate in the
/// counter-commodity.
struct Lot {
    qty: Decimal,
    cost_per_unit: Decimal,
    cost_commodity: String,
    date: Date,
}

/// One lot closed by a disposal: how much was taken, at what cost, and
/// when it was acquired — enough to render one split leg.
struct ClosedLot {
    qty: Decimal,
    cost_per_unit: Decimal,
    cost_commodity: String,
    date: Date,
}

/// A disposal that realized a capital gain/loss. Phase 1 collects these
/// (immutably), phase 2 rewrites the originating posting into one leg
/// per closed lot and injects the gain. `posting_idx` locates the
/// disposal leg within its transaction.
///
/// `gain` is in `gain_commodity`: the target currency under `-X` (the
/// market move), or the counter-commodity natively (the trade gain). The
/// rebalancer converts it to the target at the disposal's own date.
struct Disposal {
    tx_idx: usize,
    posting_idx: usize,
    /// Traded commodity (e.g. ETH) and the source posting's decimals.
    commodity: String,
    decimals: usize,
    /// Virtual flags copied from the source leg so the split legs match.
    is_virtual: bool,
    balanced: bool,
    /// Sell price per unit (the proceeds rate) — used as the `@` cost on
    /// the split legs when the source had none.
    proceeds_per_unit: Decimal,
    gain: Decimal,
    gain_commodity: String,
    /// `true` when the closing posting was an acquisition (closing a
    /// short), `false` for a disposal (closing longs). Sets the sign of
    /// the rewritten legs.
    acquisition: bool,
    lots: Vec<ClosedLot>,
}

/// The accounts a realized gain/loss is booked to: `capital` for a gain
/// (income), the loss account for a loss (expense).
pub struct CapitalAccounts<'a> {
    pub capital_gain: &'a str,
    pub capital_loss: &'a str,
}

/// Track lots FIFO and inject one realized capital-gain/loss posting per
/// disposal. See module docs for the valuation semantics.
pub fn realize_capital(
    txs: &mut Vec<Located<Transaction>>,
    accounts: &CapitalAccounts,
    target: Option<&str>,
    db: &Index,
    precisions: &HashMap<String, usize>,
) {
    let mut lots: HashMap<(String, String), VecDeque<Lot>> = HashMap::new();
    let mut disposals: Vec<Disposal> = Vec::new();

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

        for (p_idx, lp) in lt.value.postings.iter().enumerate() {
            if !contributes(&lp.value) {
                continue;
            }
            let Some(a) = &lp.value.amount else { continue };
            // With `-X` the target commodity is money: it neither opens
            // lots nor realizes gains. Only non-target assets do.
            if let Some(t) = target
                && a.commodity == t {
                    continue;
                }
            // Per-unit value and its commodity. Under `-X` this is the
            // commodity's market value in the target on this date (price
            // DB); natively it is the booked trade rate in the counter-
            // commodity. A leg with no derivable value is skipped.
            let Some((unit_value, value_commodity)) = (match target {
                Some(t) => db.find(&a.commodity, t, &date).map(|r| (r, t.to_string())),
                None => posting_value(&lp.value, &sums),
            }) else {
                continue;
            };
            let key = (lp.value.account.clone(), a.commodity.clone());

            // A `{}` lot-cost means the user books the gain by hand
            // (ledger style) — acc consumes lots for FIFO consistency but
            // injects nothing and opens no lot.
            let manual = lp.value.lot_cost.is_some();

            let queue = lots.entry(key.clone()).or_default();

            let mut remaining = a.value; // signed: + acquires, − disposes
            let mut gain = Decimal::zero();
            let mut closed: Vec<ClosedLot> = Vec::new();
            let mut mixed = false;

            // Close opposite-sign lots front-to-back: a long lot (qty > 0)
            // by a disposal (remaining < 0), a short lot (qty < 0) by an
            // acquisition (remaining > 0). Same sign extends the position,
            // so stop. Shorts are only ever opened for a position traded
            // against the target money — see the open condition below.
            while !remaining.is_zero() {
                let Some(front) = queue.front() else { break };
                if front.qty.is_negative() == remaining.is_negative() {
                    break;
                }
                let short = front.qty.is_negative();
                let take = remaining.abs().min(front.qty.abs());
                let front = queue.front_mut().unwrap();
                // Cost vs proceeds in different commodities (mixed currency,
                // native mode) can't be netted — skip the gain but still
                // consume the lot for FIFO consistency. Under `-X` both are
                // the target, so this always matches.
                if front.cost_commodity == value_commodity {
                    // (close − open) for a long lot, (open − close) for a
                    // short — `unit_value − cost`, sign-flipped for shorts.
                    let per = unit_value - front.cost_per_unit;
                    let per = if short { Decimal::zero() - per } else { per };
                    gain += take.mul_rounded(per);
                    closed.push(ClosedLot {
                        qty: take,
                        cost_per_unit: front.cost_per_unit,
                        cost_commodity: front.cost_commodity.clone(),
                        date: front.date,
                    });
                } else {
                    mixed = true;
                }
                front.qty = if short { front.qty + take } else { front.qty - take };
                remaining = if remaining.is_negative() {
                    remaining + take
                } else {
                    remaining - take
                };
                if front.qty.is_zero() {
                    queue.pop_front();
                }
            }

            if manual {
                // User-booked disposal: lots consumed, nothing injected,
                // no new lot opened.
                continue;
            }

            // Open a lot for the unclosed remainder. A positive remainder
            // (acquisition) always opens a long. A negative remainder (a
            // disposal that outran the queue) opens a SHORT only when the
            // position is traded against the target money — a 2-commodity
            // trade whose counter IS the target (e.g. USD sold for EUR):
            // there an uncovered disposal is a genuine short, closed by a
            // later purchase. For crypto↔crypto (counter ≠ target) it is
            // the counter-side of a normal trade or a missing cost basis;
            // opening a short there books phantom capital when a later
            // acquisition "closes" it. rewrite_tx keeps such an uncovered
            // amount as a plain proceeds leg — no cost basis, no gain.
            let against_target =
                sums.len() == 2 && target.is_some_and(|t| sums.contains_key(t));
            if !remaining.is_zero() && (!remaining.is_negative() || against_target) {
                queue.push_back(Lot {
                    qty: remaining,
                    cost_per_unit: unit_value,
                    cost_commodity: value_commodity.clone(),
                    date: lt.value.date,
                });
            }

            // Record a realization if lots were closed with a real gain.
            if mixed || closed.is_empty() {
                continue;
            }
            let prec = precisions.get(&value_commodity).copied().unwrap_or(2);
            if gain.is_display_zero(prec) {
                continue;
            }
            disposals.push(Disposal {
                tx_idx: idx,
                posting_idx: p_idx,
                commodity: a.commodity.clone(),
                decimals: a.decimals,
                is_virtual: lp.value.is_virtual,
                balanced: lp.value.balanced,
                proceeds_per_unit: unit_value,
                gain,
                gain_commodity: value_commodity,
                acquisition: !a.value.is_negative(),
                lots: closed,
            });
        }
    }

    // Phase 2: rewrite each disposal's posting into one leg per closed
    // lot (annotated `{cost} [lot-date] @ proceeds`) and inject the gain
    // as a real capital posting in the same transaction.
    let mut by_tx: HashMap<usize, Vec<Disposal>> = HashMap::new();
    for d in disposals {
        by_tx.entry(d.tx_idx).or_default().push(d);
    }
    for (tx_idx, disps) in by_tx {
        rewrite_tx(&mut txs[tx_idx], &disps, accounts, precisions);
    }
}

/// Rewrite a transaction's disposal postings: each becomes one leg per
/// closed lot (carrying `{cost}`, `[lot-date]` and an `@` proceeds
/// cost), and one real capital posting per disposal carries the realized
/// gain (`income` for a gain, `expense` for a loss). Original
/// non-disposal postings pass through unchanged; capital postings are
/// appended after, so the sale and its gain read together.
fn rewrite_tx(
    lt: &mut Located<Transaction>,
    disps: &[Disposal],
    accounts: &CapitalAccounts,
    precisions: &HashMap<String, usize>,
) {
    let file = lt.file.clone();
    let line = lt.line;
    let mut rewritten: Vec<Located<Posting>> = Vec::new();
    let mut capitals: Vec<Located<Posting>> = Vec::new();

    for (p_idx, lp) in lt.value.postings.iter().enumerate() {
        let Some(disp) = disps.iter().find(|d| d.posting_idx == p_idx) else {
            rewritten.push(lp.clone());
            continue;
        };
        let price_prec = precisions.get(&disp.gain_commodity).copied().unwrap_or(2);
        // The `@` cost for generated legs: the source posting's own cost
        // if it had one, else the disposal's proceeds rate.
        let leg_costs = || {
            lp.value.costs.clone().or_else(|| {
                Some(Costs::PerUnit(Amount {
                    commodity: disp.gain_commodity.clone(),
                    value: disp.proceeds_per_unit,
                    decimals: price_prec,
                }))
            })
        };
        // A disposal removes the commodity (negative leg); an acquisition
        // closing a short adds it back (positive).
        let signed = |qty: Decimal| {
            if disp.acquisition {
                qty
            } else {
                Decimal::zero() - qty
            }
        };
        // Split the disposal into one leg per closed lot.
        for (i, lot) in disp.lots.iter().enumerate() {
            let cost_prec = precisions.get(&lot.cost_commodity).copied().unwrap_or(2);
            rewritten.push(Located {
                file: lp.file.clone(),
                line: lp.line,
                value: Posting {
                    account: lp.value.account.clone(),
                    amount: Some(Amount {
                        commodity: disp.commodity.clone(),
                        value: signed(lot.qty),
                        decimals: disp.decimals,
                    }),
                    costs: leg_costs(),
                    lot_cost: Some(LotCost {
                        amount: Amount {
                            commodity: lot.cost_commodity.clone(),
                            value: lot.cost_per_unit,
                            decimals: cost_prec,
                        },
                        total: false,
                        fixed: false,
                    }),
                    lot_date: Some(lot.date),
                    balance_assertion: None,
                    is_virtual: disp.is_virtual,
                    balanced: disp.balanced,
                    // Keep the source posting's comments on the first leg.
                    comments: if i == 0 {
                        lp.value.comments.clone()
                    } else {
                        Vec::new()
                    },
                },
            });
        }
        // Over-sell: the closed lots cover only part of the disposal
        // (FIFO ran out — acquisitions booked outside this file). The
        // uncovered remainder has no cost basis; keep it as a plain
        // proceeds-priced leg (no `{}`) so the full disposed quantity
        // survives the rewrite and the transaction still balances.
        let covered = disp.lots.iter().fold(Decimal::zero(), |acc, l| acc + l.qty);
        let total = lp
            .value
            .amount
            .as_ref()
            .map(|a| a.value.abs())
            .unwrap_or_else(Decimal::zero);
        let uncovered = total - covered;
        if uncovered > Decimal::zero() {
            rewritten.push(Located {
                file: lp.file.clone(),
                line: lp.line,
                value: Posting {
                    account: lp.value.account.clone(),
                    amount: Some(Amount {
                        commodity: disp.commodity.clone(),
                        value: signed(uncovered),
                        decimals: disp.decimals,
                    }),
                    costs: leg_costs(),
                    lot_cost: None,
                    lot_date: None,
                    balance_assertion: None,
                    is_virtual: disp.is_virtual,
                    balanced: disp.balanced,
                    comments: Vec::new(),
                },
            });
        }
        // Inject the realized gain as a real capital posting in the gain
        // commodity (the rebalancer converts it to the target at the
        // disposal date). It balances against the `{}` cost-basis on the
        // asset legs, so the tx still sums to zero. The execution spread
        // (fx) is booked separately by the realizer — not here.
        if !disp.gain.is_display_zero(price_prec) {
            let account = if disp.gain.is_negative() {
                accounts.capital_loss
            } else {
                accounts.capital_gain
            };
            capitals.push(Located {
                file: file.clone(),
                line,
                value: Posting {
                    account: account.to_string(),
                    amount: Some(Amount {
                        commodity: disp.gain_commodity.clone(),
                        value: -disp.gain,
                        decimals: price_prec,
                    }),
                    costs: None,
                    lot_cost: None,
                    lot_date: None,
                    balance_assertion: None,
                    // Real (not virtual) → 1:1 copyable, survives re-load.
                    // Real postings are always balance-contributing.
                    is_virtual: false,
                    balanced: true,
                    comments: Vec::new(),
                },
            });
        }
    }

    rewritten.extend(capitals);
    lt.value.postings = rewritten;
}

/// Balance-contributing postings: real and bracket-virtual `[account]`.
/// A paren-virtual `(account)` posting stays out of the balance.
fn contributes(p: &Posting) -> bool {
    !p.is_virtual || p.balanced
}

/// Per-unit *booked* (trade) value of a posting in the counter-commodity.
///
/// An explicit `@` cost wins; otherwise the implied rate of a clean
/// two-commodity exchange (the other leg's sum over this leg's sum).
/// `None` when no rate is derivable. Used in native mode only — under
/// `-X` the lotter values legs at the market rate (price DB) instead.
fn posting_value(p: &Posting, sums: &HashMap<String, Decimal>) -> Option<(Decimal, String)> {
    let a = p.amount.as_ref()?;
    // A zero-quantity leg has no per-unit value — and dividing a total
    // cost by it would panic. It contributes nothing to any position, so
    // skip it.
    if a.value.is_zero() {
        return None;
    }
    // An explicit cost annotation wins.
    if let Some(costs) = &p.costs {
        return Some(match costs {
            Costs::PerUnit(c) => (c.value, c.commodity.clone()),
            Costs::Total(c) => (c.value.div_rounded(a.value.abs()), c.commodity.clone()),
        });
    }
    // Implied rate of a clean two-commodity exchange.
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

    /// The standard capital-account config used by the tests.
    fn caps() -> CapitalAccounts<'static> {
        CapitalAccounts {
            capital_gain: "income:capital",
            capital_loss: "expenses:capital",
        }
    }

    /// Summed value of all postings booking to `account` (the realized
    /// gain/loss; income is negative, expense positive).
    fn gain_on(txs: &[Located<Transaction>], account: &str) -> Decimal {
        let mut sum = Decimal::zero();
        for lt in txs {
            for lp in &lt.value.postings {
                if lp.value.account == account
                    && let Some(a) = &lp.value.amount {
                        sum += a.value;
                    }
            }
        }
        sum
    }

    /// True if any posting books to a capital account.
    fn any_capital(txs: &[Located<Transaction>]) -> bool {
        txs.iter().any(|lt| {
            lt.value
                .postings
                .iter()
                .any(|lp| lp.value.account.contains("capital"))
        })
    }

    /// Split legs of `commodity`: disposal postings acc annotated with a
    /// `{cost}` lot (one per closed lot).
    fn split_legs<'a>(txs: &'a [Located<Transaction>], commodity: &str) -> Vec<&'a Posting> {
        txs.iter()
            .flat_map(|lt| lt.value.postings.iter())
            .map(|lp| &lp.value)
            .filter(|p| {
                p.lot_cost.is_some()
                    && p.amount
                        .as_ref()
                        .map(|a| a.commodity == commodity)
                        .unwrap_or(false)
            })
            .collect()
    }

    #[test]
    fn native_total_gain_without_x() {
        // BTC bought for 30000 USD, sold for 50000 USD. No -X: the
        // native total gain straight from the books = 20000 USD.
        let src = "\
            2024-01-01 buy\n\
            \tassets:btc    1 BTC @ 30000 USD\n\
            \tassets:cash  -30000 USD\n\
            2024-06-01 sell\n\
            \tassets:btc   -1 BTC @ 50000 USD\n\
            \tassets:cash   50000 USD\n";
        let (mut txs, db, prec) = setup(src);
        realize_capital(&mut txs, &caps(), None, &db, &prec);
        // Gain booked on the capital account, income negative.
        assert_eq!(gain_on(&txs, "income:capital"), Decimal::parse("-20000").unwrap());
    }

    #[test]
    fn market_move_is_capital_under_x() {
        // Bought 1 BTC when the market was 30000, sold when it was 48000.
        // Target = USD. The lotter books the *market move* as capital:
        //   48000 − 30000 = 18000.
        // The trade-day spread (booked 29000 vs market 30000 on the buy,
        // 51000 vs 48000 on the sell) is the realizer's fx — not here.
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
        realize_capital(&mut txs, &caps(), Some("USD"), &db, &prec);
        assert_eq!(gain_on(&txs, "income:capital"), Decimal::parse("-18000").unwrap());
    }

    #[test]
    fn fifo_partial_sale() {
        // Buy 1 BTC @30000, buy 1 BTC @40000, sell 1 BTC @50000. FIFO
        // takes the first lot: gain = 50000 − 30000 = 20000.
        let src = "\
            2024-01-01 buy a\n\
            \tassets:btc    1 BTC @ 30000 USD\n\
            \tassets:cash  -30000 USD\n\
            2024-02-01 buy b\n\
            \tassets:btc    1 BTC @ 40000 USD\n\
            \tassets:cash  -40000 USD\n\
            2024-06-01 sell\n\
            \tassets:btc   -1 BTC @ 50000 USD\n\
            \tassets:cash   50000 USD\n";
        let (mut txs, db, prec) = setup(src);
        realize_capital(&mut txs, &caps(), None, &db, &prec);
        // FIFO closes lot 1 only (50000 − 30000 = 20000).
        assert_eq!(gain_on(&txs, "income:capital"), Decimal::parse("-20000").unwrap());
    }

    #[test]
    fn loss_routes_to_loss_account() {
        // Bought for 50000, sold for 30000: a 20000 loss → expenses:capital,
        // positive (expense).
        let src = "\
            2024-01-01 buy\n\
            \tassets:btc    1 BTC @ 50000 USD\n\
            \tassets:cash  -50000 USD\n\
            2024-06-01 sell\n\
            \tassets:btc   -1 BTC @ 30000 USD\n\
            \tassets:cash   30000 USD\n";
        let (mut txs, db, prec) = setup(src);
        realize_capital(&mut txs, &caps(), None, &db, &prec);
        // Loss routes to the expense account, positive.
        assert_eq!(gain_on(&txs, "expenses:capital"), Decimal::parse("20000").unwrap());
        assert_eq!(gain_on(&txs, "income:capital"), Decimal::zero());
    }

    #[test]
    fn manual_lot_cost_disposal_is_left_alone() {
        // A `{}` on the disposal signals the user booked the gain by
        // hand (here on income:trade) — acc must inject nothing, else it
        // double-counts. Only the manual posting remains.
        let src = "\
            2024-01-01 buy\n\
            \tassets:btc    1 BTC @ 100 USD\n\
            \tassets:cash  -100 USD\n\
            2024-06-01 sell\n\
            \tassets:btc   -1 BTC {100 USD} @ 150 USD\n\
            \tassets:cash   150 USD\n\
            \tincome:trade     -50 USD\n";
        let (mut txs, db, prec) = setup(src);
        realize_capital(&mut txs, &caps(), None, &db, &prec);
        assert!(!any_capital(&txs));
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
        realize_capital(&mut txs, &caps(), None, &db, &prec);
        assert!(!any_capital(&txs));
    }

    #[test]
    fn disposal_leg_gets_lot_annotation_and_gain() {
        // The disposal posting is annotated in place with `{cost} [date]`
        // and the gain is booked as a real capital posting.
        let src = "\
            2024-01-01 buy\n\
            \tassets:btc    1 BTC @ 100 USD\n\
            \tassets:cash  -100 USD\n\
            2024-06-01 sell\n\
            \tassets:btc   -1 BTC @ 150 USD\n\
            \tassets:cash   150 USD\n";
        let (mut txs, db, prec) = setup(src);
        realize_capital(&mut txs, &caps(), None, &db, &prec);
        // The single disposal leg now carries {cost} and [lot-date].
        let legs = split_legs(&txs, "BTC");
        assert_eq!(legs.len(), 1);
        assert!(legs[0].lot_cost.is_some());
        assert!(legs[0].lot_date.is_some());
        // The gain is a real posting on the capital account (balances
        // via the `{}` cost-basis on the asset leg, 1:1 copyable).
        let cap = txs
            .iter()
            .flat_map(|lt| lt.value.postings.iter())
            .find(|lp| lp.value.account == "income:capital")
            .unwrap();
        assert!(!cap.value.is_virtual);
        assert_eq!(gain_on(&txs, "income:capital"), Decimal::parse("-50").unwrap());
    }

    #[test]
    fn disposal_across_two_lots_splits_into_two_legs() {
        // A single 10-BTC sale closes lot 1 (4 BTC) and lot 2 (6 BTC):
        // the disposal posting splits into two legs, each with its own
        // lot date; the summed gain is booked once.
        let src = "\
            2024-01-01 buy a\n\
            \tassets:btc    4 BTC @ 100 USD\n\
            \tassets:cash  -400 USD\n\
            2024-02-01 buy b\n\
            \tassets:btc    6 BTC @ 110 USD\n\
            \tassets:cash  -660 USD\n\
            2024-06-01 sell\n\
            \tassets:btc  -10 BTC @ 120 USD\n\
            \tassets:cash  1200 USD\n";
        let (mut txs, db, prec) = setup(src);
        realize_capital(&mut txs, &caps(), None, &db, &prec);
        let legs = split_legs(&txs, "BTC");
        assert_eq!(legs.len(), 2);
        assert!(legs.iter().all(|p| p.lot_date.is_some()));
        // lot 1: 4×(120−100)=80, lot 2: 6×(120−110)=60 → 140.
        assert_eq!(gain_on(&txs, "income:capital"), Decimal::parse("-140").unwrap());
    }

    #[test]
    fn lot_annotation_carries_cost_date_and_price() {
        let src = "\
            2024-01-01 buy\n\
            \tassets:btc    1 BTC @ 100 USD\n\
            \tassets:cash  -100 USD\n\
            2024-06-01 sell\n\
            \tassets:btc   -1 BTC @ 150 USD\n\
            \tassets:cash   150 USD\n";
        let (mut txs, db, prec) = setup(src);
        realize_capital(&mut txs, &caps(), None, &db, &prec);
        let legs = split_legs(&txs, "BTC");
        assert_eq!(legs.len(), 1);
        let leg = legs[0];
        // {cost} = 100, [date] = 2024-01-01, @ = 150 (generated proceeds).
        assert_eq!(
            leg.lot_cost.as_ref().unwrap().amount.value,
            Decimal::parse("100").unwrap()
        );
        assert_eq!(leg.lot_date.unwrap().to_string(), "2024-01-01");
        match leg.costs.as_ref().unwrap() {
            Costs::PerUnit(a) => {
                assert_eq!(a.value, Decimal::parse("150").unwrap())
            }
            _ => panic!("expected per-unit @ cost"),
        }
    }

    #[test]
    fn oversell_preserves_uncovered_remainder() {
        // Sell more than the lots hold: buy 1 BTC, sell 2 BTC. FIFO closes
        // the 1-BTC lot (gain 100); the uncovered 1 BTC has no cost basis
        // but must survive as a plain leg so the full -2 BTC disposal is
        // not silently dropped.
        let src = "\
            2024-01-01 buy\n\
            \tassets:btc    1 BTC @ 100 USD\n\
            \tassets:cash  -100 USD\n\
            2024-06-01 sell\n\
            \tassets:btc   -2 BTC @ 200 USD\n\
            \tassets:cash   400 USD\n";
        let (mut txs, db, prec) = setup(src);
        realize_capital(&mut txs, &caps(), None, &db, &prec);
        // Full 2 BTC still disposed across the rewritten legs.
        let disposed = txs
            .iter()
            .flat_map(|lt| lt.value.postings.iter())
            .filter(|lp| lp.value.account == "assets:btc")
            .filter_map(|lp| lp.value.amount.as_ref())
            .filter(|a| a.value < Decimal::zero())
            .fold(Decimal::zero(), |acc, a| acc + a.value);
        assert_eq!(disposed, Decimal::parse("-2").unwrap());
        // Gain only on the covered lot: 1×(200−100)=100.
        assert_eq!(gain_on(&txs, "income:capital"), Decimal::parse("-100").unwrap());
        // Exactly one leg carries a {} cost (the covered lot); the
        // uncovered leg has none.
        assert_eq!(split_legs(&txs, "BTC").len(), 1);
    }

    #[test]
    fn open_position_never_realizes() {
        // Only a buy, no sell: no disposal, no gain.
        let src = "\
            2024-01-01 buy\n\
            \tassets:btc    1 BTC\n\
            \tassets:cash  -30000 USD\n";
        let (mut txs, db, prec) = setup(src);
        realize_capital(&mut txs, &caps(), None, &db, &prec);
        assert!(!any_capital(&txs));
    }

    #[test]
    fn short_then_cover_realizes_gain_with_x() {
        // Pay out 100 USD before owning any (market 1.05 €/USD), then buy
        // it back at 1.03: a short USD position. USD is traded against the
        // target (EUR), so the disposal opens a genuine short; closing it
        // below where it opened is a 100 × (1.05 − 1.03) = €2 gain.
        let src = "\
            P 2024-01-01 USD EUR 1.05\n\
            P 2024-06-01 USD EUR 1.03\n\
            2024-01-01 spend\n\
            \tassets:usd    -100 USD\n\
            \texpenses:dev   105 EUR\n\
            2024-06-01 cover\n\
            \tassets:bank   -103 EUR\n\
            \tassets:usd     100 USD\n";
        let (mut txs, db, prec) = setup(src);
        realize_capital(&mut txs, &caps(), Some("EUR"), &db, &prec);
        assert_eq!(gain_on(&txs, "income:capital"), Decimal::parse("-2").unwrap());
    }

    #[test]
    fn crypto_to_crypto_uncovered_disposal_books_no_phantom_capital() {
        // ETH↔BTC: the BTC disposal's counter is ETH, not the target — so
        // an uncovered BTC disposal must open NO short, else a later BTC
        // acquisition would close it and book phantom capital on top of
        // the real ETH gain (the crypto↔crypto double-count). Only the ETH
        // round-trip realizes (market 700→900 = 200); the BTC side stays a
        // plain leg, so there is no second, spurious capital posting.
        let src = "\
            P 2024-01-01 ETH EUR 700\n\
            P 2024-01-01 BTC EUR 10000\n\
            P 2024-06-01 ETH EUR 900\n\
            P 2024-06-01 BTC EUR 11000\n\
            2024-01-01 buy ETH with BTC\n\
            \tassets:eth      1 ETH\n\
            \tassets:btc  -0.07 BTC\n\
            2024-06-01 sell ETH for BTC\n\
            \tassets:eth     -1 ETH\n\
            \tassets:btc   0.08 BTC\n";
        let (mut txs, db, prec) = setup(src);
        realize_capital(&mut txs, &caps(), Some("EUR"), &db, &prec);
        assert_eq!(gain_on(&txs, "income:capital"), Decimal::parse("-200").unwrap());
        assert_eq!(gain_on(&txs, "expenses:capital"), Decimal::zero());
    }

    #[test]
    fn oversell_against_non_target_opens_no_short() {
        // Under -X EUR, an asset-vs-asset over-sell: buy 1 BTC for USD,
        // sell 2 BTC. The counter (USD) is not the target, but the lotter
        // values BTC at its EUR market rate, so the covered lot realizes
        // its market move (180 − 90 = 90) and the uncovered 1 BTC survives
        // as a plain leg with no cost basis — exactly one split leg.
        let src = "\
            P 2024-01-01 BTC EUR 90\n\
            P 2024-06-01 BTC EUR 180\n\
            2024-01-01 buy\n\
            \tassets:btc    1 BTC @ 100 USD\n\
            \tassets:cash  -100 USD\n\
            2024-06-01 sell\n\
            \tassets:btc   -2 BTC @ 200 USD\n\
            \tassets:cash   400 USD\n";
        let (mut txs, db, prec) = setup(src);
        realize_capital(&mut txs, &caps(), Some("EUR"), &db, &prec);
        // Exactly one covered lot carries a {} cost; the uncovered 1 BTC
        // is a plain leg.
        assert_eq!(split_legs(&txs, "BTC").len(), 1);
        assert_eq!(gain_on(&txs, "income:capital"), Decimal::parse("-90").unwrap());
    }

    #[test]
    fn zero_quantity_leg_does_not_divide_by_zero() {
        // A degenerate `0 BTC @@ 50 USD` leg balances via its cost weight
        // but has no per-unit value — dividing the total cost by the zero
        // quantity would panic. It must be skipped instead.
        let src = "\
            2024-06-01 degenerate\n\
            \tassets:btc    0 BTC @@ 50 USD\n\
            \tassets:cash  -50 USD\n";
        let (mut txs, db, prec) = setup(src);
        // Must not panic, and realizes nothing.
        realize_capital(&mut txs, &caps(), None, &db, &prec);
        assert!(!any_capital(&txs));
    }
}
