//! `signal` command — swap-cycle marks over the journal's booked rates.
//!
//! Made for a shell prompt: one dim line with the current price, the
//! buy and sell marks with the distance to each, and what acting
//! right now would bring — a loud SWAP line only when a mark is crossed.
//!
//! The two sides come from the same lot tracking as `lots`. The entry
//! side (`--entry`) is the held asset: the average booked cost of its
//! open lots is the entry rate, their quantity prices the "gain" of
//! selling now. The exit side (`--exit`) is the position holding what a
//! past sale returned: its open lots' booked cost (in the asset) is the
//! quantity given away, the inverted average is the sale rate, and both
//! price the "saved" of buying back now. One threshold is symmetric
//! around the round trip — sell at entry x (1+T/100), buy back at the
//! sale rate / (1+T/100). Multiplicative up, divisive down: a full
//! cycle lands on the original entry price again and multiplies the
//! stack by (1+T/100).
//!
//! The journal-derived rates only change when the journal does, so they
//! are cached under `$XDG_CACHE_HOME/acc/`, keyed on the defining flags
//! and invalidated by the journal files' mtimes — the warm path loads
//! neither the journal nor the price DB. The current price is read
//! fresh from the pair's price file on every run, and an interactive
//! run tops that file up with one `update` when today's rate is
//! missing. `--bar` never does: a status bar renders what is on disk
//! and marks a price that is not from today as stale, so it can run
//! every few seconds without ever going online. A positional price
//! argument plays a scenario through without touching the price DB.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use colored::Colorize;

use crate::date::Date;
use crate::decimal::Decimal;
use crate::error::Error;
use crate::filter::PatternMatcher;
use crate::loader::Journal;

/// The price the run evaluates at, with what the price DB knows about
/// it: the day it is from (`None` for a scenario price passed in) and
/// the day before, which gives the 24h change.
struct Spot {
    price: Decimal,
    date: Option<String>,
    previous: Option<Decimal>,
}

/// The flags that define one cycle, as they come from the CLI.
pub struct Cycle<'a> {
    pub entry: &'a str,
    pub exit: &'a str,
    pub pair: &'a str,
    pub threshold: &'a str,
}

/// Everything one rendering needs, bundled so the line functions take
/// one argument.
struct View<'a> {
    marks: &'a Marks,
    spot: &'a Spot,
    rates: &'a Rates,
    asset: &'a str,
    target: &'a str,
}

/// The journal-derived numbers of one cycle, cached between runs.
struct Rates {
    /// Average booked cost per asset unit of the entry side's open lots.
    entry: Decimal,
    /// The rate the exit side's holdings were sold at, per asset unit.
    exit: Decimal,
    /// Asset quantity given away for the exit side's holdings.
    given: Decimal,
    /// Asset quantity held on the entry side.
    held: Decimal,
    /// Commodity the exit side holds (names the SWAP directions).
    exit_commodity: String,
}

