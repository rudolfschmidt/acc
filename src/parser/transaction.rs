use crate::date::Date;

use super::comment::Comment;
use super::located::Located;
use super::posting::Posting;

/// A journal transaction: a dated event with two or more postings that
/// must balance to zero.
///
/// `date` is a typed `Date` (u32 days since 1970-01-01), parsed from
/// the source. Sort and comparison are integer ops; formatting back to
/// `YYYY-MM-DD` goes through `Display`.
#[derive(Debug, Clone)]
pub struct Transaction {
    pub date: Date,
    pub state: State,
    pub code: Option<String>,
    pub description: String,
    pub postings: Vec<Located<Posting>>,
    pub comments: Vec<Located<Comment>>,
}

/// Transaction clear state, matching Ledger's `*`, `!`, and bare forms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Cleared,
    Uncleared,
    Pending,
}
