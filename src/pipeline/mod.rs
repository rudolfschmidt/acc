//! Pipeline orchestration — the journal-global enrichment phases that
//! run after [`load`](crate::load) and before any filtering, conversion
//! or sorting.
//!
//! These phases must see the *whole* journal, un-filtered: the lotter
//! tracks lots FIFO across every transaction, and the translator
//! identifies pass-through accounts by their journal-wide native sum.
//! Filtering first would change those sums and corrupt the result. They
//! are also where the subtle cross-phase rules live, so they belong
//! together in one tested place rather than inlined in the CLI:
//!
//! 1. **expander**  — apply `= /pattern/` auto-rules; later phases then
//!    see the fully expanded journal.
//! 2. **realizer**  — per-transaction slippage gain/loss: the trade-day
//!    execution spread, where each leg's market value diverges from the
//!    others. Runs on every multi-commodity transaction (buy and sell).
//! 3. **lotter**    — realized capital gains via FIFO lots: the holding-
//!    period market move of each disposed lot. Composes with the
//!    realizer — it books capital (the market move), the realizer books
//!    slippage (the execution spread), so neither double-books the other.
//! 4. **translator** — currency translation adjustment (CTA) for
//!    pass-through accounts. Lot-tracked assets enter at their market
//!    value and leave at that same value as the `{}` cost basis, so they
//!    net to zero under conversion — CTA sees no drift there.
//! 5. **revaluator** — opt-in (`-X` with `--unrealized`) mark-to-market of
//!    every open foreign balance to the latest available rate, booking the
//!    unrealized revaluation to the `holding` accounts. Off by default, so the
//!    historical (realized) view is untouched.
//!
//! `rebalance`, `filter` and `sort` are deliberately *not* here. They are
//! driven by CLI flags (pattern, date range, `-X` target, sort keys) and
//! run afterwards against the already-enriched journal. Conversion is
//! per-posting and local, so it can run after filtering; these four
//! cannot.

use crate::loader::Journal;

/// Run the journal-global enrichment phases in order, mutating
/// `journal.transactions` in place.
///
/// `target` is the resolved `-X` commodity, or `None` in native mode.
/// Phases that only make sense under conversion (realizer, translator)
/// are skipped when it is `None`; the lotter always runs when capital
/// accounts are declared (it realizes in the booked commodity either
/// way).
pub fn enrich(journal: &mut Journal, target: Option<&str>, unrealized: bool) {
    crate::expander::expand(&mut journal.transactions, &journal.auto_rules);

    // The realizer books the per-trade execution spread (slippage) on every
    // multi-commodity transaction; the lotter books the holding-period
    // market move (capital) at each disposal. They compose: the lotter's
    // `{cost}` shifts the disposal leg by the market move, which its own
    // capital posting offsets, leaving the realizer's slippage intact.
    if let (Some(t), Some(gain), Some(loss)) =
        (target, journal.slippage_gain.as_deref(), journal.slippage_loss.as_deref())
    {
        crate::realizer::realize(
            &mut journal.transactions,
            t,
            &journal.prices,
            &journal.precisions,
            gain,
            loss,
        );
    }

    if let (Some(cg), Some(cl)) =
        (journal.capital_gain.as_deref(), journal.capital_loss.as_deref())
    {
        let accounts = crate::lotter::CapitalAccounts {
            capital_gain: cg,
            capital_loss: cl,
        };
        crate::lotter::realize_capital(
            &mut journal.transactions,
            &accounts,
            target,
            &journal.prices,
            &journal.precisions,
        );
    }

    if let (Some(t), Some(cta_gain), Some(cta_loss)) =
        (target, journal.cta_gain.as_deref(), journal.cta_loss.as_deref())
    {
        let precision = journal.precisions.get(t).copied().unwrap_or(2);
        crate::translator::translate(
            &mut journal.transactions,
            t,
            &journal.prices,
            cta_gain,
            cta_loss,
            precision,
        );
    }

    // `--unrealized`: mark open foreign positions to the latest available
    // rate, booking the unrealized revaluation to the `holding` accounts.
    // Opt-in and separate from the historical default, so the realized
    // (default) view stays unchanged.
    if let (Some(t), true, Some(rg), Some(rl)) = (
        target,
        unrealized,
        journal.holding_gain.as_deref(),
        journal.holding_loss.as_deref(),
    ) {
        let precision = journal.precisions.get(t).copied().unwrap_or(2);
        crate::revaluator::revaluate(
            &mut journal.transactions,
            t,
            &journal.prices,
            &crate::revaluator::RevaluationAccounts { gain: rg, loss: rl },
            precision,
        );
    }
}
