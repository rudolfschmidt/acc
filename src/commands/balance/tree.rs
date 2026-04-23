//! `balance` tree mode — hierarchical totals with an indent per
//! account-path segment.
//!
//! The account tree is built from the already-filtered journal, so
//! every branch contains at least one matching posting. `show_empty`
//! controls whether branches with a zero display-rounded total are
//! rendered.

use std::collections::BTreeMap;

use colored::Colorize;

use super::common::print_commodity_amount;
use crate::commands::account::Account;
use crate::commands::util::format_amount;
use crate::decimal::Decimal;
use crate::loader::Journal;

pub(super) fn print(journal: &Journal, show_empty: bool) {
    let precisions = &journal.precisions;
    let root = Account::from_transactions(&journal.transactions);
    let width = calculate_width(&root, precisions);

    for child in root.children.values() {
        if show_empty || child.has_balance(precisions) {
            print_account("", child, width, precisions, show_empty);
        }
    }

    if root.children.is_empty() {
        return;
    }

    println!("{}", "-".repeat(width));
    let total = root.total();
    if total.values().all(|v| v.is_zero()) {
        println!("{:>w$} ", 0, w = width);
    } else {
        for (commodity, value) in &total {
            if !value.is_zero() {
                print_commodity_amount(commodity, *value, width, precisions);
                println!();
            }
        }
    }
}

fn print_account(
    indent: &str,
    account: &Account,
    width: usize,
    precisions: &std::collections::HashMap<String, usize>,
    show_empty: bool,
) {
    let total = account.total();
    let non_zero: Vec<_> = total.iter().filter(|(_, v)| !v.is_zero()).collect();

    let child_indent = if non_zero.is_empty() {
        // Empty branch: show the name alone (caller already checked
        // `show_empty || has_balance` before descending).
        indent.to_string()
    } else {
        for (i, (commodity, value)) in non_zero.iter().enumerate() {
            print_commodity_amount(commodity, **value, width, precisions);
            if i < non_zero.len() - 1 {
                println!();
            }
        }
        println!("{}{}", indent, account.name.blue());
        format!("{}  ", indent)
    };

    // Always descend — a branch can net to zero while its children
    // individually have non-zero commodities that offset.
    for child in account.children.values() {
        if show_empty || child.has_balance(precisions) {
            print_account(&child_indent, child, width, precisions, show_empty);
        }
    }
}

fn calculate_width(
    root: &Account,
    precisions: &std::collections::HashMap<String, usize>,
) -> usize {
    let mut max_width = 0;
    let mut visit = |acc: &Account| {
        for (commodity, value) in acc.total() {
            let w = format_amount(&commodity, &value, precisions).chars().count();
            if w > max_width {
                max_width = w;
            }
        }
    };
    root.walk(&mut visit);
    // Grand total over the root also contributes.
    let grand: BTreeMap<String, Decimal> = root.total();
    for (commodity, value) in &grand {
        let w = format_amount(commodity, value, precisions).chars().count();
        if w > max_width {
            max_width = w;
        }
    }
    max_width
}
