//! Cost-basis migration: the business rules for capital gains when a
//! commodity travels between accounts before it is disposed of. Each
//! test states one accounting requirement and derives its expected
//! number from first principles — proceeds minus original cost — rather
//! than from what the code happens to produce.
//!
//! The requirements under test:
//!
//! - **R1** A transfer is never a taxable event: no capital, no
//!   slippage, no matter how far the market moved while holding.
//! - **R2** Cost basis and acquisition date survive any route: the same
//!   buy/sell pair realizes the same capital whether the asset moved
//!   zero, one or three times in between.
//! - **R3** FIFO applies across accounts: a partial transfer carries the
//!   *oldest* lots away; source and destination each realize against
//!   the lots they actually hold.
//! - **R4** A fee-split transfer conserves the basis: the slivers realize
//!   independently, and together they sum to proceeds − original cost.
//! - **R5** A gap in history fabricates nothing: quantity arriving
//!   without a basis realizes no gain, ever.
//! - **R6** A round trip (A → B → A) is a no-op for basis and date.
//! - **R7** Losses after a transfer route to the loss account.
//! - **R8** Foreign cash is a position too: its FX basis migrates
//!   between banks and realizes on the final disposal.
//! - **R9** A chain with an intermediate swap realizes each holding
//!   period once: total capital equals the sum of the per-leg market
//!   moves, which equals the economic result of the whole chain.
//! - **R10** Same-day chains (buy, withdraw, land — one date) keep their
//!   file order and the basis still arrives before the sale.
//! - **R11** With capital + slippage + CTA all declared, a fully closed
//!   position satisfies the economic identity: capital + slippage =
//!   proceeds − cost, CTA nets to zero, and the converted journal
//!   balances.
//! - **R12** Per-account FIFO is *arrival* order: lots queue on the
//!   receiving account in the order they land there, each keeping its
//!   own acquisition date.
//! - **R13** Quantity arriving on an account with an open (uncovered)
//!   short neutralizes the short silently and never jams the queue:
//!   later disposals realize against the remainder's real basis.

mod common;

use acc::decimal::Decimal;
use acc::parser::located::Located;
use acc::parser::transaction::Transaction;

const ACCOUNTS: &str = "\
    [capital]\ngain = \"income:cap\"\nloss = \"expenses:cap\"\n\
    [slippage]\ngain = \"income:slippage\"\nloss = \"expenses:slippage\"\n\
    [cta]\ngain = \"income:cta\"\nloss = \"expenses:cta\"\n\
    [holding]\ngain = \"income:holding\"\nloss = \"expenses:holding\"\n";

fn dec(s: &str) -> Decimal {
    Decimal::parse(s).unwrap()
}

fn capital(txs: &[Located<Transaction>], commodity: &str) -> Decimal {
    common::balance(txs, "income:cap", commodity)
        + common::balance(txs, "expenses:cap", commodity)
}

fn slippage(txs: &[Located<Transaction>], commodity: &str) -> Decimal {
    common::balance(txs, "income:slippage", commodity)
        + common::balance(txs, "expenses:slippage", commodity)
}

fn cta(txs: &[Located<Transaction>], commodity: &str) -> Decimal {
    common::balance(txs, "income:cta", commodity)
        + common::balance(txs, "expenses:cta", commodity)
}

// ─── R1: a transfer is never a taxable event ──────────────────────────

#[test]
fn transfer_never_realizes_despite_market_move() {
    // Bought at 100, market at 130 on the transfer day. Moving the coin
    // is not a disposal — nothing may land on capital or slippage. The
    // rate drift across the hop is the translator's CTA business.
    let src = "\
         P 2024-01-01 BTC GBP 100\n\
         P 2024-02-01 BTC GBP 130\n\
         2024-01-01 * buy\n\
         \tassets:exchange     1 BTC\n\
         \tassets:cash      -100 GBP\n\
         2024-02-01 * withdraw\n\
         \tassets:exchange    -1 BTC\n\
         \tassets:wallet       1 BTC\n";
    let txs = common::run_x_cfg(ACCOUNTS, src, "GBP");
    assert_eq!(capital(&txs, "GBP"), Decimal::zero(), "a transfer realizes nothing");
    assert_eq!(slippage(&txs, "GBP"), Decimal::zero(), "a transfer has no execution spread");
    assert!(
        common::balance(&txs, "", "GBP").is_display_zero(2),
        "the converted journal still balances"
    );
}

// ─── R2: basis survives any route ─────────────────────────────────────

