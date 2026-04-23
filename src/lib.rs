//! Pipeline:
//!
//! ```text
//! parser → resolver → booker → indexer → loader → realizer → filter → rebalancer → sorter → commands
//!                                                 (realizer/rebalancer only with -x)
//! ```

pub mod booker;
pub mod commands;
pub mod date;
pub mod decimal;
pub mod error;
pub mod filter;
pub mod indexer;
pub mod loader;
pub mod parser;
pub mod realizer;
pub mod rebalancer;
pub mod resolver;
pub mod sorter;

pub(crate) mod i256;

pub use error::Error;
pub use loader::{load, Journal, LoadError};
