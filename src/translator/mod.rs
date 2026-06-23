//! Translator phase — book Currency Translation Adjustment (CTA).
//!
//! Runs between realizer and filter when the user has declared an
//! account with `fx translation`. For every (account, native_commodity) pair
//! whose native amounts sum to zero over the reporting period (a
//! "transit account"), the phase walks postings chronologically and
//! converts each to the `-x` target at its lookup rate. When the
//! running native sum hits zero but the running target sum has
//! drifted (because rates moved between inflow and outflow), a
//! synthetic transaction is emitted at that date:
//!
//! ```text
//! <date> * currency translation adjustment
//!     [<transit-account>]   -drift TARGET
//!     [<cta-account>]       +drift TARGET
//! ```
//!
//! Both postings balance against each other in the target currency:
//! the transit account's accumulated drift is driven to zero, and
//! the drift lands on the declared CTA account. The drift does not
//! belong to any single original transaction — it is the product of
//! rate movement across many — so attaching it to one arbitrary tx
//! would misrepresent the event. A separate transaction with its
//! own date and description keeps the audit trail clean.
//!
//! CTA and the realizer (fx gain / fx loss) are complementary, not
//! mutually exclusive: the realizer books the trade-day deviation
//! (implied vs market rate) on multi-commodity transactions, while CTA
//! books the holding-period drift (market-rate movement between inflow
//! and outflow) on any account whose native sum is zero — including
//! multi-commodity clearing accounts. The two measure different
//! quantities and never double-book; the CTA transaction is
//! self-balancing. Groups with any missing price-DB rate are skipped
//! (drift cannot be reliably computed).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::date::Date;
use crate::decimal::Decimal;
use crate::indexer::Index;
use crate::parser::located::Located;
use crate::parser::posting::{Amount, Posting};
use crate::parser::transaction::{State, Transaction};

/// Inject currency-translation-adjustment transactions into `txs` for
/// every transit account whose native sum is zero but whose
/// per-posting-tx.date target sum drifts. Positive drift routes to
/// `cta_loss`, negative drift to `cta_gain`.
///
/// Applies to every pass-through account, single- or multi-commodity:
/// CTA books the holding-period drift (market-rate movement between
/// inflow and outflow), which is independent of the realizer's
/// trade-day fx gain/loss — the two never double-book the same amount.
pub fn translate(
    txs: &mut Vec<Located<Transaction>>,
    target: &str,
    db: &Index,
    cta_gain: &str,
    cta_loss: &str,
    precision: usize,
    exclude: &HashSet<(String, String)>,
) {
    let mut transit = identify_transit_groups(txs);
    // Drop (account, commodity) pairs the lotter already realized a
    // capital gain on: CTA and capital both book the holding-period
    // drift, so leaving them in would double-count.
    transit.retain(|k| !exclude.contains(k));
    if transit.is_empty() {
        return;
    }

    let adjustments = collect_adjustments(txs, &transit, target, db, precision);

    for adj in adjustments {
        // Sign convention: running_target (drift) > 0 means the
        // transit account retained more target-currency value than
        // it should (native went back to 0 but target is positive)
        // — that's a loss from the target-currency perspective
        // (we held the native while it weakened). Negative drift =
        // gain (held native while it strengthened).
        let cta = if adj.drift.is_negative() { cta_gain } else { cta_loss };
        txs.push(build_release_tx(&adj, target, cta, precision));
    }

    txs.sort_by(|a, b| a.value.date.cmp(&b.value.date));
}

fn identify_transit_groups(
    txs: &[Located<Transaction>],
) -> HashSet<(String, String)> {
    // A transit (pass-through) account is one whose native amounts sum
    // to exactly zero per (account, commodity) over the journal — money
    // came in and went back out. Both single- and multi-commodity
    // accounts qualify: CTA books only the holding-period drift, which
    // is a different quantity than the realizer's trade-day fx gain/loss
    // (proven 2026-06-21), so the two never double-book.
    let mut sums: HashMap<(String, String), Decimal> = HashMap::new();
    for lt in txs {
        for lp in &lt.value.postings {
            if let Some(a) = &lp.value.amount {
                let key = (lp.value.account.clone(), a.commodity.clone());
                let v = sums.entry(key).or_insert(Decimal::zero());
                *v = *v + a.value;
            }
        }
    }
    sums.into_iter()
        .filter(|(_k, v)| v.is_zero())
        .map(|(k, _)| k)
        .collect()
}

