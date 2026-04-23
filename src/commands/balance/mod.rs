//! `balance` command — sum per account per commodity, rendered as
//! a flat list or an indented tree.
//!
//! Runs after the filter phase, so the journal already contains only
//! the postings the user wants summed. The `--empty` flag controls
//! whether branches with a zero total are rendered in tree mode.

mod common;
mod flat;
mod tree;

use crate::loader::Journal;

pub fn run(journal: &Journal, tree_mode: bool, show_empty: bool) {
    if tree_mode {
        tree::print(journal, show_empty);
    } else {
        flat::print(journal);
    }
}