pub fn run(
    cycle: &Cycle,
    exchange: Option<&str>,
    scenario: Option<&str>,
    bar: bool,
    journal_paths: &[PathBuf],
    load: &mut dyn FnMut() -> Result<Journal, Error>,
) -> Result<(), Error> {
    let Cycle { entry, exit, pair, threshold } = *cycle;
    let Some(target) = exchange else {
        return Err(Error::new(
            "signal needs a valuation target: pass -X or set the config's [exchange] default"
                .to_string(),
        ));
    };
    let Some((base, quote)) = pair.split_once('/') else {
        return Err(Error::new(format!("--pair wants BASE/QUOTE, got '{}'", pair)));
    };
    // Every figure is the price times a quantity, so it lives in the
    // unit the pair quotes in. Valuing in the asset itself would label
    // those figures with the asset and leave the entry side without a
    // rate (an asset priced in itself has none).
    if target.eq_ignore_ascii_case(base) {
        return Err(Error::new(format!(
            "-X {} is the base of --pair {} — value in what the pair quotes in ({})",
            target, pair, quote,
        )));
    }
    let t = Decimal::parse(threshold)
        .map_err(|e| Error::new(format!("--threshold '{}': {}", threshold, e)))?;

    let cache = cache_path(entry, exit, pair, target);
    let rates = match cache_fresh(&cache, journal_paths)
        .then(|| std::fs::read_to_string(&cache).ok().and_then(|s| parse_cache(&s)))
        .flatten()
    {
        Some(r) => r,
        None => {
            let journal = load()?;
            let r = compute(&journal, entry, exit, target, base);
            write_cache(&cache, &r);
            r
        }
    };

    // Nothing matched at all — a typo in either pattern would otherwise
    // print a bare price line under a green tick.
    if rates.entry.is_zero() && rates.exit.is_zero() {
        if bar {
            println!("no position");
        } else {
            println!("{} no position matched --entry / --exit", "!".yellow());
        }
        return Ok(());
    }

    let spot = match scenario {
        Some(s) => {
            let price = Decimal::parse(s).map_err(|e| Error::new(format!("price '{}': {}", s, e)))?;
            Some(Spot { price, date: None, previous: None })
        }
        // A bar never goes online: it renders what the price DB holds
        // and says how old that is. Only the interactive run tops the
        // file up when today's rate is missing.
        None => current_price(pair, base, quote, !bar)?,
    };
    let Some(spot) = spot.filter(|s| !s.price.is_zero()) else {
        if bar {
            println!("{} no price", base);
        } else {
            println!("{} no price for {}", "!".yellow(), pair);
        }
        return Ok(());
    };

    let marks = evaluate(&rates, spot.price, t);
    let view = View { marks: &marks, spot: &spot, rates: &rates, asset: base, target };
    if bar {
        println!("{}", status_line(&view, true));
        return Ok(());
    }
    let fired = marks.buy.as_ref().is_some_and(|s| s.crossed)
        || marks.sell.as_ref().is_some_and(|s| s.crossed);
    // Unlike every other command this one runs unasked on each shell
    // start, so the quiet case stays a single dim line — a green tick
    // in front of it would be the loudest thing on the prompt. Only a
    // crossed mark earns the `!` (and the SWAP lines below it).
    let status = status_line(&view, false).dimmed();
    if fired {
        println!("{} {}", "!".yellow(), status);
    } else {
        println!("{}", status);
    }

    // The direction names come from the journal (what the exit side
    // actually holds); before any sale the pair's quote stands in.
    let counter = if rates.exit_commodity.is_empty() { quote } else { &rates.exit_commodity };
    for (text, green) in swap_lines(&marks, base, counter, target) {
        let painted = if green { text.green().bold() } else { text.yellow().bold() };
        println!("{}", painted);
    }
    Ok(())
}

/// One side of the cycle at the current price: where the mark sits, how
/// far the price is from it (negative = still away, positive = crossed),
/// and the edge acting now would bring — in the asset for the buy-back
/// side (the extra units a buy-back yields), in the target for the sell
/// side (the sale's lead over the entry).
struct Side {
    /// The rate the mark is derived from: the sale rate on the buy
    /// side, the average cost on the sell side.
    anchor: Decimal,
    mark: Decimal,
    distance: Decimal,
    edge: Option<Decimal>,
    crossed: bool,
}

/// Both sides, either missing when the journal holds nothing for it.
/// Both can be crossed at once — with a threshold smaller than the
/// spread between entry and sale rate the zones overlap, and selling
/// part while buying back part is a legitimate answer.
struct Marks {
    buy: Option<Side>,
    sell: Option<Side>,
}

