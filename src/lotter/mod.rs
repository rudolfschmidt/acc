//! Lotter phase — realized capital gains via FIFO lot tracking.
//!
//! Runs whenever the journal declares both a `capital gain` and a
//! `capital loss` account. It walks transactions chronologically and
//! tracks a per-(account, commodity) FIFO queue of lots: an exchange
//! posting that *acquires* a commodity opens a lot, a posting that
//! *disposes* of it closes lots front-to-back and realizes the gain.
//!
//! A transaction in which every commodity nets to zero is a *transfer*,
//! not a trade (a plain move, a fee-split withdrawal, a bundled
//! multi-commodity move): it realizes nothing, but its lots migrate
//! FIFO from the outgoing to the incoming legs — cost basis and
//! acquisition date intact — so a commodity bought on one account and
//! sold from another (exchange → transit → wallet → sale) still
//! realizes against its original basis.
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
//! [realizer](crate::realizer) books it as `slippage` on every multi-commodity
//! transaction, buy and sell. The two compose: the lotter's `{cost}`
//! shifts the disposal leg by the market move, and its capital posting
//! offsets that shift exactly, so the realizer's slippage stays valid and the
//! transaction still sums to zero.
//!
//! ## Without `-X`: the native trade gain
//!
//! Natively there is no market to compare against, so the lotter falls
//! back to the **booked** (trade) rate: a lot opens at its booked cost in
//! the counter-commodity, and a disposal realizes `proceeds − cost` as a
//! single `capital` posting. Mixed-currency native disposals (bought in
//! GBP, sold in USD) can't be netted and are skipped; they need `-X`.
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
    /// The *booked* (trade) per-unit price in the target currency — what
    /// was actually paid, converted at the acquisition date. Carried for
    /// the `lots` report only; the capital math runs on `cost_per_unit`.
    /// `None` natively (there `cost_per_unit` already is the booked
    /// rate) and for lots without a derivable trade rate.
    booked_per_unit: Option<Decimal>,
}

