//! Sort phase — reorder transactions for report display.
//!
//! Runs after the booker has already validated assertions against the
//! natural date-sorted order, so the user-visible sort is purely a
//! presentation choice. The booker-enforced date order is preserved
//! until this phase explicitly replaces it.
//!
//! Fields (any of, comma-independent — pass each as a separate `-S`
//! argument or comma-join in the CLI):
//!
//! ```text
//! date | d              transaction date (default)
//! amount | amt          first posting's amount, as f64
//! account | acc         first posting's account name
//! description | desc    transaction description
//! payee                 alias for description
//! ```
//!
//! Prefix any field with `-` for reverse order, e.g. `--sort=-date`.
//! Ties are broken by the next criterion in the list.

use crate::parser::located::Located;
use crate::parser::transaction::Transaction;

/// Sort transactions in place by the given ordered list of criteria.
/// An empty `fields` list is a no-op — the caller's prior order is
/// preserved.
pub fn sort(transactions: &mut [Located<Transaction>], fields: &[String]) {
    if fields.is_empty() {
        return;
    }
    let criteria: Vec<_> = fields.iter().map(|f| parse_criterion(f)).collect();

    transactions.sort_by(|a, b| {
        for (field, reverse) in &criteria {
            let ord = compare(&a.value, &b.value, field);
            if ord != std::cmp::Ordering::Equal {
                return if *reverse { ord.reverse() } else { ord };
            }
        }
        std::cmp::Ordering::Equal
    });
}

#[derive(Debug)]
enum Field {
    Date,
    Amount,
    Account,
    Description,
}

fn parse_criterion(s: &str) -> (Field, bool) {
    let (name, reverse) = match s.strip_prefix('-') {
        Some(rest) => (rest, true),
        None => (s, false),
    };
    let field = match name {
        "date" | "d" => Field::Date,
        "amount" | "amt" => Field::Amount,
        "account" | "acc" => Field::Account,
        "description" | "desc" | "payee" => Field::Description,
        // Unknown field → fall back to date rather than error.
        _ => Field::Date,
    };
    (field, reverse)
}

fn compare(a: &Transaction, b: &Transaction, field: &Field) -> std::cmp::Ordering {
    match field {
        Field::Date => a.date.cmp(&b.date),
        Field::Description => a.description.cmp(&b.description),
        Field::Account => first_account(a).cmp(first_account(b)),
        Field::Amount => first_amount(a)
            .partial_cmp(&first_amount(b))
            .unwrap_or(std::cmp::Ordering::Equal),
    }
}

fn first_account(tx: &Transaction) -> &str {
    tx.postings
        .first()
        .map(|lp| lp.value.account.as_str())
        .unwrap_or("")
}

fn first_amount(tx: &Transaction) -> f64 {
    tx.postings
        .first()
        .and_then(|lp| lp.value.amount.as_ref())
        .map(|a| a.value.to_f64())
        .unwrap_or(0.0)
}
