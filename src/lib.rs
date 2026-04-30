//! Pipeline:
//!
//! ```text
//! parser → resolver → booker → indexer → loader → realizer → filter → translator → rebalancer → sorter → commands
//!                                                 (realizer/translator/rebalancer only with -x)
//! ```

pub mod booker;
pub mod commands;
pub mod date;
pub mod decimal;
pub mod error;
pub mod expander;
pub mod filter;
pub mod indexer;
pub mod loader;
pub mod parser;
pub mod realizer;
pub mod rebalancer;
pub mod resolver;
pub mod sorter;
pub mod translator;

pub(crate) mod i256;

pub use error::Error;
pub use loader::{load, Journal, LoadError};

/// Extensions treated as journal files when walking a directory.
/// Explicit `-f FILE` arguments bypass this filter — when the user
/// names a path directly, acc reads it whatever the extension.
pub const JOURNAL_EXTENSIONS: &[&str] = &["ledger", "j", "journal", "hledger", "dat", "txt"];

/// True if `path` has one of the recognised journal extensions.
/// Used by directory walkers; never used to reject explicit `-f` paths.
pub fn is_journal_file(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| JOURNAL_EXTENSIONS.contains(&ext))
}