/// One lot closed by a disposal: how much was taken, at what cost, and
/// when it was acquired — enough to render one split leg.
struct ClosedLot {
    qty: Decimal,
    cost_per_unit: Decimal,
    cost_commodity: String,
    date: Date,
    /// The closed lot's booked (trade) per-unit price — see [`Lot`].
    booked_per_unit: Option<Decimal>,
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
    /// The disposal's booked (trade) per-unit price in the target — what
    /// was actually received. `None` natively or without a trade rate.
    booked_proceeds_per_unit: Option<Decimal>,
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

/// The per-(account, commodity) FIFO queues the tracker maintains.
type LotQueues = HashMap<(String, String), VecDeque<Lot>>;

/// One still-open lot at the end of tracking, exported for the `lots`
/// report: the position it belongs to, the remaining quantity, its cost
/// basis and the acquisition date. A negative `qty` is an open short.
pub struct OpenLot {
    pub account: String,
    pub commodity: String,
    pub qty: Decimal,
    pub cost_per_unit: Decimal,
    pub cost_commodity: String,
    pub date: Date,
    /// The booked (trade) per-unit price in the target — what was
    /// actually paid. `None` natively (`cost_per_unit` is the booked
    /// rate there) and for lots without a derivable trade rate.
    pub booked_per_unit: Option<Decimal>,
}

/// One realized sale for the `lots` report — one entry per disposal
/// (trade), with its FIFO cost already aggregated over the closed lots.
/// Amounts, not rates: `booked_cost` is what was actually paid for the
/// disposed quantity (summed over the lots that carry a booked rate —
/// `None` when none does), `booked_proceeds` what the sale actually
/// returned. The market figures (the capital basis) are always present.
/// `short` marks a purchase closing a short position.
pub struct Realization {
    pub account: String,
    pub commodity: String,
    /// Total disposed quantity (positive).
    pub qty: Decimal,
    /// Acquisition date of the earliest lot the sale pulled.
    pub first_buy_date: Date,
    pub sell_date: Date,
    /// The valuation commodity: the `-X` target, or natively the
    /// counter-commodity of the trade.
    pub value_commodity: String,
    pub market_cost: Decimal,
    pub market_proceeds: Decimal,
    pub booked_cost: Option<Decimal>,
    pub booked_proceeds: Option<Decimal>,
    pub short: bool,
}

/// Run the FIFO tracking over `txs` and return every realized sale, one
/// entry per disposal, in (account, commodity, sell date) order.
/// Includes zero-gain sales — this is the report path, not the booking
/// path. Read-only: nothing is rewritten.
pub fn realizations(
    txs: &[Located<Transaction>],
    target: Option<&str>,
    db: &Index,
) -> Vec<Realization> {
    let (_, disposals) = track(txs, target, db);
    let mut out = Vec::new();
    for d in &disposals {
        let tx = &txs[d.tx_idx].value;
        let account = &tx.postings[d.posting_idx].value.account;
        let qty = d.lots.iter().fold(Decimal::zero(), |a, l| a + l.qty);
        let market_cost = d
            .lots
            .iter()
            .fold(Decimal::zero(), |a, l| a + l.qty.mul_rounded(l.cost_per_unit));
        let booked: Vec<Decimal> = d
            .lots
            .iter()
            .filter_map(|l| l.booked_per_unit.map(|b| l.qty.mul_rounded(b)))
            .collect();
        let booked_cost =
            (!booked.is_empty()).then(|| booked.iter().fold(Decimal::zero(), |a, v| a + *v));
        out.push(Realization {
            account: account.clone(),
            commodity: d.commodity.clone(),
            qty,
            first_buy_date: d.lots.iter().map(|l| l.date).min().expect("closed is non-empty"),
            sell_date: tx.date,
            value_commodity: d.gain_commodity.clone(),
            market_cost,
            market_proceeds: qty.mul_rounded(d.proceeds_per_unit),
            booked_cost,
            booked_proceeds: d.booked_proceeds_per_unit.map(|b| qty.mul_rounded(b)),
            short: d.acquisition,
        });
    }
    out.sort_by(|a, b| {
        (&a.account, &a.commodity, a.sell_date, a.first_buy_date)
            .cmp(&(&b.account, &b.commodity, b.sell_date, b.first_buy_date))
    });
    out
}

/// Run the FIFO tracking over `txs` and return the still-open lots,
/// flattened in (account, commodity, queue) order — the first lot of a
/// position is its FIFO front, the one the next disposal will close.
/// Read-only: nothing is realized, no posting is rewritten.
pub fn open_lots(
    txs: &[Located<Transaction>],
    target: Option<&str>,
    db: &Index,
) -> Vec<OpenLot> {
    let (lots, _) = track(txs, target, db);
    let mut keys: Vec<&(String, String)> = lots.keys().collect();
    keys.sort();
    let mut out = Vec::new();
    for key in keys {
        for lot in &lots[key] {
            out.push(OpenLot {
                account: key.0.clone(),
                commodity: key.1.clone(),
                qty: lot.qty,
                cost_per_unit: lot.cost_per_unit,
                cost_commodity: lot.cost_commodity.clone(),
                date: lot.date,
                booked_per_unit: lot.booked_per_unit,
            });
        }
    }
    out
}

/// Track lots FIFO and inject one realized capital-gain/loss posting per
/// disposal. See module docs for the valuation semantics.
pub fn realize_capital(
    txs: &mut [Located<Transaction>],
    accounts: &CapitalAccounts,
    target: Option<&str>,
    db: &Index,
    precisions: &HashMap<String, usize>,
) {
    let (_, mut disposals) = track(txs, target, db);

    // Only disposals with a displayable gain get rewritten — a zero-gain
    // sale needs no split legs and no capital posting. (Formerly filtered
    // inside `track`; moved here so the report path sees every sale.)
    disposals.retain(|d| {
        let prec = precisions.get(&d.gain_commodity).copied().unwrap_or(2);
        !d.gain.is_display_zero(prec)
    });

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

/// Phase 1 — walk the journal chronologically, building the per-(account,
/// commodity) FIFO queues and collecting the disposals that realized a
/// gain. Shared by [`realize_capital`] (which then rewrites) and
/// [`open_lots`] (which reports the queues). Read-only on `txs`.
fn track(
    txs: &[Located<Transaction>],
    target: Option<&str>,
    db: &Index,
) -> (LotQueues, Vec<Disposal>) {
    let mut lots: LotQueues = HashMap::new();
    // Disposals keyed by (tx, posting) so the day-settling trade retry
    // can top up an existing disposal when late-arriving lots cover
    // more of it.
    let mut disposals: HashMap<(usize, usize), Disposal> = HashMap::new();
    // Per-(tx, posting) quantity already migrated, so a day-settling
    // re-pass only moves what a transfer leg still owes — never twice.
    let mut migrated: HashMap<(usize, usize), Decimal> = HashMap::new();
    // Per-(tx, posting) still-owed disposal quantity (positive) that
    // found no lots and opened no short — the trade-retry's ledger.
    let mut uncovered: HashMap<(usize, usize), Decimal> = HashMap::new();
    // The current day's transfer transactions, for the settling pass,
    // and its trades that left something uncovered.
    let mut day_transfers: Vec<usize> = Vec::new();
    let mut day_trades: Vec<usize> = Vec::new();
    let mut current_day: Option<Date> = None;

    for (idx, lt) in txs.iter().enumerate() {
        // Within one day, file order decides processing order — but a
        // transfer chain (exchange → transit → wallet) may well be
        // booked with the arrival before the departure, since the legs
        // live in different files. So at each day boundary, re-run the
        // finished day's transfers until no lot moves any more: lots
        // that landed late still reach every onward hop of the same
        // day. Across days no such settling happens — an arrival dated
        // before its departure is a data error, not an ordering one.
        if current_day != Some(lt.value.date) {
            settle_day(
                txs,
                &day_transfers,
                &day_trades,
                target,
                db,
                &mut lots,
                &mut migrated,
                &mut uncovered,
                &mut disposals,
            );
            day_transfers.clear();
            day_trades.clear();
            current_day = Some(lt.value.date);
        }
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
        // A transaction in which EVERY commodity nets to zero exchanged
        // nothing into anything — it only moved holdings between
        // accounts: a plain transfer, a fee-split withdrawal, a bundled
        // multi-commodity move. That is not a realization event; the
        // lots migrate with the commodity instead, cost basis and
        // acquisition date intact. Merely skipping (the old behaviour)
        // stranded the basis on the source account — the buy on the
        // exchange and the sell from the wallet never met — and running
        // a bundled move through the trade path below would realize
        // phantom capital on a mere relocation. Only when a commodity's
        // balance crosses into another commodity is there a trade.
        if sums.values().all(|s| s.is_zero()) {
            migrate(lt, idx, target, &mut lots, &mut migrated);
            day_transfers.push(idx);
            continue;
        }
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

            // The *booked* trade rate of this leg in the target currency —
            // what was actually paid or received per unit, converted at
            // this date. Only meaningful under `-X` (natively `unit_value`
            // already IS the booked rate); `None` when the transaction
            // carries no derivable trade rate.
            let booked = target.and_then(|t| {
                posting_value(&lp.value, &sums).and_then(|(rate, counter)| {
                    if counter == t {
                        Some(rate)
                    } else {
                        db.find(&counter, t, &date).map(|fx| rate.mul_rounded(fx))
                    }
                })
            });

            // A `{}` lot-cost means the user books the gain by hand
            // (ledger style) — acc consumes lots for FIFO consistency but
            // injects nothing and opens no lot.
            let manual = lp.value.lot_cost.is_some();

            let queue = lots.entry(key.clone()).or_default();

            let (closed, gain, mixed, remaining) =
                close_against(queue, a.value, unit_value, &value_commodity);

            if manual {
                // The meaning of a user-written `{}` depends on the side.
                // On a DISPOSAL the user books the gain by hand (ledger
                // style): lots consumed, nothing injected, no new lot. On
                // an ACQUISITION it *declares the cost basis* — a fork
                // credited at `{0}`, an airdrop, an opening balance with a
                // known basis. Ignoring it there would strand the whole
                // position basis-less, so open the lot at the declared
                // cost (and at the `[date]` if the user wrote one).
                if !remaining.is_zero() && !remaining.is_negative() {
                    let lot = lp.value.lot_cost.as_ref().unwrap();
                    let cost_per_unit = if lot.total {
                        lot.amount.value.div_rounded(remaining.abs())
                    } else {
                        lot.amount.value
                    };
                    queue.push_back(Lot {
                        qty: remaining,
                        cost_per_unit,
                        cost_commodity: lot.amount.commodity.clone(),
                        date: lp.value.lot_date.unwrap_or(lt.value.date),
                        // A user-written `{}` on an acquisition IS the
                        // declared real cost — convert it to the target
                        // for the booked view.
                        booked_per_unit: target.and_then(|t| {
                            if lot.amount.commodity == t {
                                Some(cost_per_unit)
                            } else {
                                db.find(&lot.amount.commodity, t, &date)
                                    .map(|fx| cost_per_unit.mul_rounded(fx))
                            }
                        }),
                    });
                }
                continue;
            }

            // Open a lot for the unclosed remainder of an ACQUISITION —
            // always a long.
            if !remaining.is_zero() && !remaining.is_negative() {
                queue.push_back(Lot {
                    qty: remaining,
                    cost_per_unit: unit_value,
                    cost_commodity: value_commodity.clone(),
                    date: lt.value.date,
                    booked_per_unit: booked,
                });
            }

            // A disposal the queue could not fully cover: remember the
            // owed rest so the day-settling retry can close it against
            // lots arriving later the same day. Whatever is STILL owed
            // at the day's end becomes a genuine SHORT when the position
            // trades against the target money (see `settle_day`) —
            // deferred there so same-day coverage always wins, keeping
            // the result independent of the `-X` target. Other uncovered
            // rests stay basis-less: rewrite_tx keeps them as plain
            // proceeds legs — no cost basis, no gain.
            if remaining.is_negative() {
                uncovered.insert((idx, p_idx), remaining.abs());
                if day_trades.last() != Some(&idx) {
                    day_trades.push(idx);
                }
            }

            // Record a realization whenever lots were closed. Zero-gain
            // sales stay in — the `lots` report shows every sale;
            // `realize_capital` filters them before rewriting.
            if mixed || closed.is_empty() {
                continue;
            }
            disposals.insert((idx, p_idx), Disposal {
                tx_idx: idx,
                posting_idx: p_idx,
                commodity: a.commodity.clone(),
                decimals: a.decimals,
                is_virtual: lp.value.is_virtual,
                balanced: lp.value.balanced,
                proceeds_per_unit: unit_value,
                booked_proceeds_per_unit: booked,
                gain,
                gain_commodity: value_commodity,
                acquisition: !a.value.is_negative(),
                lots: closed,
            });
        }
    }

    // Settle the trailing day (no boundary follows it).
    settle_day(
        txs,
        &day_transfers,
        &day_trades,
        target,
        db,
        &mut lots,
        &mut migrated,
        &mut uncovered,
        &mut disposals,
    );

    // Deterministic order for phase 2 and the report.
    let mut disposals: Vec<Disposal> = disposals.into_values().collect();
    disposals.sort_by_key(|d| (d.tx_idx, d.posting_idx));
    (lots, disposals)
}

/// Close opposite-sign lots front-to-back against `remaining` (signed:
/// positive acquires / covers shorts, negative disposes of longs — same
/// sign as the front extends the position, so it stops). Returns the
/// closed lots, the realized gain, whether lots with a foreign cost
/// commodity were consumed gain-free (mixed), and the unclosed
/// remainder.
fn close_against(
    queue: &mut VecDeque<Lot>,
    mut remaining: Decimal,
    unit_value: Decimal,
    value_commodity: &str,
) -> (Vec<ClosedLot>, Decimal, bool, Decimal) {
    let mut gain = Decimal::zero();
    let mut closed: Vec<ClosedLot> = Vec::new();
    let mut mixed = false;
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
                booked_per_unit: front.booked_per_unit,
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
    (closed, gain, mixed, remaining)
}

/// Day-settling retry for a trade that left a disposal (partly)
/// uncovered: close the still-owed rest against lots that arrived later
/// the same day, topping up the existing disposal (or creating one).
/// Without this, a wallet that receives and sells on the same day
/// strands the arriving lots forever whenever file order books the sale
/// first — the sale realizes nothing and the lots never leave. Returns
/// whether any quantity was closed.
#[allow(clippy::too_many_arguments)]
fn retry_trade(
    txs: &[Located<Transaction>],
    tx_idx: usize,
    target: Option<&str>,
    db: &Index,
    lots: &mut LotQueues,
    uncovered: &mut HashMap<(usize, usize), Decimal>,
    disposals: &mut HashMap<(usize, usize), Disposal>,
) -> bool {
    let lt = &txs[tx_idx];
    let date = lt.value.date.to_string();
    // The same per-commodity sums as the main pass, for the implied rate.
    let mut sums: HashMap<String, Decimal> = HashMap::new();
    for lp in &lt.value.postings {
        if !contributes(&lp.value) {
            continue;
        }
        if let Some(a) = &lp.value.amount {
            *sums.entry(a.commodity.clone()).or_insert(Decimal::zero()) += a.value;
        }
    }
    let mut moved = false;
    for (p_idx, lp) in lt.value.postings.iter().enumerate() {
        let Some(rest) = uncovered.get_mut(&(tx_idx, p_idx)) else { continue };
        if rest.is_zero() {
            continue;
        }
        let Some(a) = &lp.value.amount else { continue };
        let Some((unit_value, value_commodity)) = (match target {
            Some(t) => db.find(&a.commodity, t, &date).map(|r| (r, t.to_string())),
            None => posting_value(&lp.value, &sums),
        }) else {
            continue;
        };
        let booked = target.and_then(|t| {
            posting_value(&lp.value, &sums).and_then(|(rate, counter)| {
                if counter == t {
                    Some(rate)
                } else {
                    db.find(&counter, t, &date).map(|fx| rate.mul_rounded(fx))
                }
            })
        });
        let queue = lots
            .entry((lp.value.account.clone(), a.commodity.clone()))
            .or_default();
        let (closed, gain, mixed, remaining) =
            close_against(queue, Decimal::zero() - *rest, unit_value, &value_commodity);
        if closed.is_empty() {
            continue;
        }
        moved = true;
        *rest = remaining.abs();
        if mixed {
            continue;
        }
        let d = disposals.entry((tx_idx, p_idx)).or_insert_with(|| Disposal {
            tx_idx,
            posting_idx: p_idx,
            commodity: a.commodity.clone(),
            decimals: a.decimals,
            is_virtual: lp.value.is_virtual,
            balanced: lp.value.balanced,
            proceeds_per_unit: unit_value,
            booked_proceeds_per_unit: booked,
            gain: Decimal::zero(),
            gain_commodity: value_commodity,
            acquisition: false,
            lots: Vec::new(),
        });
        d.gain += gain;
        d.lots.extend(closed);
    }
    moved
}

/// Re-run a finished day's transfers AND uncovered trades until no lot
/// moves any more. Within one day, file order is arbitrary — a chain's
/// arrival may be booked before its departure, and a sale before the
/// arrival that funds it — so lots that landed late must still reach
/// every onward hop and every still-owed disposal of the same day. The
/// `migrated` / `uncovered` ledgers guarantee a re-pass only moves what
/// a leg still owes, so settling is idempotent and the loop terminates
/// as soon as a full pass moves nothing. One transfer alone is reason
/// enough: the lot it carries on may be opened by a trade booked after
/// it, and an ACQUIRING trade never enters `trades` — that list holds
/// the day's uncovered disposals. Under `-X` such a trade is invisible
/// twice over, since its counter leg is the target money and is skipped
/// outright. Across days no settling happens — an arrival dated before
/// its departure is a data error, not an ordering one.
#[allow(clippy::too_many_arguments)]
fn settle_day(
    txs: &[Located<Transaction>],
    transfers: &[usize],
    trades: &[usize],
    target: Option<&str>,
    db: &Index,
    lots: &mut LotQueues,
    migrated: &mut HashMap<(usize, usize), Decimal>,
    uncovered: &mut HashMap<(usize, usize), Decimal>,
    disposals: &mut HashMap<(usize, usize), Disposal>,
) {
    if trades.is_empty() && transfers.is_empty() {
        return;
    }
    loop {
        let mut moved = false;
        for &t in transfers {
            moved |= migrate(&txs[t], t, target, lots, migrated);
        }
        for &t in trades {
            moved |= retry_trade(txs, t, target, db, lots, uncovered, disposals);
        }
        if !moved {
            break;
        }
    }
    // What an against-target disposal still owes after the day settled
    // is a genuine short — a 2-commodity trade whose counter IS the
    // target money (e.g. USD sold for GBP), later closed by a purchase.
    // Opening it here, not in the main pass, lets same-day arrivals
    // cover the sale first, so realization does not depend on file
    // order or on which commodity happens to be the `-X` target. For
    // crypto↔crypto (counter ≠ target) no short opens: a later
    // acquisition "closing" it would book phantom capital.
    for &t in trades {
        open_short_rest(txs, t, target, db, lots, uncovered);
    }
}

/// Open the deferred short for a day's still-uncovered against-target
/// disposals — see the trailing step of [`settle_day`].
fn open_short_rest(
    txs: &[Located<Transaction>],
    tx_idx: usize,
    target: Option<&str>,
    db: &Index,
    lots: &mut LotQueues,
    uncovered: &mut HashMap<(usize, usize), Decimal>,
) {
    let lt = &txs[tx_idx];
    let date = lt.value.date.to_string();
    let mut sums: HashMap<String, Decimal> = HashMap::new();
    for lp in &lt.value.postings {
        if !contributes(&lp.value) {
            continue;
        }
        if let Some(a) = &lp.value.amount {
            *sums.entry(a.commodity.clone()).or_insert(Decimal::zero()) += a.value;
        }
    }
    if !(sums.len() == 2 && target.is_some_and(|t| sums.contains_key(t))) {
        return;
    }
    for (p_idx, lp) in lt.value.postings.iter().enumerate() {
        let Some(rest) = uncovered.get_mut(&(tx_idx, p_idx)) else { continue };
        if rest.is_zero() {
            continue;
        }
        let Some(a) = &lp.value.amount else { continue };
        let Some((unit_value, value_commodity)) = (match target {
            Some(t) => db.find(&a.commodity, t, &date).map(|r| (r, t.to_string())),
            None => posting_value(&lp.value, &sums),
        }) else {
            continue;
        };
        let booked = target.and_then(|t| {
            posting_value(&lp.value, &sums).and_then(|(rate, counter)| {
                if counter == t {
                    Some(rate)
                } else {
                    db.find(&counter, t, &date).map(|fx| rate.mul_rounded(fx))
                }
            })
        });
        lots.entry((lp.value.account.clone(), a.commodity.clone()))
            .or_default()
            .push_back(Lot {
                qty: Decimal::zero() - *rest,
                cost_per_unit: unit_value,
                cost_commodity: value_commodity,
                date: lt.value.date,
                booked_per_unit: booked,
            });
        *rest = Decimal::zero();
    }
}

/// Migrate lots through a transfer transaction (every commodity nets to
/// zero): outgoing (negative) legs shed their lots FIFO into a
/// per-commodity stream, incoming (positive) legs receive them in
/// posting order. Cost basis and acquisition date travel unchanged — a
/// transfer is not a disposal, so nothing is realized and no posting is
/// rewritten. Legs in the target commodity are money, not a position,
/// and are skipped (matching the disposal path); short lots stay put —
/// an uncovered outflow just migrates nothing, it never fabricates a
/// basis. The `migrated` ledger records each leg's already-moved
/// quantity so day-settling re-passes only move the still-owed rest;
/// returns whether any quantity moved.
fn migrate(
    lt: &Located<Transaction>,
    tx_idx: usize,
    target: Option<&str>,
    lots: &mut HashMap<(String, String), VecDeque<Lot>>,
    migrated: &mut HashMap<(usize, usize), Decimal>,
) -> bool {
    let mut moved = false;
    // Shed: collect the outgoing legs' lots FIFO, one stream per
    // commodity — a bundled move must never hand one commodity's basis
    // to another commodity's leg.
    let mut streams: HashMap<String, VecDeque<Lot>> = HashMap::new();
    for (p_idx, lp) in lt.value.postings.iter().enumerate() {
        if !contributes(&lp.value) {
            continue;
        }
        let Some(a) = &lp.value.amount else { continue };
        if target.is_some_and(|t| a.commodity == t) {
            continue;
        }
        if !a.value.is_negative() {
            continue;
        }
        let Some(queue) = lots.get_mut(&(lp.value.account.clone(), a.commodity.clone())) else {
            continue;
        };
        let done = migrated.entry((tx_idx, p_idx)).or_insert(Decimal::zero());
        let stream = streams.entry(a.commodity.clone()).or_default();
        let mut remaining = a.value.abs() - *done;
        while !remaining.is_zero() {
            let Some(front) = queue.front_mut() else { break };
            if front.qty.is_negative() {
                break;
            }
            let take = remaining.min(front.qty);
            stream.push_back(Lot {
                qty: take,
                cost_per_unit: front.cost_per_unit,
                cost_commodity: front.cost_commodity.clone(),
                date: front.date,
                booked_per_unit: front.booked_per_unit,
            });
            front.qty -= take;
            remaining -= take;
            *done += take;
            moved = true;
            if front.qty.is_zero() {
                queue.pop_front();
            }
        }
    }
    if streams.is_empty() {
        return moved;
    }
    // Receive: deal each commodity's stream out to its incoming legs in
    // posting order. A leg beyond the stream (basis unknown) stays
    // lot-less, exactly like an uncovered acquisition today.
    for (p_idx, lp) in lt.value.postings.iter().enumerate() {
        if !contributes(&lp.value) {
            continue;
        }
        let Some(a) = &lp.value.amount else { continue };
        if a.value.is_negative() || a.value.is_zero() {
            continue;
        }
        let Some(stream) = streams.get_mut(&a.commodity) else {
            continue;
        };
        let done = migrated.entry((tx_idx, p_idx)).or_insert(Decimal::zero());
        let queue = lots
            .entry((lp.value.account.clone(), a.commodity.clone()))
            .or_default();
        let mut remaining = a.value - *done;
        while !remaining.is_zero() {
            let Some(piece) = stream.front_mut() else { break };
            let take = remaining.min(piece.qty);
            // An open short at the queue's front means this account
            // disposed without a basis (uncovered, booked gain-free).
            // The arriving quantity neutralizes it silently — its basis
            // is spent covering the short, no gain is realized either
            // way. Appending behind the short instead would jam the
            // queue: every later disposal stops at the short and the
            // account never realizes again.
            if let Some(front) = queue.front_mut()
                && front.qty.is_negative()
            {
                let net = take.min(front.qty.abs());
                front.qty += net;
                if front.qty.is_zero() {
                    queue.pop_front();
                }
                piece.qty -= net;
                remaining -= net;
                *done += net;
                moved = true;
                if piece.qty.is_zero() {
                    stream.pop_front();
                }
                continue;
            }
            queue.push_back(Lot {
                qty: take,
                cost_per_unit: piece.cost_per_unit,
                cost_commodity: piece.cost_commodity.clone(),
                date: piece.date,
                booked_per_unit: piece.booked_per_unit,
            });
            piece.qty -= take;
            remaining -= take;
            *done += take;
            moved = true;
            if piece.qty.is_zero() {
                stream.pop_front();
            }
        }
    }
    moved
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
        // (slippage) is booked separately by the realizer — not here.
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
        // 51000 vs 48000 on the sell) is the realizer's slippage — not here.
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
        // Pay out 100 USD before owning any (market 1.05 £/USD), then buy
        // it back at 1.03: a short USD position. USD is traded against the
        // target (GBP), so the disposal opens a genuine short; closing it
        // below where it opened is a 100 × (1.05 − 1.03) = £2 gain.
        let src = "\
            P 2024-01-01 USD GBP 1.05\n\
            P 2024-06-01 USD GBP 1.03\n\
            2024-01-01 spend\n\
            \tassets:usd    -100 USD\n\
            \texpenses:work   105 GBP\n\
            2024-06-01 cover\n\
            \tassets:bank   -103 GBP\n\
            \tassets:usd     100 USD\n";
        let (mut txs, db, prec) = setup(src);
        realize_capital(&mut txs, &caps(), Some("GBP"), &db, &prec);
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
            P 2024-01-01 ETH GBP 700\n\
            P 2024-01-01 BTC GBP 10000\n\
            P 2024-06-01 ETH GBP 900\n\
            P 2024-06-01 BTC GBP 11000\n\
            2024-01-01 buy ETH with BTC\n\
            \tassets:eth      1 ETH\n\
            \tassets:btc  -0.07 BTC\n\
            2024-06-01 sell ETH for BTC\n\
            \tassets:eth     -1 ETH\n\
            \tassets:btc   0.08 BTC\n";
        let (mut txs, db, prec) = setup(src);
        realize_capital(&mut txs, &caps(), Some("GBP"), &db, &prec);
        assert_eq!(gain_on(&txs, "income:capital"), Decimal::parse("-200").unwrap());
        assert_eq!(gain_on(&txs, "expenses:capital"), Decimal::zero());
    }

