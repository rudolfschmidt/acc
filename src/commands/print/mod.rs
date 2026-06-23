//! `print` command — write each booked transaction back in canonical,
//! colorized ledger format.
//!
//! Layout per transaction:
//!
//! ```text
//! DATE [STATE] [(CODE)] DESCRIPTION
//!     ; transaction comment
//!     ACCOUNT              COMMODITY VALUE
//!     ; posting comment
//! ```
//!
//! Every indent and gap is emitted via `print_spaces(GAP)` so column
//! alignment is consistent across rows. Accounts and amounts have
//! global max-widths: the account column is left-aligned within
//! `account_max`, the amount column right-aligned within `amount_max`.
//!
//! Runs after the filter phase — the journal is already scoped.

use colored::Colorize;

use super::util::{format_amount, print_spaces};
use crate::loader::Journal;
use crate::parser::posting::{Costs, Posting};
use crate::parser::transaction::{State, Transaction};

const GAP: usize = 4;

pub fn run(journal: &Journal) {
    let account_max = max_account_width(journal);
    let amount_max = max_amount_width(journal);
    let mut iter = journal.transactions.iter().peekable();

    while let Some(lt) = iter.next() {
        let tx = &lt.value;
        print_header(tx);
        for lp in &tx.postings {
            print_posting(&lp.value, account_max, amount_max, &journal.precisions);
        }
        if iter.peek().is_some() {
            println!();
        }
    }
}

fn print_header(tx: &Transaction) {
    // State marker mirrors ledger: cleared `* `, pending `! `, and no
    // marker at all when the state is absent — just `date description`,
    // not an artificial blank column.
    let marker = match tx.state {
        State::Cleared => " * ".green().to_string(),
        State::Uncleared => " ".to_string(),
        State::Pending => " ! ".yellow().to_string(),
    };
    let code = tx
        .code
        .as_deref()
        .map(|c| format!("({}) ", c).yellow().to_string())
        .unwrap_or_default();
    println!("{}{}{}{}", tx.date, marker, code, tx.description.bold());
    for comment in &tx.comments {
        print_spaces(GAP);
        println!("{}", format!("; {}", comment.value.text).dimmed());
    }
}

fn print_posting(
    p: &Posting,
    account_max: usize,
    amount_max: usize,
    precisions: &std::collections::HashMap<String, usize>,
) {
    let display = render_account(p);
    let display_width = display.chars().count();

    print_spaces(GAP);
    print!("{}", display.blue());

    if let Some(amount) = &p.amount {
        print_spaces(account_max.saturating_sub(display_width) + GAP);
        let formatted = format_amount(&amount.commodity, &amount.value, precisions);
        print_spaces(amount_max.saturating_sub(formatted.chars().count()));
        if amount.value.is_negative() {
            print!("{}", formatted.red());
        } else {
            print!("{}", formatted);
        }
    }

    // Lot annotations follow the amount, ledger-style:
    //   AMOUNT {cost} [lot-date] @ price
    // `{cost}` is the lot basis, `[lot-date]` the (acc-generated)
    // acquisition date of the closed lot, `@`/`@@` the unit/total cost.
    // `= assertion` stays internal (verified at load, not rendered).
    if let Some(lot) = &p.lot_cost {
        let a = lot.amount();
        print!(" {{{}}}", format_amount(&a.commodity, &a.value, precisions));
    }
    if let Some(d) = &p.lot_date {
        print!(" [{}]", d);
    }
    if let Some(costs) = &p.costs {
        match costs {
            Costs::PerUnit(a) => {
                print!(" @ {}", format_amount(&a.commodity, &a.value, precisions))
            }
            Costs::Total(a) => {
                print!(" @@ {}", format_amount(&a.commodity, &a.value, precisions))
            }
        }
    }

    println!();

    for comment in &p.comments {
        print_spaces(GAP);
        println!("{}", format!("; {}", comment.value.text).dimmed());
    }
}

/// Account column content: real `account`, balanced-virtual
/// `[account]`, or paren-virtual `(account)` — matching ledger's own
/// print/reg output (verified against ledger 3.4.1), and consistent
/// with acc's own `register` / `format` rendering.
fn render_account(p: &Posting) -> String {
    match (p.is_virtual, p.balanced) {
        (true, true) => format!("[{}]", p.account),
        (true, false) => format!("({})", p.account),
        (false, _) => p.account.clone(),
    }
}

fn max_account_width(journal: &Journal) -> usize {
    journal
        .transactions
        .iter()
        .flat_map(|tx| tx.value.postings.iter())
        .map(|lp| render_account(&lp.value).chars().count())
        .max()
        .unwrap_or(0)
}

fn max_amount_width(journal: &Journal) -> usize {
    journal
        .transactions
        .iter()
        .flat_map(|tx| tx.value.postings.iter())
        .filter_map(|lp| {
            lp.value.amount.as_ref().map(|a| {
                format_amount(&a.commodity, &a.value, &journal.precisions)
                    .chars()
                    .count()
            })
        })
        .max()
        .unwrap_or(0)
}
