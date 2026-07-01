//! `balance` tree mode — hierarchical totals with an indent per
//! account-path segment.
//!
//! The account tree is built from the already-filtered journal, so
//! every branch contains at least one matching posting. `show_empty`
//! controls whether branches with a zero display-rounded total are
//! rendered.

use std::collections::{BTreeMap, HashMap};

use colored::Colorize;

use super::common::{label_suffix, print_commodity_amount};
use crate::commands::account::Account;
use crate::commands::util::format_amount;
use crate::decimal::Decimal;
use crate::loader::Journal;

/// Invariant rendering context threaded through the recursion, so
/// `print_account` only carries the per-node arguments (indent, path,
/// account) alongside it.
struct Ctx<'a> {
    width: usize,
    precisions: &'a HashMap<String, usize>,
    labels: &'a HashMap<String, String>,
    show_empty: bool,
}

pub(super) fn print(journal: &Journal, show_empty: bool) {
    let root = Account::from_transactions(&journal.transactions);
    let ctx = Ctx {
        width: calculate_width(&root, &journal.precisions),
        precisions: &journal.precisions,
        labels: &journal.labels,
        show_empty,
    };

    for child in root.children.values() {
        if show_empty || child.has_balance(ctx.precisions) {
            print_account(&ctx, "", &child.name, child);
        }
    }

    if root.children.is_empty() {
        return;
    }

    println!("{}", "-".repeat(ctx.width));
    let total = root.total();
    if total.values().all(|v| v.is_zero()) {
        println!("{:>w$} ", 0, w = ctx.width);
    } else {
        for (commodity, value) in &total {
            if !value.is_zero() {
                print_commodity_amount(commodity, *value, ctx.width, ctx.precisions);
                println!();
            }
        }
    }
}

/// `path` is the full account name at this node (segments joined by
/// `:`), used to look up its label; `indent` is the leading whitespace
/// for this depth. `account.name` is only the last segment.
fn print_account(ctx: &Ctx, indent: &str, path: &str, account: &Account) {
    let total = account.total();
    let non_zero: Vec<_> = total.iter().filter(|(_, v)| !v.is_zero()).collect();

    let child_indent = if non_zero.is_empty() {
        // Account whose total nets to zero. Under `-E`, render a `0`
        // line + name so the empty account is visible; otherwise keep
        // the name hidden and let any non-zero descendants render at
        // the parent's indent (a branch can net to zero while its
        // children individually offset).
        if ctx.show_empty {
            print!("{:>w$} ", 0, w = ctx.width);
            println!("{}{}{}", indent, account.name.blue(), label_suffix(path, ctx.labels));
            format!("{}  ", indent)
        } else {
            indent.to_string()
        }
    } else {
        for (i, (commodity, value)) in non_zero.iter().enumerate() {
            print_commodity_amount(commodity, **value, ctx.width, ctx.precisions);
            if i < non_zero.len() - 1 {
                println!();
            }
        }
        println!("{}{}{}", indent, account.name.blue(), label_suffix(path, ctx.labels));
        format!("{}  ", indent)
    };

    // Always descend — a branch can net to zero while its children
    // individually have non-zero commodities that offset.
    for child in account.children.values() {
        if ctx.show_empty || child.has_balance(ctx.precisions) {
            let child_path = format!("{}:{}", path, child.name);
            print_account(ctx, &child_indent, &child_path, child);
        }
    }
}

fn calculate_width(root: &Account, precisions: &HashMap<String, usize>) -> usize {
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
