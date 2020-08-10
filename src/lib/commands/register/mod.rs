use super::super::model::Item;
use super::super::model::Posting;
use super::super::model::State;

use colored::Colorize;
use num::Signed;
use num::Zero;
use std::collections::BTreeMap;

const WIDTH_OFFSET: usize = 4;

type Rational = num::rational::Rational64;

struct Row {
	title: String,
	accounts: Vec<Account>,
}
struct Account {
	account: String,
	commodity: String,
	amount: String,
	total: BTreeMap<String, Rational>,
}

// Maybe I consider the terminal width in the future
// let terminal_width = std::process::Command::new("sh")
// .arg("-c")
// .arg("tput cols")
// .output()
// .expect("failed to fetch terminal width");

pub fn print(items: Vec<Item>) -> Result<(), String> {
	let mut rows = Vec::new();

	let mut total = BTreeMap::new();

	for item in items {
		match item {
			Item::Transaction {
				date,
				state,
				description,
				postings,
				..
			} if !postings.is_empty() => {
				let mut row = Row {
					title: format!(
						"{}{}{}",
						date,
						match state {
							State::Cleared => " * ",
							State::Uncleared => " ",
							State::Pending => " ! ",
						},
						description
					),
					accounts: Vec::new(),
				};
				for posting in postings {
					match posting {
						Posting::BalancedPosting {
							account,
							balanced_amount,
							..
						} => {
							total
								.entry(balanced_amount.commodity.to_owned())
								.and_modify(|a| *a += balanced_amount.value)
								.or_insert(balanced_amount.value);
							row.accounts.push(Account {
								account,
								commodity: balanced_amount.commodity.to_owned(),
								amount: super::format_amount(&balanced_amount.value),
								total: total
									.iter()
									.fold(BTreeMap::new(), |mut acc, (commodity, amount)| {
										acc.insert(commodity.to_owned(), *amount);
										acc
									}),
							});
						}
						Posting::EquityPosting { account, amount } => {
							total
								.entry(amount.commodity.to_owned())
								.and_modify(|a| *a += amount.value)
								.or_insert(amount.value);
							row.accounts.push(Account {
								account,
								commodity: amount.commodity.to_owned(),
								amount: super::format_amount(&amount.value),
								total: total
									.iter()
									.fold(BTreeMap::new(), |mut acc, (commodity, amount)| {
										acc.insert(commodity.to_owned(), *amount);
										acc
									}),
							})
						}
						_ => {}
					}
				}
				rows.push(row);
			}
			_ => {}
		}
	}

	let header_width = rows
		.iter()
		.map(|t| t.title.chars().count())
		.max()
		.unwrap_or(0);

	let account_width = rows
		.iter()
		.flat_map(|t| t.accounts.iter())
		.map(|a| a.account.chars().count())
		.max()
		.unwrap_or(0);

	let amount_width = rows
		.iter()
		.flat_map(|t| t.accounts.iter())
		.map(|a| a.commodity.chars().count() + a.amount.chars().count())
		.max()
		.unwrap_or(0);

	let total_amount_width = rows
		.iter()
		.flat_map(|t| t.accounts.iter())
		.flat_map(|a| a.total.iter())
		.map(|(c, a)| c.chars().count() + super::format_amount(a).chars().count())
		.max()
		.unwrap_or(0);

	for row in rows {
		print!(
			"{:<header_width$}",
			row.title,
			header_width = header_width + WIDTH_OFFSET
		);

		for (index, account) in row.accounts.iter().enumerate() {
			if index > 0 {
				println!();
				for _ in 0..header_width + WIDTH_OFFSET {
					print!(" ");
				}
			}

			print!(
				"{:<account_width$}",
				account.account.blue(),
				account_width = account_width + WIDTH_OFFSET
			);

			let commodity = &account.commodity;
			let amount = &account.amount;

			if amount.starts_with('-') {
				print!(
					"{}",
					format_commodity_amount(&commodity, &amount, amount_width).red()
				);
			} else {
				print!(
					"{}",
					format_commodity_amount(&commodity, &amount, amount_width)
				);
			}

			if account
				.total
				.iter()
				.map(|(_, amount)| amount)
				.all(|amount| amount.is_zero())
			{
				print!("{:>w$}", "0", w = total_amount_width);
			} else {
				let mut total_iter = account.total.iter().filter(|(_, amount)| !amount.is_zero());
				if let Some((total_commodity, total_amount)) = total_iter.next() {
					if total_amount.is_zero() {
						print!(
							"{:>w$}",
							format_with_zero(total_amount),
							w = total_amount_width
						)
					} else if total_amount.is_negative() {
						print!(
							"{}",
							format_total_commodity_amount(
								total_commodity,
								&format_with_zero(total_amount),
								total_amount_width
							)
							.red()
						);
					} else {
						print!(
							"{}",
							format_total_commodity_amount(
								total_commodity,
								&format_with_zero(total_amount),
								total_amount_width
							)
						);
					}
				}
				for (total_commodity, total_amount) in total_iter {
					if total_amount.is_zero() {
						print!(
							"{}",
							format_total_amount_offset(
								&format_with_zero(total_amount),
								header_width,
								account_width,
								amount_width,
								total_amount_width
							)
						);
					} else if total_amount.is_negative() {
						print!(
							"{}",
							format_total_commodity_amount_offset(
								total_commodity,
								&format_with_zero(total_amount),
								header_width,
								account_width,
								amount_width,
								total_amount_width
							)
							.red()
						);
					} else {
						print!(
							"{}",
							format_total_commodity_amount_offset(
								total_commodity,
								&format_with_zero(total_amount),
								header_width,
								account_width,
								amount_width,
								total_amount_width
							)
						);
					}
				}
			}
		}

		println!();
	}

	Ok(())
}

fn format_commodity_amount(commodity: &str, amount: &str, amount_width: usize) -> String {
	format!(
		"{:>amount_width$}{:<offset_width$}",
		format!("{}{}", commodity, amount),
		"",
		amount_width = amount_width,
		offset_width = WIDTH_OFFSET
	)
}

fn format_total_commodity_amount(commodity: &str, amount: &str, amount_width: usize) -> String {
	format!(
		"{:>amount_width$}",
		format!("{}{}", commodity, amount),
		amount_width = amount_width,
	)
}

fn format_total_amount_offset(
	amount: &str,
	header_width: usize,
	account_width: usize,
	amount_width: usize,
	total_amount_width: usize,
) -> String {
	format!(
		"\n{:>offset$}{:>total_amount_width$}",
		"",
		amount,
		offset =
			header_width + WIDTH_OFFSET + account_width + WIDTH_OFFSET + amount_width + WIDTH_OFFSET,
		total_amount_width = total_amount_width,
	)
}

fn format_total_commodity_amount_offset(
	commodity: &str,
	amount: &str,
	header_width: usize,
	account_width: usize,
	amount_width: usize,
	total_amount_width: usize,
) -> String {
	format!(
		"\n{:>offset$}{:>total_amount_width$}",
		"",
		format!("{}{}", commodity, amount),
		offset =
			header_width + WIDTH_OFFSET + account_width + WIDTH_OFFSET + amount_width + WIDTH_OFFSET,
		total_amount_width = total_amount_width,
	)
}

fn format_with_zero(num: &Rational) -> String {
	if num.is_zero() {
		String::from("0")
	} else {
		super::format_amount(num)
	}
}
