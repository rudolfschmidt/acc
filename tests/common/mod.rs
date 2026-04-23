//! Shared test helpers. Not a test file itself — the `mod.rs` layout
//! keeps Cargo from running it as an independent test binary.
//!
//! Each integration test file (`tests/pipeline.rs`, etc.) is compiled
//! as its own binary and pulls in this module via `mod common;`. That
//! means each binary gets its own copy, and any helper a given binary
//! doesn't happen to use is flagged as dead code. The blanket
//! `#[allow(dead_code)]` silences those per-binary warnings without
//! suppressing real dead code elsewhere.
#![allow(dead_code)]

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// Write `contents` to a fresh `.ledger` file in a per-test temp dir
/// and return the path. The caller owns cleanup via `TempJournal::drop`.
pub struct TempJournal {
    pub path: PathBuf,
    dir: PathBuf,
}

impl TempJournal {
    pub fn new(contents: &str) -> Self {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "acc-it-{}-{}",
            std::process::id(),
            id
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("journal.ledger");
        let mut f = std::fs::File::create(&path).expect("create journal file");
        f.write_all(contents.as_bytes()).expect("write journal");
        TempJournal { path, dir }
    }
}

impl Drop for TempJournal {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Load a journal from an inline source string. Panics on any error —
/// use the error-path tests for failure cases.
pub fn load(src: &str) -> acc::Journal {
    let tmp = TempJournal::new(src);
    acc::load(&[&tmp.path]).expect("load should succeed")
}