#[test]
fn same_capital_with_and_without_intermediate_hops() {
    // The identical buy/sell pair, once traded on one account and once
    // routed exchange → transit → wallet → vault before the sale. The
    // realized capital must not depend on the route.
    let direct = "\
         P 2024-01-01 BTC GBP 100\n\
         P 2024-06-01 BTC GBP 250\n\
         2024-01-01 * buy\n\
         \tassets:a          1 BTC\n\
         \tassets:cash    -100 GBP\n\
         2024-06-01 * sell\n\
         \tassets:a         -1 BTC\n\
         \tassets:cash     250 GBP\n";
    let routed = "\
         P 2024-01-01 BTC GBP 100\n\
         P 2024-02-01 BTC GBP 140\n\
         P 2024-03-01 BTC GBP 90\n\
         P 2024-04-01 BTC GBP 180\n\
         P 2024-06-01 BTC GBP 250\n\
         2024-01-01 * buy\n\
         \tassets:a          1 BTC\n\
         \tassets:cash    -100 GBP\n\
         2024-02-01 * hop one\n\
         \tassets:a         -1 BTC\n\
         \tassets:transit    1 BTC\n\
         2024-03-01 * hop two\n\
         \tassets:transit   -1 BTC\n\
         \tassets:wallet     1 BTC\n\
         2024-04-01 * hop three\n\
         \tassets:wallet    -1 BTC\n\
         \tassets:vault      1 BTC\n\
         2024-06-01 * sell\n\
         \tassets:vault     -1 BTC\n\
         \tassets:cash     250 GBP\n";
    let d = common::run_x_cfg(ACCOUNTS, direct, "GBP");
    let r = common::run_x_cfg(ACCOUNTS, routed, "GBP");
    assert_eq!(capital(&d, "GBP"), dec("-150"), "market move 100 → 250");
    assert_eq!(
        capital(&r, "GBP"),
        capital(&d, "GBP"),
        "three hops must not change the realized capital"
    );
    assert!(
        common::balance(&r, "", "GBP").is_display_zero(2),
        "the routed journal balances too"
    );
}

// ─── R3: FIFO across accounts — oldest lot migrates first ─────────────

#[test]
fn partial_transfer_carries_the_oldest_lot() {
    // Two lots on the exchange: 1 BTC @ 100 (January), 1 BTC @ 200
    // (February). One coin moves to the wallet — FIFO says the January
    // lot goes. Selling both sides at 300 must therefore realize 100 on
    // the exchange (against the remaining February lot) and 200 on the
    // wallet (against the migrated January lot).
    let src = "\
         2024-01-01 * buy january\n\
         \tassets:exchange     1 BTC @ 100 USD\n\
         \tassets:cash      -100 USD\n\
         2024-02-01 * buy february\n\
         \tassets:exchange     1 BTC @ 200 USD\n\
         \tassets:cash      -200 USD\n\
         2024-03-01 * move one coin\n\
         \tassets:exchange    -1 BTC\n\
         \tassets:wallet       1 BTC\n\
         2024-06-01 * sell on the exchange\n\
         \tassets:exchange    -1 BTC @ 300 USD\n\
         \tassets:cash       300 USD\n\
         2024-07-01 * sell from the wallet\n\
         \tassets:wallet      -1 BTC @ 300 USD\n\
         \tassets:cash       300 USD\n";
    let txs = common::run_native_cfg(ACCOUNTS, src);
    // Total: proceeds 600 − cost 300 = 300 realized.
    assert_eq!(capital(&txs, "USD"), dec("-300"));
    // The wallet leg realized against the JANUARY basis — its rewritten
    // disposal leg carries the January acquisition date.
    let wallet_lot_dates: Vec<String> = txs
        .iter()
        .flat_map(|lt| lt.value.postings.iter())
        .filter(|lp| lp.value.account == "assets:wallet" && lp.value.lot_date.is_some())
        .map(|lp| lp.value.lot_date.unwrap().to_string())
        .collect();
    assert_eq!(wallet_lot_dates, vec!["2024-01-01"], "the oldest lot migrated");
}

// ─── R4: a fee-split transfer conserves the basis ─────────────────────

#[test]
fn fee_split_slivers_realize_to_the_full_economic_gain() {
    // 1 BTC bought for 100. The withdrawal sheds it as 0.1 fee + 0.9
    // transferred. Both slivers are later sold at 300. Whatever the
    // split, the books must realize proceeds − original cost:
    // (0.1 + 0.9) × 300 − 100 = 200.
    let src = "\
         2024-01-01 * buy\n\
         \tassets:exchange       1 BTC @ 100 USD\n\
         \tassets:cash        -100 USD\n\
         2024-02-01 * withdraw\n\
         \tassets:exchange      -1 BTC\n\
         \tassets:fees         0.1 BTC\n\
         \tassets:wallet       0.9 BTC\n\
         2024-06-01 * sell the coins\n\
         \tassets:wallet      -0.9 BTC @ 300 USD\n\
         \tassets:cash        270 USD\n\
         2024-07-01 * recover the fee sliver\n\
         \tassets:fees        -0.1 BTC @ 300 USD\n\
         \tassets:cash         30 USD\n";
    let txs = common::run_native_cfg(ACCOUNTS, src);
    assert_eq!(capital(&txs, "USD"), dec("-200"), "300 proceeds − 100 cost");
}

