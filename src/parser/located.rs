use std::sync::Arc;

/// Pairs any parsed value with its source location. Keeping the
/// location outside the value's struct avoids duplicating the field
/// across every record type and allows the same wrapper to be used
/// for sub-records like individual postings inside a transaction.
///
/// `file` is an `Arc<str>` so that interning a single path across
/// 100k+ postings costs one pointer clone per record instead of a
/// heap allocation.
#[derive(Debug, Clone)]
pub struct Located<T> {
    pub file: Arc<str>,
    pub line: usize,
    pub value: T,
}
