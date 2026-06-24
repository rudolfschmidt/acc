//! Rebalance phase — convert every posting's amount into a target
//! commodity using the journal's PriceDB.
//!
//! Each posting is converted at the rate on its own transaction date
//! (historical valuation). Postings whose commodity has no rate path to
//! `target` stay unchanged — downstream reports show them as remainders
//! in their original commodity.
//!
//! A posting that carries a `{}` cost-basis is converted via its balance
//! *weight* (`quantity × cost-basis`), not its market value — exactly
//! beancount's `get_weight`. This is what keeps a disposal transaction
//! balanced after conversion: the asset leaves at its cost basis, the
//! realized gain (already in the target currency) makes up the rest, and
//! the converted transaction sums to zero. After converting, the now-
//! redundant lot annotations (`{}`, `[date]`, `@`) are dropped so the
//! output is clean.
//!
//! Conversion keeps full Decimal precision — it does NOT round to the
//! target's display precision. Rounding here would make pass-through
//! accounts (whose native legs net to zero across transactions) drift by
//! sub-cent amounts once each leg is rounded independently. Display
//! rounding is the printer's job; `bal`/`reg` need exact sums.

use std::collections::HashMap;

use crate::decimal::Decimal;
use crate::indexer::Index;
use crate::parser::located::Located;
use crate::parser::posting::{Costs, Posting};
use crate::parser::transaction::Transaction;

/// Convert every posting's amount to `target` in place, each at the
/// exchange rate on its own transaction date.
pub fn rebalance(transactions: &mut [Located<Transaction>], target: &str, db: &Index) {
    for lt in transactions {
        let lookup_date: String = lt.value.date.to_string();
        for lp in &mut lt.value.postings {
            convert(&mut lp.value, target, db, &lookup_date);
        }
    }
}

/// Round every `target`-commodity amount to display precision and absorb
/// the per-transaction round-off into the largest leg, so the *printed*
/// (rounded) amounts still sum to zero and `print -X` output reloads
/// cleanly. **Print-only**: `bal`/`reg` keep full precision so that
/// pass-through accounts (whose native legs net to zero across many
/// transactions) don't accumulate sub-cent drift from per-leg rounding.
pub fn round_for_print(
    transactions: &mut [Located<Transaction>],
    target: &str,
    precisions: &HashMap<String, usize>,
) {
    let prec = precisions.get(target).copied().unwrap_or(2);
    let unit = display_unit(prec);
    for lt in transactions {
        for lp in &mut lt.value.postings {
            if let Some(a) = lp.value.amount.as_mut() {
                if a.commodity == target {
                    a.value = a.value.round(prec);
                    a.decimals = prec;
                }
            }
        }
        settle_round_off(&mut lt.value, target, unit);
    }
}

/// `10^-precision` as a Decimal (e.g. precision 2 → 0.01).
fn display_unit(precision: usize) -> Decimal {
    let ten = Decimal::from(10);
    let mut unit = Decimal::from(1);
    for _ in 0..precision {
        unit = unit.div_rounded(ten);
    }
    unit
}

/// Absorb a transaction's rounding residual into its largest target leg.
///
/// Two guards keep this from corrupting genuinely-unbalanced output:
/// - bail if any balance-contributing leg is *not* in `target` (the
///   transaction isn't fully converted, so a non-zero sum is a real
///   remainder, not round-off);
/// - only absorb a residual no larger than one display `unit` per leg.
///   A bigger sum means the transaction is incomplete — e.g. an account
///   pattern filter dropped its counter-postings — and is left untouched.
fn settle_round_off(tx: &mut Transaction, target: &str, unit: Decimal) {
    let mut sum = Decimal::zero();
    let mut count: i64 = 0;
    let mut largest: Option<usize> = None;
    let mut largest_abs = Decimal::zero();
    for (i, lp) in tx.postings.iter().enumerate() {
        if lp.value.is_virtual && !lp.value.balanced {
            continue;
        }
        let Some(a) = &lp.value.amount else { continue };
        if a.commodity != target {
            return;
        }
        sum = sum + a.value;
        count += 1;
        if a.value.abs() > largest_abs {
            largest_abs = a.value.abs();
            largest = Some(i);
        }
    }
    if sum.is_zero() {
        return;
    }
    let max_residual = unit.mul_rounded(Decimal::from(count));
    if sum.abs() > max_residual {
        return;
    }
    if let Some(i) = largest {
        let amount = tx.postings[i].value.amount.as_mut().unwrap();
        amount.value = amount.value - sum;
    }
}

