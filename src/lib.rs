//! Pipeline:
//!
//! ```text
//! load:    parser → resolver → booker → indexer ───────────► Journal
//! enrich:  expander → realizer → lotter → translator         (journal-global)
//! report:  filter → rebalancer → sorter → commands           (CLI-driven)
//! ```
//!
//! [`load`] builds a validated `Journal`. [`pipeline::enrich`] runs the
//! journal-global phases that must see every transaction (realizer and
//! translator only under `-X`; lotter needs capital accounts). The report
//! phases are driven by CLI flags and run last, per command.

pub mod booker;
pub mod commands;
pub mod date;
pub mod decimal;
pub mod error;
pub mod expander;
pub mod filter;
pub mod indexer;
pub mod loader;
pub mod lotter;
pub mod parser;
pub mod pipeline;
pub mod realizer;
pub mod rebalancer;
pub mod resolver;
pub mod revaluator;
pub mod sorter;
pub mod translator;

pub(crate) mod i256;

pub use error::Error;
pub use loader::{load, Journal, LoadError};

/// Extension treated as a journal file when walking a directory.
/// Only `.ledger` is picked up by the recursive walk, so non-journal
/// files that happen to live in the tree (`.txt` meter readings, notes,
/// data dumps) are left alone. Explicit `-f FILE` arguments bypass this
/// filter entirely — when the user names a path directly, acc reads it
/// whatever the extension.
pub const JOURNAL_EXTENSIONS: &[&str] = &["ledger"];

/// True if `path` has the recognised journal extension.
/// Used by directory walkers; never used to reject explicit `-f` paths.
pub fn is_journal_file(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| JOURNAL_EXTENSIONS.contains(&ext))
}