/// Pure math over the cached rates, so it tests without a journal.
fn evaluate(r: &Rates, price: Decimal, t: Decimal) -> Marks {
    let hundred = Decimal::parse("100").expect("literal");
    let one = Decimal::parse("1").expect("literal");
    let factor = one + t.div_rounded(hundred);
    // No Ord on Decimal: a >= b as "a - b is not negative".
    let ge = |a: Decimal, b: Decimal| !(a - b).is_negative();

    let buy = (!r.exit.is_zero()).then(|| {
        let mark = r.exit.div_rounded(factor);
        let ahead = r.exit.div_rounded(price) - one;
        Side {
            anchor: r.exit,
            mark,
            distance: (mark.div_rounded(price) - one).mul_rounded(hundred),
            edge: (!r.given.is_zero()).then(|| r.given.mul_rounded(ahead)),
            crossed: ge(ahead.mul_rounded(hundred), t),
        }
    });
    let sell = (!r.entry.is_zero()).then(|| {
        let mark = r.entry.mul_rounded(factor);
        Side {
            anchor: r.entry,
            mark,
            distance: (price.div_rounded(mark) - one).mul_rounded(hundred),
            edge: (!r.held.is_zero()).then(|| (price - r.entry).mul_rounded(r.held)),
            crossed: ge((price.div_rounded(r.entry) - one).mul_rounded(hundred), t),
        }
    });
    Marks { buy, sell }
}

/// One grammar for both renderings: the figure first, its annotations
/// in parentheses, commas inside. The quiet prompt line separates its
/// segments with `·` and, having no room for the position blocks, lets
/// the edge follow its mark plainly. A negative edge is not a
/// smaller saving but the opposite outcome, so it switches the word
/// (`saved`/`gained` → `lost`) and prints unsigned — a "saved -4.85"
/// would have to be read twice.
///
/// `bar` renders for a status bar, which has the room the prompt has
/// not, in one grammar for every block: the figure first, its
/// annotations in parentheses — no dots, the `|` between blocks stays
/// the strongest break on the line. First both marks (the calls to
/// act), then both positions as acc writes them in the journal
/// (`sold FOO2.00 @ $150`) with their outcome at the current price —
/// the outcome belongs to the position, not to the mark. A crossed mark
/// shouts its label so the bar can paint that segment (it gets no
/// colour from here). The threshold does not appear: the marks already
/// carry it.
fn status_line(v: &View, bar: bool) -> String {
    let (m, price, asset, target) = (v.marks, v.spot.price, v.asset, v.target);
    // Whole units: a bar reading is a glance, and the cents of a mark
    // never decide anything.
    let money = |v: &Decimal| format!("{}{}", target, v.format_decimal(0));
    let qty = |v: &Decimal| format!("{}{}", asset, v.format_decimal(2));

    // Where the price is heading — the marks only say where it stands.
    let change = v.spot.previous.filter(|p| !p.is_zero()).map(|prev| {
        let hundred = Decimal::parse("100").expect("literal");
        let one = Decimal::parse("1").expect("literal");
        (price.div_rounded(prev) - one).mul_rounded(hundred)
    });
    // A price that is not from today means the DB was not topped up —
    // silently deciding on yesterday's rate is the one real hazard here.
    let stale = v.spot.date.as_deref().filter(|d| *d != Date::today().to_string());

    let mut line = format!("{} {}", asset, money(&price));
    let mut notes = Vec::new();
    if let Some(c) = &change {
        notes.push(format!("{}% 24h", signed(c)));
    }
    if let Some(d) = stale {
        // Caps only in the bar, where they are the paint signal.
        notes.push(format!("{} {}", if bar { "STALE" } else { "stale" }, d));
    }
    if !notes.is_empty() {
        line.push_str(&format!(" ({})", notes.join(", ")));
    }
    if bar {
        let label = |text: &str, crossed: bool| {
            if crossed { text.to_uppercase() } else { text.to_string() }
        };
        // The calls to act first, the analysis they rest on after.
        if let Some(s) = &m.buy {
            line.push_str(&format!(
                " | {} {} ({}%)",
                label("buy", s.crossed),
                money(&s.mark),
                signed(&s.distance),
            ));
        }
        if let Some(s) = &m.sell {
            line.push_str(&format!(
                " | {} {} ({}%)",
                label("sell", s.crossed),
                money(&s.mark),
                signed(&s.distance),
            ));
        }
        // Each position as acc writes it in the journal (quantity @
        // rate), its outcome at the current price in parentheses.
        if let Some(s) = m.buy.as_ref().filter(|_| !v.rates.given.is_zero()) {
            line.push_str(&format!(" | sold {} @ {}", qty(&v.rates.given), money(&s.anchor)));
            if let Some(e) = &s.edge {
                line.push_str(&format!(
                    " ({} {} ≈ {})",
                    if e.is_negative() { "lost" } else { "saved" },
                    qty(&e.abs()),
                    money(&e.abs().mul_rounded(price)),
                ));
            }
        }
        if let Some(s) = m.sell.as_ref().filter(|_| !v.rates.held.is_zero()) {
            line.push_str(&format!(" | hold {} @ {}", qty(&v.rates.held), money(&s.anchor)));
            if let Some(e) = &s.edge {
                line.push_str(&format!(
                    " ({} {})",
                    if e.is_negative() { "lost" } else { "gained" },
                    money(&e.abs()),
                ));
            }
        }
        return line;
    }
    // The prompt has no room for the position blocks, so the edge
    // follows its mark plainly instead.
    if let Some(s) = &m.buy {
        line.push_str(&format!(" · buy {} ({}%)", money(&s.mark), signed(&s.distance)));
        if let Some(e) = &s.edge {
            line.push_str(&format!(
                " {} {} ≈ {}",
                if e.is_negative() { "lost" } else { "saved" },
                qty(&e.abs()),
                money(&e.abs().mul_rounded(price)),
            ));
        }
    }
    if let Some(s) = &m.sell {
        line.push_str(&format!(" · sell {} ({}%)", money(&s.mark), signed(&s.distance)));
        if let Some(e) = &s.edge {
            line.push_str(&format!(
                " {} {}",
                if e.is_negative() { "lost" } else { "gained" },
                money(&e.abs()),
            ));
        }
    }
    line
}

