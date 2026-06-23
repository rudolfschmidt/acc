//! `sweep` command — close the open balance of a pass-through account.
//!
//! Conceptually `reg <account>`: pair equal-and-opposite amounts on the
//! pass-through account across the whole account (per commodity, over all
//! dates). Whatever stays unmatched is still open, so sweep writes one
//! offsetting entry per open posting — at that posting's date — booking
//! it onto income / expense to bring the account back to zero. A debit
//! posting (> 0) is an expense, a credit posting (< 0) an income.
//!
//! This is inherently idempotent and file-agnostic: once a posting's
//! offset exists anywhere in the loaded journal, the two cancel and drop
//! out — no markers, no file-name parsing, just the balance. A
//! time-shifted counter movement (an invoice settled weeks later)
//! cancels the same way, so genuine round-trips are left alone. Renaming
//! or moving the generated file changes nothing, as long as it stays part
//! of the loaded journal.
//!
//! Reuses existing phases: `load` parses/normalises and books, `filter`
//! scopes to the account, `format_amount` renders amounts, and
//! `format_in_place` aligns *and date-sorts* the result, so newly
//! appended transactions land in chronological order.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use colored::Colorize;

use crate::commands::util::format_amount;
use crate::decimal::Decimal;
use crate::error::Error;
use crate::loader::Journal;
use crate::parser::located::Located;
use crate::parser::transaction::Transaction;

pub fn run(
    journal: Journal,
    account: &str,
    segment: &str,
    income: &str,
    expense: &str,
) -> Result<(), Error> {
    // Precisions are needed by `format_amount` after `filter` consumes
    // the journal, so clone them out first.
    let precisions = journal.precisions.clone();

    // Title and output file derive from the account's last segment, with
    // any pattern anchors (`^` / `$`) stripped.
    let title = account
        .rsplit(':')
        .next()
        .unwrap_or(account)
        .trim_matches(|c| c == '^' || c == '$')
        .to_string();

    // Scope to postings on the pass-through account (real legs only).
    let pattern = [account.to_string()];
    let scoped = crate::filter::filter(journal, &pattern, None, None, false, false);

    let (out, count) =
        render_entries(&scoped.transactions, &title, segment, income, expense, &precisions);

    if count == 0 {
        println!("{} nothing to sweep", "!".yellow());
        return Ok(());
    }

    let path = PathBuf::from(format!("{}.ledger", title));
    append(&path, &out).map_err(|e| Error::from(format!("write {}: {}", path.display(), e)))?;

    // Align the generated file silently and date-sort it (format prints
    // nothing here), then report once formatting has succeeded. Sorting
    // slots newly appended transactions into chronological order.
    crate::commands::format::format_in_place(&path, true)?;

    let label = if count == 1 { "transaction" } else { "transactions" };
    println!("{} swept {} {} in {}", "✓".green(), count, label, path.display());
    Ok(())
}

/// Emit one offsetting entry per still-open posting on the pass-through
/// account. Postings are paired off across the **whole account** (per
/// commodity, over all dates): an amount that is later balanced by an
/// equal-and-opposite amount — its offset, or a time-shifted counter
/// movement like an invoice settled weeks later — cancels and is left
/// alone. Only the unmatched remainder is swept, each posting mirrored
/// at its own date. Pure (no I/O) so the pairing can be tested.
fn render_entries(
    transactions: &[Located<Transaction>],
    title: &str,
    segment: &str,
    income: &str,
    expense: &str,
    precisions: &HashMap<String, usize>,
) -> (String, usize) {
    // Collect the pass-through postings per (account, commodity) across
    // all dates. Already-swept legs are present as their offsets, so they
    // pair off below and drop out — that is what makes sweep idempotent
    // and independent of which file the offset lives in.
    let mut groups: HashMap<(String, String), Vec<(String, Decimal)>> = HashMap::new();
    for lt in transactions {
        let date = lt.value.date.to_string();
        for lp in &lt.value.postings {
            let Some(a) = &lp.value.amount else { continue };
            groups
                .entry((lp.value.account.clone(), a.commodity.clone()))
                .or_default()
                .push((date.clone(), a.value));
        }
    }

    // Pair within each group, flatten the open remainder, sort by date.
    let mut open: Vec<(String, String, String, Decimal)> = Vec::new();
    for ((account, commodity), postings) in groups {
        for (date, amount) in open_postings(postings) {
            open.push((date, account.clone(), commodity.clone(), amount));
        }
    }
    open.sort();

    let mut out = String::new();
    for (date, acct, commodity, amount) in &open {
        // Negate the open posting to zero the pass-through account; the
        // counter leg (elided — the booker infers it) takes income for a
        // credit posting (< 0), expense for a debit posting (> 0).
        let prefix = if amount.is_negative() { income } else { expense };
        out.push_str(date);
        out.push_str(" * ");
        out.push_str(title);
        out.push('\n');
        out.push_str(&format!(
            "\t{}\t{}\n",
            acct,
            format_amount(commodity, &(-*amount), precisions)
        ));
        out.push_str(&format!("\t{}:{}\n", prefix, segment));
        out.push('\n');
    }

    (out, open.len())
}

