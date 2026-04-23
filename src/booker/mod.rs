//! Booker phase — the only phase with cross-transaction state.
//!
//! Walks the date-sorted transactions and maintains a running balance
//! per `(account, commodity)` pair. For each transaction, three steps
//! run in order:
//!
//! 1. **Assignment resolution** — any posting shaped `ACCOUNT = TARGET`
//!    with no amount has its amount inferred so the account balance
//!    reaches `TARGET`. Uses the running balance that has accumulated
//!    from prior transactions.
//!
//! 2. **Transaction-local balance** — each commodity's sum must be
//!    zero across the postings (cost-aware for `@` / `@@`). The one
//!    allowed omitted amount is inferred.
//!
//! 3. **Apply + assertion check** — each posting's amount is applied
//!    to the running balance; any `= TARGET` assertion (with an
//!    explicit amount) is verified.
//!
//! Step 2 is implemented in [`balance`]; the cross-tx steps 1 and 3
//! live in this file because they share the running-balance state.

pub mod balance;
pub mod error;

pub use error::{BookError, BookErrorKind};

use std::collections::HashMap;
use std::sync::Arc;

use crate::decimal::Decimal;
use crate::parser::located::Located;
use crate::parser::posting::{Amount, Posting};
use crate::parser::transaction::Transaction;

/// Book every transaction. Input must be date-sorted (resolver
/// guarantees this). Returns transactions with every posting's amount
/// filled in and every balance assertion verified.
pub fn book(
    transactions: Vec<Located<Transaction>>,
) -> Result<Vec<Located<Transaction>>, BookError> {
    let mut balances: HashMap<(String, String), Decimal> = HashMap::new();
    let mut result = Vec::with_capacity(transactions.len());

    for Located { file, line, mut value } in transactions {
        let end_line = value
            .postings
            .iter()
            .map(|p| p.line)
            .max()
            .unwrap_or(line);
        // 1. Resolve balance-assignment postings (amount from running
        // balance vs target).
        for lp in &mut value.postings {
            resolve_assignment(&mut lp.value, &balances);
        }
        // 2. Transaction-local balance (sum = 0, cost-aware).
        balance::balance_tx(&mut value, &file, line, end_line)?;
        // 3. Apply each posting to the running balance and check any
        // assertion targets.
        for lp in &value.postings {
            apply_and_check(&lp.value, &file, line, end_line, &mut balances)?;
        }
        result.push(Located { file, line, value });
    }

    Ok(result)
}

/// If the posting is a balance-assignment (`= TARGET` with no amount),
/// fill its amount so that the account balance reaches `TARGET` after
/// this posting.
fn resolve_assignment(
    posting: &mut Posting,
    balances: &HashMap<(String, String), Decimal>,
) {
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

/// Apply a posting's amount to the running balance and check any
/// attached assertion.
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
    use crate::parser;
    use crate::resolver;

    fn pipeline(src: &str) -> Result<Vec<Located<Transaction>>, BookError> {
        let entries = parser::parse(src).unwrap();
        let resolved = resolver::resolve(entries).unwrap();
        book(resolved.transactions)
    }

    #[test]
    fn assignment_infers_amount_to_reach_target() {
        let src = "2024-01-01 * Opening\n    assets:bank    = 100 USD\n    equity:opening\n";
        let out = pipeline(src).unwrap();
        let bank = &out[0].value.postings[0].value;
        assert_eq!(bank.amount.as_ref().unwrap().value, Decimal::from(100));
        let equity = &out[0].value.postings[1].value;
        assert_eq!(equity.amount.as_ref().unwrap().value, Decimal::from(-100));
    }

    #[test]
    fn assignment_respects_prior_balance() {
        let src = "2024-01-01 * Initial\n    assets:bank    40 USD\n    equity:opening  -40 USD\n\
                   2024-01-05 * Adjust to target\n    assets:bank    = 100 USD\n    equity:adjust\n";
        let out = pipeline(src).unwrap();
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
        let out = pipeline(src).unwrap();
        let tx_b_bank = &out[1].value.postings[0].value;
        assert_eq!(tx_b_bank.amount.as_ref().unwrap().value, Decimal::zero());
    }

    #[test]
    fn assertion_passes_when_balance_matches() {
        let src = "2024-01-01 * Deposit\n    assets:bank   100 USD = 100 USD\n    equity:opening  -100 USD\n";
        assert!(pipeline(src).is_ok());
    }

    #[test]
    fn assertion_fails_on_mismatch() {
        let src = "2024-01-01 * Deposit\n    assets:bank   100 USD = 999 USD\n    equity:opening  -100 USD\n";
        let err = pipeline(src).unwrap_err();
        match err.kind {
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
        assert!(pipeline(src).is_ok());
    }

    #[test]
    fn independent_accounts_track_separately() {
        let src = "2024-01-01 * A\n    assets:bank   100 USD\n    equity:opening  -100 USD\n\
                   2024-01-02 * B\n    assets:cash    50 USD = 50 USD\n    equity:opening  -50 USD\n";
        assert!(pipeline(src).is_ok());
    }

    #[test]
    fn commodity_tracked_separately_per_account() {
        let src = "2024-01-01 * A\n    assets:bank   100 USD\n    equity:a  -100 USD\n\
                   2024-01-02 * B\n    assets:bank   50 EUR = 50 EUR\n    equity:b  -50 EUR\n";
        assert!(pipeline(src).is_ok());
    }
}