/// The loud lines, one per crossed mark: the direction and the edge —
/// the same figure the status line carries, not a second percentage on
/// a different base. Returns (text, paint-green).
fn swap_lines(m: &Marks, asset: &str, counter: &str, target: &str) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    if let Some(s) = m.buy.as_ref().filter(|s| s.crossed) {
        let edge = s.edge.map_or_else(
            || format!("the mark of {}{}", target, s.mark.format_decimal(0)),
            |e| format!("{}{} more than you gave", asset, e.format_decimal(2)),
        );
        out.push((format!("SWAP {}->{}: buy back now — {}", counter, asset, edge), true));
    }
    if let Some(s) = m.sell.as_ref().filter(|s| s.crossed) {
        let edge = s.edge.map_or_else(
            || format!("the mark of {}{}", target, s.mark.format_decimal(0)),
            |e| format!("{}{} over your cost", target, e.format_decimal(0)),
        );
        out.push((format!("SWAP {}->{}: sell now — {}", asset, counter, edge), false));
    }
    out
}

/// One decimal with an explicit sign, for the distance percentages.
fn signed(v: &Decimal) -> String {
    let s = v.format_decimal(1);
    if s.starts_with('-') { s } else { format!("+{}", s) }
}

/// Derive both sides' rates from the journal — the cold path, run only
/// when a journal file changed. Two trackings: the entry side values in
/// the display target (its booked average is the entry rate), the exit
/// side in the asset itself (its booked cost is the quantity given
/// away; the inverted average is the rate it was sold at).
fn compute(journal: &Journal, entry: &str, exit: &str, target: &str, asset: &str) -> Rates {
    let resolve =
        |s: &str| journal.aliases.get(s).cloned().unwrap_or_else(|| s.to_string());
    let split = |p: &str| p.split_whitespace().map(String::from).collect::<Vec<_>>();
    let entry_m = PatternMatcher::from_parts(&split(entry));
    let exit_m = PatternMatcher::from_parts(&split(exit));

    let open = crate::lotter::open_lots(&journal.transactions, Some(&resolve(target)), &journal.prices);
    let mut held = Decimal::zero();
    let mut cost = Decimal::zero();
    for l in open.iter().filter(|l| entry_m.matches_position(&l.account, &l.commodity)) {
        if let Some(b) = l.booked_per_unit {
            held += l.qty;
            cost += l.qty.mul_rounded(b);
        }
    }
    let entry_rate = if held.is_zero() { Decimal::zero() } else { cost.div_rounded(held) };

    let open = crate::lotter::open_lots(&journal.transactions, Some(&resolve(asset)), &journal.prices);
    let mut qty = Decimal::zero();
    let mut given = Decimal::zero();
    let mut exit_commodity = String::new();
    for l in open.iter().filter(|l| exit_m.matches_position(&l.account, &l.commodity)) {
        if let Some(b) = l.booked_per_unit {
            qty += l.qty;
            given += l.qty.mul_rounded(b);
            exit_commodity = l.commodity.clone();
        }
    }
    let exit_rate = if given.is_zero() { Decimal::zero() } else { qty.div_rounded(given) };

    Rates { entry: entry_rate, exit: exit_rate, given, held, exit_commodity }
}

