//! Top-level record variants emitted by the parser.

use std::sync::Arc;

use crate::date::Date;
use crate::decimal::Decimal;

use super::located::Located;
use super::posting::Posting;
use super::transaction::Transaction;

/// One raw record from a journal file.
///
/// Block directives (`commodity`, `account`) carry their indented
/// sub-directives inline: a `commodity` block folds its `alias` children
/// into `Commodity.aliases`. An `account` block with an indented role
/// sub-directive (`slippage gain`, `capital loss`, …) is upgraded to a
/// `RoleAccount` carrying that role. The parser accumulates these by
/// mutating the last emitted entry when a new indented line arrives,
/// which lets it remain state-less between lines.
///
/// Alias resolution and price-DB construction happen in the resolve
/// phase after parsing.
#[derive(Debug, Clone)]
pub enum Entry {
    Transaction(Transaction),
    Price(Price),

    /// `commodity SYMBOL` + any number of indented children:
    /// - `alias OTHER` → adds OTHER to `aliases`
    /// - `parity OTHER` → adds OTHER to `parities`: SYMBOL keeps its own
    ///   display (no alias fold) but converts 1:1 to OTHER — a *fixed*
    ///   rate, unlike a dated `P` price. Resolve emits it as a synthetic
    ///   1:1 price so the normal valuation path can chain it.
    /// - `precision N` → sets the display precision override, overriding
    ///   the precision inferred from posting amounts in reports.
    Commodity {
        symbol: String,
        aliases: Vec<String>,
        parities: Vec<String>,
        precision: Option<usize>,
    },

    /// `account NAME` without (or before) a sub-directive. Acts as a
    /// scaffold the parser upgrades to a `RoleAccount` when a role
    /// sub-directive arrives. If no sub-directive follows, the
    /// entry stays and resolve drops it.
    Account(String),

    /// Produced when `account NAME` is followed by an indented role
    /// sub-directive such as `slippage gain`, `cta loss`, or `capital gain`.
    /// `role` is the directive text verbatim (whitespace-collapsed),
    /// `account` the declared name. The role string is the single source
    /// of truth: the resolver indexes these by role, the pipeline phases
    /// look up the ones they consume, and a `$role:slot` posting
    /// reference resolves against the same index — so a new role costs
    /// no parser/resolver change, only a declaration.
    RoleAccount { role: String, account: String },

    /// A display label from a `[[label]]` config table. `account` is the account
    /// it decorates; `skip` is how many leading segments to skip when matching
    /// (`0` = an exact name, `1` = match `<any one segment>:account`, …); and
    /// `balance` / `register` are the label texts shown in each view. Cosmetic
    /// only — never filtered or computed on.
    Label {
        account: String,
        skip: usize,
        balance: String,
        register: String,
    },

    /// A top-level comment line (`;` or `#` at column 0).
    Comment(String),

    /// `~ PERIOD [CADENCE]` block + indented postings — a *periodic*
    /// transaction. The resolver expands it into real, ordinary transactions:
    /// one per cadence occurrence in the year, dated at the occurrence start,
    /// with the written amounts (the period *total*) divided across them
    /// (`monthly` → ÷12, `daily` → ÷days; the last occurrence absorbs the
    /// rounding remainder). `$year`/`$month`/`$day` in an account are filled
    /// from each occurrence's date. Unlike ledger's `~` (an unbounded
    /// budget/forecast that *repeats* the amount), acc's are bounded to the year
    /// and *split* the total; the generated transactions are real — they book,
    /// balance, and auto-fill a bare posting like any hand-written entry.
    Periodic {
        /// The period token — a bare year `YYYY`.
        period: String,
        /// How to spread the block across the year (`~ 2021 monthly …`).
        cadence: Cadence,
        /// Optional description for the generated transactions (`~ 2021 note`);
        /// empty when the header carries only the period (and cadence).
        description: String,
        postings: Vec<Located<Posting>>,
    },
}

/// How a `~` periodic block spreads its amounts across the year. The written
/// amounts are the period *total*; a cadence divides them across its
/// occurrences — the last occurrence absorbs any rounding remainder so the sum
/// stays exact — and dates one transaction at the start of each occurrence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cadence {
    /// No cadence keyword — one transaction, `YYYY-01-01`, the full amount.
    Yearly,
    /// `monthly` — 12 transactions, `YYYY-MM-01`, each the total ÷ 12.
    Monthly,
    /// `daily` — one transaction per day of the year, each the total ÷ days.
    Daily,
}

/// A `P DATE BASE QUOTE RATE` directive. Commodities are stored as
/// `Arc<str>` so the resolver (and downstream phases) can intern them
/// without cloning string buffers. Alias resolution is deferred to the
/// resolve phase.
#[derive(Debug, Clone)]
pub struct Price {
    pub date: Date,
    pub base: Arc<str>,
    pub quote: Arc<str>,
    pub rate: Decimal,
}