    #[test]
    fn oversell_against_non_target_opens_no_short() {
        // Under -X GBP, an asset-vs-asset over-sell: buy 1 BTC for USD,
        // sell 2 BTC. The counter (USD) is not the target, but the lotter
        // values BTC at its GBP market rate, so the covered lot realizes
        // its market move (180 − 90 = 90) and the uncovered 1 BTC survives
        // as a plain leg with no cost basis — exactly one split leg.
        let src = "\
            P 2024-01-01 BTC GBP 90\n\
            P 2024-06-01 BTC GBP 180\n\
            2024-01-01 buy\n\
            \tassets:btc    1 BTC @ 100 USD\n\
            \tassets:cash  -100 USD\n\
            2024-06-01 sell\n\
            \tassets:btc   -2 BTC @ 200 USD\n\
            \tassets:cash   400 USD\n";
        let (mut txs, db, prec) = setup(src);
        realize_capital(&mut txs, &caps(), Some("GBP"), &db, &prec);
        // Exactly one covered lot carries a {} cost; the uncovered 1 BTC
        // is a plain leg.
        assert_eq!(split_legs(&txs, "BTC").len(), 1);
        assert_eq!(gain_on(&txs, "income:capital"), Decimal::parse("-90").unwrap());
    }

    #[test]
    fn transfer_migrates_cost_basis_native() {
        // Buy on the exchange account, transfer to the wallet account,
        // sell from the wallet. The transfer carries the lot across, so
        // the wallet disposal realizes against the exchange's cost:
        // 1 × (150 − 100) = 50.
        let src = "\
            2024-01-01 buy\n\
            \tassets:exchange   1 BTC @ 100 USD\n\
            \tassets:cash    -100 USD\n\
            2024-02-01 transfer\n\
            \tassets:exchange  -1 BTC\n\
            \tassets:wallet     1 BTC\n\
            2024-06-01 sell\n\
            \tassets:wallet    -1 BTC @ 150 USD\n\
            \tassets:cash     150 USD\n";
        let (mut txs, db, prec) = setup(src);
        realize_capital(&mut txs, &caps(), None, &db, &prec);
        assert_eq!(gain_on(&txs, "income:capital"), Decimal::parse("-50").unwrap());
        // The disposal leg carries the ORIGINAL acquisition date.
        let legs = split_legs(&txs, "BTC");
        assert_eq!(legs.len(), 1);
        assert_eq!(legs[0].lot_date.unwrap().to_string(), "2024-01-01");
    }