struct Adjustment {
    date: Date,
    file: Arc<str>,
    line: usize,
    account: String,
    drift: Decimal,
}

fn collect_adjustments(
    txs: &[Located<Transaction>],
    transit: &HashSet<(String, String)>,
    target: &str,
    db: &Index,
    precision: usize,
) -> Vec<Adjustment> {
    // Per-group running (native_sum, target_sum, rate_missing).
    let mut running: HashMap<(String, String), (Decimal, Decimal, bool)> =
        HashMap::new();
    let mut out = Vec::new();

    for lt in txs.iter() {
        let lookup_date: String = lt.value.date.to_string();
        for lp in &lt.value.postings {
            let Some(a) = &lp.value.amount else { continue };
            let key = (lp.value.account.clone(), a.commodity.clone());
            if !transit.contains(&key) {
                continue;
            }
            let target_val = if a.commodity == target {
                Some(a.value)
            } else {
                db.find(&a.commodity, target, &lookup_date)
                    .map(|r| a.value.mul_rounded(r))
            };
            let entry = running
                .entry(key.clone())
                .or_insert((Decimal::zero(), Decimal::zero(), false));
            entry.0 = entry.0 + a.value;
            match target_val {
                Some(v) => entry.1 = entry.1 + v,
                None => entry.2 = true,
            }
            if !entry.2
                && entry.0.is_zero()
                && !entry.1.is_display_zero(precision)
            {
                out.push(Adjustment {
                    date: lt.value.date,
                    file: lt.file.clone(),
                    line: lt.line,
                    account: key.0.clone(),
                    drift: entry.1,
                });
                entry.1 = Decimal::zero();
            }
        }
    }
    out
}

