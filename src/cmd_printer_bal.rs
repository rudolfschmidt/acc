use super::cmd_printer;
use super::model::Posting;
use super::model::Transaction;

use colored::Colorize;
use num::Signed;
use std::collections::BTreeMap;

pub fn group_postings_by_account(
	transactions: &[Transaction],
) -> Result<BTreeMap<String, BTreeMap<String, num::rational::Rational64>>, String> {
	let mut result = BTreeMap::<String, BTreeMap<String, num::rational::Rational64>>::new();

	for post in transactions
		.iter()
		.flat_map(|t| t.postings.iter())
		.collect::<Vec<&Posting>>()
	{
		match result.get_mut(&post.account) {
			Some(result_account) => {
				if result_account.contains_key(&post.balanced_amount.as_ref().unwrap().commodity) {
					result_account.insert(
						post.balanced_amount.as_ref().unwrap().commodity.to_owned(),
						result_account
							.get(&post.balanced_amount.as_ref().unwrap().commodity)
							.unwrap() + post.balanced_amount.as_ref().unwrap().amount,
					);
				} else {
					result_account.insert(
						post.balanced_amount.as_ref().unwrap().commodity.to_string(),
						post.balanced_amount.as_ref().unwrap().amount,
					);
				}
			}
			None => {
				let mut tree: BTreeMap<String, num::rational::Rational64> = BTreeMap::new();
				tree.insert(
					post.balanced_amount.as_ref().unwrap().commodity.to_string(),
					post.balanced_amount.as_ref().unwrap().amount,
				);
				result.insert(post.account.to_string(), tree);
			}
		}
	}
	Ok(result)
}

pub fn print_commodity_amount(
	commodity: &str,
	amount: &num::rational::Rational64,
	amount_width: usize,
) {
	let value = &cmd_printer::format_amount(amount);
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
