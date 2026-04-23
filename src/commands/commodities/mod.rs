//! `commodities` command — list every commodity observed in postings,
//! optionally with the earliest date on which it appeared. Runs after
//! the filter phase.

use std::collections::BTreeMap;

use crate::date::Date;
use crate::loader::Journal;
use crate::parser::posting::{Amount, Costs, Posting};

pub fn run(journal: &Journal, show_date: bool) {
    // commodity → earliest tx.date it occurred on
    let mut first_seen: BTreeMap<String, Date> = BTreeMap::new();

    for tx in &journal.transactions {
        for lp in &tx.value.postings {
            for commodity in commodities_in_posting(&lp.value) {
                first_seen
                    .entry(commodity)
                    .and_modify(|d| {
                        if tx.value.date < *d {
                            *d = tx.value.date;
                        }
                    })
                    .or_insert(tx.value.date);
            }
        }
    }

    if show_date {
        let mut entries: Vec<(&String, &Date)> = first_seen.iter().collect();
        entries.sort_by(|a, b| a.1.cmp(b.1).then_with(|| a.0.cmp(b.0)));
        let width = entries
            .iter()
            .map(|(c, _)| c.chars().count())
            .max()
            .unwrap_or(0);
        for (commodity, date) in entries {
            println!("{:<w$}  {}", commodity, date, w = width);
        }
    } else {
        for commodity in first_seen.keys() {
            println!("{}", commodity);
        }
    }
}

fn commodities_in_posting(p: &Posting) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(a) = &p.amount {
        out.push(a.commodity.clone());
    }
    if let Some(c) = &p.costs {
        out.push(cost_commodity(c));
    }
    out
}

fn cost_commodity(costs: &Costs) -> String {
    match costs {
        Costs::PerUnit(Amount { commodity, .. }) | Costs::Total(Amount { commodity, .. }) => {
            commodity.clone()
        }
    }
}
