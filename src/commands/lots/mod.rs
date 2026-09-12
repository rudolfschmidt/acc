//! `lots` command — realized sales and open FIFO lots per position.
//!
//! One table in the style of a trade report: per (account, commodity)
//! position first the realized sales (one line per closed lot, newest
//! window selectable with `-b`), closed by a labelled `Sold` summary,
//! then the still-open lots in FIFO order (the first line is the front
//! the next disposal closes), closed by an `Open` summary; a `Total`
//! line sums cost, value and profit across everything shown.
//!
//! The Entry/Exit columns are *booked* rates — what was actually paid
//! and received per unit, converted into the target at the trade date —
//! not the acquisition-day market rates the capital math runs on. A
//! line's Profit therefore spans market move plus execution spread,
//! i.e. it reconciles with `capital` + `slippage` together, not with
//! `capital` alone. Lots whose trade rate is not derivable from the
//! journal show empty Entry/Cost cells rather than a guessed number.
//!
//! Open lots price their Exit/Value columns only under `--unrealized`,
//! at the latest available rate (the revaluator's "latest, no date"
//! convention); realized sales always carry their real sell rate.
//!
//! The journal reaching here is date-scoped (`-e`, future cutoff) but
//! NOT account-filtered: the queues need every transaction — a lot
//! bought on one account migrates to another via transfers. The account
//! pattern only selects which positions are *shown*.

use std::collections::{BTreeSet, HashMap};

use colored::Colorize;

use crate::commands::util::format_amount;
use crate::date::Date;
use crate::decimal::Decimal;
use crate::filter::PatternMatcher;
use crate::loader::Journal;
use crate::lotter::{OpenLot, Realization};

/// Far-future sentinel: `find(commodity, target, LATEST)` returns the
/// most recent rate on record (same convention as the revaluator).
const LATEST: &str = "9999-12-31";

/// Column order: Date, Size, Entry, Cost, Exit, Value, Profit, ROI,
/// ROI/m, Held.
const COLS: usize = 10;
const HEADER: [&str; COLS] =
    ["Date", "Size", "Entry", "Cost", "Exit", "Value", "Profit", "ROI", "ROI/m", "Held"];

#[derive(Clone, Copy, PartialEq)]
enum Tone {
    Plain,
    Dim,
    Red,
    Green,
}

#[derive(Clone)]
struct Cell {
    text: String,
    tone: Tone,
}

impl Cell {
    fn empty() -> Self {
        Cell { text: String::new(), tone: Tone::Plain }
    }
    fn plain(text: String) -> Self {
        Cell { text, tone: Tone::Plain }
    }
    fn dim(text: String) -> Self {
        Cell { text, tone: Tone::Dim }
    }
    /// Green for a gain, red for a loss — the profit/ROI colouring.
    fn signed(text: String, negative: bool) -> Self {
        Cell { text, tone: if negative { Tone::Red } else { Tone::Green } }
    }
}

// A report holds a few hundred lines at most — boxing the row array
// would buy nothing but indirection.
#[allow(clippy::large_enum_variant)]
enum Line {
    Account(String),
    Row([Cell; COLS]),
    Rule,
}

/// The numeric content of one row, kept for the section summaries: the
/// (absolute) quantity, and cost / value / profit where derivable.
struct Nums {
    qty: Decimal,
    cost: Option<Decimal>,
    value: Option<Decimal>,
    profit: Option<Decimal>,
}

