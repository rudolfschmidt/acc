//! Realizer phase — inject FX gain/loss postings.
//!
//! Runs inside the enrich pipeline (after the expander), i.e. *before*
//! filtering and rebalance, and only when the user passes `-X TARGET`.
//! For every multi-commodity transaction we
//! convert each balance-contributing posting into `target` at the
//! market rate on `tx.date` (from the price DB). The sum of those
//! converted amounts is the *realized delta* between what the books
//! say and what the market says:
//!
//! - `delta > 0` → the user got more value than the market implied →
//!   **gain** → credit the `fx-realized gain` account (income, negative posting)
//! - `delta < 0` → the user got less value → **loss** → debit the
//!   `fx-realized loss` account (expense, positive posting)
//! - `|delta|` below the target commodity's display precision → noop
//!   (rounding artefact from per-unit cost math)
//!
//! Skipped when:
//! - the journal declares no `fx-realized gain` / `fx-realized loss` accounts,
//! - a conversion rate is missing for any posting (the transaction's
//!   implied rate can't be compared to a market that isn't known).

use std::collections::{HashMap, HashSet};

use crate::decimal::Decimal;
use crate::indexer::Index;
use crate::parser::located::Located;
use crate::parser::posting::{Amount, Posting};
use crate::parser::transaction::Transaction;

/// Augment every transaction in-place with an FX gain/loss posting
/// where the implied rate diverges from the market rate. See the
/// module docs for the full semantics.
pub fn realize(
    txs: &mut [Located<Transaction>],
    target: &str,
    db: &Index,
    precisions: &HashMap<String, usize>,
    fx_realized_gain: &str,
    fx_realized_loss: &str,
) {
    let precision = precisions.get(target).copied().unwrap_or(2);
    for lt in txs.iter_mut() {
        augment(lt, target, db, precision, fx_realized_gain, fx_realized_loss);
    }
}

