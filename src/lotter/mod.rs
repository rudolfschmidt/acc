//! Lotter phase — realized capital gains via FIFO lot tracking.
//!
//! Runs whenever the journal declares both a `capital gain` and a
//! `capital loss` account. It walks transactions chronologically and
//! tracks a per-(account, commodity) FIFO queue of lots: an exchange
//! posting that *acquires* a commodity opens a lot at its booked cost, a
//! posting that *disposes* of it closes lots front-to-back and realizes
//! the gain (proceeds − cost basis).
//!
//! ## Valuation
//!
//! The gain is **always** computed in the counter-commodity at the
//! *booked* (trade) rate — an explicit `@` cost, or the implied rate of a
//! two-commodity exchange. The realized gain is the trade gain
//! (proceeds − what you paid), never the target-value drift of the held
//! commodity. The disposal legs carry that booked rate as a `{}`
//! cost-basis, so the rebalancer later converts everything to the target
//! at the *disposal's own date* — the realization rate.
//!
//! ## With `-X TARGET`: market / spread split
//!
//! The gain is split into two parts (both still in the counter-commodity;
//! the rebalancer converts them at realization):
//!
//! - **market** — the market-price movement of the traded commodity
//!   against the counter-commodity over the holding period
//!   (`market_rate(sell) − market_rate(buy)`). Booked to the `capital`
//!   account. `market_rate` goes *via the target* to avoid inconsistent
//!   crypto↔crypto graph paths — see [`market_rate`].
//! - **spread** — the rest (`gain − market`): how the trade executed
//!   relative to the market price (slippage/fees). Booked to the `fx`
//!   account if declared, else folded back into `capital`.
//!
//! Without `-X`, the whole gain is one `capital` posting in the booked
//! commodity (no split — there is no market rate to compare against).
//! Mixed-currency native disposals (bought in EUR, sold in USD) can't be
//! netted and are skipped; they need `-X`.
//!
//! ## Injection
//!
//! The gain posting(s) are **real** (not virtual) and live inside the
//! disposal transaction, balancing against the `{}` cost-basis on the
//! asset legs. So `print` is 1:1 copy-pasteable and survives a reload.
//!
//! The lotter pins each realized leg to its booked rate, so a tracked
//! account's legs already sum to zero under conversion (the gain lands on
//! the capital account, not the transit account). CTA can therefore run
//! over every pass-through account without excluding lot-tracked ones —
//! it sees no drift where the lotter already booked it, and no
//! double-count results.

use std::collections::{HashMap, VecDeque};

use crate::date::Date;
use crate::decimal::Decimal;
use crate::indexer::Index;
use crate::parser::located::Located;
use crate::parser::posting::{Amount, Costs, LotCost, Posting};
use crate::parser::transaction::Transaction;

/// One open lot: a remaining quantity carrying its per-unit cost and
/// the date it was acquired (FIFO order is insertion order; the date
/// feeds the holding period and the title).
///
/// `cost_per_unit` is the *booked* (trade) rate in the counter-commodity
/// — e.g. `0.0904 BTC` per ETH. `market_at_buy` is the *market* rate of
/// the traded commodity in that same counter-commodity on the buy date
/// (price DB); only set under `-X`, it splits the realized gain into a
/// market-movement part and a trade-day-spread part.
struct Lot {
    qty: Decimal,
    cost_per_unit: Decimal,
    cost_commodity: String,
    market_at_buy: Option<Decimal>,
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
/// All amounts are in the counter-commodity (`gain_commodity`); the
/// rebalancer converts them to the target at the disposal's own date
/// (the realization rate). `market` is the market-movement part of the
/// gain (`Some` only under `-X`); the spread part is `gain − market`.
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
    /// the split legs when the source had none (e.g. under `-X`).
    proceeds_per_unit: Decimal,
    gain: Decimal,
    gain_commodity: String,
    market: Option<Decimal>,
    /// `true` when the closing posting was an acquisition (closing short
    /// lots), `false` for a disposal (closing long lots). Drives the sign
    /// of the rewritten legs.
    acquisition: bool,
    lots: Vec<ClosedLot>,
}

