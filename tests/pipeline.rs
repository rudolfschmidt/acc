//! End-to-end pipeline tests: `acc::load()` from a file through the
//! full parser → resolver → booker → indexer chain. Validates the
//! shape of the resulting `Journal` — commodity aliases resolved,
//! transactions date-sorted, assertions verified, amounts inferred.

mod common;

use acc::decimal::Decimal;

#[test]
fn loads_a_balanced_single_transaction() {
    let j = common::load(
        "2024-06-15 * Coffee\n\
         \texpenses:food   5 USD\n\
         \tassets:cash   -5 USD\n",
    );
    assert_eq!(j.transactions.len(), 1);
    let tx = &j.transactions[0].value;
    assert_eq!(tx.description, "Coffee");
    assert_eq!(tx.postings.len(), 2);
}

#[test]
fn infers_missing_amount_single_commodity() {
    let j = common::load(
        "2024-06-15 * X\n\
         \texpenses:food   5 USD\n\
         \tassets:cash\n",
    );
    let inferred = &j.transactions[0].value.postings[1].value;
    let amt = inferred.amount.as_ref().expect("should be inferred");
    assert_eq!(amt.value, Decimal::from(-5));
}

#[test]
fn expands_missing_amount_into_per_commodity_postings() {
    // Multi-commodity trailing posting with no amount: acc splits it
    // into one posting per commodity, matching ledger-cli.
    let j = common::load(
        "2024-06-15 * writedown\n\
         \tassets:foo   FOO -1000\n\
         \tassets:usd   $-50\n\
         \texpenses:wo\n",
    );
    let tx = &j.transactions[0].value;
    assert_eq!(tx.postings.len(), 4); // original 3 + one extra from expansion
    let tail: Vec<&str> = tx.postings[2..]
        .iter()
        .map(|lp| lp.value.account.as_str())
        .collect();
    assert_eq!(tail, vec!["expenses:wo", "expenses:wo"]);
}

#[test]
fn resolves_commodity_aliases() {
    let j = common::load(
        "commodity USD\n    alias $\n\
         2024-06-15 * X\n\
         \tassets:usd   $100\n\
         \tequity       $-100\n",
    );
    let amt = j.transactions[0].value.postings[0].value.amount.as_ref().unwrap();
    assert_eq!(amt.commodity, "USD");
}

#[test]
fn sorts_transactions_by_date() {
    let j = common::load(
        "2024-06-20 * Later\n\
         \ta  1 USD\n\
         \tb  -1 USD\n\
         2024-06-10 * Earlier\n\
         \ta  2 USD\n\
         \tb  -2 USD\n",
    );
    assert_eq!(j.transactions[0].value.description, "Earlier");
    assert_eq!(j.transactions[1].value.description, "Later");
}

#[test]
fn fx_accounts_extracted_into_journal() {
    let j = common::load(
        "account Equity:FxGain\n    fx gain\n\
         account Equity:FxLoss\n    fx loss\n",
    );
    assert_eq!(j.fx_gain.as_deref(), Some("Equity:FxGain"));
    assert_eq!(j.fx_loss.as_deref(), Some("Equity:FxLoss"));
}

#[test]
fn balance_assertion_passes() {
    let j = common::load(
        "2024-01-01 * deposit\n\
         \tassets:bank   100 USD = 100 USD\n\
         \tequity:open  -100 USD\n",
    );
    assert_eq!(j.transactions.len(), 1);
}

#[test]
fn price_directives_land_in_index() {
    let j = common::load(
        "P 2024-06-15 USD EUR 0.92\n\
         P 2024-06-16 USD EUR 0.93\n",
    );
    assert_eq!(j.prices.len(), 2);
    let rate = j.prices.find("USD", "EUR", "2024-06-17").unwrap();
    assert_eq!(rate, Decimal::parse("0.93").unwrap());
}

#[test]
fn precision_directive_overrides_observed() {
    // Source has a 5-decimal amount but `precision 2` pins EUR at 2.
    let j = common::load(
        "commodity EUR\n    precision 2\n\
         2024-06-15 * x\n\
         \tassets:bank   123.12345 EUR\n\
         \tequity:open  -123.12345 EUR\n",
    );
    let prec = j.precisions.get("EUR").copied().unwrap();
    assert_eq!(prec, 2);
}

#[test]
fn observed_precision_wins_without_override() {
    let j = common::load(
        "2024-06-15 * x\n\
         \tassets:btc    0.12345678 BTC\n\
         \tequity:open  -0.12345678 BTC\n",
    );
    let prec = j.precisions.get("BTC").copied().unwrap();
    assert_eq!(prec, 8);
}
