//! Revaluator phase — mark-to-market revaluation of open positions.
//!
//! Runs only under `-X TARGET` with `--unrealized`, and only when both a
//! `fx-unrealized gain` and a `fx-unrealized loss` account are declared. For
//! every account holding an **open** (non-zero native) position in a
//! non-target commodity, it injects one synthetic transaction that moves
//! the account's *converted* value from its historical sum to
//! `native_balance × latest available rate`, booking the difference — the
//! **unrealized** FX — to the revaluation accounts.
//!
//! Unlike the translator (CTA), which releases the **realized** drift when
//! a position returns to zero, the revaluator marks **open** positions to
//! the latest rate on demand. The default (no `--unrealized`) stays purely
//! historical, so the realized / tax-relevant view is unchanged.
//!
//! "Latest available" is a single rate per commodity — the most recent on
//! record, regardless of date (the `--unrealized` flag carries no date by
//! design). A group whose conversion rate is missing for any posting is
//! skipped: its converted balance can't be measured, so it can't be
//! marked to market.

use std::collections::HashMap;

use crate::date::Date;
use crate::decimal::Decimal;
use crate::indexer::Index;
use crate::parser::located::Located;
use crate::parser::posting::{Amount, Posting};
use crate::parser::transaction::{State, Transaction};

/// The accounts an unrealized revaluation is booked to: a gain (income)
/// when the position is worth more at the latest rate than its historical
/// cost, a loss (expense) when worth less.
pub struct RevaluationAccounts<'a> {
    pub gain: &'a str,
    pub loss: &'a str,
}

/// Far-future sentinel: `find(commodity, target, LATEST)` returns the most
/// recent rate on record for the pair (the "latest available" rate).
const LATEST: &str = "9999-12-31";

/// Inject one mark-to-market revaluation transaction per open foreign
/// position. See the module docs for the semantics.
pub fn revaluate(
    txs: &mut Vec<Located<Transaction>>,
    target: &str,
    db: &Index,
    accounts: &RevaluationAccounts,
    precision: usize,
) {
    if txs.is_empty() {
        return;
    }

    // Per (account, commodity): net native balance, historical target
    // value (Σ of each posting's weight converted at its own date — what
    // the rebalancer will leave on the account), and whether any posting
    // lacked a rate (then the group can't be measured and is skipped).
    let mut groups: HashMap<(String, String), (Decimal, Decimal, bool)> = HashMap::new();
    // The revaluation is "as of now" — date it today so it lands in the
    // default report. Dating it at the journal's last transaction would
    // hide it whenever the journal carries forward-dated entries (their
    // max date is in the future, which the default future cutoff drops).
    let reval_date = Date::today();

    for lt in txs.iter() {
        let date = lt.value.date.to_string();
        for lp in &lt.value.postings {
            let Some(a) = &lp.value.amount else { continue };
            let key = (lp.value.account.clone(), a.commodity.clone());
            let e = groups.entry(key).or_insert((Decimal::zero(), Decimal::zero(), false));
            e.0 = e.0 + a.value;
            match crate::rebalancer::target_value(&lp.value, target, db, &date) {
                Some(v) => e.1 = e.1 + v,
                None => e.2 = true,
            }
        }
    }

    let file = txs[0].file.clone();
    let line = txs[0].line;

    // Deterministic order for the emitted transactions.
    let mut keys: Vec<&(String, String)> = groups.keys().collect();
    keys.sort();

    let mut out: Vec<Located<Transaction>> = Vec::new();
    for key in keys {
        let (account, commodity) = key;
        // The target itself needs no revaluation; a missing rate can't be
        // measured; a closed position (native zero) is the translator's
        // job, not the revaluator's.
        if commodity == target {
            continue;
        }
        let (native_bal, historical, rate_missing) = &groups[key];
        if *rate_missing || native_bal.is_zero() {
            continue;
        }
        let Some(rate) = db.find(commodity, target, LATEST) else {
            continue;
        };
        let current = native_bal.mul_rounded(rate);
        let diff = current - *historical;
        if diff.is_display_zero(precision) {
            continue;
        }
        let reval = if diff.is_negative() { accounts.loss } else { accounts.gain };
        out.push(build_tx(&file, line, reval_date, target, account, commodity, reval, diff, precision));
    }

    txs.extend(out);
    txs.sort_by(|a, b| a.value.date.cmp(&b.value.date));
}

