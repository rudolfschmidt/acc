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
//! <date> * translation adjustment
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
//! Multi-commodity transactions are handled by the realizer
//! (fx gain / fx loss); any (account, commodity) group that appears
//! on such a transaction is excluded from CTA to avoid double-
//! booking. Groups with any missing price-DB rate are also skipped
//! (drift cannot be reliably computed).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::date::Date;
use crate::decimal::Decimal;
use crate::indexer::Index;
use crate::parser::located::Located;
use crate::parser::posting::{Amount, Posting};
use crate::parser::transaction::{State, Transaction};

/// Inject translation-adjustment transactions into `txs` for every
/// transit account whose native sum is zero but whose per-posting-
/// tx.date target sum drifts. Positive drift routes to `cta_gain`,
/// negative drift to `cta_loss`. `fixed_date` mirrors the
/// rebalancer: None = per-posting `tx.date`, Some(d) = the market-
/// snapshot date.
pub fn translate(
    txs: &mut Vec<Located<Transaction>>,
    target: &str,
    db: &Index,
    fixed_date: Option<&str>,
    cta_gain: &str,
    cta_loss: &str,
    precision: usize,
) {
    let transit = identify_transit_groups(txs);
    if transit.is_empty() {
        return;
    }

    let adjustments = collect_adjustments(
        txs,
        &transit,
        target,
        db,
        fixed_date,
        precision,
    );

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
    let mut sums: HashMap<(String, String), Decimal> = HashMap::new();
    // (account, commodity) pairs touched by any multi-commodity tx
    // belong to the realizer's domain (fx gain/loss). Exclude them
    // from CTA to avoid double-booking the same drift.
    let mut tainted: HashSet<(String, String)> = HashSet::new();
    for lt in txs {
        let multi = !is_single_commodity(&lt.value);
        for lp in &lt.value.postings {
            if let Some(a) = &lp.value.amount {
                let key = (lp.value.account.clone(), a.commodity.clone());
                if multi {
                    tainted.insert(key.clone());
                }
                let v = sums.entry(key).or_insert(Decimal::zero());
                *v = *v + a.value;
            }
        }
    }
    sums.into_iter()
        .filter(|(k, v)| v.is_zero() && !tainted.contains(k))
        .map(|(k, _)| k)
        .collect()
}

/// Single-commodity iff every balance-contributing posting uses the
/// same commodity. Paren-virtual postings (realizer-injected gain/
/// loss labels) are informational and ignored in this check.
fn is_single_commodity(tx: &Transaction) -> bool {
    let mut seen: Option<String> = None;
    for lp in &tx.postings {
        if lp.value.is_virtual && !lp.value.balanced {
            continue;
        }
        let Some(a) = &lp.value.amount else { continue };
        match &seen {
            None => seen = Some(a.commodity.clone()),
            Some(c) if *c == a.commodity => {}
            _ => return false,
        }
    }
    true
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
    fixed_date: Option<&str>,
    precision: usize,
) -> Vec<Adjustment> {
    // Per-group running (native_sum, target_sum, rate_missing).
    let mut running: HashMap<(String, String), (Decimal, Decimal, bool)> =
        HashMap::new();
    let mut out = Vec::new();

    for lt in txs.iter() {
        let lookup_date: String = fixed_date
            .map(str::to_string)
            .unwrap_or_else(|| lt.value.date.to_string());
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
    let description = "translation adjustment".to_string();
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
        translate(&mut txs, "USD", &db, None, "in:cta", "ex:cta", 2);

        // Three txs now: 2 originals + 1 synthetic release.
        assert_eq!(txs.len(), 3);
        let release = txs
            .iter()
            .find(|lt| lt.value.description == "translation adjustment")
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
        translate(&mut txs, "USD", &db, None, "in:cta", "ex:cta", 2);
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
        translate(&mut txs, "USD", &db, None, "in:cta", "ex:cta", 2);
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
        translate(&mut txs, "USD", &db, None, "in:cta", "ex:cta", 2);
        assert_eq!(txs.len(), original);
    }

    #[test]
    fn multi_commodity_tx_skipped_to_avoid_double_booking_with_realizer() {
        let src = "\
            P 2024-06-15 EUR USD 1.05\n\
            2024-06-15 * fx trade\n\
            \tassets:usd   -100 USD\n\
            \tassets:eur     95 EUR\n";
        let (mut txs, db) = setup(src);
        let original = txs.len();
        translate(&mut txs, "USD", &db, None, "in:cta", "ex:cta", 2);
        assert_eq!(
            txs.len(),
            original,
            "CTA must not fire on multi-commodity tx"
        );
    }

    #[test]
    fn market_snapshot_single_rate_produces_no_drift() {
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
        let original = txs.len();
        translate(&mut txs, "USD", &db, Some("2024-06-15"), "in:cta", "ex:cta", 2);
        assert_eq!(txs.len(), original);
    }
}
