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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{booker, parser, resolver};

    fn setup(src: &str) -> Vec<Located<Transaction>> {
        let entries = parser::parse(src).unwrap();
        let resolved = resolver::resolve(entries).unwrap();
        booker::book(resolved.transactions).unwrap()
    }

    /// Descriptions in transaction order after sorting by `fields`.
    fn order(txs: &[Located<Transaction>]) -> Vec<&str> {
        txs.iter().map(|t| t.value.description.as_str()).collect()
    }

    const SRC: &str = "\
        2024-03-01 * banana\n\
        \tassets:checking   30 USD\n\
        \tincome:x         -30 USD\n\
        2024-01-01 * cherry\n\
        \tassets:savings    10 USD\n\
        \tincome:x         -10 USD\n\
        2024-02-01 * apple\n\
        \tassets:brokerage  20 USD\n\
        \tincome:x         -20 USD\n";

    #[test]
    fn empty_fields_is_a_noop() {
        let mut txs = setup(SRC);
        sort(&mut txs, &[]);
        // Booker keeps natural date order; an empty sort must not touch it.
        assert_eq!(order(&txs), ["cherry", "apple", "banana"]);
    }

    #[test]
    fn sort_by_date_ascending() {
        let mut txs = setup(SRC);
        sort(&mut txs, &["date".into()]);
        assert_eq!(order(&txs), ["cherry", "apple", "banana"]);
    }

    #[test]
    fn sort_by_date_reverse() {
        let mut txs = setup(SRC);
        sort(&mut txs, &["-date".into()]);
        assert_eq!(order(&txs), ["banana", "apple", "cherry"]);
    }

    #[test]
    fn sort_by_amount_uses_first_posting() {
        let mut txs = setup(SRC);
        sort(&mut txs, &["amount".into()]);
        // First postings: 10, 20, 30 → cherry, apple, banana.
        assert_eq!(order(&txs), ["cherry", "apple", "banana"]);
    }

    #[test]
    fn sort_by_account_uses_first_posting() {
        let mut txs = setup(SRC);
        sort(&mut txs, &["account".into()]);
        // assets:brokerage < assets:checking < assets:savings.
        assert_eq!(order(&txs), ["apple", "banana", "cherry"]);
    }

    #[test]
    fn sort_by_description() {
        let mut txs = setup(SRC);
        sort(&mut txs, &["description".into()]);
        assert_eq!(order(&txs), ["apple", "banana", "cherry"]);
    }

    #[test]
    fn payee_is_an_alias_for_description() {
        let mut txs = setup(SRC);
        sort(&mut txs, &["payee".into()]);
        assert_eq!(order(&txs), ["apple", "banana", "cherry"]);
    }

    #[test]
    fn unknown_field_falls_back_to_date() {
        let mut txs = setup(SRC);
        sort(&mut txs, &["nonsense".into()]);
        assert_eq!(order(&txs), ["cherry", "apple", "banana"]);
    }

    #[test]
    fn later_criteria_break_ties() {
        // Two transactions on the same date, different descriptions:
        // primary key (date) ties, secondary (description) decides.
        let src = "\
            2024-01-01 * zebra\n\
            \ta  1 USD\n\
            \tb -1 USD\n\
            2024-01-01 * alpha\n\
            \ta  2 USD\n\
            \tb -2 USD\n";
        let mut txs = setup(src);
        sort(&mut txs, &["date".into(), "description".into()]);
        assert_eq!(order(&txs), ["alpha", "zebra"]);
    }
}