    #[test]
    fn transfer_migrates_market_basis_under_x() {
        // Under -X the lot opens at the acquisition-date market value.
        // The transfer moves it unchanged — the wallet sale realizes the
        // move since ACQUISITION (90 → 180), not since the transfer.
        let src = "\
            P 2024-01-01 BTC GBP 90\n\
            P 2024-02-01 BTC GBP 120\n\
            P 2024-06-01 BTC GBP 180\n\
            2024-01-01 buy\n\
            \tassets:exchange   1 BTC\n\
            \tassets:cash     -90 GBP\n\
            2024-02-01 transfer\n\
            \tassets:exchange  -1 BTC\n\
            \tassets:wallet     1 BTC\n\
            2024-06-01 sell\n\
            \tassets:wallet    -1 BTC\n\
            \tassets:cash     180 GBP\n";
        let (mut txs, db, prec) = setup(src);
        realize_capital(&mut txs, &caps(), Some("GBP"), &db, &prec);
        assert_eq!(gain_on(&txs, "income:capital"), Decimal::parse("-90").unwrap());
    }

    #[test]
    fn transfer_with_fee_leg_splits_the_migrating_lot() {
        // A withdrawal sheds 1 BTC and lands as a fee sliver plus the
        // transferred remainder — three legs, one commodity. The lot
        // splits across the receivers; the wallet sale realizes only on
        // its 0.9 share: 0.9 × (200 − 100) = 90.
        let src = "\
            2024-01-01 buy\n\
            \tassets:exchange     1 BTC @ 100 USD\n\
            \tassets:cash      -100 USD\n\
            2024-02-01 withdraw\n\
            \tassets:exchange    -1 BTC\n\
            \tassets:fees       0.1 BTC\n\
            \tassets:wallet     0.9 BTC\n\
            2024-06-01 sell\n\
            \tassets:wallet    -0.9 BTC @ 200 USD\n\
            \tassets:cash      180 USD\n";
        let (mut txs, db, prec) = setup(src);
        realize_capital(&mut txs, &caps(), None, &db, &prec);
        assert_eq!(gain_on(&txs, "income:capital"), Decimal::parse("-90").unwrap());
    }