pub fn run(
    journal: &Journal,
    target: Option<&str>,
    unrealized: bool,
    as_of: Date,
    begin: Option<Date>,
    patterns: &[String],
) {
    let open = crate::lotter::open_lots(&journal.transactions, target, &journal.prices);
    let sold = crate::lotter::realizations(&journal.transactions, target, &journal.prices);
    let matcher = (!patterns.is_empty()).then(|| PatternMatcher::from_parts(patterns));
    // Positions are the unit of selection, so `com XMR` narrows to the
    // commodity — the same pattern semantics `signal` uses.
    let keep = |account: &str, commodity: &str| {
        matcher.as_ref().is_none_or(|m| m.matches_position(account, commodity))
    };
    let open: Vec<&OpenLot> = open.iter().filter(|l| keep(&l.account, &l.commodity)).collect();
    let sold: Vec<&Realization> = sold
        .iter()
        .filter(|r| keep(&r.account, &r.commodity))
        .filter(|r| begin.is_none_or(|b| r.sell_date >= b))
        .collect();

    // Every column caps at 8 display decimals — a 12- or 18-decimal
    // tail buries the number, in the quantities as much as in the
    // XMR-target value columns.
    let mut qty_prec = journal.precisions.clone();
    for v in qty_prec.values_mut() {
        *v = (*v).min(8);
    }

    let money = Money { target, precisions: &qty_prec };

    // Native balance per (account, commodity), summed over the same
    // postings the tracker sees (real + bracket-virtual) — the yardstick
    // the open lots must add up to; the difference is held quantity
    // without a basis.
    let mut balances: HashMap<(String, String), Decimal> = HashMap::new();
    for lt in &journal.transactions {
        for lp in &lt.value.postings {
            if lp.value.is_virtual && !lp.value.balanced {
                continue;
            }
            if let Some(a) = &lp.value.amount {
                *balances
                    .entry((lp.value.account.clone(), a.commodity.clone()))
                    .or_insert_with(Decimal::zero) += a.value;
            }
        }
    }

    // Every position that has something to show, in sorted order.
    let keys: BTreeSet<(&str, &str)> = open
        .iter()
        .map(|l| (l.account.as_str(), l.commodity.as_str()))
        .chain(sold.iter().map(|r| (r.account.as_str(), r.commodity.as_str())))
        .collect();

    let mut lines: Vec<Line> = Vec::new();
    let mut totals: Vec<Nums> = Vec::new();
    // Earliest acquisition anywhere in the report — the Total line's
    // holding period runs from there to the report's as-of day, i.e. it
    // asks what the whole book returned per month if everything still
    // open were sold today.
    let mut first_buy: Option<Date> = None;
    let mut summaries = 0usize;
    let mut last_account = "";

    for (account, commodity) in keys {
        if account != last_account {
            lines.push(Line::Account(account.to_string()));
            last_account = account;
        }

        let position_open: Vec<&&OpenLot> = open
            .iter()
            .filter(|l| l.account == account && l.commodity == commodity)
            .collect();

        // Held quantity no lot covers — arrivals whose source carried
        // no basis (history starting mid-way, fee splits of basis-less
        // balance). Shown as its own line so the Open total reconciles
        // with the account balance instead of silently under-reporting.
        let held_qty = balances
            .get(&(account.to_string(), commodity.to_string()))
            .copied()
            .unwrap_or_else(Decimal::zero);
        let lot_qty = position_open
            .iter()
            .fold(Decimal::zero(), |a, l| a + l.qty);
        let residual = held_qty - lot_qty;
        let full_prec = journal.precisions.get(commodity).copied().unwrap_or(2);
        let show_residual = !residual.is_display_zero(full_prec);

        // Realized sales of this position, one line per disposal; the
        // Sold summary spans earliest buy to latest sale.
        let mut section: Vec<Nums> = Vec::new();
        let mut buy_min: Option<Date> = None;
        let mut sell_max: Option<Date> = None;
        for r in sold.iter().filter(|r| r.account == account && r.commodity == commodity) {
            buy_min = Some(buy_min.map_or(r.first_buy_date, |m: Date| m.min(r.first_buy_date)));
            first_buy = Some(first_buy.map_or(r.first_buy_date, |m: Date| m.min(r.first_buy_date)));
            sell_max = Some(sell_max.map_or(r.sell_date, |m: Date| m.max(r.sell_date)));
            let (cells, nums) = sale_row(r, &money, &qty_prec);
            lines.push(Line::Row(cells));
            section.push(nums);
        }
        if !section.is_empty() {
            let held = buy_min.zip(sell_max);
            let (cells, sum) =
                summary_row("Sold", &section, commodity, true, held, &money, &qty_prec);
            lines.push(Line::Row(cells));
            totals.push(sum);
            summaries += 1;
            if !position_open.is_empty() || show_residual {
                lines.push(Line::Rule);
            }
        }

        // Open lots of this position, FIFO order. The spot rate prices
        // their Sell/Value columns only under `--unrealized`.
        let spot = target
            .filter(|_| unrealized)
            .and_then(|t| journal.prices.find(commodity, t, LATEST));
        let mut section: Vec<Nums> = Vec::new();
        let mut open_min: Option<Date> = None;
        for l in position_open {
            open_min = Some(open_min.map_or(l.date, |m: Date| m.min(l.date)));
            first_buy = Some(first_buy.map_or(l.date, |m: Date| m.min(l.date)));
            let (cells, nums) = open_row(l, spot, as_of, &money, &qty_prec);
            lines.push(Line::Row(cells));
            section.push(nums);
        }
        if show_residual {
            let value = spot.map(|s| residual.mul_rounded(s));
            let vc = target.unwrap_or(commodity);
            let mut cells: [Cell; COLS] = std::array::from_fn(|_| Cell::empty());
            cells[0] = Cell::dim("-".to_string());
            let text = format_amount(commodity, &residual, &qty_prec);
            cells[1] = if residual.is_negative() {
                Cell { text, tone: Tone::Red }
            } else {
                Cell::plain(text)
            };
            if let Some(s) = &spot {
                cells[4] = Cell::plain(money.rate(s, vc));
            }
            if let Some(v) = &value {
                cells[5] = Cell::plain(money.rate(v, vc));
            }
            lines.push(Line::Row(cells));
            section.push(Nums { qty: residual, cost: None, value, profit: None });
        }
        if !section.is_empty() {
            let held = open_min.map(|m| (m, as_of));
            let (cells, sum) =
                summary_row("Open", &section, commodity, false, held, &money, &qty_prec);
            lines.push(Line::Row(cells));
            totals.push(sum);
            summaries += 1;
        }
        lines.push(Line::Rule);
    }

    // A grand total only where it adds up: a single valuation commodity
    // (`-X`) and more than one summary to combine.
    if target.is_some() && summaries > 1 {
        let span = first_buy.map(|f| (f, as_of));
        let (cells, _) = summary_row("Total", &totals, "", false, span, &money, &qty_prec);
        let mut cells = cells;
        cells[1] = Cell::empty(); // quantities across commodities don't add
        cells[2] = Cell::empty();
        cells[4] = Cell::empty();
        lines.push(Line::Row(cells));
    }

    print_lines(&lines);
    if !lines.is_empty() {
        println!();
    }

    let (n_open, n_sold) = (open.len(), sold.len());
    if n_open == 0 && n_sold == 0 {
        println!("{} no lots", "!".yellow());
    } else {
        println!(
            "{} {} open lot{}, {} sale{}",
            "✓".green(),
            n_open,
            if n_open == 1 { "" } else { "s" },
            n_sold,
            if n_sold == 1 { "" } else { "s" },
        );
    }
}

