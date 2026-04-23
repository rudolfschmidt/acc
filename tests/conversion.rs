//! Rebalancer + realizer behaviour under `-x TARGET` and `--market`.
//! These tests pin down the semantics of currency conversion.

mod common;

use acc::decimal::Decimal;

#[test]
fn rebalance_uses_txdate_rate() {
    let mut j = common::load(
        "P 2024-01-01 USD EUR 0.90\n\
         P 2024-06-01 USD EUR 0.95\n\
         2024-06-15 * x\n\
         \tassets:usd   100 USD\n\
         \tequity:open -100 USD\n",
    );
    acc::rebalancer::rebalance(&mut j.transactions, "EUR", &j.prices, None);
    let amt = j.transactions[0].value.postings[0].value.amount.as_ref().unwrap();
    // Latest rate ≤ 2024-06-15 is 0.95 → 100 × 0.95 = 95
    assert_eq!(amt.value, Decimal::from(95));
    assert_eq!(amt.commodity, "EUR");
}

#[test]
fn rebalance_market_snapshot_uses_fixed_date() {
    let mut j = common::load(
        "P 2024-01-01 USD EUR 0.90\n\
         P 2024-06-01 USD EUR 0.95\n\
         P 2024-12-01 USD EUR 1.00\n\
         2024-06-15 * x\n\
         \tassets:usd   100 USD\n\
         \tequity:open -100 USD\n",
    );
    // --market 2024-12-15 → use 2024-12-01 rate = 1.00
    acc::rebalancer::rebalance(
        &mut j.transactions,
        "EUR",
        &j.prices,
        Some("2024-12-15"),
    );
    let amt = j.transactions[0].value.postings[0].value.amount.as_ref().unwrap();
    assert_eq!(amt.value, Decimal::from(100));
}

#[test]
fn rebalance_uses_inverse_rate_when_direct_missing() {
    let mut j = common::load(
        "P 2024-06-15 USD EUR 0.5\n\
         2024-06-15 * x\n\
         \ta  100 EUR\n\
         \tb  -100 EUR\n",
    );
    acc::rebalancer::rebalance(&mut j.transactions, "USD", &j.prices, None);
    // No direct EUR→USD stored; inverse of USD→EUR 0.5 = 2.0
    let amt = j.transactions[0].value.postings[0].value.amount.as_ref().unwrap();
    assert_eq!(amt.value, Decimal::from(200));
    assert_eq!(amt.commodity, "USD");
}

#[test]
fn rebalance_multi_hop_via_cross_commodity() {
    let mut j = common::load(
        "P 2024-06-15 USD CHF 0.9\n\
         P 2024-06-15 CHF EUR 1.02\n\
         2024-06-15 * x\n\
         \ta  100 USD\n\
         \tb  -100 USD\n",
    );
    acc::rebalancer::rebalance(&mut j.transactions, "EUR", &j.prices, None);
    let amt = j.transactions[0].value.postings[0].value.amount.as_ref().unwrap();
    // 100 × 0.9 × 1.02 = 91.8
    let expected = Decimal::from(100) * Decimal::parse("0.9").unwrap() * Decimal::parse("1.02").unwrap();
    assert_eq!(amt.value, expected);
}

#[test]
fn missing_rate_leaves_amount_unchanged() {
    let mut j = common::load(
        "2024-06-15 * x\n\
         \ta  100 USD\n\
         \tb  -100 USD\n",
    );
    acc::rebalancer::rebalance(&mut j.transactions, "EUR", &j.prices, None);
    let amt = j.transactions[0].value.postings[0].value.amount.as_ref().unwrap();
    // No P-directive → rebalancer leaves it as USD
    assert_eq!(amt.commodity, "USD");
    assert_eq!(amt.value, Decimal::from(100));
}

#[test]
fn same_commodity_is_noop() {
    let mut j = common::load(
        "2024-06-15 * x\n\
         \ta  100 EUR\n\
         \tb  -100 EUR\n",
    );
    acc::rebalancer::rebalance(&mut j.transactions, "EUR", &j.prices, None);
    let amt = j.transactions[0].value.postings[0].value.amount.as_ref().unwrap();
    assert_eq!(amt.value, Decimal::from(100));
    assert_eq!(amt.commodity, "EUR");
}
