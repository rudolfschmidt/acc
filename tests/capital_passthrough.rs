//! Capital-gains + CTA integration: the cross-phase behaviour that the
//! lotter (FIFO lots, long & short), the translator (currency
//! translation adjustment) and the rebalancer produce *together* under
//! `-X TARGET`. Each test runs the full post-load pipeline via
//! `common::run_x` / `run_native` and asserts on converted balances.
//!
//! The invariant that ties these together: a pass-through account (native
//! sum zero) must end at zero under `-X`, with its holding-period drift
//! named — as a realized capital gain where a genuine trade occurred, as
//! CTA where only a same-commodity transfer did, and never double-booked.

mod common;

use acc::decimal::Decimal;

/// Capital + CTA account declarations shared by the scenarios.
const ACCOUNTS: &str = "\
    account in:cap\n    capital gain\n\
    account ex:cap\n    capital loss\n\
    account in:cta\n    cta gain\n\
    account ex:cta\n    cta loss\n";

fn dec(s: &str) -> Decimal {
    Decimal::parse(s).unwrap()
}

// ─── Short lots ──────────────────────────────────────────────────────

#[test]
fn short_against_target_realizes_and_account_zeroes() {
    // Spend 100 USD before owning any (out at 1.05 €/USD), buy it back at
    // 1.03: a short, closed €2 below where it opened → €2 capital gain.
    // The pass-through account nets to zero.
    let src = format!(
        "{ACCOUNTS}\
         2024-01-01 * spend\n\
         \tcp:x          -100 USD\n\
         \texpenses:dev   105 EUR\n\
         2024-06-01 * cover\n\
         \tassets:bank   -103 EUR\n\
         \tcp:x           100 USD\n"
    );
    let txs = common::run_x(&src, "EUR");
    assert_eq!(common::balance(&txs, "cp:x", "EUR"), Decimal::zero());
    assert_eq!(common::balance(&txs, "in:cap", "EUR"), dec("-2"));
}

#[test]
fn short_then_long_sequence_books_both_gains() {
    // short: 100 USD out @1.05, back @1.03 → €2. long: 100 USD in @1.06,
    // out @1.08 → €2. Total €4 capital gain; account flat.
    let src = format!(
        "{ACCOUNTS}\
         2024-01-01 * spend1\n\
         \tcp:x          -100 USD\n\
         \texpenses:dev   105 EUR\n\
         2024-06-01 * buy1\n\
         \tassets:bank   -103 EUR\n\
         \tcp:x           100 USD\n\
         2024-09-01 * buy2\n\
         \tassets:bank   -106 EUR\n\
         \tcp:x           100 USD\n\
         2024-12-01 * spend2\n\
         \tcp:x          -100 USD\n\
         \texpenses:dev   108 EUR\n"
    );
    let txs = common::run_x(&src, "EUR");
    assert_eq!(common::balance(&txs, "cp:x", "EUR"), Decimal::zero());
    assert_eq!(common::balance(&txs, "in:cap", "EUR"), dec("-4"));
}

#[test]
fn short_closed_above_open_routes_to_loss() {
    // Spend 100 USD out at 1.05 €/USD, buy it back dearer at 1.07: the
    // short closed €2 *above* where it opened → a €2 loss, routed to the
    // expense capital account, not income.
    let src = format!(
        "{ACCOUNTS}\
         2024-01-01 * spend\n\
         \tcp:x          -100 USD\n\
         \texpenses:dev   105 EUR\n\
         2024-06-01 * cover\n\
         \tassets:bank   -107 EUR\n\
         \tcp:x           100 USD\n"
    );
    let txs = common::run_x(&src, "EUR");
    assert_eq!(common::balance(&txs, "cp:x", "EUR"), Decimal::zero());
    assert_eq!(common::balance(&txs, "ex:cap", "EUR"), dec("2"));
    assert_eq!(common::balance(&txs, "in:cap", "EUR"), Decimal::zero());
}

// ─── Pure 2-commodity pass-through: capital only, no CTA ──────────────