/// A posting's balance *weight* in `target` (beancount `get_weight`,
/// then converted): `{}` cost-basis → quantity × cost; `@`/`@@` price →
/// quantity × price; otherwise the amount itself — each in its own
/// commodity, then converted to `target` at `date`. `None` if there is
/// no amount or no rate path.
///
/// This is the single source of truth for what a posting is worth in the
/// target currency. The rebalancer uses it to rewrite amounts, and the
/// translator uses it to measure pass-through drift — they MUST agree, or
/// a converted transaction won't balance (weighting by the booked rate
/// keeps both legs of a trade netting to zero).
pub fn target_value(p: &Posting, target: &str, db: &Index, date: &str) -> Option<Decimal> {
    let amount = p.amount.as_ref()?;
    let (value, from) = if let Some(lot) = &p.lot_cost {
        (lot.weight(amount.value), lot.amount.commodity.as_str())
    } else if let Some(costs) = &p.costs {
        match costs {
            Costs::PerUnit(c) => (amount.value.mul_rounded(c.value), c.commodity.as_str()),
            // Total cost is the whole leg; carry the amount's sign.
            Costs::Total(c) if amount.value.is_negative() => (-c.value, c.commodity.as_str()),
            Costs::Total(c) => (c.value, c.commodity.as_str()),
        }
    } else {
        (amount.value, amount.commodity.as_str())
    };

    if from == target {
        return Some(value);
    }
    // `mul_rounded` instead of `*` because inverse-rate lookups from the
    // PriceDB can serve a 28-digit tail which would overflow strict `*`.
    db.find(from, target, date).map(|rate| value.mul_rounded(rate))
}

