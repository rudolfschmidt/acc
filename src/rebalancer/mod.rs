//! Rebalance phase — convert every posting's amount into a target
//! commodity using the journal's PriceDB.
//!
//! Per-posting lookup date is either the transaction's own `tx.date`
//! (historical conversion) or a user-fixed date (`--market`, snapshot
//! mode). Postings whose commodity has no rate path to `target` stay
//! unchanged — downstream reports show them as remainders in their
//! original commodity.

use crate::indexer::Index;
use crate::parser::located::Located;
use crate::parser::posting::Posting;
use crate::parser::transaction::Transaction;

/// Convert every posting's amount to `target` in place. `fixed_date`
/// = None → each posting uses its tx.date; Some(d) → every posting
/// uses `d` regardless.
pub fn rebalance(
    transactions: &mut [Located<Transaction>],
    target: &str,
    db: &Index,
    fixed_date: Option<&str>,
) {
    for lt in transactions {
        let lookup_date: String = match fixed_date {
            Some(d) => d.to_string(),
            None => lt.value.date.to_string(),
        };
        for lp in &mut lt.value.postings {
            convert(&mut lp.value, target, db, &lookup_date);
        }
    }
}

fn convert(p: &mut Posting, target: &str, db: &Index, date: &str) {
    let Some(amount) = p.amount.as_mut() else { return };
    if amount.commodity == target {
        return;
    }
    let Some(rate) = db.find(&amount.commodity, target, date) else {
        // No rate path — keep the posting in its original commodity.
        return;
    };
    // `mul_rounded` instead of `*` because inverse-rate lookups from
    // the PriceDB can serve a 28-digit tail which would make strict
    // multiplication overflow the scale.
    amount.value = amount.value.mul_rounded(rate);
    amount.commodity = target.to_string();
}