/// Determine the still-open postings on one (account, commodity) group.
/// Two cancelling stages, both pairing equal-and-opposite amounts:
///
///  1. **within the same date** first — an offset sweep is always written
///     at its original's date, so this matches each original to its own
///     offset and leaves a genuinely unoffset posting on its true date,
///     rather than shuffling the "open" one onto a same-amount sibling on
///     another date;
///  2. **across all dates** for whatever is left — a real round-trip such
///     as an invoice settled weeks later nets out here.
///
/// What survives both stages is open, returned as (date, amount).
fn open_postings(postings: Vec<(String, Decimal)>) -> Vec<(String, Decimal)> {
    // Stage 1: cancel opposites date by date.
    let mut by_date: HashMap<String, Vec<(String, Decimal)>> = HashMap::new();
    for entry in postings {
        by_date.entry(entry.0.clone()).or_default().push(entry);
    }
    let mut remainder = Vec::new();
    for group in by_date.into_values() {
        remainder.extend(cancel(group));
    }
    // Stage 2: cancel opposites across dates.
    cancel(remainder)
}

/// Pair equal-and-opposite amounts (by magnitude) among `postings` and
/// return the unmatched remainder. For a given magnitude, if more of one
/// sign remain than the other, the latest-dated ones stay; the earliest
/// are taken as settled first (FIFO).
fn cancel(postings: Vec<(String, Decimal)>) -> Vec<(String, Decimal)> {
    // Dates grouped by signed amount.
    let mut by_amount: HashMap<Decimal, Vec<String>> = HashMap::new();
    for (date, amount) in postings {
        if amount.is_zero() {
            continue;
        }
        by_amount.entry(amount).or_default().push(date);
    }

    let mut rest = Vec::new();
    let mut seen: HashSet<Decimal> = HashSet::new();
    let amounts: Vec<Decimal> = by_amount.keys().copied().collect();
    for amount in amounts {
        let abs = amount.abs();
        if !seen.insert(abs) {
            continue; // +abs and -abs share one magnitude; handle once
        }
        let mut pos = by_amount.remove(&abs).unwrap_or_default();
        let mut neg = by_amount.remove(&(-abs)).unwrap_or_default();
        pos.sort();
        neg.sort();
        let net = pos.len() as i64 - neg.len() as i64;
        if net > 0 {
            for date in pos.into_iter().rev().take(net as usize) {
                rest.push((date, abs));
            }
        } else if net < 0 {
            for date in neg.into_iter().rev().take((-net) as usize) {
                rest.push((date, -abs));
            }
        }
    }
    rest
}

