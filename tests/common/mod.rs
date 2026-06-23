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

use acc::decimal::Decimal;
use acc::parser::located::Located;
use acc::parser::transaction::Transaction;

/// Run the post-load `-X TARGET` pipeline: the journal-global enrichment
/// phases (`pipeline::enrich`) followed by conversion to the target. This
/// drives the *real* orchestration — the same `enrich` `main.rs` calls —
/// so a change to any enrichment phase is exercised here directly.
/// Returns the transformed transactions for target-currency assertions.
pub fn run_x(src: &str, target: &str) -> Vec<Located<Transaction>> {
    let mut j = load(src);
    acc::pipeline::enrich(&mut j, Some(target));
    acc::rebalancer::rebalance(&mut j.transactions, target, &j.prices);
    j.transactions
}

/// Run the enrichment pipeline natively (no `-X`): only the lotter does
/// anything here (realizing in the booked commodity). For native-mode
/// capital-gain assertions.
pub fn run_native(src: &str) -> Vec<Located<Transaction>> {
    let mut j = load(src);
    acc::pipeline::enrich(&mut j, None);
    j.transactions
}

/// Sum the `commodity` amounts of every posting whose account starts with
/// `prefix`. Postings left in another commodity (no rate path) are
/// ignored — pass the target after `-X`, or the native commodity in
/// native mode.
pub fn balance(
    txs: &[Located<Transaction>],
    prefix: &str,
    commodity: &str,
) -> Decimal {
    let mut sum = Decimal::zero();
    for lt in txs {
        for lp in &lt.value.postings {
            if !lp.value.account.starts_with(prefix) {
                continue;
            }
            if let Some(a) = &lp.value.amount {
                if a.commodity == commodity {
                    sum = sum + a.value;
                }
            }
        }
    }
    sum
}