#[test]
fn pure_trade_passthrough_is_all_capital_no_cta() {
    // A foreign commodity flows in and out entirely against the target
    // money (every leg is 2-commodity). The lotter realizes the whole
    // holding-period drift as a capital gain and pins both legs, so CTA
    // sees no drift and books nothing — no double-count.
    let src = format!(
        "{ACCOUNTS}\
         2024-05-02 * out\n\
         \tcp:partner   -1000000 INR\n\
         \texpenses:dev    12000 EUR\n\
         2024-05-03 * back\n\
         \tassets:wise    -11900 EUR\n\
         \tcp:partner    1000000 INR\n"
    );
    let txs = common::run_x(&src, "EUR");
    assert_eq!(common::balance(&txs, "cp:partner", "EUR"), Decimal::zero());
    // The full €100 drift lands on capital (12000 out, 11900 back in).
    assert_eq!(common::balance(&txs, "in:cap", "EUR"), dec("-100"));
    // CTA must NOT also book it.
    assert_eq!(common::balance(&txs, "in:cta", "EUR"), Decimal::zero());
    assert_eq!(common::balance(&txs, "ex:cta", "EUR"), Decimal::zero());
}

// ─── Pure same-commodity transfer: CTA only, no capital ───────────────

#[test]
fn pure_transfer_passthrough_is_all_cta_no_capital() {
    // The same commodity passes through with no exchange (every leg is
    // single-commodity USD↔USD). There is no trade to realize, but the
    // holding-period rate moved 0.83 → 0.85, so CTA books the €2 drift
    // and the account zeroes.
    let src = format!(
        "{ACCOUNTS}\
         P 2024-01-01 USD EUR 0.83\n\
         P 2024-06-01 USD EUR 0.85\n\
         2024-01-01 * fund\n\
         \tassets:src   -100 USD\n\
         \tcp:t          100 USD\n\
         2024-06-01 * out\n\
         \tcp:t         -100 USD\n\
         \texpenses:s    100 USD\n"
    );
    let txs = common::run_x(&src, "EUR");
    assert_eq!(common::balance(&txs, "cp:t", "EUR"), Decimal::zero());
    // No 2-commodity trade → no capital gain.
    assert_eq!(common::balance(&txs, "in:cap", "EUR"), Decimal::zero());
    assert_eq!(common::balance(&txs, "ex:cap", "EUR"), Decimal::zero());
    // Drift went up (held USD while it strengthened) → CTA non-zero.
    let cta = common::balance(&txs, "in:cta", "EUR")
        + common::balance(&txs, "ex:cta", "EUR");
    assert_ne!(cta, Decimal::zero());
}

// ─── Mixed pass-through: capital for the trade, CTA for the transfer ──

#[test]
fn mixed_passthrough_splits_capital_and_cta_and_zeroes() {
    // USD enters via a same-commodity transfer (no lot, valued at
    // market), leaves against EUR (a trade → short), is bought back
    // against EUR (closes the short → capital), and finally leaves via a
    // same-commodity transfer again. The 2-commodity legs realize as
    // capital; the single-commodity legs drift and are absorbed by CTA.
    // Neither alone zeroes the account — together they must.
    let src = format!(
        "{ACCOUNTS}\
         P 2018-01-01 USD EUR 0.8326\n\
         P 2018-06-01 USD EUR 0.862\n\
         P 2018-09-01 USD EUR 0.851\n\
         2018-01-01 * fund (single-commodity)\n\
         \tassets:pp     -32.88 USD\n\
         \tcp:nc          32.88 USD\n\
         2018-06-01 * spend (2-commodity → short)\n\
         \tcp:nc         -32.88 USD\n\
         \texpenses:dom   28.35 EUR\n\
         2018-09-01 * buyback (2-commodity → closes short)\n\
         \tassets:bank   -27.50 EUR\n\
         \tcp:nc          32.88 USD\n\
         2018-09-01 * spend2 (single-commodity)\n\
         \tcp:nc         -32.88 USD\n\
         \texpenses:dom   32.88 USD\n"
    );
    let txs = common::run_x(&src, "EUR");
    // The account must net to zero.
    assert_eq!(common::balance(&txs, "cp:nc", "EUR"), Decimal::zero());
    // Both mechanisms contributed.
    let capital = common::balance(&txs, "in:cap", "EUR")
        + common::balance(&txs, "ex:cap", "EUR");
    let cta = common::balance(&txs, "in:cta", "EUR")
        + common::balance(&txs, "ex:cta", "EUR");
    assert_ne!(capital, Decimal::zero(), "the trade legs realize capital");
    assert_ne!(cta, Decimal::zero(), "the transfer legs drift to CTA");
}

