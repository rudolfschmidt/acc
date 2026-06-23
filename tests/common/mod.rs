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

/// Run the post-load `-X TARGET` pipeline exactly as `main.rs` orders it:
/// expander → realizer → lotter → translator (CTA) → rebalancer. Returns
/// the transformed transactions so a test can sum account balances in the
/// target currency.
///
/// This MIRRORS the orchestration in `src/main.rs`. The wiring rules it
/// reproduces are the ones the capital/CTA behaviour depends on:
///   * realizer is skipped when capital-tracking is active (both
///     `capital gain`/`loss` declared) — the lotter then owns the spread;
///   * CTA runs with NO exclusion set — a lot-tracked account's pinned
///     legs already sum to zero under conversion, so CTA only books the
///     un-realized (e.g. single-commodity transfer) drift.
/// If `main.rs` changes how these phases compose, update here too.
pub fn run_x(src: &str, target: &str) -> Vec<Located<Transaction>> {
    let mut j = load(src);
    acc::expander::expand(&mut j.transactions, &j.auto_rules);

    let capital_active = j.capital_gain.is_some() && j.capital_loss.is_some();
    if let (Some(g), Some(l)) = (j.fx_gain.as_deref(), j.fx_loss.as_deref()) {
        if !capital_active {
            acc::realizer::realize(
                &mut j.transactions,
                target,
                &j.prices,
                &j.precisions,
                g,
                l,
            );
        }
    }
    if let (Some(cg), Some(cl)) =
        (j.capital_gain.as_deref(), j.capital_loss.as_deref())
    {
        acc::lotter::realize_capital(
            &mut j.transactions,
            cg,
            cl,
            j.fx_gain.as_deref(),
            j.fx_loss.as_deref(),
            Some(target),
            &j.prices,
            &j.precisions,
        );
    }
    if let (Some(cg), Some(cl)) = (j.cta_gain.as_deref(), j.cta_loss.as_deref())
    {
        let prec = j.precisions.get(target).copied().unwrap_or(2);
        acc::translator::translate(
            &mut j.transactions,
            target,
            &j.prices,
            cg,
            cl,
            prec,
            &std::collections::HashSet::new(),
        );
    }
    acc::rebalancer::rebalance(&mut j.transactions, target, &j.prices);
    j.transactions
}

/// Run the lotter natively (no `-X`): expander → lotter only. Returns the
/// transformed transactions for native-mode capital-gain assertions.
pub fn run_native(src: &str) -> Vec<Located<Transaction>> {
    let mut j = load(src);
    acc::expander::expand(&mut j.transactions, &j.auto_rules);
    if let (Some(cg), Some(cl)) =
        (j.capital_gain.as_deref(), j.capital_loss.as_deref())
    {
        acc::lotter::realize_capital(
            &mut j.transactions,
            cg,
            cl,
            j.fx_gain.as_deref(),
            j.fx_loss.as_deref(),
            None,
            &j.prices,
            &j.precisions,
        );
    }
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