// ─── R5: gaps fabricate nothing ───────────────────────────────────────

#[test]
fn quantity_without_basis_realizes_no_gain() {
    // 1 BTC arrives by transfer from an account with no recorded lots
    // (history starts mid-way); 1 BTC is bought properly at 200. Selling
    // both may only realize on the bought coin: (300 − 200) × 1 = 100.
    // The gap coin's gain is unknowable and must stay unbooked.
    let src = "\
         2024-02-01 * transfer from before recorded history\n\
         \tassets:elsewhere   -1 BTC\n\
         \tassets:wallet       1 BTC\n\
         2024-03-01 * buy\n\
         \tassets:wallet       1 BTC @ 200 USD\n\
         \tassets:cash      -200 USD\n\
         2024-06-01 * sell everything\n\
         \tassets:wallet      -2 BTC @ 300 USD\n\
         \tassets:cash       600 USD\n";
    let txs = common::run_native_cfg(ACCOUNTS, src);
    assert_eq!(capital(&txs, "USD"), dec("-100"), "only the bought coin realizes");
}

// ─── R6: a round trip is a no-op ──────────────────────────────────────

#[test]
fn round_trip_keeps_basis_and_acquisition_date() {
    // A → B → back to A. The sale on A must realize exactly as if the
    // coin never travelled, and the disposal leg must still carry the
    // original January acquisition date.
    let src = "\
         2024-01-01 * buy\n\
         \tassets:a            1 BTC @ 100 USD\n\
         \tassets:cash      -100 USD\n\
         2024-02-01 * away\n\
         \tassets:a           -1 BTC\n\
         \tassets:b            1 BTC\n\
         2024-03-01 * and back\n\
         \tassets:b           -1 BTC\n\
         \tassets:a            1 BTC\n\
         2024-06-01 * sell\n\
         \tassets:a           -1 BTC @ 150 USD\n\
         \tassets:cash       150 USD\n";
    let txs = common::run_native_cfg(ACCOUNTS, src);
    assert_eq!(capital(&txs, "USD"), dec("-50"));
    let dates: Vec<String> = txs
        .iter()
        .flat_map(|lt| lt.value.postings.iter())
        .filter(|lp| lp.value.lot_date.is_some())
        .map(|lp| lp.value.lot_date.unwrap().to_string())
        .collect();
    assert_eq!(dates, vec!["2024-01-01"], "the acquisition date survived the trip");
}

// ─── R7: losses route to the loss account ─────────────────────────────

#[test]
fn loss_after_transfer_routes_to_loss_account() {
    // Bought at 200, sold from the wallet at 120: an 80 loss, booked on
    // the expense side, nothing on income.
    let src = "\
         2024-01-01 * buy\n\
         \tassets:exchange     1 BTC @ 200 USD\n\
         \tassets:cash      -200 USD\n\
         2024-02-01 * withdraw\n\
         \tassets:exchange    -1 BTC\n\
         \tassets:wallet       1 BTC\n\
         2024-06-01 * sell at a loss\n\
         \tassets:wallet      -1 BTC @ 120 USD\n\
         \tassets:cash       120 USD\n";
    let txs = common::run_native_cfg(ACCOUNTS, src);
    assert_eq!(common::balance(&txs, "expenses:cap", "USD"), dec("80"));
    assert_eq!(common::balance(&txs, "income:cap", "USD"), Decimal::zero());
}

// ─── R8: foreign cash migrates like any position ──────────────────────

#[test]
fn fx_basis_migrates_between_banks() {
    // 1000 USD bought at 0.80 GBP on bank A, wired to bank B, spent at
    // 0.90: the currency gain 1000 × (0.90 − 0.80) = 100 realizes on the
    // final disposal, against the ORIGINAL acquisition rate.
    let src = "\
         P 2024-01-01 USD GBP 0.80\n\
         P 2024-02-01 USD GBP 0.85\n\
         P 2024-06-01 USD GBP 0.90\n\
         2024-01-01 * buy dollars\n\
         \tassets:bank-a    1000 USD\n\
         \tassets:cash      -800 GBP\n\
         2024-02-01 * wire to the other bank\n\
         \tassets:bank-a   -1000 USD\n\
         \tassets:bank-b    1000 USD\n\
         2024-06-01 * spend\n\
         \tassets:bank-b   -1000 USD\n\
         \texpenses:rent     900 GBP\n";
    let txs = common::run_x_cfg(ACCOUNTS, src, "GBP");
    assert_eq!(capital(&txs, "GBP"), dec("-100"), "1000 × (0.90 − 0.80)");
    assert!(
        common::balance(&txs, "", "GBP").is_display_zero(2),
        "the converted journal balances"
    );
}

// ─── R9: a chain with an intermediate swap ────────────────────────────

