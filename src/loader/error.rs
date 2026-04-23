use std::fmt;

use crate::booker::BookError;
use crate::parser::ParseError;
use crate::resolver::ResolveError;
use crate::error;

/// Unified error type for the full load pipeline. Wraps the
/// individual phase errors plus any I/O trouble during file reads.
#[derive(Debug)]
pub enum LoadError {
    Io { path: String, source: std::io::Error },
    Parse { path: String, source: ParseError },
    Resolve(ResolveError),
    Book(BookError),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Io { path, source } => write!(f, "reading `{}`: {}", path, source),
            LoadError::Parse { path, source } => {
                error::render_at_line(f, path, source.line, &source.message)
            }
            LoadError::Resolve(e) => write!(f, "{}", e),
            LoadError::Book(e) => write!(f, "{}", e),
        }
    }
}

impl std::error::Error for LoadError {}

impl From<ResolveError> for LoadError {
    fn from(e: ResolveError) -> Self {
        LoadError::Resolve(e)
    }
}

impl From<BookError> for LoadError {
    fn from(e: BookError) -> Self {
        LoadError::Book(e)
    }
}
