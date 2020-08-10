use super::super::super::model::Item;
use super::super::super::model::Posting;
use super::super::format_amount;

use colored::Colorize;
use num::Signed;
use std::collections::BTreeMap;

pub(super) fn group_postings_by_account(
	items: Vec<Item>,
) -> Result<BTreeMap<String, BTreeMap<String, num::rational::Rational64>>, String> {
	let mut result = BTreeMap::<String, BTreeMap<String, num::rational::Rational64>>::new();

	for posting in items
		.into_iter()
		.filter_map(|item| match item {
			Item::Transaction { postings, .. } => Some(postings),
			_ => None,
		})
		.flat_map(|postings| postings.into_iter())
		.collect::<Vec<Posting>>()
	{
		match posting {
			Posting::BalancedPosting {
				account,
				balanced_amount,
				..
			} => {
				result
					.entry(account.to_owned())
					.and_modify(|tree| {
						tree
							.entry(balanced_amount.commodity.to_owned())
							.and_modify(|value| *value += balanced_amount.value)
							.or_insert(balanced_amount.value);
					})
					.or_insert_with(|| {
						let mut tree: BTreeMap<String, num::rational::Rational64> = BTreeMap::new();
						tree.insert(balanced_amount.commodity.to_string(), balanced_amount.value);
						tree
					});
			}
			Posting::EquityPosting { account, amount } => {
				result
					.entry(account.to_owned())
					.and_modify(|tree| {
						tree
							.entry(amount.commodity.to_owned())
							.and_modify(|value| *value += amount.value)
							.or_insert(amount.value);
					})
					.or_insert_with(|| {
						let mut tree: BTreeMap<String, num::rational::Rational64> = BTreeMap::new();
						tree.insert(amount.commodity.to_string(), amount.value);
						tree
					});
			}
			_ => {}
		}
	}
	Ok(result)
}

pub(super) fn print_commodity_amount(
	commodity: &str,
	amount: num::rational::Rational64,
	amount_width: usize,
) {
	let value = format_amount(&amount);
	if amount.is_negative() {
		print!(
			"{:>w$} ",
			(commodity.to_string() + &value).red(),
			w = amount_width
		);
	} else {
		print!("{:>w$} ", commodity.to_string() + &value, w = amount_width);
	}
}