#[test]
fn swap_chain_realizes_each_holding_period_once() {
    // XMR bought at 100, moved, swapped into BTC when XMR is 150 (the
    // swap prices exactly at market, so no slippage), the BTC moved
    // again and sold when BTC went 10000 → 12000. Two holding periods,
    // each realized once:
    //   XMR leg: 10 × (150 − 100)          =  500
    //   BTC leg: 0.15 × (12000 − 10000)    =  300
    // Total capital 800 — exactly the economic result of the chain
    // (1800 proceeds − 1000 invested).
    let src = "\
         P 2024-01-01 XMR GBP 100\n\
         P 2024-01-15 XMR GBP 120\n\
         P 2024-02-01 XMR GBP 150\n\
         P 2024-02-01 BTC GBP 10000\n\
         P 2024-02-15 BTC GBP 11000\n\
         P 2024-03-01 BTC GBP 12000\n\
         2024-01-01 * buy xmr\n\
         \tassets:xmr-exchange     10 XMR\n\
         \tassets:cash          -1000 GBP\n\
         2024-01-15 * withdraw xmr\n\
         \tassets:xmr-exchange    -10 XMR\n\
         \tassets:xmr-wallet       10 XMR\n\
         2024-02-01 * swap xmr for btc\n\
         \tassets:xmr-wallet      -10 XMR\n\
         \tassets:btc-wallet     0.15 BTC\n\
         2024-02-15 * move btc\n\
         \tassets:btc-wallet    -0.15 BTC\n\
         \tassets:btc-vault      0.15 BTC\n\
         2024-03-01 * sell btc\n\
         \tassets:btc-vault     -0.15 BTC\n\
         \tassets:cash           1800 GBP\n";
    let txs = common::run_x_cfg(ACCOUNTS, src, "GBP");
    assert_eq!(capital(&txs, "GBP"), dec("-800"), "500 XMR move + 300 BTC move");
    assert_eq!(slippage(&txs, "GBP"), Decimal::zero(), "every trade priced at market");
    assert!(
        common::balance(&txs, "", "GBP").is_display_zero(2),
        "the converted journal balances"
    );
}

// ─── R10: same-day chains keep their file order ───────────────────────

#[test]
fn same_day_buy_withdraw_land_then_later_sale() {
    // Buy, withdrawal and landing all dated the same day, in file order
    // — the real-world exchange pattern. The basis must still arrive on
    // the wallet before the later sale realizes against it.
    let src = "\
         2024-01-01 * buy\n\
         \tassets:exchange     1 BTC @ 100 USD\n\
         \tassets:cash      -100 USD\n\
         2024-01-01 * withdraw\n\
         \tassets:exchange    -1 BTC\n\
         \tassets:transit      1 BTC\n\
         2024-01-01 * land\n\
         \tassets:transit     -1 BTC\n\
         \tassets:wallet       1 BTC\n\
         2024-06-01 * sell\n\
         \tassets:wallet      -1 BTC @ 180 USD\n\
         \tassets:cash       180 USD\n";
    let txs = common::run_native_cfg(ACCOUNTS, src);
    assert_eq!(capital(&txs, "USD"), dec("-80"));
}

// ─── R11: the economic identity of a closed position ──────────────────

#[test]
fn closed_position_satisfies_the_economic_identity() {
    // Off-market buy (paid 29000 when market said 30000), a transfer
    // while the market stood at 34000, off-market sell (got 51000 when
    // market said 50000). The three roles split the 22000 economic
    // profit without overlap:
    //   capital  = market move        = 50000 − 30000 = 20000
    //   slippage = both spreads       =  1000 + 1000  =  2000
    //   cta      = transfer drift     nets to zero on a closed position
    // and capital + slippage = proceeds − cost = 51000 − 29000.
    let src = "\
         P 2024-01-01 BTC GBP 30000\n\
         P 2024-02-01 BTC GBP 34000\n\
         P 2024-06-01 BTC GBP 50000\n\
         2024-01-01 * buy below market\n\
         \tassets:exchange       1 BTC\n\
         \tassets:cash      -29000 GBP\n\
         2024-02-01 * withdraw\n\
         \tassets:exchange      -1 BTC\n\
         \tassets:wallet         1 BTC\n\
         2024-06-01 * sell above market\n\
         \tassets:wallet        -1 BTC\n\
         \tassets:cash       51000 GBP\n";
    let txs = common::run_x_cfg(ACCOUNTS, src, "GBP");
    assert_eq!(capital(&txs, "GBP"), dec("-20000"), "the market move");
    assert_eq!(slippage(&txs, "GBP"), dec("-2000"), "both execution spreads");
    assert!(
        cta(&txs, "GBP").is_display_zero(2),
        "CTA pairs cancel on a closed position"
    );
    assert!(
        common::balance(&txs, "assets:exchange", "GBP").is_display_zero(2),
        "the exchange account ends flat"
    );
    assert!(
        common::balance(&txs, "assets:wallet", "GBP").is_display_zero(2),
        "the wallet ends flat"
    );
    assert!(
        common::balance(&txs, "", "GBP").is_display_zero(2),
        "the converted journal balances"
    );
}