// ─── Asset ↔ asset: no spurious short, no double-count ────────────────

#[test]
fn asset_vs_asset_oversell_books_no_short() {
    // Under -X EUR, a BTC sold against USD (counter ≠ target) with no
    // prior lot must NOT open a short — that disposal is the other side
    // of a normal trade. Buy 1 BTC, sell 2: only the covered lot
    // realizes; the uncovered BTC stays a plain leg, no extra gain.
    let src = format!(
        "{ACCOUNTS}\
         P 2024-01-01 BTC EUR 90\n\
         P 2024-06-01 BTC EUR 180\n\
         P 2024-01-01 USD EUR 0.9\n\
         2024-01-01 * buy\n\
         \tassets:btc    1 BTC @ 100 USD\n\
         \tassets:cash  -100 USD\n\
         2024-06-01 * sell\n\
         \tassets:btc   -2 BTC @ 200 USD\n\
         \tassets:cash   400 USD\n"
    );
    let txs = common::run_x(&src, "EUR");
    // Only one covered lot's gain — the uncovered BTC opens no short, so
    // the capital figure reflects a single 1-BTC disposal, not two.
    // (1 BTC: market 90→180 = €90 on the covered lot.)
    let capital = common::balance(&txs, "in:cap", "EUR")
        + common::balance(&txs, "ex:cap", "EUR");
    assert_eq!(capital, dec("-90"));
}

// ─── Native mode (no -X): trade gain straight from the books ──────────

#[test]
fn native_capital_gain_no_conversion() {
    // BTC bought for 30000 USD, sold for 50000 USD, no -X: the native
    // realized gain is 20000 USD on the capital account.
    let src = format!(
        "{ACCOUNTS}\
         2024-01-01 * buy\n\
         \tassets:btc    1 BTC @ 30000 USD\n\
         \tassets:cash  -30000 USD\n\
         2024-06-01 * sell\n\
         \tassets:btc   -1 BTC @ 50000 USD\n\
         \tassets:cash   50000 USD\n"
    );
    let txs = common::run_native(&src);
    assert_eq!(common::balance(&txs, "in:cap", "USD"), dec("-20000"));
}

// ─── Whole-journal invariant: every converted tx still balances ───────

#[test]
fn mixed_passthrough_whole_journal_balances_in_target() {
    // After the lotter pins legs, injects gains, and CTA releases drift,
    // every transaction must still sum to zero in the target — so the
    // whole journal's EUR balance is exactly zero (no value created or
    // lost by the capital/CTA machinery).
    let src = format!(
        "{ACCOUNTS}\
         P 2018-01-01 USD EUR 0.8326\n\
         P 2018-06-01 USD EUR 0.862\n\
         P 2018-09-01 USD EUR 0.851\n\
         2018-01-01 * fund\n\
         \tassets:pp     -32.88 USD\n\
         \tcp:nc          32.88 USD\n\
         2018-06-01 * spend\n\
         \tcp:nc         -32.88 USD\n\
         \texpenses:dom   28.35 EUR\n\
         2018-09-01 * buyback\n\
         \tassets:bank   -27.50 EUR\n\
         \tcp:nc          32.88 USD\n\
         2018-09-01 * spend2\n\
         \tcp:nc         -32.88 USD\n\
         \texpenses:dom   32.88 USD\n"
    );
    let txs = common::run_x(&src, "EUR");
    // Sum of every EUR posting across the whole journal — zero to display
    // precision (full-precision conversion leaves a sub-cent rounding
    // tail, which the printer absorbs; bal/reg never show it).
    assert!(
        common::balance(&txs, "", "EUR").is_display_zero(2),
        "whole-journal EUR balance must be zero to display precision"
    );
}