/// Value renderer: every cell carries its commodity symbol, so a row
/// reads self-contained (`$334.01`, `XMR0.02586727`).
struct Money<'a> {
    target: Option<&'a str>,
    precisions: &'a HashMap<String, usize>,
}

impl Money<'_> {
    fn rate(&self, v: &Decimal, commodity: &str) -> String {
        format_amount(commodity, v, self.precisions)
    }
}

/// Percent return of `profit` on `cost`, one decimal, explicit sign.
fn roi_cell(profit: &Decimal, cost: &Decimal) -> Cell {
    if cost.is_zero() {
        return Cell::empty();
    }
    let hundred = Decimal::parse("100").expect("literal");
    let pct = profit.mul_rounded(hundred).div_rounded(cost.abs());
    Cell::signed(format!("{}%", pct.format_decimal(1)), pct.is_negative())
}

/// Days in an average month — the Gregorian year's 365.2425 over
/// twelve, so a holding period converts to months without a calendar
/// walk.
const DAYS_PER_MONTH: f64 = 30.436875;

/// The same return restated as a MONTHLY rate: the rate that, compounded
/// over the holding period, yields exactly it — `(1 + roi)^(1/months) −
/// 1`. Only this makes two positions of unequal holding period
/// comparable, since 60% over three months is a different investment
/// from 60% over twelve. Dividing instead of taking the root would
/// overstate every span beyond a month.
///
/// The root has no exact decimal form, so it runs in `f64`. That is
/// sound here because the figure is display-only and derived from two
/// exact decimals; nothing downstream computes with it. A position that
/// lost more than its cost has no real root and stays empty, as does a
/// span of less than a day.
fn roi_month_cell(profit: &Decimal, cost: &Decimal, from: Date, to: Date) -> Cell {
    let days = from.days_until(to);
    if cost.is_zero() || days <= 0 {
        return Cell::empty();
    }
    let factor = 1.0 + profit.to_f64() / cost.abs().to_f64();
    if factor <= 0.0 {
        return Cell::empty();
    }
    let monthly = factor.powf(DAYS_PER_MONTH / days as f64) - 1.0;
    let pct = monthly * 100.0;
    Cell::signed(format!("{:.1}%", pct), pct < 0.0)
}