fn convert(p: &mut Posting, target: &str, db: &Index, date: &str) {
    let Some(converted) = target_value(p, target, db, date) else {
        // No amount or no rate path — keep the posting unchanged.
        return;
    };
    let amount = p.amount.as_mut().unwrap();
    amount.value = converted;
    amount.commodity = target.to_string();

    // The converted amount already embodies the weight; the lot/cost
    // annotations carry the old commodity and would now be wrong, so
    // drop them. Output reads as plain target-currency values.
    p.lot_cost = None;
    p.costs = None;
    p.lot_date = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::posting::Posting;
    use crate::{booker, indexer, parser, resolver};

    /// Parse → resolve → book a source string and index its prices.
    fn setup(src: &str) -> (Vec<Located<Transaction>>, Index) {
        let entries = parser::parse(src).unwrap();
        let resolved = resolver::resolve(entries).unwrap();
        let txs = booker::book(resolved.transactions).unwrap();
        let db = indexer::index(resolved.prices);
        (txs, db)
    }

    /// The first posting whose account starts with `prefix`.
    fn leg<'a>(txs: &'a [Located<Transaction>], prefix: &str) -> &'a Posting {
        txs.iter()
            .flat_map(|t| t.value.postings.iter())
            .map(|lp| &lp.value)
            .find(|p| p.account.starts_with(prefix))
            .expect("posting not found")
    }

    fn dec(s: &str) -> Decimal {
        Decimal::parse(s).unwrap()
    }

    // ── target_value: the get_weight ladder ──────────────────────────

    #[test]
    fn target_value_plain_amount_converts_at_txdate_rate() {
        let (txs, db) = setup(
            "P 2024-06-01 USD EUR 0.9\n\
             2024-06-01 * x\n\
             \tassets:usd   100 USD\n\
             \tequity:open -100 USD\n",
        );
        let v = target_value(leg(&txs, "assets:usd"), "EUR", &db, "2024-06-01");
        assert_eq!(v, Some(dec("90"))); // 100 × 0.9
    }

    #[test]
    fn target_value_lot_cost_weights_by_cost_not_market() {
        // `{95 EUR}` cost-basis weight is quantity × cost (-950), NOT the
        // market value (-10 × 200 = -2000).
        let (txs, db) = setup(
            "P 2024-06-01 ASSET EUR 200\n\
             2024-06-01 * sell\n\
             \tassets:broker  -10 ASSET {95 EUR}\n\
             \tassets:cash    950 EUR\n",
        );
        let v = target_value(leg(&txs, "assets:broker"), "EUR", &db, "2024-06-01");
        assert_eq!(v, Some(dec("-950")));
    }

    #[test]
    fn target_value_per_unit_price_weight() {
        // `@ 95 EUR` per-unit price: quantity × price.
        let (txs, db) = setup(
            "2024-06-01 * sell\n\
             \tassets:broker  -10 ASSET @ 95 EUR\n\
             \tassets:cash    950 EUR\n",
        );
        let v = target_value(leg(&txs, "assets:broker"), "EUR", &db, "2024-06-01");
        assert_eq!(v, Some(dec("-950")));
    }

    #[test]
    fn target_value_total_cost_carries_amount_sign() {
        // `@@ 950 EUR` total cost on a negative leg weighs -950.
        let (txs, db) = setup(
            "2024-06-01 * sell\n\
             \tassets:broker  -10 ASSET @@ 950 EUR\n\
             \tassets:cash    950 EUR\n",
        );
        let v = target_value(leg(&txs, "assets:broker"), "EUR", &db, "2024-06-01");
        assert_eq!(v, Some(dec("-950")));
    }

    #[test]
    fn target_value_same_commodity_is_identity() {
        let (txs, db) = setup(
            "2024-06-01 * x\n\
             \tassets:eur   100 EUR\n\
             \tequity:open -100 EUR\n",
        );
        let v = target_value(leg(&txs, "assets:eur"), "EUR", &db, "2024-06-01");
        assert_eq!(v, Some(dec("100"))); // no rate lookup needed
    }

    #[test]
    fn target_value_missing_rate_is_none() {
        let (txs, db) = setup(
            "2024-06-01 * x\n\
             \tassets:usd   100 USD\n\
             \tequity:open -100 USD\n",
        );
        // No P-directive for USD→EUR.
        assert_eq!(target_value(leg(&txs, "assets:usd"), "EUR", &db, "2024-06-01"), None);
    }

    // ── round_for_print: display rounding + residual absorption ───────

    fn eur_leg<'a>(tx: &'a Transaction, prefix: &str) -> &'a Posting {
        tx.postings
            .iter()
            .map(|lp| &lp.value)
            .find(|p| p.account.starts_with(prefix))
            .unwrap()
    }

    #[test]
    fn round_for_print_rounds_to_display_precision() {
        let (mut txs, db) = setup(
            "P 2024-06-01 USD EUR 0.93331\n\
             2024-06-01 * x\n\
             \tassets:usd   100 USD\n\
             \tequity:open -100 USD\n",
        );
        rebalance(&mut txs, "EUR", &db);
        let prec = HashMap::from([("EUR".to_string(), 2usize)]);
        round_for_print(&mut txs, "EUR", &prec);
        // 100 × 0.93331 = 93.331 → 93.33 at 2 decimals.
        let v = eur_leg(&txs[0].value, "assets:usd").amount.as_ref().unwrap();
        assert_eq!(v.value, dec("93.33"));
    }

    #[test]
    fn round_for_print_absorbs_residual_into_largest_leg() {
        // Three legs that each round independently can leave a sub-cent
        // residual; it must be absorbed into the largest leg so the
        // printed amounts still sum to zero.
        let (mut txs, db) = setup(
            "P 2024-06-01 USD EUR 0.33335\n\
             2024-06-01 * split\n\
             \texpenses:a    10 USD\n\
             \texpenses:b    10 USD\n\
             \tassets:cash  -20 USD\n",
        );
        rebalance(&mut txs, "EUR", &db);
        let prec = HashMap::from([("EUR".to_string(), 2usize)]);
        round_for_print(&mut txs, "EUR", &prec);
        let sum: Decimal = txs[0]
            .value
            .postings
            .iter()
            .filter_map(|lp| lp.value.amount.as_ref())
            .fold(Decimal::zero(), |acc, a| acc + a.value);
        assert!(sum.is_zero(), "rounded legs must still sum to zero, got {:?}", sum);
    }

    #[test]
    fn round_for_print_leaves_unconverted_leg_alone() {
        // A leg with no rate path stays in its native commodity, so the
        // transaction is not fully in the target — the residual is real,
        // not round-off, and must not be absorbed.
        let (mut txs, db) = setup(
            "P 2024-06-01 USD EUR 0.9\n\
             2024-06-01 * mixed\n\
             \tassets:usd   100 USD\n\
             \tassets:gbp  -100 GBP\n",
        );
        rebalance(&mut txs, "EUR", &db);
        let prec = HashMap::from([("EUR".to_string(), 2usize)]);
        round_for_print(&mut txs, "EUR", &prec);
        // GBP leg has no rate → stays GBP; USD leg became 90 EUR. The
        // mismatch is untouched (no spurious absorption).
        let usd = eur_leg(&txs[0].value, "assets:usd").amount.as_ref().unwrap();
        assert_eq!(usd.value, dec("90"));
        let gbp = eur_leg(&txs[0].value, "assets:gbp").amount.as_ref().unwrap();
        assert_eq!(gbp.commodity, "GBP");
    }
}