    #[test]
    fn transfer_through_transit_account_chains() {
        // Real journals book a transfer as TWO transactions meeting on a
        // transit account (source → transit, transit → destination). The
        // lot must survive both hops.
        let src = "\
            2024-01-01 buy\n\
            \tassets:exchange       1 BTC @ 100 USD\n\
            \tassets:cash        -100 USD\n\
            2024-02-01 out\n\
            \tassets:exchange      -1 BTC\n\
            \tassets:transit        1 BTC\n\
            2024-02-01 in\n\
            \tassets:transit       -1 BTC\n\
            \tassets:wallet         1 BTC\n\
            2024-06-01 sell\n\
            \tassets:wallet        -1 BTC @ 150 USD\n\
            \tassets:cash         150 USD\n";
        let (mut txs, db, prec) = setup(src);
        realize_capital(&mut txs, &caps(), None, &db, &prec);
        assert_eq!(gain_on(&txs, "income:capital"), Decimal::parse("-50").unwrap());
    }

    #[test]
    fn transfer_without_lots_migrates_nothing() {
        // A transfer whose source has no lots (history starts mid-way)
        // must not fabricate a basis: the later sale stays uncovered and
        // realizes nothing, exactly as before.
        let src = "\
            2024-02-01 transfer\n\
            \tassets:exchange  -1 BTC\n\
            \tassets:wallet     1 BTC\n\
            2024-06-01 sell\n\
            \tassets:wallet    -1 BTC @ 150 USD\n\
            \tassets:cash     150 USD\n";
        let (mut txs, db, prec) = setup(src);
        realize_capital(&mut txs, &caps(), None, &db, &prec);
        assert!(!any_capital(&txs));
    }

