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
    /// What the exit side actually holds, read as a plain balance rather
    /// than derived from the lots — a lot that arrives without a basis
    /// would otherwise go missing from a figure the wallet shows.
    stock: Decimal,
    /// The counter stock valued in the display target, so the head can
    /// name what the whole cycle is worth. A stablecoin is not assumed
    /// to be at par — the rate comes from the price DB like any other.
    stock_value: Decimal,
    /// Profit already closed on the entry side, in the display target.
    realized: Decimal,
    /// What those closed sales originally cost, so the cycle's return
    /// can be measured against everything that was ever put in.
    sold_cost: Decimal,
    /// Earliest day anything of the entry side was acquired, open lots
    /// and closed sales alike — the start of the holding period the
    /// return is spread over.
    first_buy: Option<Date>,
    /// The closed sales' own span: earliest acquisition to last sale.
    /// It ends where the sales ended, not today, since a return already
    /// booked stops accruing at the moment it is booked.
    sold_from: Option<Date>,
    sold_to: Option<Date>,
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
/// (`sold FOO2 @ $150`) with their outcome at the current price —
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
    // A position is read in whole coins; the fractions decide nothing
    // at a glance. The sub-unit results keep their decimals, since
    // rounding those to zero would erase the figure itself.
    let whole = |v: &Decimal| format!("{}{}", asset, v.format_decimal(0));

    // Where the price is heading — the marks only say where it stands.
    let change = v.spot.previous.filter(|p| !p.is_zero()).map(|prev| {
        let hundred = Decimal::parse("100").expect("literal");
        let one = Decimal::parse("1").expect("literal");
        (price.div_rounded(prev) - one).mul_rounded(hundred)
    });
    // A price that is not from today means the DB was not topped up —
    // silently deciding on yesterday's rate is the one real hazard here.
    let stale = v.spot.date.as_deref().filter(|d| *d != Date::today().to_string());

    // A money amount that carries its sign, for the figures whose word
    // names the axis rather than the direction (`unrealized +$60`).
    let plus = |v: &Decimal| {
        let sign = if v.is_negative() { "-" } else { "+" };
        format!("{}{}", sign, money(&v.abs()))
    };

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
        // Both summaries of the cycle stand together in the head: what
        // it is worth and what it has made. `value` is the held asset at
        // today's price plus the counter stock — not the `lots` Value
        // column, which counts the proceeds of EVERY past sale, even
        // those whose money went elsewhere; this counts what is still
        // there. `p&l` is `up&l` plus `rp&l`, measured against
        // everything ever put in, and it is not called `total`, which
        // would leave open what it is the total OF.
        let worth = v.rates.held.mul_rounded(price) + v.rates.stock_value;
        if !worth.is_zero() {
            line.push_str(&format!(" value {}", money(&worth)));
        }
        let open = m.sell.as_ref().and_then(|s| s.edge).unwrap_or_else(Decimal::zero);
        let total = open + v.rates.realized;
        let invested = v.rates.held.mul_rounded(v.rates.entry) + v.rates.sold_cost;
        if !invested.is_zero() {
            let hundred = Decimal::parse("100").expect("literal");
            let roi = total.mul_rounded(hundred).div_rounded(invested);
            line.push_str(&format!(" p&l {} ({}%", plus(&total), signed(&roi)));
            // The open side is still running, so its span ends today.
            // The return carries the period it ran over, the rate beside
            // it the period it is normalised to — same form twice.
            if let Some(f) = v.rates.first_buy {
                line.push_str(&format!("/{}", months(f, Date::today())));
                if let Some(r) = monthly(&total, &invested, f, Date::today()) {
                    line.push_str(&format!(" {}", r));
                }
            }
            line.push(')');
        }
        // One block per SIDE of the cycle, not one per figure. A mark
        // and the position it governs are halves of the same thought —
        // `sell` says when to part with what `hold` names — and listing
        // them as five peers left the reader to pair them up again. The
        // mark leads because it carries the action, the position follows
        // as the reason, and the outcome follows it.
        //
        // Parentheses tie a figure to the one it is computed from: the
        // percentages to the price and the marks, the paper result to
        // the quantity and rate it is measured on. What stands on its
        // own — a booked total, a quantity a buy-back would bring —
        // stands free, since a bracket would demote it to an aside.
        let label = |text: &str, crossed: bool| {
            if crossed { text.to_uppercase() } else { text.to_string() }
        };
        // `≤` / `≥` rather than a bare price: the number alone never
        // said in which direction it triggers. Not "limit", which would
        // claim an order rests at an exchange — nothing rests anywhere.
        if let Some(s) = &m.sell {
            line.push_str(&format!(
                " | {} ≥{} ({}%)",
                label("sell", s.crossed),
                money(&s.mark),
                signed(&s.distance),
            ));
            if !v.rates.held.is_zero() {
                // `stock` on both sides, never `hold`: behind an
                // imperative like `sell`, a second verb reads as a
                // second instruction that contradicts the first.
                line.push_str(&format!(" stock {} @ {}", whole(&v.rates.held), money(&s.anchor)));
                // The word names the AXIS (is the position closed?), the
                // sign names the direction. Collapsing both into one
                // word — gained, lost, realized side by side — hid which
                // of the two a figure was talking about.
                if let Some(e) = &s.edge {
                    line.push_str(&format!(" (up&l {}", plus(e)));
                    // Measured on what these lots cost. No monthly rate:
                    // the head already carries one for the cycle, and
                    // the open side makes up most of it — a third would
                    // repeat the same figure.
                    let cost = v.rates.held.mul_rounded(v.rates.entry);
                    if !cost.is_zero() {
                        let hundred = Decimal::parse("100").expect("literal");
                        line.push_str(&format!(
                            " {}%",
                            signed(&e.mul_rounded(hundred).div_rounded(cost)),
                        ));
                    }
                    line.push(')');
                }
            }
        }
        if let Some(s) = &m.buy {
            line.push_str(&format!(
                " | {} ≤{} ({}%)",
                label("buy", s.crossed),
                money(&s.mark),
                signed(&s.distance),
            ));
            // The counter side holds what the sale returned, so it is
            // named in its own commodity and not in the display target.
            if !v.rates.stock.is_zero() && !v.rates.exit_commodity.is_empty() {
                line.push_str(&format!(
                    " stock {}{}",
                    v.rates.exit_commodity,
                    v.rates.stock.format_decimal(0),
                ));
                // The same word as the entry side, because it is the
                // same thing: what today's price does to the position
                // named before it. Only the yardstick differs, since a
                // stablecoin holding does not move in the target at all
                // — it moves in the asset, which is what the swap is
                // about. The unit says which, so no second word is
                // needed.
                if let Some(e) = &s.edge {
                    let sign = if e.is_negative() { "-" } else { "+" };
                    line.push_str(&format!(
                        " (up&l {}{} ≈ {}",
                        sign,
                        qty(&e.abs()),
                        money(&e.abs().mul_rounded(price)),
                    ));
                    // How much cheaper or dearer the asset is than when
                    // it was sold, which is exactly what a buy-back
                    // gains or costs. No monthly rate here: this leg is
                    // days old, and compounding a day's move into a
                    // month yields a figure that changes wildly with
                    // every tick and says nothing.
                    if !v.rates.given.is_zero() {
                        let hundred = Decimal::parse("100").expect("literal");
                        let roi = e.mul_rounded(hundred).div_rounded(v.rates.given);
                        line.push_str(&format!(" {}%", signed(&roi)));
                    }
                    line.push(')');
                }
                // The sale this stock came out of, closing the block:
                // quantity and the rate it went at, with the profit it
                // booked. Every figure carries its own result right
                // behind it, so the brackets never share a subject.
                //
                // `up&l` open, `rp&l` closed, `p&l` the sum of the two —
                // one family of names whose arithmetic is visible from
                // the names alone.
                if !v.rates.given.is_zero() {
                    line.push_str(&format!(
                        " sold {} @ {}",
                        whole(&v.rates.given),
                        money(&s.anchor),
                    ));
                    if !v.rates.realized.is_zero() {
                        line.push_str(&format!(" (rp&l {}", plus(&v.rates.realized)));
                        if !v.rates.sold_cost.is_zero() {
                            let hundred = Decimal::parse("100").expect("literal");
                            let roi = v
                                .rates
                                .realized
                                .mul_rounded(hundred)
                                .div_rounded(v.rates.sold_cost);
                            line.push_str(&format!(" {}%", signed(&roi)));
                            // A booked return stops accruing when it is
                            // booked, so this span ends at the last sale
                            // and not today.
                            if let Some((f, t)) = v.rates.sold_from.zip(v.rates.sold_to) {
                                line.push_str(&format!("/{}", months(f, t)));
                                if let Some(r) =
                                    monthly(&v.rates.realized, &v.rates.sold_cost, f, t)
                                {
                                    line.push_str(&format!(" {}", r));
                                }
                            }
                        }
                        line.push(')');
                    }
                }
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

/// A span in whole months, the only unit the line uses for a holding
/// period — days and years would put three different scales next to
/// each other for a figure that only says what the return ran over.
fn months(from: Date, to: Date) -> String {
    // 30.436875 = the Gregorian year's 365.2425 days over twelve.
    let m = (from.days_until(to) as f64 / 30.436875).round().max(0.0);
    format!("{:.0}m", m)
}

/// The same return as a MONTHLY rate: the one that, compounded over the
/// span, multiplies up to it — `(1 + roi)^(1/months) - 1`. Only this
/// makes two positions of unequal age comparable, since 61% in four
/// months is not 61% in four years.
///
/// The root has no exact decimal form and runs in `f64`, which is sound
/// for a display figure nothing downstream computes with. `None` where
/// there is nothing to spread: no base, no span, or a loss deeper than
/// the base, which has no real root.
fn monthly(profit: &Decimal, base: &Decimal, from: Date, to: Date) -> Option<String> {
    let days = from.days_until(to);
    if base.is_zero() || days <= 0 {
        return None;
    }
    let factor = 1.0 + profit.to_f64() / base.to_f64();
    if factor <= 0.0 {
        return None;
    }
    // 30.436875 = the Gregorian year's 365.2425 days over twelve.
    let rate = (factor.powf(30.436875 / days as f64) - 1.0) * 100.0;
    Some(format!("{}{:.0}%/m", if rate < 0.0 { "" } else { "+" }, rate))
}

/// A whole percent with an explicit sign. No decimal: every percentage
/// on the line sits in brackets as an aside to the figure before it,
/// and a tenth of a percent decides nothing there — the marks
/// themselves carry the price to the dollar.
fn signed(v: &Decimal) -> String {
    let s = v.format_decimal(0);
    // The sign follows the VALUE, never the rounded string: -0.14
    // prints as "0", and a "+" in front of that would claim a gain
    // where there is a loss. Anything that rounds to zero carries no
    // sign at all, since neither direction is left in the figure.
    if s.trim_start_matches('-') == "0" {
        return "0".to_string();
    }
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
    let mut first_buy: Option<Date> = None;
    for l in open.iter().filter(|l| entry_m.matches_position(&l.account, &l.commodity)) {
        if let Some(b) = l.booked_per_unit {
            held += l.qty;
            cost += l.qty.mul_rounded(b);
            first_buy = Some(first_buy.map_or(l.date, |f: Date| f.min(l.date)));
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

    // The stock is a balance, not a lot sum: what the wallet shows is
    // every unit sitting there, whether or not acc could derive a basis
    // for it. Bracket-virtuals count, the same postings the tracker sees.
    let mut stock = Decimal::zero();
    for lt in &journal.transactions {
        for lp in &lt.value.postings {
            if lp.value.is_virtual && !lp.value.balanced {
                continue;
            }
            if let Some(a) = &lp.value.amount
                && exit_m.matches_position(&lp.value.account, &a.commodity)
            {
                stock += a.value;
            }
        }
    }

    // Profit already closed on the entry side. Under a target both
    // booked figures are the real payments; a sale without a derivable
    // trade rate contributes nothing rather than a guess.
    let sold = crate::lotter::realizations(
        &journal.transactions,
        Some(&resolve(target)),
        &journal.prices,
    );
    // Latest available rate, the same "latest, no date" convention the
    // revaluator and `lots` use for an open position.
    let stock_value = journal
        .prices
        .find(&exit_commodity, &resolve(target), "9999-12-31")
        .map_or_else(Decimal::zero, |r| stock.mul_rounded(r));

    let mut realized = Decimal::zero();
    let mut sold_cost = Decimal::zero();
    let mut sold_from: Option<Date> = None;
    let mut sold_to: Option<Date> = None;
    for r in sold.iter().filter(|r| entry_m.matches_position(&r.account, &r.commodity)) {
        let Some((c, p)) = r.booked_cost.zip(r.booked_proceeds) else { continue };
        // A long sale gains when it sold above cost, a short cover when
        // it bought back below the rate it opened at — the same sign
        // rule the `lots` report applies.
        realized += if r.short { c - p } else { p - c };
        sold_cost += c;
        first_buy = Some(first_buy.map_or(r.first_buy_date, |f: Date| f.min(r.first_buy_date)));
        sold_from = Some(sold_from.map_or(r.first_buy_date, |f: Date| f.min(r.first_buy_date)));
        sold_to = Some(sold_to.map_or(r.sell_date, |t: Date| t.max(r.sell_date)));
    }

    Rates {
        entry: entry_rate,
        exit: exit_rate,
        given,
        held,
        stock,
        stock_value,
        realized,
        sold_cost,
        first_buy,
        sold_from,
        sold_to,
        exit_commodity,
    }
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
        // No bounds: continue at the last stored day, which the fetch
        // refreshes because it may still have been open when written.
        let range = crate::period::Range { periods: Vec::new(), begin: None, end: None };
        let _ = crate::commands::update::run(
            &[pair.to_string()],
            &range,
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

/// Fresh means: the cache exists and no input is newer — neither a
/// journal file nor the running binary. The binary counts because these
/// rates are a COMPUTED result, not a copy of the journal: a rebuilt acc
/// may derive them differently from the very same files, and a cache
/// keyed on the journal alone would serve the old answer until the
/// journal happened to change. One `stat` on the executable, so the
/// warm path stays a couple of file reads; hashing its contents would
/// cost more than the recompute it saves.
fn cache_fresh(cache: &Path, journal_paths: &[PathBuf]) -> bool {
    let Ok(cache_mtime) = std::fs::metadata(cache).and_then(|m| m.modified()) else {
        return false;
    };
    let own = std::env::current_exe()
        .ok()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok());
    let newest = journal_paths
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok().and_then(|m| m.modified().ok()))
        .chain(own)
        .max();
    newest.is_none_or(|n| n <= cache_mtime)
}

fn parse_cache(s: &str) -> Option<Rates> {
    let mut it = s.split_whitespace();
    let entry = Decimal::parse(it.next()?).ok()?;
    let exit = Decimal::parse(it.next()?).ok()?;
    let given = Decimal::parse(it.next()?).ok()?;
    let held = Decimal::parse(it.next()?).ok()?;
    let stock = Decimal::parse(it.next()?).ok()?;
    let stock_value = Decimal::parse(it.next()?).ok()?;
    let realized = Decimal::parse(it.next()?).ok()?;
    let sold_cost = Decimal::parse(it.next()?).ok()?;
    let day = |t: &str| -> Option<Option<Date>> {
        match t {
            "-" => Some(None),
            d => Date::parse(d).ok().map(Some),
        }
    };
    let first_buy = day(it.next()?)?;
    let sold_from = day(it.next()?)?;
    let sold_to = day(it.next()?)?;
    let exit_commodity = match it.next()? {
        "-" => String::new(),
        c => c.to_string(),
    };
    Some(Rates {
        entry,
        exit,
        given,
        held,
        stock,
        stock_value,
        realized,
        sold_cost,
        first_buy,
        sold_from,
        sold_to,
        exit_commodity,
    })
}

/// Best effort: an unwritable cache only costs the next run a recompute.
fn write_cache(path: &Path, r: &Rates) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let exit_commodity = if r.exit_commodity.is_empty() { "-" } else { &r.exit_commodity };
    let stamp = |d: Option<Date>| d.map_or_else(|| "-".to_string(), |d| d.to_string());
    let (first_buy, sold_from, sold_to) =
        (stamp(r.first_buy), stamp(r.sold_from), stamp(r.sold_to));
    let _ = std::fs::write(
        path,
        format!(
            "{} {} {} {} {} {} {} {} {} {} {} {}\n",
            r.entry.format_decimal(12),
            r.exit.format_decimal(12),
            r.given.format_decimal(12),
            r.held.format_decimal(12),
            r.stock.format_decimal(12),
            r.stock_value.format_decimal(12),
            r.realized.format_decimal(12),
            r.sold_cost.format_decimal(12),
            first_buy,
            sold_from,
            sold_to,
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
            stock: dec("300"),
            stock_value: dec("300"),
            realized: dec("120"),
            sold_cost: dec("180"),
            first_buy: Some(Date::parse("2024-01-01").expect("literal")),
            sold_from: Some(Date::parse("2024-01-01").expect("literal")),
            sold_to: Some(Date::parse("2024-07-01").expect("literal")),
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
        assert!(line.contains("sell $150 (-20%"), "line: {}", line);
        assert!(line.contains("buy $100 (-17%"), "line: {}", line);
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
            stock: Decimal::zero(),
            stock_value: Decimal::zero(),
            realized: Decimal::zero(),
            sold_cost: Decimal::zero(),
            first_buy: None,
            sold_from: None,
            sold_to: None,
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
    fn the_bar_form_groups_each_mark_with_its_position() {
        // One block per side of the cycle: the mark leads, the position
        // it governs follows, its outcome closes in parentheses. `≥`/`≤`
        // give the direction the bare price left open, and only the
        // crossed label shouts so the bar can paint that segment.
        assert_eq!(
            line(dec("100"), dec("50"), true),
            "FOO $100 value $600 p&l +$120 (+25%/32m +1%/m) \
             | sell ≥$150 (-33%) stock FOO3 @ $100 (up&l +$0 0%) \
             | BUY ≤$100 (0%) stock BAR300 (up&l +FOO1.00 ≈ $100 +50%) \
               sold FOO2 @ $150 (rp&l +$120 +67%/6m +9%/m)",
        );
        // The prompt line stays lean — no positions, no capitals; the
        // edge follows its mark plainly.
        assert_eq!(
            line(dec("120"), dec("50"), false),
            "FOO $120 · buy $100 (-17%) saved FOO0.50 ≈ $60 \
             · sell $150 (-20%) gained $60",
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
        assert!(rendered.starts_with("FOO $120 (+20% 24h, stale 2020-01-01)"), "{}", rendered);
        // A bar shouts the staleness so the wrapper can paint it, and
        // folds the ticker's annotations into one parenthesis.
        assert!(status_line(&view, true).contains("(+20% 24h, STALE 2020-01-01)"));
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
