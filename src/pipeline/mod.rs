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
//! 2. **realizer**  — per-transaction fx gain/loss where the implied rate
//!    diverges from the market rate. Skipped when capital-tracking is
//!    active: the lotter then owns the spread (split per disposal at the
//!    realization rate), so a per-transaction realizer would double-book.
//! 3. **lotter**    — realized capital gains via FIFO lots (long & short).
//! 4. **translator** — currency translation adjustment (CTA) for
//!    pass-through accounts. Runs over *every* such account, including
//!    lot-tracked ones: the lotter pins its realized legs to the booked
//!    rate, so those legs already sum to zero under conversion and CTA
//!    sees no drift there — no exclusion needed, no double-count.
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
pub fn enrich(journal: &mut Journal, target: Option<&str>) {
    crate::expander::expand(&mut journal.transactions, &journal.auto_rules);

    // The lotter and the realizer are mutually exclusive: when capital
    // accounts are declared, the lotter owns the fx spread.
    let capital_active =
        journal.capital_gain.is_some() && journal.capital_loss.is_some();

    if let (Some(t), Some(gain), Some(loss)) =
        (target, journal.fx_gain.as_deref(), journal.fx_loss.as_deref())
    {
        if !capital_active {
            crate::realizer::realize(
                &mut journal.transactions,
                t,
                &journal.prices,
                &journal.precisions,
                gain,
                loss,
            );
        }
    }

    if let (Some(cg), Some(cl)) =
        (journal.capital_gain.as_deref(), journal.capital_loss.as_deref())
    {
        let accounts = crate::lotter::CapitalAccounts {
            capital_gain: cg,
            capital_loss: cl,
            fx_gain: journal.fx_gain.as_deref(),
            fx_loss: journal.fx_loss.as_deref(),
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
}