/// One realized sale: Date is the sell date, Size the sold quantity
/// (negative — it left the account; positive for a short cover), Entry /
/// Exit the booked per-unit rates, Held the holding period from the
/// earliest lot the sale pulled.
fn sale_row(
    r: &Realization,
    money: &Money,
    qty_prec: &HashMap<String, usize>,
) -> ([Cell; COLS], Nums) {
    let vc = &r.value_commodity;
    // Under `-X` the booked figures are the real payments (empty where
    // no trade rate was derivable); natively the market figures already
    // ARE the booked trade values.
    let (cost, value) = match money.target {
        Some(_) => (r.booked_cost, r.booked_proceeds),
        None => (Some(r.market_cost), Some(r.market_proceeds)),
    };
    // A long sale gains when it sold above cost; a short cover gains
    // when it bought back below the opening rate.
    let profit = cost
        .zip(value)
        .map(|(c, v)| if r.short { c - v } else { v - c });
    let shown_qty = if r.short { r.qty } else { Decimal::zero() - r.qty };

    let mut cells = [
        Cell::dim(r.sell_date.to_string()),
        {
            let text = format_amount(&r.commodity, &shown_qty, qty_prec);
            if shown_qty.is_negative() { Cell { text, tone: Tone::Red } } else { Cell::plain(text) }
        },
        Cell::empty(),
        Cell::empty(),
        Cell::empty(),
        Cell::empty(),
        Cell::empty(),
        Cell::empty(),
        Cell::empty(),
        Cell::dim(age(r.first_buy_date, r.sell_date)),
    ];
    if let Some(c) = &cost {
        cells[2] = Cell::plain(money.rate(&c.div_rounded(r.qty), vc));
        cells[3] = Cell::plain(money.rate(c, vc));
    }
    if let Some(v) = &value {
        cells[4] = Cell::plain(money.rate(&v.div_rounded(r.qty), vc));
        cells[5] = Cell::plain(money.rate(v, vc));
    }
    if let Some(p) = &profit {
        cells[6] = Cell::signed(money.rate(p, vc), p.is_negative());
        if let Some(c) = &cost {
            cells[7] = roi_cell(p, c);
            cells[8] = roi_month_cell(p, c, r.first_buy_date, r.sell_date);
        }
    }
    (cells, Nums { qty: r.qty, cost, value, profit })
}

