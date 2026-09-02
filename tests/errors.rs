//! Error-path integration: journals with problems must fail with the
//! right `LoadError` variant and a message that names the file.

mod common;

use common::TempJournal;

fn load_err(src: &str) -> acc::LoadError {
    let tmp = TempJournal::new(src);
    acc::load(&[&tmp.path]).expect_err("load should fail")
}

#[test]
fn unbalanced_transaction() {
    let e = load_err(
        "2024-06-15 * X\n\
         \ta  5 USD\n\
         \tb  -3 USD\n",
    );
    assert!(matches!(e, acc::LoadError::Book(_)));
}

#[test]
fn balance_assertion_mismatch() {
    let e = load_err(
        "2024-06-15 * X\n\
         \tassets:bank  5 USD = 99 USD\n\
         \tequity      -5 USD\n",
    );
    assert!(matches!(e, acc::LoadError::Book(_)));
}

#[test]
fn conflicting_commodity_aliases() {
    // Two commodities claiming the same `$` alias — a single config file.
    let tmp = TempJournal::with_config(
        "[[commodity]]\nsymbol = \"USD\"\naliases = [\"$\"]\nprecision = 2\n\
         [[commodity]]\nsymbol = \"GBP\"\naliases = [\"$\"]\nprecision = 2\n",
        "",
    );
    let e = acc::load(&tmp.files()).expect_err("load should fail");
    assert!(matches!(e, acc::LoadError::Resolve(_)));
}

#[test]
fn duplicate_slippage_gain_accounts() {
    // The same role declared to different accounts across two config files —
    // within one file `[slippage] gain` is a single key, so the conflict only
    // arises when two files each set it.
    let a = TempJournal::with_config("[slippage]\ngain = \"Equity:A\"\n", "");
    let b = TempJournal::with_config("[slippage]\ngain = \"Equity:B\"\n", "");
    let mut files = a.files();
    files.extend(b.files());
    let e = acc::load(&files).expect_err("load should fail");
    assert!(matches!(e, acc::LoadError::Resolve(_)));
}

#[test]
fn missing_amount_with_nothing_to_infer() {
    // Both postings have no amount — nothing to infer from.
    let e = load_err("2024-06-15 * X\n\ta\n\tb\n");
    assert!(matches!(e, acc::LoadError::Book(_)));
}

#[test]
fn single_posting_rejected() {
    // A transaction with only one posting can't balance. Resolver
    // requires ≥2 postings per transaction.
    let e = load_err("2024-06-15 * X\n\tassets:cash   5 USD\n");
    assert!(matches!(e, acc::LoadError::Resolve(_)));
}

#[test]
fn invalid_price_rate() {
    let e = load_err("P 2024-06-15 USD GBP not-a-number\n");
    assert!(matches!(e, acc::LoadError::Parse { .. }));
}

#[test]
fn expression_division_by_zero() {
    let e = load_err(
        "2024-06-15 * X\n\
         \tassets:bank   (£100/0)\n\
         \tequity:open  £-100\n",
    );
    assert!(matches!(e, acc::LoadError::Parse { .. }));
}

#[test]
fn expression_with_two_commodities_fails() {
    let e = load_err(
        "2024-06-15 * X\n\
         \tassets:bank   (£100 + $50)\n\
         \tequity:open  £-100\n",
    );
    assert!(matches!(e, acc::LoadError::Parse { .. }));
}
