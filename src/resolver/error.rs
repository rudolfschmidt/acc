use std::fmt;
use std::sync::Arc;

use crate::error;

#[derive(Debug, Clone)]
pub struct ResolveError {
    pub file: Arc<str>,
    pub line: usize,
    pub message: String,
}

impl ResolveError {
    pub fn new(file: Arc<str>, line: usize, message: impl Into<String>) -> Self {
        Self {
            file,
            line,
            message: message.into(),
        }
    }
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        error::render_at_line(f, &self.file, self.line, &self.message)
    }
}

impl std::error::Error for ResolveError {}
