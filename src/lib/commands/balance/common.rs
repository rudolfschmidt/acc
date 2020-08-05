use super::super::super::model::BalancedPosting;
use super::super::super::model::Transaction;
use super::super::format_amount;

use colored::Colorize;
use num::Signed;
use std::collections::BTreeMap;

pub(super) fn group_postings_by_account(
	transactions: Vec<Transaction>,
) -> Result<BTreeMap<String, BTreeMap<String, num::rational::Rational64>>, String> {
	let mut result = BTreeMap::<String, BTreeMap<String, num::rational::Rational64>>::new();

	for post in transactions
		.iter()
		.flat_map(|t| t.balanced_postings.iter())
		.collect::<Vec<&BalancedPosting>>()
	{
		match result.get_mut(&post.unbalanced_posting.account) {
			Some(result_account) => {
				if result_account.contains_key(&post.balanced_amount.commodity) {
					result_account.insert(
						post.balanced_amount.commodity.to_owned(),
						result_account.get(&post.balanced_amount.commodity).unwrap()
							+ post.balanced_amount.value,
					);
				} else {
					result_account.insert(
						post.balanced_amount.commodity.to_string(),
						post.balanced_amount.value,
					);
				}
			}
			None => {
				let mut tree: BTreeMap<String, num::rational::Rational64> = BTreeMap::new();
				tree.insert(
					post.balanced_amount.commodity.to_string(),
					post.balanced_amount.value,
				);
				result.insert(post.unbalanced_posting.account.to_string(), tree);
			}
		}
	}
	Ok(result)
}

pub(super) fn print_commodity_amount(
	commodity: &str,
	amount: &num::rational::Rational64,
	amount_width: usize,
) {
	let value = &format_amount(amount);
	if amount.is_negative() {
		print!(
			"{:>w$} ",
			(commodity.to_string() + value).red(),
			w = amount_width
		);
	} else {
		print!("{:>w$} ", commodity.to_string() + value, w = amount_width);
	}
}