fn build_release_tx(
    adj: &Adjustment,
    target: &str,
    cta_account: &str,
    precision: usize,
) -> Located<Transaction> {
    let description = "currency translation adjustment".to_string();
    // Bracket-virtual (`[account]`): virtual (rendered in brackets)
    // and balance-contributing. The tx balances against itself and
    // the transit account's target sum returns to zero.
    let debit = Posting {
        account: adj.account.clone(),
        amount: Some(Amount {
            commodity: target.to_string(),
            value: -adj.drift,
            decimals: precision,
        }),
        costs: None,
        lot_cost: None,
        balance_assertion: None,
        is_virtual: true,
        balanced: true,
        comments: Vec::new(),
    };
    let credit = Posting {
        account: cta_account.to_string(),
        amount: Some(Amount {
            commodity: target.to_string(),
            value: adj.drift,
            decimals: precision,
        }),
        costs: None,
        lot_cost: None,
        balance_assertion: None,
        is_virtual: true,
        balanced: true,
        comments: Vec::new(),
    };
    Located {
        file: adj.file.clone(),
        line: adj.line,
        value: Transaction {
            date: adj.date,
            state: State::Cleared,
            code: None,
            description,
            postings: vec![
                Located {
                    file: adj.file.clone(),
                    line: adj.line,
                    value: debit,
                },
                Located {
                    file: adj.file.clone(),
                    line: adj.line,
                    value: credit,
                },
            ],
            comments: Vec::new(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;
    use crate::resolver;

    fn setup(src: &str) -> (Vec<Located<Transaction>>, Index) {
        let entries = parser::parse(src).unwrap();
        let resolved = resolver::resolve(entries).unwrap();
        let prices = crate::indexer::index(resolved.prices);
        let txs = crate::booker::book(resolved.transactions).unwrap();
        (txs, prices)
    }

    #[test]
    fn transit_drift_emits_cta_release_tx() {
        let src = "\
            P 2024-01-15 EUR USD 1.10\n\
            P 2024-06-15 EUR USD 1.05\n\
            2024-01-15 * receive\n\
            \tassets:checking   10 EUR\n\
            \tincome:salary    -10 EUR\n\
            2024-06-15 * spend\n\
            \texpenses:food     10 EUR\n\
            \tassets:checking  -10 EUR\n";
        let (mut txs, db) = setup(src);
        translate(&mut txs, "USD", &db, "in:cta", "ex:cta", 2, &HashSet::new());

        // Three txs now: 2 originals + 1 synthetic release.
        assert_eq!(txs.len(), 3);
        let release = txs
            .iter()
            .find(|lt| lt.value.description == "currency translation adjustment")
            .expect("release tx missing");
        assert_eq!(release.value.postings.len(), 2);
        // +$11 - $10.50 = +$0.50 drift on checking.
        let debit = &release.value.postings[0].value;
        assert_eq!(debit.account, "assets:checking");
        assert_eq!(
            debit.amount.as_ref().unwrap().value,
            Decimal::parse("-0.50").unwrap()
        );
        let credit = &release.value.postings[1].value;
        // EUR weakened 1.10→1.05 during the holding period; the
        // transit held less target-value at outflow than inflow, so
        // the positive running drift routes to the loss account.
        assert_eq!(credit.account, "ex:cta");
        assert_eq!(
            credit.amount.as_ref().unwrap().value,
            Decimal::parse("0.50").unwrap()
        );
    }

    #[test]
    fn non_transit_account_untouched() {
        let src = "\
            P 2024-01-15 EUR USD 1.10\n\
            2024-01-15 * receive only\n\
            \tassets:checking   10 EUR\n\
            \tincome:salary    -10 EUR\n";
        let (mut txs, db) = setup(src);
        let original = txs.len();
        translate(&mut txs, "USD", &db, "in:cta", "ex:cta", 2, &HashSet::new());
        assert_eq!(txs.len(), original);
    }

    #[test]
    fn no_drift_no_release() {
        let src = "\
            P 2024-01-15 EUR USD 1.10\n\
            P 2024-06-15 EUR USD 1.10\n\
            2024-01-15 * receive\n\
            \tassets:checking   10 EUR\n\
            \tincome:salary    -10 EUR\n\
            2024-06-15 * spend\n\
            \texpenses:food     10 EUR\n\
            \tassets:checking  -10 EUR\n";
        let (mut txs, db) = setup(src);
        let original = txs.len();
        translate(&mut txs, "USD", &db, "in:cta", "ex:cta", 2, &HashSet::new());
        assert_eq!(txs.len(), original);
    }

    #[test]
    fn missing_rate_skips_group() {
        let src = "\
            2024-01-15 * receive\n\
            \tassets:checking   10 EUR\n\
            \tincome:salary    -10 EUR\n\
            2024-06-15 * spend\n\
            \texpenses:food     10 EUR\n\
            \tassets:checking  -10 EUR\n";
        let (mut txs, db) = setup(src);
        let original = txs.len();
        translate(&mut txs, "USD", &db, "in:cta", "ex:cta", 2, &HashSet::new());
        assert_eq!(txs.len(), original);
    }

    #[test]
    fn multi_commodity_pass_through_account_gets_cta() {
        // A pass-through account in a foreign commodity that nets to
        // zero natively must still get its holding-period drift booked,
        // even though every leg is a multi-commodity trade (fx) — the
        // taint exclusion was proven unnecessary (2026-06-21). USD flows
        // in @1.10 and back out @1.05; native sum 0, target drift 0.50.
        let src = "\
            P 2024-01-15 USD EUR 1.10\n\
            P 2024-06-15 USD EUR 1.05\n\
            2024-01-15 * in\n\
            \tcp:partner    -100 USD\n\
            \tincome:sales   110 EUR\n\
            2024-06-15 * out\n\
            \tcp:partner     100 USD\n\
            \tassets:bank   -105 EUR\n";
        let (mut txs, db) = setup(src);
        translate(&mut txs, "EUR", &db, "in:cta", "ex:cta", 2, &HashSet::new());
        let release = txs
            .iter()
            .find(|lt| lt.value.description == "currency translation adjustment")
            .expect("CTA must fire on a multi-commodity pass-through account");
        // cp:partner: -100×1.10 + 100×1.05 = -110 + 105 = -5 drift.
        let debit = &release.value.postings[0].value;
        assert_eq!(debit.account, "cp:partner");
        assert_eq!(
            debit.amount.as_ref().unwrap().value,
            Decimal::parse("5").unwrap()
        );
    }
}