// ─── R12: per-account FIFO is arrival order ───────────────────────────

#[test]
fn receiving_account_queues_lots_in_arrival_order() {
    // The YOUNG lot (February @ 300) lands on the wallet first, the OLD
    // lot (January @ 100) second. Per-account FIFO consumes in arrival
    // order, so a 1-coin sale at 400 realizes against the February
    // basis: 400 − 300 = 100 — while the leg still carries February's
    // acquisition date.
    let src = "\
         2024-01-01 * buy on exchange a\n\
         \tassets:exchange-a    1 BTC @ 100 USD\n\
         \tassets:cash       -100 USD\n\
         2024-02-01 * buy on exchange b\n\
         \tassets:exchange-b    1 BTC @ 300 USD\n\
         \tassets:cash       -300 USD\n\
         2024-03-01 * b lands first\n\
         \tassets:exchange-b   -1 BTC\n\
         \tassets:wallet        1 BTC\n\
         2024-04-01 * a lands second\n\
         \tassets:exchange-a   -1 BTC\n\
         \tassets:wallet        1 BTC\n\
         2024-06-01 * sell one coin\n\
         \tassets:wallet       -1 BTC @ 400 USD\n\
         \tassets:cash        400 USD\n";
    let txs = common::run_native_cfg(ACCOUNTS, src);
    assert_eq!(capital(&txs, "USD"), dec("-100"), "against the first-arrived basis");
    let dates: Vec<String> = txs
        .iter()
        .flat_map(|lt| lt.value.postings.iter())
        .filter(|lp| lp.value.account == "assets:wallet" && lp.value.lot_date.is_some())
        .map(|lp| lp.value.lot_date.unwrap().to_string())
        .collect();
    assert_eq!(dates, vec!["2024-02-01"], "the first arrival was the February lot");
}

// ─── R14: a bundled transfer is still a transfer ──────────────────────

#[test]
fn bundled_multi_commodity_transfer_realizes_nothing() {
    // Both coins move in ONE transaction — every commodity nets to
    // zero, nothing was exchanged for anything. The move must realize
    // no capital (a transfer is not a taxable event) and must not reset
    // the holding period: the whole gain realizes at the SALE, against
    // the original basis — BTC 1 × (200 − 100) + ETH 5 × (20 − 10) = 150.
    let src = "\
         P 2024-01-01 BTC GBP 100\n\
         P 2024-01-01 ETH GBP 10\n\
         P 2024-03-01 BTC GBP 150\n\
         P 2024-03-01 ETH GBP 12\n\
         P 2024-06-01 BTC GBP 200\n\
         P 2024-06-01 ETH GBP 20\n\
         2024-01-01 * buy both\n\
         \tassets:exchange      1 BTC\n\
         \tassets:exchange      5 ETH\n\
         \tassets:cash       -150 GBP\n\
         2024-03-01 * move both\n\
         \tassets:exchange     -1 BTC\n\
         \tassets:exchange     -5 ETH\n\
         \tassets:wallet        1 BTC\n\
         \tassets:wallet        5 ETH\n\
         2024-06-01 * sell both\n\
         \tassets:wallet       -1 BTC\n\
         \tassets:wallet       -5 ETH\n\
         \tassets:cash        300 GBP\n";
    let txs = common::run_x_cfg(ACCOUNTS, src, "GBP");
    assert_eq!(capital(&txs, "GBP"), dec("-150"), "the full original-basis gain");
    // No capital posting may sit inside the move transaction itself.
    let move_tx = txs
        .iter()
        .find(|lt| lt.value.description == "move both")
        .expect("move transaction present");
    assert!(
        !move_tx
            .value
            .postings
            .iter()
            .any(|lp| lp.value.account.contains(":cap")),
        "a transfer must not be a realization event"
    );
    assert!(
        common::balance(&txs, "", "GBP").is_display_zero(2),
        "the converted journal balances"
    );
}

// ─── R15: real-world fractional quantities stay exact ─────────────────
//
// (A `@@` cost memo on a both-legs-explicit transfer is unrepresentable:
// the annotation unbalances the transaction and the booker rejects it.
// With a blank counter-leg the filler turns the memo into a one-sided
// money leg — the transaction stops being a transfer at the journal
// level, before the lotter ever sees it. So there is no lotter rule to
// test; keeping cost memos off transfers is journal hygiene.)

