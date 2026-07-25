//! Booker phase — the only phase with cross-transaction state.
//!
//! Booking runs in two passes so an auto-rule expander can sit between
//! them (report path only):
//!
//! 1. [`fill`](filler::fill) — **transaction-local balance.** Each
//!    ordinary transaction's one omitted amount is inferred so its
//!    commodities sum to zero (cost-aware for `@` / `@@`). This gives
//!    every posting a concrete amount — the basis auto-rules scale
//!    against. Transactions carrying a balance *assignment* (`= TARGET`
//!    with no amount) are skipped here: their value depends on the
//!    running balance that only pass 2 has.
//!
//! 2. [`assert_balances`](asserter::assert_balances) — **cross-transaction
//!    assignments and assertions.** Walks the date-sorted transactions
//!    maintaining a running balance per `(account, commodity)`, resolving
//!    `= TARGET` assignments, applying every posting, and verifying any
//!    `= TARGET` assertion.
//!
//! [`book`] runs both back to back — no injection between — for the paths
//! that carry no auto-rules (lint, sweep, format, tests). The report load
//! path instead calls [`fill`](filler::fill), expands auto-rules, then
//! [`assert_balances`](asserter::assert_balances), so a `= TARGET`
//! assignment accounts for the injected postings already in the running
//! balance.
//!
//! Pass 1 lives in [`filler`], pass 2 in [`asserter`]; both share the
//! transaction-local math in [`balance`].

pub mod asserter;
pub mod balance;
pub mod error;
pub mod filler;

pub use asserter::assert_balances;
pub use error::{BookError, BookErrorKind};
pub use filler::fill;

use crate::parser::located::Located;
use crate::parser::transaction::Transaction;

/// Book every transaction: [`fill`] then [`assert_balances`], with no
/// auto-rule injection between. Input must be date-sorted (resolver
/// guarantees this). Returns transactions with every posting's amount
/// filled in and every balance assertion verified. The report load path
/// runs the two passes itself, expanding auto-rules in between.
pub fn book(
    transactions: Vec<Located<Transaction>>,
) -> Result<Vec<Located<Transaction>>, BookError> {
    assert_balances(fill(transactions)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decimal::Decimal;
    use crate::{parser, resolver};

    #[test]
    fn fill_then_expand_then_assert_fires_on_a_bare_trigger() {
        // The trigger `inter:send` is the *bare* (auto-balanced) leg. Only
        // because pass 1 (`fill`) resolves it to +20 first can the auto-rule
        // scale against it — the whole point of the split. The injected
        // `[budget]` posting then sits in the running balance, so the later
        // `budget = 0` assignment zeroes the full balance (seed + injection),
        // not just the seed. This is the fill -> expand -> assert order the
        // report load path uses; reverting it (expand before fill) would leave
        // the trigger unfilled and the rule silent.
        let src = "\
            = /^inter:send/\n\
            \t[budget]  -amount\n\
            \t[absorbed]\n\
            \n\
            2025-01-01 * seed\n\
            \tbudget   50 USD\n\
            \tequity   -50 USD\n\
            \n\
            2025-06-01 * transfer\n\
            \tassets:bank  -20 USD\n\
            \tinter:send\n\
            \n\
            2025-12-31 * deprecate\n\
            \tbudget   = 0 USD\n\
            \tequity:writeoff\n";
        let entries = parser::parse(src).unwrap();
        let resolved = resolver::resolve(entries).unwrap();
        let mut txs = fill(resolved.transactions).unwrap();
        crate::expander::expand(&mut txs, &resolved.auto_rules);
        let txs = assert_balances(txs).unwrap();

        // Rule fired on the filled bare trigger: an injected `[budget]` leg
        // worth -20 (the +20 trigger, negated) landed on the transfer.
        let transfer = &txs[1].value;
        let injected = transfer
            .postings
            .iter()
            .find(|lp| lp.value.account == "budget")
            .expect("rule should inject a budget posting");
        assert_eq!(injected.value.amount.as_ref().unwrap().value, Decimal::from(-20));

        // `budget = 0` accounts for the injection: 50 seed - 20 injected = 30,
        // so the deprecation books -30 to reach zero.
        let budget = &txs[2].value.postings[0].value;
        assert_eq!(budget.amount.as_ref().unwrap().value, Decimal::from(-30));
    }
}
