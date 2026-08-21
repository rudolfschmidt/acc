//! Booker phase — the only phase with cross-transaction state.
//!
//! Booking runs in two passes so an inject hook can sit between
//! them (report path only):
//!
//! 1. [`fill`](filler::fill) — **transaction-local balance.** Each
//!    ordinary transaction's one omitted amount is inferred so its
//!    commodities sum to zero (cost-aware for `@` / `@@`). This gives
//!    every posting a concrete amount — the basis the inject hook
//!    computes against. Transactions carrying a balance *assignment* (`= TARGET`
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
//! that inject nothing (lint, sweep, format, tests). The report load
//! path instead calls [`fill`](filler::fill), runs the inject hook, then
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
/// inject hook between. Input must be date-sorted (resolver
/// guarantees this). Returns transactions with every posting's amount
/// filled in and every balance assertion verified. The report load path
/// runs the two passes itself, running the inject hook in between.
pub fn book(
    transactions: Vec<Located<Transaction>>,
) -> Result<Vec<Located<Transaction>>, BookError> {
    assert_balances(fill(transactions)?)
}