/// The last two rows of the pair's price file — the current rate with
/// its date, and the day before for the 24h change. With `fetch`, a
/// rate that is not from today is topped up by one `update` for exactly
/// this pair first; a failed fetch (offline) is not fatal, the last
/// stored rate still carries the line and shows its age.
fn current_price(
    pair: &str,
    base: &str,
    quote: &str,
    fetch: bool,
) -> Result<Option<Spot>, Error> {
    let path = crate::commands::update::file::path_for(base, quote)?;
    let today = Date::today().to_string();
    let mut rows = crate::commands::update::file::read_existing(&path)?;
    if fetch && rows.last().map(|(d, _)| d.as_str()) != Some(today.as_str()) {
        let _ = crate::commands::update::run(
            &[pair.to_string()],
            None,
            None,
            crate::commands::update::Cadence::Daily,
            false,
            crate::commands::update::UpdateFlags { crypto: true, fiat: false },
        );
        rows = crate::commands::update::file::read_existing(&path)?;
    }
    let Some((date, rate)) = rows.last() else {
        return Ok(None);
    };
    let Ok(price) = Decimal::parse(rate) else {
        return Ok(None);
    };
    let previous = rows
        .len()
        .checked_sub(2)
        .and_then(|i| Decimal::parse(&rows[i].1).ok());
    Ok(Some(Spot { price, date: Some(date.clone()), previous }))
}

/// Cache file for one cycle: keyed on the flags that define it, so two
/// different cycles never share a cache.
fn cache_path(entry: &str, exit: &str, pair: &str, target: &str) -> PathBuf {
    let mut h = DefaultHasher::new();
    (entry, exit, pair, target).hash(&mut h);
    let dir = std::env::var("XDG_CACHE_HOME").map(PathBuf::from).unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        Path::new(&home).join(".cache")
    });
    dir.join("acc").join(format!("signal-{:016x}", h.finish()))
}

/// Fresh means: the cache exists and no journal file is newer.
fn cache_fresh(cache: &Path, journal_paths: &[PathBuf]) -> bool {
    let Ok(cache_mtime) = std::fs::metadata(cache).and_then(|m| m.modified()) else {
        return false;
    };
    let newest = journal_paths
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok().and_then(|m| m.modified().ok()))
        .max();
    newest.is_none_or(|n| n <= cache_mtime)
}

fn parse_cache(s: &str) -> Option<Rates> {
    let mut it = s.split_whitespace();
    let entry = Decimal::parse(it.next()?).ok()?;
    let exit = Decimal::parse(it.next()?).ok()?;
    let given = Decimal::parse(it.next()?).ok()?;
    let held = Decimal::parse(it.next()?).ok()?;
    let exit_commodity = match it.next()? {
        "-" => String::new(),
        c => c.to_string(),
    };
    Some(Rates { entry, exit, given, held, exit_commodity })
}

