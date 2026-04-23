//! Lot-annotation + expression amount scenarios — the harder parser
//! features that touch booker balance math. Values are synthetic.

mod common;

use acc::decimal::Decimal;

#[test]
fn lot_cost_used_for_balance() {
    // Selling 1 XYZ bought at 10 ABC/unit for 12 ABC/unit, plus a
    // 2 ABC gain posting. Balance sums in ABC using the LOT price
    // (10), not the @-cost (12). -10 + 12 + (-2) = 0 ✓
    let j = common::load(
        "2024-06-15 * sell\n\
         \tassets:xyz   XYZ -1 {ABC 10} @ ABC 12\n\
         \tassets:abc   ABC 12\n\
         \tincome:gain  ABC -2\n",
    );
    assert_eq!(j.transactions.len(), 1);
}

#[test]
fn fixed_lot_cost_same_semantics_as_floating() {
    // `{=COST}` should balance identically to `{COST}` — only display
    // semantics differ.
    let j = common::load(
        "2024-06-15 * sell\n\
         \tassets:xyz   XYZ -1 {=ABC 10} @ ABC 12\n\
         \tassets:abc   ABC 12\n\
         \tincome:gain  ABC -2\n",
    );
    assert_eq!(j.transactions.len(), 1);
}

#[test]
fn lot_date_annotation_is_ignored() {
    let j = common::load(
        "2024-06-15 * x\n\
         \tassets:foo   FOO 100 {=$5} [2020-01-01]\n\
         \tincome:bar   FOO -100\n",
    );
    assert_eq!(j.transactions.len(), 1);
}

#[test]
fn residual_within_display_precision_accepted() {
    // 0.1 XYZ @ $123.45 = $12.345; against $-10 + $-2.35 the residual
    // is -$0.005 — rounded at 2-decimal $ display precision that's
    // zero, so the tx is accepted.
    let j = common::load(
        "2024-06-15 * round\n\
         \tassets:xyz   XYZ 0.1 @ $123.45\n\
         \tassets:cash  $-10.00\n\
         \texpenses:fee $-2.35\n",
    );
    assert_eq!(j.transactions.len(), 1);
}

#[test]
fn expression_amount_evaluates_at_parse_time() {
    // `($1200/12)` should store as $100 per month.
    let j = common::load(
        "2024-06-15 * monthly\n\
         \tincome:fee    ($1200/12)\n\
         \tassets:cash   $-100\n",
    );
    let amt = j.transactions[0].value.postings[0].value.amount.as_ref().unwrap();
    assert_eq!(amt.value, Decimal::from(100));
    assert_eq!(amt.commodity, "$");
}

#[test]
fn expression_precedence_respects_math() {
    // 1 + 2 * 3 = 7, not (1+2)*3 = 9
    let j = common::load(
        "2024-06-15 * x\n\
         \ta   ($1 + 2 * 3)\n\
         \tb   $-7\n",
    );
    let amt = j.transactions[0].value.postings[0].value.amount.as_ref().unwrap();
    assert_eq!(amt.value, Decimal::from(7));
}

#[test]
fn expression_with_parentheses_overrides_precedence() {
    // (($1 + 2) * 3) = 9
    let j = common::load(
        "2024-06-15 * x\n\
         \ta   (($1 + 2) * 3)\n\
         \tb   $-9\n",
    );
    let amt = j.transactions[0].value.postings[0].value.amount.as_ref().unwrap();
    assert_eq!(amt.value, Decimal::from(9));
}

#[test]
fn cost_annotation_stripped_at_balance() {
    // `@@`-total-cost: FOO -100 @@ $50 → effective $-50 against $50.
    let j = common::load(
        "2024-06-15 * exchange\n\
         \tassets:foo    FOO -100 @@ $50.00\n\
         \texpenses:buy  $50.00\n",
    );
    assert_eq!(j.transactions.len(), 1);
}