/// One open lot: Date is the acquisition date, Entry/Cost the booked
/// entry, Exit/Value the spot pricing (only under `--unrealized`), Held
/// the holding period to the report's as-of day.
fn open_row(
    l: &OpenLot,
    spot: Option<Decimal>,
    as_of: Date,
    money: &Money,
    qty_prec: &HashMap<String, usize>,
) -> ([Cell; COLS], Nums) {
    // Under `-X` values live in the target; natively the booked rate is
    // the lot's own cost commodity.
    let vc = money.target.unwrap_or(&l.cost_commodity);
    let booked = match money.target {
        Some(_) => l.booked_per_unit,
        None => Some(l.cost_per_unit),
    };
    let cost = booked.map(|b| l.qty.mul_rounded(b));
    let value = spot.map(|s| l.qty.mul_rounded(s));
    let profit = cost.zip(value).map(|(c, v)| v - c);

    let mut cells = [
        Cell::dim(l.date.to_string()),
        {
            let text = format_amount(&l.commodity, &l.qty, qty_prec);
            if l.qty.is_negative() { Cell { text, tone: Tone::Red } } else { Cell::plain(text) }
        },
        Cell::empty(),
        Cell::empty(),
        Cell::empty(),
        Cell::empty(),
        Cell::empty(),
        Cell::empty(),
        Cell::empty(),
        Cell::dim(age(l.date, as_of)),
    ];
    if let Some(b) = &booked {
        cells[2] = Cell::plain(money.rate(b, vc));
    }
    if let Some(c) = &cost {
        cells[3] = Cell::plain(money.rate(c, vc));
    }
    if let Some(s) = &spot {
        cells[4] = Cell::plain(money.rate(s, vc));
    }
    if let Some(v) = &value {
        cells[5] = Cell::plain(money.rate(v, vc));
    }
    if let Some(p) = &profit {
        cells[6] = Cell::signed(money.rate(p, vc), p.is_negative());
        if let Some(c) = &cost {
            cells[7] = roi_cell(p, c);
            cells[8] = roi_month_cell(p, c, l.date, as_of);
        }
    }
    (cells, Nums { qty: l.qty, cost, value, profit })
}

/// A labelled summary over a section's rows: total quantity, average
/// buy/sell over the covered rows, summed cost/value/profit and the
/// resulting ROI. Cells whose inputs are entirely missing stay empty.
fn summary_row(
    label: &str,
    rows: &[Nums],
    commodity: &str,
    negate_qty: bool,
    held: Option<(Date, Date)>,
    money: &Money,
    qty_prec: &HashMap<String, usize>,
) -> ([Cell; COLS], Nums) {
    let vc = money.target.unwrap_or("");
    let qty: Decimal = rows.iter().fold(Decimal::zero(), |a, r| a + r.qty);
    let shown_qty = if negate_qty { Decimal::zero() - qty } else { qty };
    let sum = |f: fn(&Nums) -> Option<Decimal>| -> Option<Decimal> {
        let vals: Vec<Decimal> = rows.iter().filter_map(f).collect();
        (!vals.is_empty()).then(|| vals.iter().fold(Decimal::zero(), |a, v| a + *v))
    };
    // Covered quantity per column: averages divide only by the rows
    // that actually carried the figure.
    let covered = |f: fn(&Nums) -> Option<Decimal>| -> Decimal {
        rows.iter()
            .filter(|r| f(r).is_some())
            .fold(Decimal::zero(), |a, r| a + r.qty)
    };
    let cost = sum(|r| r.cost);
    let value = sum(|r| r.value);
    let profit = sum(|r| r.profit);

    let mut cells = [
        Cell::plain(label.to_string()),
        if commodity.is_empty() {
            Cell::empty()
        } else {
            let text = format_amount(commodity, &shown_qty, qty_prec);
            if shown_qty.is_negative() { Cell { text, tone: Tone::Red } } else { Cell::plain(text) }
        },
        Cell::empty(),
        Cell::empty(),
        Cell::empty(),
        Cell::empty(),
        Cell::empty(),
        Cell::empty(),
        Cell::empty(),
        held.map(|(f, t)| Cell::dim(age(f, t))).unwrap_or_else(Cell::empty),
    ];
    if money.target.is_some() {
        if let Some(c) = &cost {
            let q = covered(|r| r.cost);
            if !q.is_zero() {
                cells[2] = Cell::plain(money.rate(&c.div_rounded(q), vc));
            }
            cells[3] = Cell::plain(money.rate(c, vc));
        }
        if let Some(v) = &value {
            let q = covered(|r| r.value);
            if !q.is_zero() {
                cells[4] = Cell::plain(money.rate(&v.div_rounded(q), vc));
            }
            cells[5] = Cell::plain(money.rate(v, vc));
        }
        if let Some(p) = &profit {
            cells[6] = Cell::signed(money.rate(p, vc), p.is_negative());
            if let Some(c) = &cost {
                cells[7] = roi_cell(p, c);
                if let Some((f, t)) = held {
                    cells[8] = roi_month_cell(p, c, f, t);
                }
            }
        }
    }
    (cells, Nums { qty, cost, value, profit })
}

