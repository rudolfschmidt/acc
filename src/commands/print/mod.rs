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
use crate::parser::posting::Posting;
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
    // Uniform 3-char state marker keeps the description column stable.
    let marker = match tx.state {
        State::Cleared => " * ".green().to_string(),
        State::Uncleared => "   ".to_string(),
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

    // `@` / `@@` cost annotations and `=` balance assertions are
    // internal to the booker and not rendered here: costs drive the
    // balance math, assertions are verified during load, neither
    // carries new information for the reader.

    println!();

    for comment in &p.comments {
        print_spaces(GAP);
        println!("{}", format!("; {}", comment.value.text).dimmed());
    }
}

/// Virtual postings are wrapped in `(...)`. The parser distinguishes
/// `[balanced]` vs `(unbalanced)`, but print collapses both to `(...)`
/// following the old reference output.
fn render_account(p: &Posting) -> String {
    if p.is_virtual {
        format!("({})", p.account)
    } else {
        p.account.clone()
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
