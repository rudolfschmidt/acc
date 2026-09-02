//! Booker pass 2 — cross-transaction balance assignments and assertions.
//!
//! Walks the date-sorted transactions maintaining a running balance per
//! `(account, commodity)`. For each transaction: resolve any `= TARGET`
//! assignment (infer the amount that reaches the target), fill any
//! remaining counter, apply every posting to the running balance, and
//! verify any `= TARGET` assertion. Runs after [`fill`](super::filler::fill)
//! and, on the report path, after the inject hook — so an assignment
//! like `budget = 0` accounts for the injected postings already in the
//! running balance.

use std::collections::HashMap;
use std::sync::Arc;

use crate::decimal::Decimal;
use crate::parser::located::Located;
use crate::parser::posting::{Amount, Posting};
use crate::parser::transaction::Transaction;

use super::balance;
use super::error::{BookError, BookErrorKind};

/// Pass 2 — resolve assignments, balance, apply, and verify assertions
/// against the running balance (see the module docs). Transactions already
/// balanced by [`fill`](super::filler::fill) pass through the balancing
/// step as a no-op; assignment transactions are balanced here for the
/// first time.
pub fn assert_balances(
    transactions: Vec<Located<Transaction>>,
) -> Result<Vec<Located<Transaction>>, BookError> {
    let mut balances: HashMap<(String, String), Decimal> = HashMap::new();
    // Max fractional digits seen per commodity, so a `= *0` correction posting
    // carries the same display precision as the balance it zeroes.
    let mut decimals: HashMap<String, usize> = HashMap::new();
    let mut result = Vec::with_capacity(transactions.len());

    for Located { file, line, mut value } in transactions {
        let end_line = value
            .postings
            .iter()
            .map(|p| p.line)
            .max()
            .unwrap_or(line);
        // 0. Expand any `= *0` all-commodity zero into one correction posting per
        // non-zero commodity the account currently holds.
        expand_all_commodity_zero(&mut value, &balances, &decimals, &file, line, end_line)?;
        // 1. Resolve balance-assignment postings (amount from running
        // balance vs target).
        for lp in &mut value.postings {
            resolve_assignment(&mut lp.value, &balances);
        }
        // 2. Transaction-local balance (sum = 0, cost-aware). A no-op for
        // transactions `fill` already balanced.
        balance::balance_tx(&mut value, &file, line, end_line)?;
        // 3. Apply each posting to the running balance and check any
        // assertion targets.
        for lp in &value.postings {
            apply_and_check(&lp.value, &file, line, end_line, &mut balances)?;
            if let Some(amt) = &lp.value.amount {
                let seen = decimals.entry(amt.commodity.clone()).or_insert(0);
                *seen = (*seen).max(amt.decimals);
            }
        }
        result.push(Located { file, line, value });
    }

    Ok(result)
}

/// Expand every `= *0` posting (all-commodity zero, commodity `*`) into one
/// correction posting per commodity the account currently holds a *non-zero*
/// balance in, each driven to zero. Commodities already at zero produce no
/// posting — a mixed account flattens with no `0`-amount clutter. A non-zero
/// `= *N` target is rejected: you cannot set every commodity to one value.
fn expand_all_commodity_zero(
    tx: &mut Transaction,
    balances: &HashMap<(String, String), Decimal>,
    decimals: &HashMap<String, usize>,
    file: &Arc<str>,
    start_line: usize,
    end_line: usize,
) -> Result<(), BookError> {
    if !tx.postings.iter().any(|lp| is_all_commodity_zero(&lp.value)) {
        return Ok(());
    }
    let mut out: Vec<Located<Posting>> = Vec::with_capacity(tx.postings.len());
    for lp in tx.postings.drain(..) {
        if !is_all_commodity_zero(&lp.value) {
            out.push(lp);
            continue;
        }
        if let Some(target) = &lp.value.balance_assertion
            && !target.value.is_zero()
        {
            return Err(BookError::new(
                file.clone(),
                start_line,
                end_line,
                BookErrorKind::AllCommodityNonZero { account: lp.value.account.clone() },
            ));
        }
        // One correction posting per non-zero commodity of this account, in a
        // deterministic order so output is stable.
        let mut held: Vec<(String, Decimal)> = balances
            .iter()
            .filter(|((acc, _), bal)| acc == &lp.value.account && !bal.is_zero())
            .map(|((_, com), bal)| (com.clone(), *bal))
            .collect();
        held.sort_by(|a, b| a.0.cmp(&b.0));
        if held.is_empty() {
            // Account already flat in every commodity — nothing to correct.
            // Degrade to a plain empty-commodity `= 0` so the transaction keeps
            // a balancing basis (its bare counter infers 0) instead of vanishing
            // and leaving that counter with no amounts to infer from.
            let mut p = lp;
            p.value.balance_assertion =
                Some(Amount { commodity: String::new(), value: Decimal::zero(), decimals: 0 });
            out.push(p);
            continue;
        }
        for (commodity, bal) in held {
            let dec = decimals.get(&commodity).copied().unwrap_or(0);
            let mut p = lp.clone();
            p.value.amount = Some(Amount { commodity: commodity.clone(), value: -bal, decimals: dec });
            p.value.balance_assertion =
                Some(Amount { commodity, value: Decimal::zero(), decimals: dec });
            out.push(p);
        }
    }
    tx.postings = out;
    Ok(())
}