#[test]
fn twelve_decimal_fee_split_realizes_exactly() {
    // The real exchange-withdrawal shape: 1.864 XMR bought, the network
    // keeps 0.001950000000, the wallet receives 1.862050000000, all at
    // 12 decimals. Selling the wallet's share must realize exactly
    // 1.862050000000 × (300 − 230) = 130.3435 — carried exact so the
    // transaction balances (the 2-decimal target precision is display
    // rounding only), with no residue lost to the split.
    let src = "\
         P 2025-09-08 XMR GBP 230\n\
         P 2025-09-10 XMR GBP 235\n\
         P 2025-11-20 XMR GBP 300\n\
         2025-09-08 * buy\n\
         \tassets:exchange   XMR1.864000000000\n\
         \tassets:cash       -428.72 GBP\n\
         2025-09-10 * withdraw\n\
         \tassets:exchange  XMR-1.864000000000\n\
         \tassets:fees       XMR0.001950000000\n\
         \tassets:wallet     XMR1.862050000000\n\
         2025-11-20 * sell\n\
         \tassets:wallet    XMR-1.862050000000\n\
         \tassets:cash        558.62 GBP\n";
    let txs = common::run_x_cfg(ACCOUNTS, src, "GBP");
    assert_eq!(capital(&txs, "GBP"), dec("-130.3435"), "1.862050000000 × 70, exact");
    assert!(
        common::balance(&txs, "", "GBP").is_display_zero(2),
        "the converted journal balances"
    );
}

// ─── R17: an open position after a transfer is unrealized only ────────

#[test]
fn open_position_after_transfer_shows_no_realized_capital() {
    // Bought at 100, moved, never sold; the market now says 180. The 80
    // appreciation is UNREALIZED: capital must stay empty, and under
    // `--unrealized` the whole 80 shows up across the holding/CTA
    // valuation accounts instead.
    let src = "\
         P 2024-01-01 BTC GBP 100\n\
         P 2024-02-01 BTC GBP 130\n\
         P 2024-06-01 BTC GBP 180\n\
         2024-01-01 * buy\n\
         \tassets:exchange     1 BTC\n\
         \tassets:cash      -100 GBP\n\
         2024-02-01 * withdraw\n\
         \tassets:exchange    -1 BTC\n\
         \tassets:wallet       1 BTC\n";
    let txs = common::run_unrealized_cfg(ACCOUNTS, src, "GBP");
    assert_eq!(capital(&txs, "GBP"), Decimal::zero(), "nothing was disposed");
    let holding = common::balance(&txs, "income:holding", "GBP")
        + common::balance(&txs, "expenses:holding", "GBP");
    assert_eq!(
        holding + cta(&txs, "GBP"),
        dec("-80"),
        "the full appreciation is unrealized valuation, not capital"
    );
    assert!(
        common::balance(&txs, "", "GBP").is_display_zero(2),
        "the converted journal balances"
    );
}

// ─── R18: one lot, split between a sale and a migration ───────────────

#[test]
fn lot_split_between_local_sale_and_migrated_remainder() {
    // ONE lot of 1.5 BTC @ 100. One coin is sold on the exchange at
    // 140; the remaining 0.5 wanders to the wallet and is sold there at
    // 200. Each fraction realizes against its share of the SAME basis:
    //   exchange: 1   × (140 − 100) =  40
    //   wallet:   0.5 × (200 − 100) =  50
    // and together they equal total proceeds − total cost:
    //   (140 + 100) − 150 = 90.
    let src = "\
         2024-01-01 * buy\n\
         \tassets:exchange   1.5 BTC @ 100 USD\n\
         \tassets:cash      -150 USD\n\
         2024-02-01 * sell one coin locally\n\
         \tassets:exchange    -1 BTC @ 140 USD\n\
         \tassets:cash       140 USD\n\
         2024-03-01 * move the remainder\n\
         \tassets:exchange  -0.5 BTC\n\
         \tassets:wallet     0.5 BTC\n\
         2024-06-01 * sell the remainder for fiat\n\
         \tassets:wallet    -0.5 BTC @ 200 USD\n\
         \tassets:cash       100 USD\n";
    let txs = common::run_native_cfg(ACCOUNTS, src);
    assert_eq!(capital(&txs, "USD"), dec("-90"), "40 on the exchange + 50 on the wallet");
    // Both fractions carry the original January acquisition date.
    let dates: Vec<String> = txs
        .iter()
        .flat_map(|lt| lt.value.postings.iter())
        .filter(|lp| lp.value.lot_date.is_some())
        .map(|lp| lp.value.lot_date.unwrap().to_string())
        .collect();
    assert_eq!(dates, vec!["2024-01-01", "2024-01-01"]);
}