    #[test]
    fn money_transfer_under_x_is_ignored() {
        // A plain bank transfer in the target commodity is money moving,
        // not a position — the migration must leave it alone.
        let src = "\
            2024-02-01 transfer\n\
            \tassets:bank-a   -100 GBP\n\
            \tassets:bank-b    100 GBP\n";
        let (mut txs, db, prec) = setup(src);
        realize_capital(&mut txs, &caps(), Some("GBP"), &db, &prec);
        assert!(!any_capital(&txs));
    }

    #[test]
    fn open_lots_reports_remaining_fifo_queue() {
        // Two buys, one partial sale: FIFO consumes lot 1 entirely and
        // 1 BTC of lot 2 — the report shows the 1-BTC remainder of lot 2
        // at its own basis and date, front first.
        let src = "\
            2024-01-01 buy a\n\
            \tassets:btc    1 BTC @ 30000 USD\n\
            \tassets:cash  -30000 USD\n\
            2024-02-01 buy b\n\
            \tassets:btc    2 BTC @ 40000 USD\n\
            \tassets:cash  -80000 USD\n\
            2024-06-01 sell\n\
            \tassets:btc   -2 BTC @ 50000 USD\n\
            \tassets:cash   100000 USD\n";
        let (txs, db, _) = setup(src);
        let lots = open_lots(&txs, None, &db);
        // Native mode tracks every commodity, so the USD cash side opens
        // lots too — pick out the BTC position.
        let btc: Vec<_> = lots.iter().filter(|l| l.commodity == "BTC").collect();
        assert_eq!(btc.len(), 1);
        assert_eq!(btc[0].account, "assets:btc");
        assert_eq!(btc[0].qty, Decimal::parse("1").unwrap());
        assert_eq!(btc[0].cost_per_unit, Decimal::parse("40000").unwrap());
        assert_eq!(btc[0].cost_commodity, "USD");
        assert_eq!(btc[0].date.to_string(), "2024-02-01");
    }

