//! `codes` command — list every distinct transaction code used in
//! the journal, one per line, sorted. Runs after the filter phase.

use std::collections::BTreeSet;

use crate::loader::Journal;

pub fn run(journal: &Journal) {
    for code in journal
        .transactions
        .iter()
        .filter_map(|tx| tx.value.code.as_deref())
        .collect::<BTreeSet<&str>>()
    {
        println!("{}", code);
    }
}
