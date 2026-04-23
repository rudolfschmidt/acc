//! `accounts` tree mode — hierarchical indented view.
//!
//! Builds an `Account` tree from the already-filtered journal and
//! walks it depth-first, indenting each level by two spaces.

use crate::commands::account::Account;
use crate::loader::Journal;

pub(super) fn print(journal: &Journal) {
    let root = Account::from_transactions(&journal.transactions);
    for child in root.children.values() {
        print_account(0, child);
    }
}

fn print_account(indent: usize, account: &Account) {
    println!("{:indent$}{}", "", account.name, indent = indent);
    for child in account.children.values() {
        print_account(indent + 2, child);
    }
}
