//! `register` command — one row per transaction with every posting's
//! amount plus the running total across all prior postings.
//!
//! Layout per transaction row:
//!
//! ```text
//! DATE STATE DESCRIPTION   account        amount   running-total
//!                          account        amount   running-total
//! ```
//!
//! The running total is cumulative across all postings seen so far and
//! is rendered per-commodity; rows with multiple non-zero commodities
//! add continuation lines for the extra commodities.
//!
//! Runs after the filter phase — the journal is already scoped.

use std::collections::BTreeMap;
use std::io::{self, BufWriter, Write};

use colored::Colorize;

use super::util::{format_amount, paint_label, render_account, shows_nonzero, write_spaces};
use crate::decimal::Decimal;
use crate::loader::{Journal, LabelView};
use crate::parser::transaction::{State, Transaction};

const GAP: usize = 2;

pub fn run(journal: &Journal) {
    let precisions = &journal.precisions;
    let rows = build_rows(journal);
    let widths = compute_widths(&rows, precisions, terminal_cols());

    // One locked, buffered writer for the whole register — see `print`.
    let stdout = io::stdout();
    let mut out = BufWriter::new(stdout.lock());

    for row in &rows {
        let title_truncated = truncate(&row.title, widths.title);
        for (i, entry) in row.entries.iter().enumerate() {
            let title = if i == 0 { title_truncated.as_str() } else { "" };
            let totals = non_zero_commodities(&entry.total, precisions);
            if totals.is_empty() {
                let _ = print_line(
                    &mut out,
                    title,
                    &entry.account,
                    entry.account_width,
                    &entry.amount,
                    entry.amount_negative,
                    "0",
                    false,
                    &widths,
                );
            } else {
                for (j, (commodity, value)) in totals.iter().enumerate() {
                    let total_str = format_amount(commodity, value, precisions);
                    if j == 0 {
                        let _ = print_line(
                            &mut out,
                            title,
                            &entry.account,
                            entry.account_width,
                            &entry.amount,
                            entry.amount_negative,
                            &total_str,
                            value.is_negative(),
                            &widths,
                        );
                    } else {
                        let _ = print_continuation(&mut out, &total_str, value.is_negative(), &widths);
                    }
                }
            }
        }
    }
    let _ = out.flush();
}

struct Row {
    title: String,
    entries: Vec<Entry>,
}

struct Entry {
    /// Pre-styled account path: segments blue, plus any register label
    /// coloured inline after the segment it belongs to (`ukdac:12 (foo):wise`).
    account: String,
    /// Visible width of `account` (excludes ANSI colour codes), for
    /// column alignment.
    account_width: usize,
    amount: String,
    amount_negative: bool,
    total: BTreeMap<String, Decimal>,
}

struct Widths {
    title: usize,
    account: usize,
    amount: usize,
    total: usize,
}

/// Walk the journal, accumulating a per-commodity running total.
/// Each posting produces one `Entry` capturing the running total at
/// the moment it was applied.
fn build_rows(journal: &Journal) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut running: BTreeMap<String, Decimal> = BTreeMap::new();

    for tx in &journal.transactions {
        let title = format_title(&tx.value);
        let mut entries = Vec::new();

        for lp in &tx.value.postings {
            let p = &lp.value;
            let Some(amount) = &p.amount else { continue };
            let name = render_account(p);
            let (account, account_width) = render_account_labeled(&name, journal);

            running
                .entry(amount.commodity.clone())
                .and_modify(|a| *a += amount.value)
                .or_insert(amount.value);

            let amount_str = format_amount(&amount.commodity, &amount.value, &journal.precisions);

            entries.push(Entry {
                account,
                account_width,
                amount: amount_str,
                amount_negative: amount.value.is_negative(),
                total: running.clone(),
            });
        }

        if !entries.is_empty() {
            rows.push(Row { title, entries });
        }
    }

    rows
}

