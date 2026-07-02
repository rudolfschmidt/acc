//! Shared helpers for the balance flat/tree renderers.

use std::collections::BTreeMap;

use colored::Colorize;

use crate::commands::util::{format_amount, paint_label};
use crate::decimal::Decimal;
use crate::loader::{Journal, LabelView};

/// Aggregate every posting amount into a `account → commodity → sum`
/// nested map. The input is already filter-scoped, so every posting
/// seen here contributes unconditionally.
pub(super) fn group_postings_by_account(
    journal: &Journal,
) -> BTreeMap<String, BTreeMap<String, Decimal>> {
    let mut result: BTreeMap<String, BTreeMap<String, Decimal>> = BTreeMap::new();
    for tx in &journal.transactions {
        for lp in &tx.value.postings {
            let p = &lp.value;
            let Some(amount) = &p.amount else { continue };
            result
                .entry(p.account.clone())
                .or_default()
                .entry(amount.commodity.clone())
                .and_modify(|v| *v += amount.value)
                .or_insert(amount.value);
        }
    }
    result
}

/// Print one commodity amount right-aligned to `width`, red for
/// negative values. Trailing space keeps consistent separation from
/// any account name that follows.
pub(super) fn print_commodity_amount(
    commodity: &str,
    value: Decimal,
    width: usize,
    precisions: &std::collections::HashMap<String, usize>,
) {
    let formatted = format_amount(commodity, &value, precisions);
    if value.is_negative() {
        print!("{:>w$} ", formatted.red(), w = width);
    } else {
        print!("{:>w$} ", formatted, w = width);
    }
}

/// The ` (label)` suffix for an account in the balance view, coloured via
/// [`paint_label`] — empty when the account carries no balance label
/// (`label-balance`, or the shared `label` fallback; exact or `$segment`
/// pattern). Display only, appended by both balance renderers next to the
/// account name.
pub(super) fn label_suffix(account: &str, journal: &Journal) -> String {
    match journal.label_for(account, LabelView::Balance) {
        Some(label) => paint_label(&format!(" ({})", label)),
        None => String::new(),
    }
}
