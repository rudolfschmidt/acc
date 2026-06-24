//! Capital / fx / CTA integration: the cross-phase behaviour the lotter,
//! the realizer and the translator produce *together* under `-X TARGET`.
//! Each test runs the full post-load pipeline via `common::run_x` /
//! `run_native` and asserts on converted balances.
//!
//! The new model splits a position's holding drift three ways, none
//! double-booking the others:
//!
//! - **capital** (lotter) — the disposed commodity's *market move* over
//!   its holding period (`market_sell − market_buy`). The disposal leg
//!   carries `market_buy` as its `{}` cost-basis, so the asset enters at
//!   market and leaves at that cost and the account nets to zero.
//! - **fx** (realizer) — the trade-day *execution spread* (booked rate vs
//!   market), booked on every multi-commodity transaction, buy and sell.
//! - **cta** (translator) — same-commodity *transfer* drift, where a
//!   pass-through holds a commodity across a rate move with no trade.
//!
//! Together they make every converted transaction sum to zero in the
//! target, so a pass-through account ends flat and the whole journal
//! balances.

mod common;

use acc::decimal::Decimal;
use acc::parser::located::Located;
use acc::parser::transaction::Transaction;

/// The full real-mode account set: capital, fx and CTA all declared —
/// the configuration the new model is built for.
const ACCOUNTS: &str = "\
    account in:cap\n    capital gain\n\
    account ex:cap\n    capital loss\n\
    account in:fx\n    fx gain\n\
    account ex:fx\n    fx loss\n\
    account in:cta\n    cta gain\n\
    account ex:cta\n    cta loss\n";

fn dec(s: &str) -> Decimal {
    Decimal::parse(s).unwrap()
}

/// Net capital booked (income negative, expense positive).
fn capital(txs: &[Located<Transaction>]) -> Decimal {
    common::balance(txs, "in:cap", "EUR") + common::balance(txs, "ex:cap", "EUR")
}

/// Net fx booked.
fn fx(txs: &[Located<Transaction>]) -> Decimal {
    common::balance(txs, "in:fx", "EUR") + common::balance(txs, "ex:fx", "EUR")
}

/// Net CTA booked.
fn cta(txs: &[Located<Transaction>]) -> Decimal {
    common::balance(txs, "in:cta", "EUR") + common::balance(txs, "ex:cta", "EUR")
}

// ─── capital = the market move ────────────────────────────────────────

#[test]
fn capital_is_the_market_move() {
    // Buy and sell exactly at market (30000 → 50000): no execution
    // spread, so fx is zero and the whole holding gain is the market
    // move — 20000 capital. The asset account nets to zero.
    let src = format!(
        "{ACCOUNTS}\
         P 2024-01-01 BTC EUR 30000\n\
         P 2024-06-01 BTC EUR 50000\n\
         2024-01-01 * buy\n\
         \tassets:btc       1 BTC\n\
         \tassets:cash  -30000 EUR\n\
         2024-06-01 * sell\n\
         \tassets:btc      -1 BTC\n\
         \tassets:cash   50000 EUR\n"
    );
    let txs = common::run_x(&src, "EUR");
    assert_eq!(capital(&txs), dec("-20000"), "market move 30000→50000");
    assert_eq!(fx(&txs), Decimal::zero(), "traded at market → no fx");
    assert_eq!(common::balance(&txs, "assets:btc", "EUR"), Decimal::zero());
}

#[test]
fn market_loss_routes_to_loss_account() {
    // Market fell 50000 → 30000: a 20000 capital loss, routed to the
    // expense account (positive), nothing on income.
    let src = format!(
        "{ACCOUNTS}\
         P 2024-01-01 BTC EUR 50000\n\
         P 2024-06-01 BTC EUR 30000\n\
         2024-01-01 * buy\n\
         \tassets:btc       1 BTC\n\
         \tassets:cash  -50000 EUR\n\
         2024-06-01 * sell\n\
         \tassets:btc      -1 BTC\n\
         \tassets:cash   30000 EUR\n"
    );
    let txs = common::run_x(&src, "EUR");
    assert_eq!(common::balance(&txs, "ex:cap", "EUR"), dec("20000"));
    assert_eq!(common::balance(&txs, "in:cap", "EUR"), Decimal::zero());
    assert_eq!(common::balance(&txs, "assets:btc", "EUR"), Decimal::zero());
}

// ─── fx = the execution spread, on every trade ────────────────────────

