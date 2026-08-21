//! Booker pass 1 — transaction-local balancing.
//!
//! Fills every ordinary transaction's one omitted amount so its
//! commodities sum to zero, giving the downstream inject hook concrete
//! numbers to scale against. Transactions carrying a balance assignment (`= TARGET`
//! with no amount) are deferred to
//! [`assert_balances`](super::asserter::assert_balances): their value
//! depends on the cross-transaction running balance that only pass 2 has,
//! so filling them here would mis-infer or error.

use crate::parser::located::Located;
use crate::parser::transaction::Transaction;

use super::balance;
use super::error::BookError;

/// Pass 1 — balance each ordinary transaction in place (see the module
/// docs), leaving assignment transactions for pass 2.
pub fn fill(
    transactions: Vec<Located<Transaction>>,
) -> Result<Vec<Located<Transaction>>, BookError> {
    let mut result = Vec::with_capacity(transactions.len());
    for Located { file, line, mut value } in transactions {
        if !has_assignment(&value) {
            let end_line = value.postings.iter().map(|p| p.line).max().unwrap_or(line);
            balance::balance_tx(&mut value, &file, line, end_line)?;
        }
        result.push(Located { file, line, value });
    }
    Ok(result)
}

/// True if the transaction carries an unresolved balance assignment — a
/// `= TARGET` posting with no amount, whose value only the cross-tx
/// running balance can supply.
fn has_assignment(tx: &Transaction) -> bool {
    tx.postings
        .iter()
        .any(|lp| lp.value.amount.is_none() && lp.value.balance_assertion.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decimal::Decimal;
    use crate::{parser, resolver};

    #[test]
    fn fills_ordinary_but_defers_assignment_transactions() {
        let src = "2024-01-01 * normal\n\tassets:bank   50 USD\n\tequity\n\
                   2024-01-02 * assign\n\tassets:bank   = 80 USD\n\tequity:adjust\n";
        let entries = parser::parse(src).unwrap();
        let resolved = resolver::resolve(entries).unwrap();
        let out = fill(resolved.transactions).unwrap();
        // Ordinary tx: the bare `equity` leg is filled to -50.
        assert_eq!(
            out[0].value.postings[1].value.amount.as_ref().unwrap().value,
            Decimal::from(-50)
        );
        // Assignment tx: left untouched — both the `= 80` posting and its bare
        // counter are still amountless, deferred to pass 2.
        assert!(out[1].value.postings[0].value.amount.is_none());
        assert!(out[1].value.postings[1].value.amount.is_none());
    }
}