    #[test]
    fn open_lots_carry_market_basis_under_x() {
        // Under -X the open lot's basis is the acquisition-day market
        // value, not the booked trade rate.
        let src = "\
            P 2024-01-01 BTC GBP 90\n\
            2024-01-01 buy\n\
            \tassets:btc    1 BTC\n\
            \tassets:cash  -85 GBP\n";
        let (txs, db, _) = setup(src);
        let lots = open_lots(&txs, Some("GBP"), &db);
        assert_eq!(lots.len(), 1);
        assert_eq!(lots[0].cost_per_unit, Decimal::parse("90").unwrap());
        assert_eq!(lots[0].cost_commodity, "GBP");
    }

    #[test]
    fn open_lots_follow_transfers_with_original_date() {
        // Buy on the exchange, transfer to the wallet: the open lot sits
        // on the wallet with the exchange's basis and acquisition date.
        let src = "\
            2024-01-01 buy\n\
            \tassets:exchange   1 BTC @ 100 USD\n\
            \tassets:cash    -100 USD\n\
            2024-02-01 transfer\n\
            \tassets:exchange  -1 BTC\n\
            \tassets:wallet     1 BTC\n";
        let (txs, db, _) = setup(src);
        let lots = open_lots(&txs, None, &db);
        assert_eq!(lots.len(), 1);
        assert_eq!(lots[0].account, "assets:wallet");
        assert_eq!(lots[0].cost_per_unit, Decimal::parse("100").unwrap());
        assert_eq!(lots[0].date.to_string(), "2024-01-01");
    }

    #[test]
    fn open_lots_do_not_mutate_the_journal() {
        // The report path must leave the transactions untouched — no
        // split legs, no injected capital postings.
        let src = "\
            2024-01-01 buy\n\
            \tassets:btc    1 BTC @ 30000 USD\n\
            \tassets:cash  -30000 USD\n\
            2024-06-01 sell\n\
            \tassets:btc   -1 BTC @ 50000 USD\n\
            \tassets:cash   50000 USD\n";
        let (txs, db, _) = setup(src);
        let _ = open_lots(&txs, None, &db);
        assert!(!any_capital(&txs));
        assert!(split_legs(&txs, "BTC").is_empty());
    }

    #[test]
    fn open_lots_carry_booked_rate_under_x() {
        // Bought at a trade rate of 100 USD while the market said 90 GBP;
        // USD/GBP = 0.8. The market basis is 90 GBP (capital math), the
        // booked basis 100 × 0.8 = 80 GBP (what was actually paid).
        let src = "\
            P 2024-01-01 BTC GBP 90\n\
            P 2024-01-01 USD GBP 0.8\n\
            2024-01-01 buy\n\
            \tassets:btc    1 BTC @ 100 USD\n\
            \tassets:cash  -100 USD\n";
        let (txs, db, _) = setup(src);
        let lots = open_lots(&txs, Some("GBP"), &db);
        let btc: Vec<_> = lots.iter().filter(|l| l.commodity == "BTC").collect();
        assert_eq!(btc.len(), 1);
        assert_eq!(btc[0].cost_per_unit, Decimal::parse("90").unwrap());
        assert_eq!(btc[0].booked_per_unit, Some(Decimal::parse("80").unwrap()));
    }

    #[test]
    fn booked_rate_survives_a_transfer() {
        let src = "\
            P 2024-01-01 BTC GBP 90\n\
            2024-01-01 buy\n\
            \tassets:exchange   1 BTC @ 85 GBP\n\
            \tassets:cash     -85 GBP\n\
            2024-02-01 transfer\n\
            \tassets:exchange  -1 BTC\n\
            \tassets:wallet     1 BTC\n";
        let (txs, db, _) = setup(src);
        let lots = open_lots(&txs, Some("GBP"), &db);
        assert_eq!(lots.len(), 1);
        assert_eq!(lots[0].account, "assets:wallet");
        assert_eq!(lots[0].booked_per_unit, Some(Decimal::parse("85").unwrap()));
    }