/// Style an account path for the register: each segment coloured blue,
/// with a register label ([`paint_label`]) inline after the segment it
/// attaches to — e.g. `ukdac:12 (brokerage):wise`. The label is looked
/// up on each `:`-joined prefix (`label-register`, else the shared
/// `label` fallback). Returns the styled string (with ANSI colour codes)
/// and its visible width (without them), so columns still align.
fn render_account_labeled(account: &str, journal: &Journal) -> (String, usize) {
    let mut styled = String::new();
    let mut width = 0;
    let mut prefix = String::new();
    for (i, segment) in account.split(':').enumerate() {
        if i > 0 {
            styled.push_str(&":".blue().to_string());
            width += 1;
            prefix.push(':');
        }
        prefix.push_str(segment);
        styled.push_str(&segment.blue().to_string());
        width += segment.chars().count();
        if let Some(label) = journal.label_for(&prefix, LabelView::Register) {
            let addition = format!("({})", label);
            width += addition.chars().count();
            styled.push_str(&paint_label(&addition));
        }
    }
    (styled, width)
}

fn compute_widths(
    rows: &[Row],
    precisions: &std::collections::HashMap<String, usize>,
    cols: usize,
) -> Widths {
    let mut widths = Widths {
        title: 0,
        account: 0,
        amount: 0,
        // at least room for "0"
        total: 1,
    };
    for row in rows {
        widths.title = widths.title.max(row.title.chars().count());
        for entry in &row.entries {
            widths.account = widths.account.max(entry.account_width);
            widths.amount = widths.amount.max(entry.amount.chars().count());
            // Only commodities that would actually print (non-display-zero)
            // count toward the total column's width.
            for (c, v) in &entry.total {
                if !shows_nonzero(c, v, precisions) {
                    continue;
                }
                let w = format_amount(c, v, precisions).chars().count();
                widths.total = widths.total.max(w);
            }
        }
    }

    let fixed = widths.account + GAP + widths.amount + GAP + widths.total + GAP;
    let title_budget = cols.saturating_sub(fixed);
    if widths.title > title_budget {
        widths.title = title_budget;
    }
    widths
}

fn non_zero_commodities(
    total: &BTreeMap<String, Decimal>,
    precisions: &std::collections::HashMap<String, usize>,
) -> Vec<(String, Decimal)> {
    total
        .iter()
        .filter(|&(c, v)| shows_nonzero(c, v, precisions))
        .map(|(c, v)| (c.clone(), *v))
        .collect()
}

fn format_title(tx: &Transaction) -> String {
    let marker = match tx.state {
        State::Cleared => " * ",
        State::Uncleared => " ",
        State::Pending => " ! ",
    };
    format!("{}{}{}", tx.date, marker, tx.description)
}


#[allow(clippy::too_many_arguments)]
fn print_line<W: Write>(
    out: &mut W,
    title: &str,
    account: &str,
    account_width: usize,
    amount: &str,
    amount_negative: bool,
    total: &str,
    total_negative: bool,
    widths: &Widths,
) -> io::Result<()> {
    print_left(out, title, title.chars().count(), widths.title + GAP)?;
    // `account` is already styled (blue segments + coloured labels); pad by
    // its precomputed visible width.
    print_left(out, account, account_width, widths.account + GAP)?;
    print_right(out, amount, amount_negative, widths.amount)?;
    write_spaces(out, GAP)?;
    print_right(out, total, total_negative, widths.total)?;
    writeln!(out)
}

fn print_continuation<W: Write>(
    out: &mut W,
    total: &str,
    total_negative: bool,
    widths: &Widths,
) -> io::Result<()> {
    let prefix = widths.title + GAP + widths.account + GAP + widths.amount + GAP;
    write_spaces(out, prefix)?;
    print_right(out, total, total_negative, widths.total)?;
    writeln!(out)
}

fn print_left<W: Write>(out: &mut W, text: &str, visible: usize, width: usize) -> io::Result<()> {
    write!(out, "{}", text)?;
    write_spaces(out, width.saturating_sub(visible))
}

fn print_right<W: Write>(out: &mut W, text: &str, negative: bool, width: usize) -> io::Result<()> {
    let visible = text.chars().count();
    write_spaces(out, width.saturating_sub(visible))?;
    if negative {
        write!(out, "{}", text.red())
    } else {
        write!(out, "{}", text)
    }
}

fn terminal_cols() -> usize {
    crossterm::terminal::size()
        .map(|(cols, _)| cols as usize)
        .ok()
        .filter(|&n| n > 0)
        .or_else(|| {
            std::env::var("COLUMNS")
                .ok()
                .and_then(|s| s.parse().ok())
                .filter(|&n: &usize| n > 0)
        })
        .unwrap_or(80)
}

fn truncate(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    if max == 1 {
        return "…".to_string();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}