#[test]
fn fx_booked_on_buy_and_sell() {
    // Market 30000 → 50000. Bought 1000 *below* market (paid 29000) and
    // sold 1000 *above* market (got 51000): a 1000 fx gain on each trade
    // → 2000 fx total. The market move (20000) stays on capital — the
    // realizer and the lotter split the 22000 total cleanly.
    let src = format!(
        "{ACCOUNTS}\
         P 2024-01-01 BTC EUR 30000\n\
         P 2024-06-01 BTC EUR 50000\n\
         2024-01-01 * buy\n\
         \tassets:btc       1 BTC\n\
         \tassets:cash  -29000 EUR\n\
         2024-06-01 * sell\n\
         \tassets:btc      -1 BTC\n\
         \tassets:cash   51000 EUR\n"
    );
    let txs = common::run_x(&src, "EUR");
    assert_eq!(fx(&txs), dec("-2000"), "1000 below on buy + 1000 above on sell");
    assert_eq!(capital(&txs), dec("-20000"), "market move untouched by fx");
}

#[test]
fn disposal_account_zeroes_and_journal_balances() {
    // The same off-market round-trip: the asset enters at market and
    // leaves at its `{cost}`, so assets:btc nets to zero — and capital +
    // fx make every converted transaction balance, so the whole journal
    // sums to zero.
    let src = format!(
        "{ACCOUNTS}\
         P 2024-01-01 BTC EUR 30000\n\
         P 2024-06-01 BTC EUR 50000\n\
         2024-01-01 * buy\n\
         \tassets:btc       1 BTC\n\
         \tassets:cash  -29000 EUR\n\
         2024-06-01 * sell\n\
         \tassets:btc      -1 BTC\n\
         \tassets:cash   51000 EUR\n"
    );
    let txs = common::run_x(&src, "EUR");
    assert_eq!(common::balance(&txs, "assets:btc", "EUR"), Decimal::zero());
    assert!(
        common::balance(&txs, "", "EUR").is_display_zero(2),
        "whole-journal EUR balance must be zero"
    );
}

// ─── realizer and lotter compose (no exclusivity) ─────────────────────

#[test]
fn realizer_runs_alongside_capital_tracking() {
    // Capital accounts are declared, yet the realizer still runs: a buy
    // 2000 below market books a 2000 fx gain. The buy only opens a lot,
    // so no capital is realized yet — proving the realizer is no longer
    // skipped when capital tracking is active. No transfer → no CTA.
    let src = format!(
        "{ACCOUNTS}\
         P 2024-06-01 BTC EUR 50000\n\
         2024-06-01 * buy\n\
         \tassets:btc       1 BTC\n\
         \tassets:cash  -48000 EUR\n"
    );
    let txs = common::run_x(&src, "EUR");
    assert_eq!(fx(&txs), dec("-2000"), "realizer books the buy spread");
    assert_eq!(capital(&txs), Decimal::zero(), "an opening buy realizes nothing");
    assert_eq!(cta(&txs), Decimal::zero(), "no same-commodity transfer → no CTA");
}

// ─── same-commodity transfer → CTA, never capital ─────────────────────

#[test]
fn same_commodity_transfer_is_cta_not_capital() {
    // USD passes through with no exchange (every leg is USD↔USD): there is
    // no trade to realize, but the holding-period rate moved 0.83 → 0.85,
    // so the translator books the drift as CTA and the account zeroes.
    // The lotter and realizer (both need ≥2 commodities) stay out of it.
    let src = format!(
        "{ACCOUNTS}\
         P 2024-01-01 USD EUR 0.83\n\
         P 2024-06-01 USD EUR 0.85\n\
         2024-01-01 * fund\n\
         \tassets:src  -100 USD\n\
         \tcp:t         100 USD\n\
         2024-06-01 * out\n\
         \tcp:t        -100 USD\n\
         \texpenses:s   100 USD\n"
    );
    let txs = common::run_x(&src, "EUR");
    assert_eq!(common::balance(&txs, "cp:t", "EUR"), Decimal::zero());
    assert_eq!(capital(&txs), Decimal::zero(), "no trade → no capital");
    assert_eq!(fx(&txs), Decimal::zero(), "no trade → no fx");
    assert_ne!(cta(&txs), Decimal::zero(), "held USD across a rate move → CTA");
}

// ─── native mode (no -X): trade gain straight from the books ──────────

#[test]
fn native_capital_gain_no_conversion() {
    // BTC bought for 30000 USD, sold for 50000 USD, no -X: the native
    // realized gain is 20000 USD on the capital account.
    let src = format!(
        "{ACCOUNTS}\
         2024-01-01 * buy\n\
         \tassets:btc       1 BTC @ 30000 USD\n\
         \tassets:cash  -30000 USD\n\
         2024-06-01 * sell\n\
         \tassets:btc      -1 BTC @ 50000 USD\n\
         \tassets:cash   50000 USD\n"
    );
    let txs = common::run_native(&src);
    assert_eq!(common::balance(&txs, "in:cap", "USD"), dec("-20000"));
}