    #[test]
    fn realizations_report_booked_cost_and_proceeds() {
        // Bought for a booked 85 GBP (market 90), sold for a booked
        // 150 GBP (market 140): the report carries the real payments —
        // cost 85, proceeds 150 — alongside the market figures.
        let src = "\
            P 2024-01-01 BTC GBP 90\n\
            P 2024-06-01 BTC GBP 140\n\
            2024-01-01 buy\n\
            \tassets:btc    1 BTC @ 85 GBP\n\
            \tassets:cash  -85 GBP\n\
            2024-06-01 sell\n\
            \tassets:btc   -1 BTC @ 150 GBP\n\
            \tassets:cash   150 GBP\n";
        let (txs, db, _) = setup(src);
        let sales = realizations(&txs, Some("GBP"), &db);
        assert_eq!(sales.len(), 1);
        let s = &sales[0];
        assert_eq!(s.account, "assets:btc");
        assert_eq!(s.qty, Decimal::parse("1").unwrap());
        assert_eq!(s.first_buy_date.to_string(), "2024-01-01");
        assert_eq!(s.sell_date.to_string(), "2024-06-01");
        assert_eq!(s.market_cost, Decimal::parse("90").unwrap());
        assert_eq!(s.market_proceeds, Decimal::parse("140").unwrap());
        assert_eq!(s.booked_cost, Some(Decimal::parse("85").unwrap()));
        assert_eq!(s.booked_proceeds, Some(Decimal::parse("150").unwrap()));
        assert!(!s.short);
    }

    #[test]
    fn realizations_include_zero_gain_sales() {
        // Buy and sell at the same market rate: realize_capital books
        // nothing (no gain), but the report still shows the sale.
        let src = "\
            2024-01-01 buy\n\
            \tassets:btc    1 BTC\n\
            \tassets:cash  -30000 USD\n\
            2024-06-01 sell\n\
            \tassets:btc   -1 BTC\n\
            \tassets:cash   30000 USD\n";
        let (txs, db, _) = setup(src);
        let sales = realizations(&txs, None, &db);
        assert!(sales.iter().any(|s| s.commodity == "BTC"));
    }

    #[test]
    fn realizations_aggregate_lots_per_sale() {
        // One sale pulling two lots is ONE report entry with the FIFO
        // cost summed: 4×100 + 6×110 = 1060, earliest buy date kept.
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
        let (txs, db, _) = setup(src);
        let sales = realizations(&txs, None, &db);
        let btc: Vec<_> = sales.iter().filter(|s| s.commodity == "BTC").collect();
        assert_eq!(btc.len(), 1);
        assert_eq!(btc[0].qty, Decimal::parse("10").unwrap());
        assert_eq!(btc[0].market_cost, Decimal::parse("1060").unwrap());
        assert_eq!(btc[0].first_buy_date.to_string(), "2024-01-01");
    }

    #[test]
    fn same_day_sale_before_arrival_still_realizes() {
        // The wallet's sale is booked BEFORE the transfer's arrival on
        // the same day (file order). The day-settling trade retry must
        // close the sale against the late-arriving lot: gain 50
        // realized, no lot stranded on the wallet.
        let src = "\
            2024-01-01 buy\n\
            \tassets:exchange   1 BTC @ 100 USD\n\
            \tassets:cash    -100 USD\n\
            2024-02-01 sell\n\
            \tassets:wallet    -1 BTC @ 150 USD\n\
            \tassets:cash     150 USD\n\
            2024-02-01 transfer\n\
            \tassets:exchange  -1 BTC\n\
            \tassets:wallet     1 BTC\n";
        let (mut txs, db, prec) = setup(src);
        realize_capital(&mut txs, &caps(), None, &db, &prec);
        assert_eq!(gain_on(&txs, "income:capital"), Decimal::parse("-50").unwrap());
    }

    #[test]
    fn same_day_sale_before_arrival_strands_no_lots() {
        let src = "\
            2024-01-01 buy\n\
            \tassets:exchange   1 BTC @ 100 USD\n\
            \tassets:cash    -100 USD\n\
            2024-02-01 sell\n\
            \tassets:wallet    -1 BTC @ 150 USD\n\
            \tassets:cash     150 USD\n\
            2024-02-01 transfer\n\
            \tassets:exchange  -1 BTC\n\
            \tassets:wallet     1 BTC\n";
        let (txs, db, _) = setup(src);
        let lots = open_lots(&txs, None, &db);
        assert!(!lots.iter().any(|l| l.commodity == "BTC"));
    }

    #[test]
    fn same_day_sale_against_target_covers_before_shorting() {
        // The sale trades BTC directly against the target (GBP) and is
        // booked before the same-day arrival. It must NOT open a short
        // that silently swallows the arrival gain-free — the day-settling
        // retry covers it first: gain 1 × (150 − 100) = 50, no lots left.
        let src = "\
            P 2024-01-01 BTC GBP 100\n\
            P 2024-02-01 BTC GBP 150\n\
            2024-01-01 buy\n\
            \tassets:exchange   1 BTC\n\
            \tassets:cash    -100 GBP\n\
            2024-02-01 sell\n\
            \tassets:wallet    -1 BTC\n\
            \tassets:cash     150 GBP\n\
            2024-02-01 transfer\n\
            \tassets:exchange  -1 BTC\n\
            \tassets:wallet     1 BTC\n";
        let (mut txs, db, prec) = setup(src);
        realize_capital(&mut txs, &caps(), Some("GBP"), &db, &prec);
        assert_eq!(gain_on(&txs, "income:capital"), Decimal::parse("-50").unwrap());
        let (txs2, db2, _) = setup(src);
        assert!(open_lots(&txs2, Some("GBP"), &db2).is_empty());
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