#[test]
fn migration_first_then_sales_assign_fifo_at_each_moment() {
    // Two lots (1 @ 100 January, 0.5 @ 200 February). The transfer runs
    // FIRST and — FIFO at that moment — carries half of the January
    // lot. The exchange sale of 1 BTC then consumes the January rest
    // plus the February lot; the wallet sale consumes its migrated
    // January half. Assignment follows the timeline, the total still
    // equals proceeds − cost: 450 − 200 = 250.
    //   exchange: 0.5 × (300 − 100) + 0.5 × (300 − 200) = 150
    //   wallet:   0.5 × (300 − 100)                     = 100
    let src = "\
         2024-01-01 * buy january\n\
         \tassets:exchange     1 BTC @ 100 USD\n\
         \tassets:cash      -100 USD\n\
         2024-02-01 * buy february\n\
         \tassets:exchange   0.5 BTC @ 200 USD\n\
         \tassets:cash      -100 USD\n\
         2024-03-01 * move half a coin\n\
         \tassets:exchange  -0.5 BTC\n\
         \tassets:wallet     0.5 BTC\n\
         2024-04-01 * sell on the exchange\n\
         \tassets:exchange    -1 BTC @ 300 USD\n\
         \tassets:cash       300 USD\n\
         2024-05-01 * sell from the wallet\n\
         \tassets:wallet    -0.5 BTC @ 300 USD\n\
         \tassets:cash       150 USD\n";
    let txs = common::run_native_cfg(ACCOUNTS, src);
    assert_eq!(capital(&txs, "USD"), dec("-250"), "proceeds 450 − cost 200");
    // The exchange sale split across two lots → two `{}` legs there,
    // one on the wallet.
    let exchange_legs = txs
        .iter()
        .flat_map(|lt| lt.value.postings.iter())
        .filter(|lp| lp.value.account == "assets:exchange" && lp.value.lot_cost.is_some())
        .count();
    assert_eq!(exchange_legs, 2, "January rest + February lot");
}

// ─── R19: a user-written `{}` on an acquisition declares the basis ────

#[test]
fn declared_zero_basis_on_a_fork_realizes_full_proceeds() {
    // A chain-fork credit booked at a declared `{0 USD}` basis against
    // equity. Selling later must realize the FULL proceeds as capital
    // (cost was zero), and the disposal leg must carry the declared
    // basis and the fork date.
    let src = "\
         2019-02-13 * fork credit\n\
         \tassets:exchange     4 ABC {0 USD}\n\
         \tequity              0 USD\n\
         2020-01-20 * sell\n\
         \tassets:exchange    -4 ABC @ 250 USD\n\
         \tassets:cash      1000 USD\n";
    let txs = common::run_native_cfg(ACCOUNTS, src);
    assert_eq!(capital(&txs, "USD"), dec("-1000"), "full proceeds — the basis is zero");
    let leg = txs
        .iter()
        .flat_map(|lt| lt.value.postings.iter())
        .find(|lp| {
            lp.value.account == "assets:exchange"
                && lp.value.lot_cost.is_some()
                && lp.value.amount.as_ref().is_some_and(|a| a.value.is_negative())
        })
        .expect("annotated disposal leg");
    assert_eq!(leg.value.lot_cost.as_ref().unwrap().amount.value, Decimal::zero());
    assert_eq!(leg.value.lot_date.unwrap().to_string(), "2019-02-13");
}

#[test]
fn declared_basis_migrates_and_realizes_on_the_other_account() {
    // The full real-world chain: fork credit at `{0}` on the exchange,
    // withdrawal through a transit (with a fee sliver), sale for fiat
    // on the second exchange. The declared zero basis must travel the
    // whole route — the sale realizes its entire proceeds.
    let src = "\
         2019-02-13 * fork credit\n\
         \tassets:exchange-a     4.24625600 ABC {0 USD}\n\
         \tequity                0 USD\n\
         2020-01-18 * withdraw\n\
         \tassets:exchange-a    -4.24625600 ABC\n\
         \tassets:transit        4.24625373 ABC\n\
         \tassets:fees           0.00000227 ABC\n\
         2020-01-18 * land\n\
         \tassets:transit       -4.24625373 ABC\n\
         \tassets:exchange-b     4.24625373 ABC\n\
         2020-01-20 * sell part\n\
         \tassets:exchange-b    -4 ABC @ 250 USD\n\
         \tassets:cash        1000 USD\n";
    let txs = common::run_native_cfg(ACCOUNTS, src);
    assert_eq!(capital(&txs, "USD"), dec("-1000"), "zero basis arrived on exchange-b");
}

#[test]
fn declared_basis_beats_the_market_rate_under_x() {
    // Acquired with a declared `{50 GBP}` basis while the market said
    // 100; sold at 180. The user's declaration wins: capital is
    // 180 − 50 = 130, not the 80 market move — the whole economic gain
    // against the declared cost. (The realizer skips user-lot
    // transactions, so nothing double-books the 50 discount.)
    let src = "\
         P 2024-01-01 BTC GBP 100\n\
         P 2024-06-01 BTC GBP 180\n\
         2024-01-01 * acquire below market\n\
         \tassets:btc          1 BTC {50 GBP}\n\
         \tassets:cash       -50 GBP\n\
         2024-06-01 * sell\n\
         \tassets:btc         -1 BTC\n\
         \tassets:cash       180 GBP\n";
    let txs = common::run_x_cfg(ACCOUNTS, src, "GBP");
    assert_eq!(capital(&txs, "GBP"), dec("-130"), "proceeds 180 − declared cost 50");
    assert!(
        common::balance(&txs, "", "GBP").is_display_zero(2),
        "the converted journal balances"
    );
}

