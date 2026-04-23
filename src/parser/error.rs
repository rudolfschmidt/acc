//! Parser error type.
//!
//! A `ParseError` always carries a source location so errors can point at
//! the exact position in the input, independent of whether the caller
//! knows which file the source came from. The filename is added by the
//! orchestrator (main.rs) when it wraps this into the top-level error.

use std::fmt;

#[derive(Debug, Clone)]
pub struct ParseError {
    pub line: usize,
    pub column: usize,
    pub message: String,
}

impl ParseError {
    pub fn new(line: usize, column: usize, message: impl Into<String>) -> Self {
        Self {
            line,
            column,
            message: message.into(),
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "line {}, col {}: {}",
            self.line, self.column, self.message
        )
    }
}

impl std::error::Error for ParseError {}