/// Build the synthetic revaluation transaction: the account gets a
/// `+diff` target-currency posting (so its converted balance ends at
/// `native × latest rate`), the revaluation account gets the offsetting
/// `-diff`. Both are real and sum to zero, so the transaction balances
/// and reloads 1:1.
#[allow(clippy::too_many_arguments)]
fn build_tx(
    file: &std::sync::Arc<str>,
    line: usize,
    date: Date,
    target: &str,
    account: &str,
    commodity: &str,
    reval_account: &str,
    diff: Decimal,
    precision: usize,
) -> Located<Transaction> {
    let asset = Posting {
        account: account.to_string(),
        amount: Some(Amount {
            commodity: target.to_string(),
            value: diff,
            decimals: precision,
        }),
        costs: None,
        lot_cost: None,
        lot_date: None,
        balance_assertion: None,
        is_virtual: false,
        balanced: true,
        comments: Vec::new(),
    };
    let counter = Posting {
        account: reval_account.to_string(),
        amount: Some(Amount {
            commodity: target.to_string(),
            value: Decimal::zero() - diff,
            decimals: precision,
        }),
        costs: None,
        lot_cost: None,
        lot_date: None,
        balance_assertion: None,
        is_virtual: false,
        balanced: true,
        comments: Vec::new(),
    };
    Located {
        file: file.clone(),
        line,
        value: Transaction {
            date,
            state: State::Cleared,
            code: None,
            description: format!("unrealized fx revaluation {commodity}"),
            postings: vec![
                Located { file: file.clone(), line, value: asset },
                Located { file: file.clone(), line, value: counter },
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

    fn setup(src: &str) -> (Vec<Located<Transaction>>, Index, usize) {
        let entries = parser::parse(src).unwrap();
        let resolved = resolver::resolve(entries).unwrap();
        let prices = crate::indexer::index(resolved.prices);
        let txs = crate::booker::book(resolved.transactions).unwrap();
        (txs, prices, 2)
    }

    fn accounts() -> RevaluationAccounts<'static> {
        RevaluationAccounts { gain: "in:reval", loss: "ex:reval" }
    }

    /// Sum of `account`'s `commodity` postings across the journal.
    fn balance(txs: &[Located<Transaction>], account: &str, commodity: &str) -> Decimal {
        let mut sum = Decimal::zero();
        for lt in txs {
            for lp in &lt.value.postings {
                if lp.value.account == account {
                    if let Some(a) = &lp.value.amount {
                        if a.commodity == commodity {
                            sum = sum + a.value;
                        }
                    }
                }
            }
        }
        sum
    }

    #[test]
    fn open_position_marked_to_latest_rate() {
        // Bought 1000 USD for 800 EUR (rate 0.80), spent 900 USD for 855
        // EUR (rate 0.95). Net open: +100 USD. Historical EUR = 800 − 855
        // = −55. Latest rate 0.95 → current = 100 × 0.95 = +95.
        // Revaluation diff = 95 − (−55) = +150 → the account is marked
        // from −55 to +95, +150 booked to the gain account.
        let src = "\
            P 2024-01-01 USD EUR 0.80\n\
            P 2024-06-01 USD EUR 0.95\n\
            2024-01-01 * buy\n\
            \tassets:usd    1000 USD\n\
            \tassets:bank   -800 EUR\n\
            2024-06-01 * spend\n\
            \texpenses:x     855 EUR\n\
            \tassets:usd    -900 USD\n";
        let (mut txs, db, prec) = setup(src);
        revaluate(&mut txs, "EUR", &db, &accounts(), prec);
        // The revaluation posting itself (target currency) on assets:usd.
        assert_eq!(balance(&txs, "assets:usd", "EUR"), Decimal::parse("150").unwrap());
        // Gain booked (income, negative).
        assert_eq!(balance(&txs, "in:reval", "EUR"), Decimal::parse("-150").unwrap());
        // Native USD untouched.
        assert_eq!(balance(&txs, "assets:usd", "USD"), Decimal::parse("100").unwrap());
    }

    #[test]
    fn closed_position_is_left_alone() {
        // 1000 USD in and 1000 USD out — net zero → no revaluation (the
        // translator handles realized transit drift, not the revaluator).
        let src = "\
            P 2024-01-01 USD EUR 0.80\n\
            P 2024-06-01 USD EUR 0.95\n\
            2024-01-01 * in\n\
            \tassets:usd    1000 USD\n\
            \tassets:bank   -800 EUR\n\
            2024-06-01 * out\n\
            \texpenses:x     950 EUR\n\
            \tassets:usd   -1000 USD\n";
        let (mut txs, db, prec) = setup(src);
        let before = txs.len();
        revaluate(&mut txs, "EUR", &db, &accounts(), prec);
        assert_eq!(txs.len(), before, "closed position must not be revalued");
    }

    #[test]
    fn target_commodity_not_revalued() {
        // A plain EUR account under -X EUR is already in the target — no
        // revaluation, no transaction added.
        let src = "\
            2024-01-01 * x\n\
            \tassets:bank   100 EUR\n\
            \tequity:open  -100 EUR\n";
        let (mut txs, db, prec) = setup(src);
        let before = txs.len();
        revaluate(&mut txs, "EUR", &db, &accounts(), prec);
        assert_eq!(txs.len(), before);
    }
}
