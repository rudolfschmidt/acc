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
//! Every indent and gap is emitted via `write_spaces(GAP)` so column
//! alignment is consistent across rows. Accounts and amounts have
//! global max-widths: the account column is left-aligned within
//! `account_max`, the amount column right-aligned within `amount_max`.
//!
//! Runs after the filter phase — the journal is already scoped.

use std::io::{self, BufWriter, Write};

use colored::Colorize;

use super::util::{format_amount, render_account, write_spaces};
use crate::loader::Journal;
use crate::parser::posting::{Costs, Posting};
use crate::parser::transaction::{State, Transaction};

const GAP: usize = 4;

pub fn run(journal: &Journal) {
    let account_max = max_account_width(journal);
    let amount_max = max_amount_width(journal);

    // Stream through one locked, buffered writer: rendering a large
    // journal emits hundreds of thousands of lines, and an unbuffered
    // `println!` would do a locked write syscall per line.
    let stdout = io::stdout();
    let mut out = BufWriter::new(stdout.lock());

    let mut iter = journal.transactions.iter().peekable();
    while let Some(lt) = iter.next() {
        let tx = &lt.value;
        let _ = print_header(&mut out, tx);
        for lp in &tx.postings {
            let _ = print_posting(&mut out, &lp.value, account_max, amount_max, &journal.precisions);
        }
        if iter.peek().is_some() {
            let _ = writeln!(out);
        }
    }
    let _ = out.flush();
}

fn print_header<W: Write>(out: &mut W, tx: &Transaction) -> io::Result<()> {
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
    writeln!(out, "{}{}{}{}", tx.date, marker, code, tx.description.bold())?;
    for comment in &tx.comments {
        write_spaces(out, GAP)?;
        writeln!(out, "{}", format!("; {}", comment.value.text).dimmed())?;
    }
    Ok(())
}

fn print_posting<W: Write>(
    out: &mut W,
    p: &Posting,
    account_max: usize,
    amount_max: usize,
    precisions: &std::collections::HashMap<String, usize>,
) -> io::Result<()> {
    let display = render_account(p);
    let display_width = display.chars().count();

    write_spaces(out, GAP)?;
    write!(out, "{}", display.blue())?;

    if let Some(amount) = &p.amount {
        write_spaces(out, account_max.saturating_sub(display_width) + GAP)?;
        let formatted = format_amount(&amount.commodity, &amount.value, precisions);
        write_spaces(out, amount_max.saturating_sub(formatted.chars().count()))?;
        if amount.value.is_negative() {
            write!(out, "{}", formatted.red())?;
        } else {
            write!(out, "{}", formatted)?;
        }
    }

    // Lot annotations follow the amount, ledger-style:
    //   AMOUNT {cost} [lot-date] @ price
    // `{cost}` is the lot basis, `[lot-date]` the (acc-generated)
    // acquisition date of the closed lot, `@`/`@@` the unit/total cost.
    // `= assertion` stays internal (verified at load, not rendered).
    if let Some(lot) = &p.lot_cost {
        let a = lot.amount();
        write!(out, " {{{}}}", format_amount(&a.commodity, &a.value, precisions))?;
    }
    if let Some(d) = &p.lot_date {
        write!(out, " [{}]", d)?;
    }
    if let Some(costs) = &p.costs {
        match costs {
            Costs::PerUnit(a) => {
                write!(out, " @ {}", format_amount(&a.commodity, &a.value, precisions))?
            }
            Costs::Total(a) => {
                write!(out, " @@ {}", format_amount(&a.commodity, &a.value, precisions))?
            }
        }
    }

    writeln!(out)?;

    for comment in &p.comments {
        write_spaces(out, GAP)?;
        writeln!(out, "{}", format!("; {}", comment.value.text).dimmed())?;
    }
    Ok(())
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
