use std::path::PathBuf;

use super::entry::Entry;
use super::located::Located;

/// The output of parsing one journal file. `path` identifies the source;
/// `entries` carries the flat stream of records in parse order, each
/// wrapped with its source line.
#[derive(Debug, Clone)]
pub struct File {
    pub path: PathBuf,
    pub entries: Vec<Located<Entry>>,
}