// ─── R20: same-day chains resolve regardless of file order ────────────

#[test]
fn same_day_chain_with_arrival_booked_before_departure() {
    // The real multi-file shape: the receiving side's file loads FIRST,
    // so the landing (transit → wallet) is processed before the
    // departure (exchange → transit) of the SAME day. The day-settling
    // pass must still deliver the basis to the wallet: the later sale
    // realizes the full 4 × (250 − 0) = 1000 — this is the exact
    // fork-coin chain that stayed basis-less before.
    let src = "\
         2019-02-13 * fork credit\n\
         \tassets:exchange-a     4.24625600 ABC {0 USD}\n\
         \tequity                0 USD\n\
         2020-01-18 * land (booked first in file order)\n\
         \tassets:transit       -4.24625373 ABC\n\
         \tassets:exchange-b     4.24625373 ABC\n\
         2020-01-18 * withdraw (booked second)\n\
         \tassets:exchange-a    -4.24625600 ABC\n\
         \tassets:transit        4.24625373 ABC\n\
         \tassets:fees           0.00000227 ABC\n\
         2020-01-20 * sell part\n\
         \tassets:exchange-b    -4 ABC @ 250 USD\n\
         \tassets:cash        1000 USD\n";
    let txs = common::run_native_cfg(ACCOUNTS, src);
    assert_eq!(capital(&txs, "USD"), dec("-1000"), "the basis caught up within the day");
}

#[test]
fn day_settling_never_moves_a_leg_twice() {
    // The transit already holds 2 BTC @ 100 from an earlier day. On day
    // two — arrival booked first — the wallet's 4-BTC landing drains
    // those 2 in the first pass; the departure then delivers 4 @ 200
    // into the transit, and the settling pass may hand the wallet only
    // the still-owed 2, not another 4. Selling everything must realize
    // exactly proceeds − cost: (4 + 2) × 300 − (200 + 800) = 800 —
    // any double-move would break this conservation.
    let src = "\
         2024-01-01 * buy into transit\n\
         \tassets:transit      2 BTC @ 100 USD\n\
         \tassets:cash      -200 USD\n\
         2024-01-01 * buy on the exchange\n\
         \tassets:exchange     4 BTC @ 200 USD\n\
         \tassets:cash      -800 USD\n\
         2024-02-01 * land (booked first)\n\
         \tassets:transit     -4 BTC\n\
         \tassets:wallet       4 BTC\n\
         2024-02-01 * depart (booked second)\n\
         \tassets:exchange    -4 BTC\n\
         \tassets:transit      4 BTC\n\
         2024-06-01 * sell the wallet\n\
         \tassets:wallet      -4 BTC @ 300 USD\n\
         \tassets:cash      1200 USD\n\
         2024-07-01 * sell the transit rest\n\
         \tassets:transit     -2 BTC @ 300 USD\n\
         \tassets:cash       600 USD\n";
    let txs = common::run_native_cfg(ACCOUNTS, src);
    assert_eq!(capital(&txs, "USD"), dec("-800"), "1800 proceeds − 1000 cost, conserved");
}

// ─── R13: arrivals neutralize an uncovered short ──────────────────────

#[test]
fn arrival_on_a_shorted_account_unjams_the_queue() {
    // Bank A sells 50 USD it has no recorded basis for (an uncovered
    // disposal — under -X against the target this opens a short, booked
    // gain-free). Then 100 USD with a 0.80 basis arrives by wire: 50
    // neutralize the short silently, 50 keep their basis. The final
    // sale must realize against that real basis — 50 × (0.90 − 0.80) =
    // 5 — not stall behind a jammed short.
    let src = "\
         P 2024-01-01 USD GBP 0.80\n\
         P 2024-02-01 USD GBP 0.85\n\
         P 2024-03-01 USD GBP 0.85\n\
         P 2024-04-01 USD GBP 0.90\n\
         2024-01-01 * bank b buys dollars\n\
         \tassets:bank-b     100 USD\n\
         \tassets:cash       -80 GBP\n\
         2024-02-01 * bank a sells dollars it never recorded buying\n\
         \tassets:bank-a     -50 USD\n\
         \tassets:cash      42.50 GBP\n\
         2024-03-01 * wire b to a\n\
         \tassets:bank-b    -100 USD\n\
         \tassets:bank-a     100 USD\n\
         2024-04-01 * bank a sells with a real basis\n\
         \tassets:bank-a     -50 USD\n\
         \tassets:cash        45 GBP\n";
    let txs = common::run_x_cfg(ACCOUNTS, src, "GBP");
    assert_eq!(capital(&txs, "GBP"), dec("-5"), "50 × (0.90 − 0.80) on the final sale");
}
