use super::cmd_printer::format_amount;
use super::cmd_printer_bal::group_postings_by_account;
use super::cmd_printer_bal::print_commodity_amount;
use super::model::BalancedPosting;
use super::model::Transaction;

use colored::Colorize;
use num::Zero;
use std::collections::BTreeMap;

pub fn print(transactions: Vec<&Transaction<BalancedPosting>>) -> Result<(), String> {
	let postings = group_postings_by_account(transactions)?;

	let total = postings
		.iter()
		.flat_map(|(_, amounts)| amounts.iter())
		.fold(
			BTreeMap::<String, num::rational::Rational64>::new(),
			|mut total, (commodity, amount)| {
				total
					.entry(commodity.to_owned())
					.and_modify(|a| *a += amount)
					.or_insert(*amount);
				total
			},
		);

	let width = postings
		.values()
		.flat_map(|a| a.iter())
		.map(|(k, v)| k.chars().count() + format_amount(&v).chars().count())
		.max()
		.unwrap_or(0);

	let width = std::cmp::max(
		width,
		total
			.iter()
			.map(|(c, a)| c.chars().count() + format_amount(&a).chars().count())
			.max()
			.unwrap_or(0),
	);

	for (account, amounts) in &postings {
		let mut it = amounts.iter().peekable();
		while let Some((commodity, amount)) = it.next() {
			print_commodity_amount(commodity, amount, width);
			if it.peek().is_some() {
				println!();
			}
		}
		println!("{}", account.blue());
	}

	for _ in 0..width {
		print!("-");
	}
	println!();

	if total.iter().all(|(_, a)| a.is_zero()) {
		println!("{:>w$} ", 0, w = width);
	} else {
		for (commodity, amount) in &total {
			print_commodity_amount(commodity, amount, width);
			println!();
		}
	}

	Ok(())
}
