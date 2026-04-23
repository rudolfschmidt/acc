//! `accounts` flat mode — sorted, deduped, one account name per line.

use std::collections::BTreeSet;

use crate::loader::Journal;

pub(super) fn print(journal: &Journal) {
    for account in journal
        .transactions
        .iter()
        .flat_map(|tx| tx.value.postings.iter())
        .map(|p| p.value.account.as_str())
        .collect::<BTreeSet<&str>>()
    {
        println!("{}", account);
    }
}