/// The accounts a realized gain/loss is booked to: capital for the
/// trade gain (income on a gain, expense on a loss), and — under `-X`,
/// when declared — the fx accounts for the trade-day spread.
pub struct CapitalAccounts<'a> {
    pub capital_gain: &'a str,
    pub capital_loss: &'a str,
    pub fx_gain: Option<&'a str>,
    pub fx_loss: Option<&'a str>,
}

/// Track lots FIFO and inject one capital-gain/loss transaction per
/// realized disposal. See module docs for the valuation semantics.
pub fn realize_capital(
    txs: &mut Vec<Located<Transaction>>,
    accounts: &CapitalAccounts,
    target: Option<&str>,
    db: &Index,
    precisions: &HashMap<String, usize>,
) {
    let mut lots: HashMap<(String, String), VecDeque<Lot>> = HashMap::new();
    let mut disposals: Vec<Disposal> = Vec::new();
    // (tx_idx, posting_idx, implied_rate, commodity) of tracked legs that
    // carry only an *implied* rate (no explicit `@`/`{}`). Under `-X` we
    // pin that rate onto the posting as an `@`, so the rebalancer weights
    // it by the booked rate — not the market rate — keeping it consistent
    // with the cost-basis on the matching disposal. See the buy/sell loop.
    let mut pin_rate: Vec<(usize, usize, Decimal, String)> = Vec::new();

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
            if let Some(t) = target {
                if a.commodity == t {
                    continue;
                }
            }
            let Some((unit_value, value_commodity)) = posting_value(&lp.value, &sums)
            else {
                continue;
            };
            // Under `-X`, a leg traded against the *target* currency at an
            // implied rate (no explicit `@`/`{}`) — e.g. `USD` paid in `€`
            // with no `@` — must be pinned with that rate, else the
            // rebalancer values it at market while the disposal cost-basis
            // uses the implied rate, leaving a drift on the account.
            // Only when the counter-commodity IS the target: for a
            // crypto↔crypto trade (counter ≠ target) the plain market
            // conversion of each leg already balances.
            if target == Some(value_commodity.as_str())
                && lp.value.costs.is_none()
                && lp.value.lot_cost.is_none()
            {
                pin_rate.push((idx, p_idx, unit_value, value_commodity.clone()));
            }
            let key = (lp.value.account.clone(), a.commodity.clone());

            // A `{}` lot-cost means the user books the gain by hand
            // (ledger style) — acc consumes lots for FIFO consistency but
            // injects nothing and opens no lot.
            let manual = lp.value.lot_cost.is_some();

            let queue = lots.entry(key.clone()).or_default();

            // Mixed-currency disposal: the proceeds are in `value_commodity`
            // but the lot being closed is costed in another commodity (e.g.
            // ETH bought for BTC, sold for €). Translate the proceeds rate
            // into the lot's cost commodity at the disposal date, so gain,
            // market and spread are all computed there — exactly as if the
            // asset had traded against the cost commodity. The cost basis'
            // own drift against the target then surfaces as CTA, instead of
            // the whole move being misclassified as a capital gain.
            //
            // Scoped to proceeds in the *target* currency (`€`-denominated
            // disposals of a foreign-costed lot). A crypto-to-crypto trade —
            // proceeds in another non-target commodity (BTC sold for ETH) —
            // is a different beast: its gain can't be split with a single
            // counter rate, so it falls through to the mixed-skip in the
            // loop below, exactly as before. Likewise when no rate is found.
            let (value_commodity, unit_value) = match queue.front() {
                Some(front)
                    if front.cost_commodity != value_commodity
                        && target == Some(value_commodity.as_str()) =>
                {
                    // Translate via the INVERSE of `cost → proceeds`, the
                    // same rate the rebalancer later uses to convert the
                    // cost-basis and gain back to the target. Using a
                    // separate `proceeds → cost` lookup would pick a
                    // possibly non-reciprocal graph path and leave the
                    // transaction a few cents unbalanced.
                    match db.find(&front.cost_commodity, &value_commodity, &date) {
                        Some(rate) if !rate.is_zero() => {
                            (front.cost_commodity.clone(), unit_value.div_rounded(rate))
                        }
                        _ => (value_commodity, unit_value),
                    }
                }
                _ => (value_commodity, unit_value),
            };

            // Market rate of the commodity in the (possibly translated)
            // counter-commodity on this date — feeds the market/spread split.
            let market_here =
                target.and_then(|t| market_rate(&a.commodity, &value_commodity, t, db, &date));
            let mut remaining = a.value; // signed: + acquires, − disposes
            let mut gain = Decimal::zero();
            let mut market = Decimal::zero();
            let mut market_ok = true;
            let mut closed: Vec<ClosedLot> = Vec::new();
            let mut mixed = false;

            // Close opposite-sign lots front-to-back. A long lot (qty > 0)
            // is closed by a disposal (remaining < 0); a short lot (qty < 0)
            // by an acquisition (remaining > 0). Same sign extends the
            // position, so stop closing.
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
                // consume the lot for FIFO consistency.
                if front.cost_commodity == value_commodity {
                    // Per-unit gain is (close − open) for a long lot and
                    // (open − close) for a short — i.e. `unit_value − cost`
                    // with the sign flipped for shorts.
                    let per = unit_value - front.cost_per_unit;
                    let per = if short { Decimal::zero() - per } else { per };
                    gain = gain + take.mul_rounded(per);
                    match (market_here, front.market_at_buy) {
                        (Some(ms), Some(mb)) => {
                            let m = ms - mb;
                            let m = if short { Decimal::zero() - m } else { m };
                            market = market + take.mul_rounded(m);
                        }
                        _ => market_ok = false,
                    }
                    closed.push(ClosedLot {
                        qty: take,
                        cost_per_unit: front.cost_per_unit,
                        cost_commodity: front.cost_commodity.clone(),
                        date: front.date,
                    });
                } else {
                    mixed = true;
                }
                // Move both toward zero by `take`.
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

            // Open a lot for the unclosed remainder. Longs always. A short
            // (negative remainder = sold before bought) only counts when
            // traded against the target *money* (e.g. `USD` paid in `€`):
            // there the disposal is a genuine short to be closed by a later
            // purchase. For an asset traded against another commodity
            // (counter ≠ target), an unmatched disposal is just the
            // counter-side of a normal trade — opening a short there would
            // double-count the gain (it's already on the asset leg).
            if !remaining.is_zero()
                && (!remaining.is_negative() || target == Some(value_commodity.as_str()))
            {
                queue.push_back(Lot {
                    qty: remaining,
                    cost_per_unit: unit_value,
                    cost_commodity: value_commodity.clone(),
                    market_at_buy: market_here,
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
            // Under `-X`, split the gain: `market` is the market move,
            // spread is the rest. If a market rate was missing for any
            // lot, attribute the whole gain to market (no spread).
            let market = target.map(|_| if market_ok { market } else { gain });
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
                market,
                acquisition: !a.value.is_negative(),
                lots: closed,
            });
        }
    }

    // Pin the implied rate as an `@` on tracked legs that lacked one
    // (acquisitions, uncovered shorts). Done before the rewrite below so
    // posting indices are still valid; disposals that get rewritten just
    // overwrite this with their split legs. Skip if the posting already
    // gained a cost annotation (e.g. a manual `{}` disposal).
    for (tx_idx, p_idx, rate, commodity) in pin_rate {
        let p = &mut txs[tx_idx].value.postings[p_idx].value;
        if p.costs.is_none() && p.lot_cost.is_none() {
            let prec = precisions.get(&commodity).copied().unwrap_or(2);
            p.costs = Some(Costs::PerUnit(Amount {
                commodity,
                value: rate,
                decimals: prec,
            }));
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
/// cost), and one real capital posting per disposal carries the
/// realized gain (`income` for a gain, `expense` for a loss). Original
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
        // Leg amount sign: a disposal removes the commodity (negative), an
        // acquisition closing a short adds it back (positive).
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
        // (FIFO ran out — a short, or acquisitions booked outside this
        // file). The uncovered remainder has no cost basis; keep it as a
        // plain proceeds-priced leg (no `{}`) so the full disposed
        // quantity survives the rewrite and the transaction still
        // balances after conversion.
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
        // Inject the realized gain as real posting(s) in the counter-
        // commodity (the rebalancer converts them to the target at the
        // disposal date — the realization rate). They balance against the
        // `{}` cost-basis on the asset legs, so the tx still sums to zero.
        //
        // Under `-X` the gain splits into the market-movement part (on the
        // capital account) and the trade-day spread (on the fx account);
        // natively it's a single capital posting. No fx accounts declared
        // → the spread folds back into capital.
        let mut parts: Vec<(Decimal, &str, &str)> = Vec::new();
        match disp.market {
            Some(market) => {
                parts.push((market, accounts.capital_gain, accounts.capital_loss));
                let spread = disp.gain - market;
                match (accounts.fx_gain, accounts.fx_loss) {
                    (Some(fg), Some(fl)) => parts.push((spread, fg, fl)),
                    _ => parts.push((spread, accounts.capital_gain, accounts.capital_loss)),
                }
            }
            None => parts.push((disp.gain, accounts.capital_gain, accounts.capital_loss)),
        }
        for (value, gain_acct, loss_acct) in parts {
            if value.is_display_zero(price_prec) {
                continue;
            }
            let account = if value.is_negative() {
                loss_acct
            } else {
                gain_acct
            };
            capitals.push(Located {
                file: file.clone(),
                line,
                value: Posting {
                    account: account.to_string(),
                    amount: Some(Amount {
                        commodity: disp.gain_commodity.clone(),
                        value: -value,
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
/// `None` when no rate is derivable.
///
/// This is used in **both** modes. Under `-X` the gain is still computed
/// in the counter-commodity here; the rebalancer converts it to the
/// target at the disposal date (the realization rate), and the market
/// rate (price DB) only feeds the market/spread split — see
/// [`market_rate`]. Valuing at the booked rate (not the market rate)
/// keeps `-X` consistent with native: the realized gain is the trade
/// gain, converted once at realization, not the target-value drift of
/// the held commodity.
fn posting_value(
    p: &Posting,
    sums: &HashMap<String, Decimal>,
) -> Option<(Decimal, String)> {
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

/// Market rate of `commodity` priced in `counter` on `date`, derived
/// *via the target currency* (`commodity→target ÷ counter→target`)
/// rather than a direct `commodity→counter` lookup.
///
/// A direct crypto↔crypto lookup (e.g. `ETH→BTC`) can pick an
/// inconsistent route through the price graph — the BFS may go through a
/// stale `USD BTC` fiat edge instead of the fresh `BTC USDT` one. Routing
/// both legs through the same target shares one consistent path; the
/// target factor cancels in the ratio. Feeds the market/spread split.
fn market_rate(
    commodity: &str,
    counter: &str,
    target: &str,
    db: &Index,
    date: &str,
) -> Option<Decimal> {
    let c = db.find(commodity, target, date)?;
    let g = db.find(counter, target, date)?;
    if g.is_zero() {
        return None;
    }
    Some(c.div_rounded(g))
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

    /// The standard capital-account config used by most tests: income/
    /// expenses:capital, no fx split.
    fn caps() -> CapitalAccounts<'static> {
        CapitalAccounts {
            capital_gain: "income:capital",
            capital_loss: "expenses:capital",
            fx_gain: None,
            fx_loss: None,
        }
    }

    /// Summed value of all postings booking to `account` (the realized
    /// gain/loss; income is negative, expense positive).
    fn gain_on(txs: &[Located<Transaction>], account: &str) -> Decimal {
        let mut sum = Decimal::zero();
        for lt in txs {
            for lp in &lt.value.postings {
                if lp.value.account == account {
                    if let Some(a) = &lp.value.amount {
                        sum = sum + a.value;
                    }
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
    fn split_legs<'a>(
        txs: &'a [Located<Transaction>],
        commodity: &str,
    ) -> Vec<&'a Posting> {
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
    fn market_spread_split_with_x() {
        // Bought 1 BTC paying 29000 USD when the market was 30000; sold
        // paying 51000 USD when the market was 48000. Target = USD, so the
        // gain stays in USD. Total = 51000 − 29000 = 22000, split into:
        //   market = 48000 − 30000 = 18000  (the price moved up)
        //   spread = 22000 − 18000 =  4000  (traded better than market:
        //                                    bought 1000 below, sold 3000 above)
        // market lands on capital, spread on the fx account.
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
        realize_capital(
            &mut txs,
            &CapitalAccounts {
                capital_gain: "income:capital",
                capital_loss: "expenses:capital",
                fx_gain: Some("income:fx"),
                fx_loss: Some("expenses:fx"),
            },
            Some("USD"),
            &db,
            &prec,
        );
        assert_eq!(gain_on(&txs, "income:capital"), Decimal::parse("-18000").unwrap());
        assert_eq!(gain_on(&txs, "income:fx"), Decimal::parse("-4000").unwrap());
        // market + spread = the full 22000 trade gain.
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
        // Pay out 100 USD before ever owning any (1.05 €/USD), then buy
        // it back at 1.03: a short. Closing a short below where it opened
        // is a 100 × (1.05 − 1.03) = €2 gain. A short only realizes when
        // the counter-commodity IS the target money (here €) — selling an
        // asset you don't hold against the cash you measure in.
        let src = "\
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
    fn short_then_long_sequence_realizes_both() {
        // A short closed at a gain, then a long opened and closed at a
        // gain, on the same account. short: 100 USD out @1.05, back @1.03
        // → €2. long: 100 USD in @1.06, out @1.08 → €2. Total €4.
        let src = "\
            2024-01-01 spend1\n\
            \tassets:usd    -100 USD\n\
            \texpenses:dev   105 EUR\n\
            2024-06-01 buy1\n\
            \tassets:bank   -103 EUR\n\
            \tassets:usd     100 USD\n\
            2024-09-01 buy2\n\
            \tassets:bank   -106 EUR\n\
            \tassets:usd     100 USD\n\
            2024-12-01 spend2\n\
            \tassets:usd    -100 USD\n\
            \texpenses:dev   108 EUR\n";
        let (mut txs, db, prec) = setup(src);
        realize_capital(&mut txs, &caps(), Some("EUR"), &db, &prec);
        assert_eq!(gain_on(&txs, "income:capital"), Decimal::parse("-4").unwrap());
    }

    #[test]
    fn oversell_against_non_target_opens_no_short() {
        // Under -X EUR, an asset-vs-asset over-sell must NOT open a short:
        // the counter-commodity (USD) is not the target, so the unmatched
        // disposal is the other side of a normal trade, already accounted
        // for on its own leg. Opening a short here would double-count.
        // Buy 1 BTC for USD, sell 2 BTC: only the covered lot realizes.
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
        // Exactly one covered lot; no short lot for the uncovered 1 BTC.
        assert_eq!(split_legs(&txs, "BTC").len(), 1);
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
