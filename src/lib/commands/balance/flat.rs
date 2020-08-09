use super::super::format_amount;
use super::common::group_postings_by_account;
use super::common::print_commodity_amount;

use super::super::super::model::Transaction;
use colored::Colorize;
use num::Zero;
use std::collections::BTreeMap;

pub(super) fn print(transactions: Vec<Transaction>) -> Result<(), String> {
	if transactions
		.iter()
		.any(|transaction| transaction.postings.is_empty())
	{
		return Ok(());
	}
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

	let width = std::cmp::max(
		postings
			.values()
			.flat_map(|accounts| accounts.iter())
			.map(|(commodity, value)| commodity.chars().count() + format_amount(value).chars().count())
			.max()
			.unwrap_or(0),
		total
			.iter()
			.map(|(commodity, value)| commodity.chars().count() + format_amount(value).chars().count())
			.max()
			.unwrap_or(0),
	);

	for (account, amounts) in postings {
		let mut it = amounts.iter().peekable();
		while let Some((commodity, amount)) = it.next() {
			if !amount.is_zero() {
				print_commodity_amount(commodity, *amount, width);
				if it.peek().is_some() {
					println!();
				}
			}
		}
		if !amounts.iter().all(|(_, value)| value.is_zero()) {
			println!("{}", account.blue());
		}
	}

	for _ in 0..width {
		print!("-");
	}
	println!();

	if total.iter().all(|(_, a)| a.is_zero()) {
		println!("{:>w$} ", 0, w = width);
	} else {
		for (commodity, amount) in &total {
			if !amount.is_zero() {
				print_commodity_amount(commodity, *amount, width);
				println!();
			}
		}
	}

	Ok(())
}
