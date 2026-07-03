//! `balance` flat mode — per-account totals, then a separator, then
//! the journal-wide grand total per commodity.

use std::collections::BTreeMap;

use colored::Colorize;

use super::common::{group_postings_by_account, label_suffix, print_commodity_amount};
use crate::commands::util::{format_amount, shows_nonzero};
use crate::decimal::Decimal;
use crate::loader::Journal;

pub(super) fn print(journal: &Journal, show_empty: bool) {
    let postings = group_postings_by_account(journal);
    let precisions = &journal.precisions;

    // Journal-wide grand total per commodity.
    let total: BTreeMap<String, Decimal> = postings
        .values()
        .flat_map(|amounts| amounts.iter())
        .fold(BTreeMap::new(), |mut total, (commodity, amount)| {
            total
                .entry(commodity.clone())
                .and_modify(|v| *v += *amount)
                .or_insert(*amount);
            total
        });

    // Column width: max of any per-account amount and any grand-total
    // amount, so the column lines up across both blocks.
    let width = std::cmp::max(
        postings
            .values()
            .flat_map(|amounts| amounts.iter())
            .map(|(c, v)| format_amount(c, v, precisions).chars().count())
            .max()
            .unwrap_or(0),
        total
            .iter()
            .map(|(c, v)| format_amount(c, v, precisions).chars().count())
            .max()
            .unwrap_or(0),
    );

    // Per-account block: one line per commodity with a non-zero sum,
    // account name printed after the last commodity line.
    for (account, amounts) in &postings {
        let non_zero: Vec<_> = amounts
            .iter()
            .filter(|(c, v)| shows_nonzero(c, v, precisions))
            .collect();
        if non_zero.is_empty() {
            // Account nets to zero. Render a `0` line + name only under
            // `-E`; otherwise skip it.
            if show_empty {
                print!("{:>w$} ", 0, w = width);
                println!("{}{}", account.blue(), label_suffix(account, journal));
            }
            continue;
        }
        for (i, (commodity, amount)) in non_zero.iter().enumerate() {
            print_commodity_amount(commodity, **amount, width, precisions);
            if i < non_zero.len() - 1 {
                println!();
            }
        }
        println!("{}{}", account.blue(), label_suffix(account, journal));
    }

    // Separator + grand total.
    println!("{}", "-".repeat(width));
    if total.iter().all(|(c, v)| !shows_nonzero(c, v, precisions)) {
        println!("{:>w$} ", 0, w = width);
    } else {
        for (commodity, amount) in &total {
            if shows_nonzero(commodity, amount, precisions) {
                print_commodity_amount(commodity, *amount, width, precisions);
                println!();
            }
        }
    }
}
