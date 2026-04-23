//! `accounts` command — list account names.
//!
//! Two render modes:
//!
//! - **flat** (default): one account name per line, sorted alphabetically,
//!   deduped.
//! - **tree** (`--tree`): hierarchical indented view, built from the
//!   colon-separated account paths.
//!
//! Runs after the filter phase — the journal is already scoped to the
//! user's query, so every posting seen here is a match.

mod flat;
mod tree;

use crate::loader::Journal;

pub fn run(journal: &Journal, tree_mode: bool) {
    if tree_mode {
        tree::print(journal);
    } else {
        flat::print(journal);
    }
}