/// True if a posting is a bare `= *…` all-commodity zero assignment (commodity
/// `*`, no explicit amount).
fn is_all_commodity_zero(posting: &Posting) -> bool {
    posting.amount.is_none()
        && posting.balance_assertion.as_ref().is_some_and(|a| a.commodity == "*")
}

/// If the posting is a balance-assignment (`= TARGET` with no amount),
/// fill its amount so that the account balance reaches `TARGET` after
/// this posting.
fn resolve_assignment(posting: &mut Posting, balances: &HashMap<(String, String), Decimal>) {
    let (amount, assertion) = (&posting.amount, &posting.balance_assertion);
    if amount.is_some() {
        return;
    }
    let Some(target) = assertion else {
        return;
    };
    let running = balances
        .get(&(posting.account.clone(), target.commodity.clone()))
        .copied()
        .unwrap_or_else(Decimal::zero);
    let diff = target.value - running;
    posting.amount = Some(Amount {
        commodity: target.commodity.clone(),
        value: diff,
        decimals: target.decimals,
    });
}

/// Apply a posting's amount to the running balance and check any attached
/// assertion.
fn apply_and_check(
    posting: &Posting,
    file: &Arc<str>,
    start_line: usize,
    end_line: usize,
    balances: &mut HashMap<(String, String), Decimal>,
) -> Result<(), BookError> {
    let Some(amt) = &posting.amount else {
        return Ok(());
    };
    let key = (posting.account.clone(), amt.commodity.clone());
    *balances.entry(key).or_insert_with(Decimal::zero) += amt.value;

    if let Some(target) = &posting.balance_assertion {
        let running = balances
            .get(&(posting.account.clone(), target.commodity.clone()))
            .copied()
            .unwrap_or_else(Decimal::zero);
        if running != target.value {
            return Err(BookError::new(
                file.clone(),
                start_line,
                end_line,
                BookErrorKind::AssertionFailed {
                    account: posting.account.clone(),
                    expected: target.value,
                    got: running,
                    commodity: target.commodity.clone(),
                    decimals: target.decimals,
                },
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{parser, resolver};

    /// Pass 2 alone fully books a source (its balancing step handles the
    /// ordinary fills too), so these assignment/assertion cases run it
    /// directly on the resolved transactions.
    fn asserted(src: &str) -> Result<Vec<Located<Transaction>>, BookError> {
        let entries = parser::parse(src).unwrap();
        let resolved = resolver::resolve(entries).unwrap();
        assert_balances(resolved.transactions)
    }

    #[test]
    fn assignment_infers_amount_to_reach_target() {
        let src = "2024-01-01 * Opening\n    assets:bank    = 100 USD\n    equity:opening\n";
        let out = asserted(src).unwrap();
        let bank = &out[0].value.postings[0].value;
        assert_eq!(bank.amount.as_ref().unwrap().value, Decimal::from(100));
        let equity = &out[0].value.postings[1].value;
        assert_eq!(equity.amount.as_ref().unwrap().value, Decimal::from(-100));
    }

    #[test]
    fn assignment_respects_prior_balance() {
        let src = "2024-01-01 * Initial\n    assets:bank    40 USD\n    equity:opening  -40 USD\n\
                   2024-01-05 * Adjust to target\n    assets:bank    = 100 USD\n    equity:adjust\n";
        let out = asserted(src).unwrap();
        let adjust = &out[1].value.postings[0].value;
        assert_eq!(adjust.amount.as_ref().unwrap().value, Decimal::from(60));
    }

    #[test]
    fn assignment_respects_inferred_prior_balance() {
        // Tx A leaves bank's amount missing; it should be inferred as
        // +100 by the tx-local balance. Tx B's assignment must see
        // the running balance that includes this inferred amount.
        let src = "2024-01-01 * A\n    equity:opening  -100 USD\n    assets:bank\n\
                   2024-01-02 * B\n    assets:bank    = 100 USD\n    equity:adjust\n";
        let out = asserted(src).unwrap();
        let tx_b_bank = &out[1].value.postings[0].value;
        assert_eq!(tx_b_bank.amount.as_ref().unwrap().value, Decimal::zero());
    }

    #[test]
    fn assertion_passes_when_balance_matches() {
        let src = "2024-01-01 * Deposit\n    assets:bank   100 USD = 100 USD\n    equity:opening  -100 USD\n";
        assert!(asserted(src).is_ok());
    }

    #[test]
    fn assertion_fails_on_mismatch() {
        let src = "2024-01-01 * Deposit\n    assets:bank   100 USD = 999 USD\n    equity:opening  -100 USD\n";
        let err = asserted(src).unwrap_err();
        match *err.kind {
            BookErrorKind::AssertionFailed { ref account, .. } => {
                assert_eq!(account, "assets:bank");
            }
            other => panic!("expected AssertionFailed, got {:?}", other),
        }
    }

    #[test]
    fn assertion_after_multiple_transactions() {
        let src = "2024-01-01 * A\n    assets:bank   50 USD\n    equity:opening  -50 USD\n\
                   2024-01-02 * B\n    assets:bank   30 USD = 80 USD\n    equity:other    -30 USD\n";
        assert!(asserted(src).is_ok());
    }

    #[test]
    fn independent_accounts_track_separately() {
        let src = "2024-01-01 * A\n    assets:bank   100 USD\n    equity:opening  -100 USD\n\
                   2024-01-02 * B\n    assets:cash    50 USD = 50 USD\n    equity:opening  -50 USD\n";
        assert!(asserted(src).is_ok());
    }

    #[test]
    fn commodity_tracked_separately_per_account() {
        let src = "2024-01-01 * A\n    assets:bank   100 USD\n    equity:a  -100 USD\n\
                   2024-01-02 * B\n    assets:bank   50 EUR = 50 EUR\n    equity:b  -50 EUR\n";
        assert!(asserted(src).is_ok());
    }

    #[test]
    fn star_zero_flattens_every_commodity_skipping_zeros() {
        // `= *0` zeroes each non-zero commodity of the account; a commodity
        // already at zero produces no correction posting.
        let src = "2024-01-01 * build\n    assets:w   30 USD\n    assets:w   2 EUR\n\
                   \tassets:w   0 GBP\n    equity:o\n\
                   2024-02-01 * flatten\n    assets:w   = *0\n    equity:reset\n";
        let out = asserted(src).unwrap();
        let flatten = &out[1].value;
        // Two corrections (USD, EUR) + the bare counter expanded per commodity.
        let corrections: Vec<_> = flatten
            .postings
            .iter()
            .filter(|lp| lp.value.account == "assets:w")
            .collect();
        assert_eq!(corrections.len(), 2, "USD + EUR zeroed, GBP (already 0) skipped");
        // The account is flat afterwards in every commodity.
        for c in ["USD", "EUR"] {
            let mut out2 = out.clone();
            // running balance check: apply everything and confirm zero.
            let bal: Decimal = out2
                .iter_mut()
                .flat_map(|t| t.value.postings.iter())
                .filter(|lp| lp.value.account == "assets:w")
                .filter_map(|lp| lp.value.amount.as_ref())
                .filter(|a| a.commodity == c)
                .map(|a| a.value)
                .fold(Decimal::zero(), |acc, v| acc + v);
            assert!(bal.is_zero(), "{c} balance should net to zero, got {bal}");
        }
    }

    #[test]
    fn star_zero_on_flat_account_is_a_balanced_noop() {
        // `= *0` on an account with no balance must not vanish and strand its
        // bare counter — it degrades to a plain `= 0` (empty commodity) no-op.
        let src = "2021-12-31 * correction\n    assets:w   = *0\n    equity:reset\n";
        let out = asserted(src).unwrap();
        let tx = &out[0].value;
        assert_eq!(tx.postings.len(), 2);
        for lp in &tx.postings {
            assert!(lp.value.amount.as_ref().unwrap().value.is_zero());
        }
    }

    #[test]
    fn star_nonzero_target_is_rejected() {
        let src = "2024-01-01 * seed\n    foo   10 USD\n    bar\n\
                   2024-02-01 * bad\n    foo   = *5\n    bar\n";
        let err = asserted(src).unwrap_err();
        assert!(matches!(*err.kind, BookErrorKind::AllCommodityNonZero { .. }));
    }
}