/// Coarse holding-period age: days below ~a month, months below a year,
/// then years with a month remainder (`12d`, `3m`, `1y3m`).
fn age(from: Date, to: Date) -> String {
    let days = from.days_until(to).max(0);
    if days < 32 {
        return format!("{}d", days);
    }
    let months = days / 30;
    if months < 12 {
        return format!("{}m", months);
    }
    let (years, rem) = (months / 12, months % 12);
    if rem == 0 { format!("{}y", years) } else { format!("{}y{}m", years, rem) }
}

/// Align and print. Every account opens its own table — the name stands
/// above it, then the header and rule — but all tables share one global
/// set of column widths so they stay aligned under each other. Columns
/// nobody filled are dropped entirely (e.g. Exit/Value/Profit/ROI
/// without `--unrealized` and sales).
fn print_lines(lines: &[Line]) {
    // Active columns: Date, Size and Age always; a value column only
    // when some row filled it.
    let mut active = [false; COLS];
    active[0] = true;
    active[1] = true;
    active[9] = true;
    for line in lines {
        if let Line::Row(cells) = line {
            for (i, c) in cells.iter().enumerate() {
                if !c.text.is_empty() {
                    active[i] = true;
                }
            }
        }
    }

    let mut widths = [0usize; COLS];
    for (i, h) in HEADER.iter().enumerate() {
        widths[i] = h.chars().count();
    }
    for line in lines {
        if let Line::Row(cells) = line {
            for (i, c) in cells.iter().enumerate() {
                widths[i] = widths[i].max(c.text.chars().count());
            }
        }
    }

    let total_width: usize = (0..COLS).filter(|&i| active[i]).map(|i| widths[i]).sum::<usize>()
        + 2 * (0..COLS).filter(|&i| active[i]).count().saturating_sub(1);

    let render = |cells: &[Cell; COLS]| -> String {
        let mut parts: Vec<String> = Vec::new();
        for i in 0..COLS {
            if !active[i] {
                continue;
            }
            let pad = widths[i].saturating_sub(cells[i].text.chars().count());
            // Date left-aligned, everything else right-aligned; the
            // bare `-` of a no-basis line aligns right like a number.
            let padded = if i == 0 && cells[i].text != "-" {
                format!("{}{}", cells[i].text, " ".repeat(pad))
            } else {
                format!("{}{}", " ".repeat(pad), cells[i].text)
            };
            parts.push(match cells[i].tone {
                Tone::Plain => padded,
                Tone::Dim => padded.dimmed().to_string(),
                Tone::Red => padded.red().to_string(),
                Tone::Green => padded.green().to_string(),
            });
        }
        parts.join("  ").trim_end().to_string()
    };

    let header: [Cell; COLS] = std::array::from_fn(|i| Cell::dim(HEADER[i].to_string()));
    let mut prev_rule = false;
    for line in lines {
        match line {
            Line::Account(name) => {
                println!("{}", name.bold());
                println!("{}", render(&header));
                println!("{}", "\u{2500}".repeat(total_width).dimmed());
                prev_rule = false;
            }
            Line::Row(cells) => {
                println!("{}", render(cells));
                prev_rule = false;
            }
            Line::Rule => {
                if !prev_rule {
                    println!("{}", "\u{2500}".repeat(total_width).dimmed());
                }
                prev_rule = true;
            }
        }
    }
}
