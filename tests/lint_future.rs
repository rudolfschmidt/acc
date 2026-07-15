//! Integration: `lint` validates the *whole* journal, forward-dated entries
//! included. Reports hide transactions after today by default (the future
//! cutoff), but a linter has to see them — a future-dated miscategorised
//! posting is still a real issue. This guards against `lint` being wired back
//! through the report pipeline, where it would inherit that cutoff and
//! silently skip everything dated past today.
//!
//! Driven through the built binary because the regression lived in dispatch
//! (`lint` must reach `try_standalone`, not the report pipeline), not in
//! `lint::run` itself.

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A throwaway `BASE/<folder>/j.ledger` tree for the `dir-category` check,
/// removed on drop.
struct TempTree {
    base: PathBuf,
    file: PathBuf,
}

impl TempTree {
    fn new(folder: &str, contents: &str) -> Self {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("acc-lint-{}-{}", std::process::id(), id));
        let dir = base.join(folder);
        std::fs::create_dir_all(&dir).expect("create temp tree");
        let file = dir.join("j.ledger");
        std::fs::write(&file, contents).expect("write journal");
        TempTree { base, file }
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

#[test]
fn lint_dir_category_flags_future_dated_posting() {
    // 2099 is well past today, so a report would hide this entry. Its `in:`
    // posting miscategorises — folder `foo-bar` maps to segments `foo:bar`,
    // so `in:wrong` should read `in:foo:bar`. lint must catch it anyway.
    let tree = TempTree::new(
        "foo-bar",
        "2099-01-01 future\n\
         \tin:wrong     -10 EUR\n\
         \tassets:cash   10 EUR\n",
    );

    let out = Command::new(env!("CARGO_BIN_EXE_acc"))
        .arg("-f")
        .arg(&tree.file)
        .args(["lint", "dir-category"])
        .arg("--base")
        .arg(&tree.base)
        .args(["--categories", "^in:"])
        .output()
        .expect("run acc");
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("in:foo:bar") && stdout.contains("in:wrong"),
        "lint must flag the future-dated miscategorised posting, got:\n{stdout}"
    );
}
