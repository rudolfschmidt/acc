use super::cmd_printer;
use super::model::BalancedPosting;
use super::model::Transaction;

use colored::Colorize;
use num::Signed;
use std::collections::BTreeMap;

pub fn group_postings_by_account(
	transactions: Vec<&Transaction<BalancedPosting>>,
) -> Result<BTreeMap<String, BTreeMap<String, num::rational::Rational64>>, String> {
	let mut result = BTreeMap::<String, BTreeMap<String, num::rational::Rational64>>::new();

	for post in transactions
		.iter()
		.flat_map(|t| t.postings.iter())
		.collect::<Vec<&BalancedPosting>>()
	{
		match result.get_mut(&post.account) {
			Some(result_account) => {
				if result_account.contains_key(&post.commodity) {
					result_account.insert(
						post.commodity.to_owned(),
						result_account.get(&post.commodity).unwrap() + post.amount,
					);
				} else {
					result_account.insert(post.commodity.to_string(), post.amount);
				}
			}
			None => {
				let mut tree: BTreeMap<String, num::rational::Rational64> = BTreeMap::new();
				tree.insert(post.commodity.to_string(), post.amount);
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
