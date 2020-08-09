use super::super::super::model::Posting;
use super::super::super::model::Transaction;
use super::super::format_amount;

use colored::Colorize;
use num::Signed;
use std::collections::BTreeMap;

pub(super) fn group_postings_by_account(
	transactions: Vec<Transaction>,
) -> Result<BTreeMap<String, BTreeMap<String, num::rational::Rational64>>, String> {
	let mut result = BTreeMap::<String, BTreeMap<String, num::rational::Rational64>>::new();

	for balanced_posting in transactions
		.into_iter()
		.flat_map(|transaction| transaction.postings.into_iter())
		.collect::<Vec<Posting>>()
	{
		result
			.entry(balanced_posting.account.to_owned())
			.and_modify(|tree| {
				tree
					.entry(
						balanced_posting
							.balanced_amount
							.as_ref()
							.expect("balanced amount not found")
							.commodity
							.to_owned(),
					)
					.and_modify(|value| {
						*value += balanced_posting
							.balanced_amount
							.as_ref()
							.expect("balanced amount not found")
							.value
					})
					.or_insert(
						balanced_posting
							.balanced_amount
							.as_ref()
							.expect("balanced amount not found")
							.value,
					);
			})
			.or_insert_with(|| {
				let mut tree: BTreeMap<String, num::rational::Rational64> = BTreeMap::new();
				tree.insert(
					balanced_posting
						.balanced_amount
						.as_ref()
						.expect("balanced amount not found")
						.commodity
						.to_string(),
					balanced_posting
						.balanced_amount
						.as_ref()
						.expect("balanced amount not found")
						.value,
				);
				tree
			});
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