/// Append `content` to `path`, creating it if absent. Never truncates,
/// so existing entries are preserved and only the new lines are added.
fn append(path: &Path, content: &str) -> std::io::Result<()> {
    let mut f = fs::OpenOptions::new().create(true).append(true).open(path)?;
    f.write_all(content.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::posting::{Amount, Posting};
    use crate::parser::transaction::State;
    use std::sync::Arc;

    fn posting(account: &str, commodity: &str, value: i64) -> Located<Posting> {
        Located {
            file: Arc::from(""),
            line: 0,
            value: Posting {
                account: account.to_string(),
                amount: Some(Amount {
                    commodity: commodity.to_string(),
                    value: Decimal::from(value),
                    decimals: 0,
                }),
                costs: None,
                lot_cost: None,
                lot_date: None,
                balance_assertion: None,
                is_virtual: false,
                balanced: true,
                comments: Vec::new(),
            },
        }
    }

    fn tx(date: &str, postings: Vec<Located<Posting>>) -> Located<Transaction> {
        Located {
            file: Arc::from(""),
            line: 1,
            value: Transaction {
                date: crate::date::Date::parse(date).unwrap(),
                state: State::Cleared,
                code: None,
                description: "x".to_string(),
                postings,
                comments: Vec::new(),
            },
        }
    }

    fn precisions() -> HashMap<String, usize> {
        let mut m = HashMap::new();
        m.insert("USD".to_string(), 2);
        m
    }

    fn render(txs: &[Located<Transaction>]) -> (String, usize) {
        render_entries(txs, "clearing", "foo:bar", "income", "expenses", &precisions())
    }

    // A debit balance (positive) closes to an expense; the offset negates
    // the remainder and the counter leg is elided.
    #[test]
    fn positive_balance_is_expense() {
        let txs = vec![tx("2026-06-01", vec![posting("clearing", "USD", 100)])];
        let (out, count) = render(&txs);
        assert_eq!(count, 1);
        assert!(out.contains("clearing\tUSD-100.00"), "{out}");
        assert!(out.contains("\texpenses:foo:bar\n"), "{out}");
        assert!(!out.contains("income:foo:bar"), "{out}");
    }

    // A credit balance (negative) closes to an income.
    #[test]
    fn negative_balance_is_income() {
        let txs = vec![tx("2026-06-01", vec![posting("clearing", "USD", -100)])];
        let (out, count) = render(&txs);
        assert_eq!(count, 1);
        assert!(out.contains("clearing\tUSD100.00"), "{out}");
        assert!(out.contains("\tincome:foo:bar\n"), "{out}");
        assert!(!out.contains("expenses:foo:bar"), "{out}");
    }

    // Same-sign postings don't pair, so each is mirrored separately —
    // five open legs on one day become five entries (not one netted sum).
    #[test]
    fn same_sign_postings_stay_separate() {
        let txs = vec![
            tx("2026-06-01", vec![posting("clearing", "USD", 10)]),
            tx("2026-06-01", vec![posting("clearing", "USD", 20)]),
            tx("2026-06-01", vec![posting("clearing", "USD", 30)]),
            tx("2026-06-01", vec![posting("clearing", "USD", 40)]),
            tx("2026-06-01", vec![posting("clearing", "USD", 50)]),
        ];
        let (out, count) = render(&txs);
        assert_eq!(count, 5);
        for v in ["10", "20", "30", "40", "50"] {
            assert!(out.contains(&format!("clearing\tUSD-{}.00", v)), "{out}");
        }
    }

    // Postings on different dates each get their own entry.
    #[test]
    fn different_dates_separate_entries() {
        let txs = vec![
            tx("2026-06-01", vec![posting("clearing", "USD", 50)]),
            tx("2026-06-02", vec![posting("clearing", "USD", 30)]),
        ];
        let (out, count) = render(&txs);
        assert_eq!(count, 2);
        assert!(out.contains("2026-06-01 * clearing"), "{out}");
        assert!(out.contains("2026-06-02 * clearing"), "{out}");
    }

    // An amount balanced by an equal-and-opposite amount weeks later
    // cancels across the whole account — nothing is swept. This is the
    // invoice-then-payment case, and also the idempotent re-run case
    // (an original plus the offset a previous run wrote).
    #[test]
    fn time_shifted_pair_cancels() {
        let txs = vec![
            tx("2026-06-01", vec![posting("clearing", "USD", 100)]),
            tx("2026-06-30", vec![posting("clearing", "USD", -100)]),
        ];
        let (out, count) = render(&txs);
        assert_eq!(count, 0);
        assert!(out.is_empty());
    }

    // Opposite amounts pair off; only the unmatched remainder is swept.
    #[test]
    fn only_remainder_after_pairing() {
        let txs = vec![
            tx("2026-06-01", vec![posting("clearing", "USD", 10)]),
            tx("2026-06-02", vec![posting("clearing", "USD", 20)]),
            tx("2026-06-03", vec![posting("clearing", "USD", 30)]),
            tx("2026-06-10", vec![posting("clearing", "USD", -10)]),
            tx("2026-06-11", vec![posting("clearing", "USD", -20)]),
        ];
        let (out, count) = render(&txs);
        assert_eq!(count, 1);
        assert!(out.contains("clearing\tUSD-30.00"), "{out}");
    }

    // When one date's offset is removed while same-amount postings on
    // other dates are still offset, the reopened posting is pulled at ITS
    // OWN date — same-date pairing runs before cross-date pairing, so the
    // open one is not shuffled onto a same-amount sibling.
    #[test]
    fn reopened_date_is_pulled_at_its_own_date() {
        let txs = vec![
            // 06-21 and 08-21: original + offset present (settled).
            tx("2022-06-21", vec![posting("clearing", "USD", 39)]),
            tx("2022-06-21", vec![posting("clearing", "USD", -39)]),
            tx("2022-08-21", vec![posting("clearing", "USD", 39)]),
            tx("2022-08-21", vec![posting("clearing", "USD", -39)]),
            // 07-21: offset removed → still open.
            tx("2022-07-21", vec![posting("clearing", "USD", 39)]),
        ];
        let (out, count) = render(&txs);
        assert_eq!(count, 1);
        assert!(out.contains("2022-07-21 * clearing"), "{out}");
        assert!(!out.contains("2022-06-21 * clearing"), "{out}");
        assert!(!out.contains("2022-08-21 * clearing"), "{out}");
    }

    // Header carries the source date, cleared marker and segment title.
    #[test]
    fn header_uses_date_and_title() {
        let txs = vec![tx("2026-06-01", vec![posting("clearing", "USD", 100)])];
        let (out, _) = render(&txs);
        assert!(out.starts_with("2026-06-01 * clearing\n"), "{out}");
    }
}