fn augment(
    lt: &mut Located<Transaction>,
    target: &str,
    db: &Index,
    precision: usize,
    fx_realized_gain: &str,
    fx_realized_loss: &str,
) {
    // Only balance-contributing postings participate: real postings
    // and bracket-virtual (`[account]`); paren-virtual (`(account)`)
    // is informational and stays out of the sum.
    let contributes = |p: &Posting| !p.is_virtual || p.balanced;

    // Need ≥2 distinct commodities — single-commodity transactions
    // can't have FX delta by definition.
    let mut commodities: HashSet<&str> = HashSet::new();
    for lp in &lt.value.postings {
        if !contributes(&lp.value) {
            continue;
        }
        if let Some(a) = &lp.value.amount {
            commodities.insert(a.commodity.as_str());
        }
    }
    if commodities.len() < 2 {
        return;
    }

    // Sum contributing postings after conversion to target. A missing
    // rate disqualifies the whole transaction from realizer treatment.
    let date = lt.value.date.to_string();
    let mut total = Decimal::zero();
    for lp in &lt.value.postings {
        if !contributes(&lp.value) {
            continue;
        }
        let Some(a) = &lp.value.amount else { continue };
        let Some(rate) = db.find(&a.commodity, target, &date) else {
            return;
        };
        total += a.value.mul_rounded(rate);
    }

    // Drop rounding noise below the target's display precision.
    if total.is_display_zero(precision) {
        return;
    }

    // The fx posting balances the legs at their MARKET value. An explicit
    // `@`/`@@` cost would otherwise make the rebalancer weight a leg by its
    // booked rate (e.g. `ETH @ BTC0.0904` → the BTC paid) instead of the
    // commodity's market value — leaving the fx unbalanced. The booked
    // rate's deviation from market is exactly what fx captures, so strip
    // the cost annotations and let every contributing leg convert at
    // market. (`{}` lot-costs don't exist yet — the lotter adds them after
    // this phase and they are intentionally kept.)
    for lp in lt.value.postings.iter_mut() {
        if contributes(&lp.value) {
            lp.value.costs = None;
        }
    }

    // Posting convention: the injected amount flips the delta's sign
    // so the books balance again in `target`. Positive delta = gain:
    // credit income (negative posting). Negative delta = loss: debit
    // expense (positive posting).
    let (account, value) = if total.is_negative() {
        (fx_realized_loss, -total)
    } else {
        (fx_realized_gain, -total)
    };

    lt.value.postings.push(Located {
        file: lt.file.clone(),
        line: lt.line,
        value: Posting {
            account: account.to_string(),
            amount: Some(Amount {
                commodity: target.to_string(),
                value,
                decimals: precision,
            }),
            costs: None,
            lot_cost: None,
            lot_date: None,
            balance_assertion: None,
            // Real posting: the spread is the trade-day delta between the
            // legs' market value (after `-X` conversion). It sits next to
            // the converted amounts and makes the transaction balance in
            // the target currency, so the output is 1:1 copyable. Real
            // postings always contribute to the balance, hence `balanced`.
            is_virtual: false,
            balanced: true,
            comments: Vec::new(),
        },
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;
    use crate::resolver;

    fn build(src: &str) -> (Vec<Located<Transaction>>, Index) {
        let entries = parser::parse(src).unwrap();
        let resolved = resolver::resolve(entries).unwrap();
        let prices = crate::indexer::index(resolved.prices);
        let txs = crate::booker::book(resolved.transactions).unwrap();
        (txs, prices)
    }

    #[test]
    fn gain_when_implied_rate_above_market() {
        // Market USD→EUR on 2024-06-15 = 0.90
        // Tx trades 100 USD for 92 EUR (implied 0.92 — user got more EUR).
        let src = "\
            P 2024-06-15 USD EUR 0.9\n\
            2024-06-15 * x\n\
            \tassets:usd  -100 USD\n\
            \tassets:eur   92 EUR\n";
        let (mut txs, db) = build(src);
        realize(&mut txs, "EUR", &db, &HashMap::new(), "in:gain", "ex:loss");
        let posted = &txs[0].value.postings;
        assert_eq!(posted.len(), 3);
        let injected = &posted[2].value;
        assert_eq!(injected.account, "in:gain");
        let amt = injected.amount.as_ref().unwrap();
        assert_eq!(amt.commodity, "EUR");
        // 100 × 0.90 = 90, +92 = +2 delta → credit income -2.
        assert_eq!(amt.value, Decimal::from(-2));
    }

    #[test]
    fn loss_when_implied_rate_below_market() {
        let src = "\
            P 2024-06-15 USD EUR 0.9\n\
            2024-06-15 * x\n\
            \tassets:usd  -100 USD\n\
            \tassets:eur   88 EUR\n";
        let (mut txs, db) = build(src);
        realize(&mut txs, "EUR", &db, &HashMap::new(), "in:gain", "ex:loss");
        let injected = &txs[0].value.postings[2].value;
        assert_eq!(injected.account, "ex:loss");
        assert_eq!(injected.amount.as_ref().unwrap().value, Decimal::from(2));
    }

    #[test]
    fn single_commodity_skipped() {
        let src = "\
            2024-06-15 * x\n\
            \texpenses:food  -5 EUR\n\
            \tassets:cash     5 EUR\n";
        let (mut txs, db) = build(src);
        realize(&mut txs, "EUR", &db, &HashMap::new(), "in:gain", "ex:loss");
        assert_eq!(txs[0].value.postings.len(), 2);
    }

    #[test]
    fn missing_rate_skipped() {
        let src = "\
            2024-06-15 * x\n\
            \tassets:usd  -100 USD\n\
            \tassets:eur    92 EUR\n";
        let (mut txs, db) = build(src);
        realize(&mut txs, "EUR", &db, &HashMap::new(), "in:gain", "ex:loss");
        assert_eq!(txs[0].value.postings.len(), 2);
    }

    #[test]
    fn delta_below_precision_skipped() {
        // 100 × 0.91999 = 91.999 + 92 = 0.001 → below 2-decimal display.
        let src = "\
            P 2024-06-15 USD EUR 0.91999\n\
            2024-06-15 * x\n\
            \tassets:usd  -100 USD\n\
            \tassets:eur   92 EUR\n";
        let (mut txs, db) = build(src);
        let mut precs = HashMap::new();
        precs.insert("EUR".to_string(), 2);
        realize(&mut txs, "EUR", &db, &precs, "in:gain", "ex:loss");
        assert_eq!(txs[0].value.postings.len(), 2);
    }
}