/// Best effort: an unwritable cache only costs the next run a recompute.
fn write_cache(path: &Path, r: &Rates) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let exit_commodity = if r.exit_commodity.is_empty() { "-" } else { &r.exit_commodity };
    let _ = std::fs::write(
        path,
        format!(
            "{} {} {} {} {}\n",
            r.entry.format_decimal(12),
            r.exit.format_decimal(12),
            r.given.format_decimal(12),
            r.held.format_decimal(12),
            exit_commodity,
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dec(s: &str) -> Decimal {
        Decimal::parse(s).expect("literal")
    }

    fn rates() -> Rates {
        Rates {
            entry: dec("100"),
            exit: dec("150"),
            given: dec("2"),
            held: dec("3"),
            exit_commodity: "BAR".to_string(),
        }
    }

    /// The status line of the fixture cycle at a price and threshold.
    fn at(price: Decimal, t: Decimal) -> String {
        line(price, t, false)
    }

    /// Either rendering of the fixture cycle, without price history.
    fn line(price: Decimal, t: Decimal, bar: bool) -> String {
        let marks = evaluate(&rates(), price, t);
        let spot = Spot { price, date: None, previous: None };
        let rates = rates();
        let view = View {
            marks: &marks,
            spot: &spot,
            rates: &rates,
            asset: "FOO",
            target: "$",
        };
        status_line(&view, bar)
    }

    #[test]
    fn marks_are_symmetric_around_the_roundtrip() {
        // sell at entry x 1.5 = 150, buy back at exit / 1.5 = 100: the
        // buy-back mark of a sale at the sell mark IS the entry again.
        let line = at(dec("120"), dec("50"));
        assert!(line.contains("sell $150"), "line: {}", line);
        assert!(line.contains("buy $100"), "line: {}", line);
    }

    #[test]
    fn distances_measure_price_to_mark() {
        // 120 -> mark 150 misses 20%; 120 -> mark 100 misses 16.7%.
        let line = at(dec("120"), dec("50"));
        assert!(line.contains("sell $150 (-20.0%"), "line: {}", line);
        assert!(line.contains("buy $100 (-16.7%"), "line: {}", line);
        let m = evaluate(&rates(), dec("120"), dec("50"));
        assert!(!m.buy.expect("side").crossed && !m.sell.expect("side").crossed);
    }

    #[test]
    fn crossed_marks_raise_the_signals() {
        let m = evaluate(&rates(), dec("150"), dec("50"));
        assert!(m.sell.expect("side").crossed, "price at the sell mark fires");
        assert!(!m.buy.expect("side").crossed);
        let m = evaluate(&rates(), dec("100"), dec("50"));
        assert!(m.buy.expect("side").crossed, "price at the buy-back mark fires");
        assert!(!m.sell.expect("side").crossed);
    }

    #[test]
    fn overlapping_zones_fire_both_sides() {
        // Threshold below the entry/exit spread: sell at 110, buy back
        // at 136.36 — a price between them warrants doing both.
        let m = evaluate(&rates(), dec("120"), dec("10"));
        assert!(
            m.buy.as_ref().expect("side").crossed && m.sell.as_ref().expect("side").crossed
        );
        let lines = swap_lines(&m, "FOO", "BAR", "$");
        assert_eq!(lines.len(), 2, "both directions are announced: {:?}", lines);
    }

    #[test]
    fn saved_names_the_extra_asset_and_its_value() {
        // At 100 the 2 FOO given come back as 3: saved FOO1.00 = 100.
        let line = at(dec("100"), dec("50"));
        assert!(line.contains("saved FOO1.00 ≈ $100"), "line: {}", line);
    }

    #[test]
    fn gain_prices_the_held_asset_over_entry() {
        // At 150 the 3 held FOO stand 50 over the 100 entry: gained $150.
        let line = at(dec("150"), dec("50"));
        assert!(line.contains("gained $150"), "line: {}", line);
    }

    #[test]
    fn the_opposite_outcome_switches_the_word_and_drops_the_sign() {
        // Above the sale rate a buy-back costs instead of saving; below
        // the entry the holding is under water.
        let line = at(dec("300"), dec("50"));
        assert!(line.contains("lost FOO1.00 ≈ $300"), "line: {}", line);
        let line = at(dec("80"), dec("50"));
        assert!(line.contains("lost $60"), "line: {}", line);
    }

    #[test]
    fn swap_lines_reuse_the_status_figures() {
        // No second percentage on a different base — the loud line
        // repeats the edge the status line already showed.
        let m = evaluate(&rates(), dec("100"), dec("50"));
        let lines = swap_lines(&m, "FOO", "BAR", "$");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].0, "SWAP BAR->FOO: buy back now — FOO1.00 more than you gave");
        assert!(lines[0].1, "the buy-back line is the green one");
    }

    #[test]
    fn missing_sides_drop_their_segments() {
        let r = Rates {
            entry: Decimal::zero(),
            exit: Decimal::zero(),
            given: Decimal::zero(),
            held: Decimal::zero(),
            exit_commodity: String::new(),
        };
        let m = evaluate(&r, dec("120"), dec("50"));
        let spot = Spot { price: dec("120"), date: None, previous: None };
        let view = View {
            marks: &m,
            spot: &spot,
            rates: &r,
            asset: "FOO",
            target: "$",
        };
        assert_eq!(status_line(&view, false), "FOO $120");
        assert!(m.buy.is_none() && m.sell.is_none());
    }

    #[test]
    fn the_bar_form_orders_marks_before_positions() {
        // The bar leads with both marks — the calls to act — and puts
        // the positions they rest on after, each with its outcome in
        // parentheses. Only the crossed label shouts, so the bar can
        // paint that segment.
        assert_eq!(
            line(dec("100"), dec("50"), true),
            "FOO $100 \
             | BUY $100 (+0.0%) | sell $150 (-33.3%) \
             | sold FOO2.00 @ $150 (saved FOO1.00 ≈ $100) \
             | hold FOO3.00 @ $100 (gained $0)",
        );
        // The prompt line stays lean — no positions, no capitals; the
        // edge follows its mark plainly.
        assert_eq!(
            line(dec("120"), dec("50"), false),
            "FOO $120 · buy $100 (-16.7%) saved FOO0.50 ≈ $60 \
             · sell $150 (-20.0%) gained $60",
        );
    }

    #[test]
    fn the_price_carries_its_change_and_its_age() {
        let marks = evaluate(&rates(), dec("120"), dec("50"));
        let rates = rates();
        // Yesterday's rate gives the direction the price is moving in.
        let spot = Spot {
            price: dec("120"),
            date: Some("2020-01-01".to_string()),
            previous: Some(dec("100")),
        };
        let view = View {
            marks: &marks,
            spot: &spot,
            rates: &rates,
            asset: "FOO",
            target: "$",
        };
        let rendered = status_line(&view, false);
        assert!(rendered.starts_with("FOO $120 (+20.0% 24h, stale 2020-01-01)"), "{}", rendered);
        // A bar shouts the staleness so the wrapper can paint it, and
        // folds the ticker's annotations into one parenthesis.
        assert!(status_line(&view, true).contains("(+20.0% 24h, STALE 2020-01-01)"));
    }

    #[test]
    fn cache_roundtrips_through_the_file_format() {
        let path = std::env::temp_dir().join("acc-signal-cache-test");
        write_cache(&path, &rates());
        let read = parse_cache(&std::fs::read_to_string(&path).expect("written")).expect("parses");
        assert_eq!(read.entry, dec("100"));
        assert_eq!(read.exit, dec("150"));
        assert_eq!(read.exit_commodity, "BAR");
        let _ = std::fs::remove_file(&path);
    }
}
