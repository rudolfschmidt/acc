//! Exchange-rate database — the output of the indexer phase.
//!
//! Layout: `from → to → BTreeMap<day, rate>`. Two HashMap probes land
//! on a per-pair time series; a range query on that BTreeMap picks
//! the latest rate at or before the requested day. Commodity symbols
//! are compared case-sensitively — `USD` and `usd` are distinct.
//! Dates are stored as `u32` days-since-epoch so comparison is an
//! integer op.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::Arc;

use crate::decimal::Decimal;

#[derive(Debug, Default)]
pub struct Index {
    prices: HashMap<Arc<str>, HashMap<Arc<str>, BTreeMap<u32, Decimal>>>,
}

impl Index {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.prices
            .values()
            .flat_map(|inner| inner.values())
            .map(|m| m.len())
            .sum()
    }

    pub fn is_empty(&self) -> bool {
        self.prices.is_empty()
    }

    /// Store a single rate. Only the given direction is stored — the
    /// inverse is computed on demand by `find`.
    pub(super) fn add(&mut self, from: Arc<str>, to: Arc<str>, day: u32, rate: Decimal) {
        if rate.is_zero() || from == to {
            return;
        }
        self.prices
            .entry(from)
            .or_default()
            .entry(to)
            .or_default()
            .insert(day, rate);
    }

    /// Rate for `from → to` at or before `date`. Uses BFS over the
    /// commodity graph so multi-hop paths (e.g. USD → CHF → EUR) work
    /// when direct edges are missing. Every stored edge is reversible
    /// on demand via its reciprocal.
    pub fn find(&self, from: &str, to: &str, date: &str) -> Option<Decimal> {
        if from == to {
            return Some(Decimal::from(1));
        }
        let day = crate::date::Date::parse(date).ok()?.days();
        if let Some(rate) = self.edge_rate(from, to, day) {
            return Some(rate);
        }
        let mut visited: HashSet<&str> = HashSet::new();
        let mut queue: VecDeque<(&str, Decimal)> = VecDeque::new();
        visited.insert(from);
        queue.push_back((from, Decimal::from(1)));
        while let Some((current, rate_so_far)) = queue.pop_front() {
            for next in self.neighbors(current) {
                if visited.contains(next) {
                    continue;
                }
                let Some(edge) = self.edge_rate(current, next, day) else {
                    continue;
                };
                // mul_rounded because inverse-rate edges served by
                // `edge_rate` can carry a 28-digit tail; strict `*`
                // would panic when chaining such edges.
                let combined = rate_so_far.mul_rounded(edge);
                if next == to {
                    return Some(combined);
                }
                visited.insert(next);
                queue.push_back((next, combined));
            }
        }
        None
    }

    fn neighbors<'a>(&'a self, from: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        let forward = self
            .prices
            .get(from)
            .into_iter()
            .flat_map(|m| m.keys().map(|a| a.as_ref()));
        let reverse = self.prices.iter().filter_map(move |(src, m)| {
            m.keys().any(|k| k.as_ref() == from).then(|| src.as_ref())
        });
        forward.chain(reverse)
    }

    fn edge_rate(&self, from: &str, to: &str, day: u32) -> Option<Decimal> {
        if let Some(dates) = self.prices.get(from).and_then(|m| m.get(to)) {
            return latest_rate(dates, day);
        }
        let reverse = self.prices.get(to).and_then(|m| m.get(from))?;
        let rate = latest_rate(reverse, day)?;
        Some(Decimal::from(1).div_rounded(rate))
    }
}

/// Latest rate with `date_key ≤ day`, or the earliest-known rate as a
/// fallback if the requested day is before any stored entry.
fn latest_rate(dates: &BTreeMap<u32, Decimal>, day: u32) -> Option<Decimal> {
    if dates.is_empty() {
        return None;
    }
    if let Some((_, rate)) = dates.range(..=day).next_back() {
        return Some(*rate);
    }
    dates.iter().next().map(|(_, rate)| *rate)
}
