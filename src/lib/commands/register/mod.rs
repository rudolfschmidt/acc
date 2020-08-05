use super::super::model::BalancedPosting;
use super::super::model::State;
use super::super::model::Transaction;

use colored::Colorize;
use std::collections::BTreeMap;

const WIDTH_OFFSET: usize = 4;

struct Row {
	title: String,
	accounts: Vec<Account>,
}

struct Account {
	account: String,
	commodity: String,
	amount: String,
	total: BTreeMap<String, String>,
}

// Maybe I consider the terminal width in the future
// let terminal_width = std::process::Command::new("sh")
// .arg("-c")
// .arg("tput cols")
// .output()
// .expect("failed to fetch terminal width");

pub fn print(transactions: Vec<Transaction<BalancedPosting>>) -> Result<(), String> {
	let mut rows = Vec::new();

	let mut total = BTreeMap::new();

	for transaction in transactions {
		if transaction.postings.is_empty() {
			continue;
		}
		let mut row = Row {
			title: format!(
				"{}{}{}",
				transaction.header.date,
				match transaction.header.state {
					State::Cleared => " * ",
					State::Uncleared => " ",
					State::Pending => " ! ",
				},
				transaction.header.description
			),
			accounts: Vec::new(),
		};
		for posting in transaction.postings {
			total
				.entry(posting.balanced_amount.commodity.to_owned())
				.and_modify(|a| *a += posting.balanced_amount.value)
				.or_insert(posting.balanced_amount.value);
			row.accounts.push(Account {
				account: posting.head.account,
				commodity: posting.balanced_amount.commodity,
				amount: super::format_amount(&posting.balanced_amount.value),
				total: total
					.iter()
					.fold(BTreeMap::new(), |mut acc, (commodity, amount)| {
						acc.insert(commodity.to_owned(), super::format_amount(amount));
						acc
					}),
			});
		}
		rows.push(row);
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

	let commodity_width = rows
		.iter()
		.flat_map(|t| t.accounts.iter())
		.map(|a| a.commodity.chars().count())
		.max()
		.unwrap_or(0);

	let amount_width = rows
		.iter()
		.flat_map(|t| t.accounts.iter())
		.map(|a| a.amount.chars().count())
		.max()
		.unwrap_or(0);

	let total_amount_width = rows
		.iter()
		.flat_map(|t| t.accounts.iter())
		.flat_map(|a| a.total.iter())
		.map(|(_, a)| a.chars().count())
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
					format_commodity_amount(&commodity, &amount, commodity_width, amount_width).red()
				);
			} else {
				print!(
					"{}",
					format_commodity_amount(&commodity, &amount, commodity_width, amount_width)
				);
			}

			let mut total_iter = account.total.iter();

			if let Some((total_commodity, total_amount)) = total_iter.next() {
				if total_amount.starts_with('-') {
					print!(
						"{}",
						format_total_commodity_amount(
							total_commodity,
							total_amount,
							commodity_width,
							total_amount_width
						)
						.red()
					);
				} else {
					print!(
						"{}",
						format_total_commodity_amount(
							total_commodity,
							total_amount,
							commodity_width,
							total_amount_width
						)
					);
				}
			}

			for (total_commodity, total_amount) in total_iter {
				if total_amount.starts_with('-') {
					print!(
						"{}",
						format_total_commodity_amount_offset(
							total_commodity,
							total_amount,
							header_width,
							account_width,
							commodity_width,
							total_amount_width
						)
						.red()
					);
				} else {
					print!(
						"{}",
						format_total_commodity_amount_offset(
							total_commodity,
							total_amount,
							header_width,
							account_width,
							commodity_width,
							total_amount_width
						)
					);
				}
			}
		}

		println!();
	}

	Ok(())
}

fn format_commodity_amount(
	commodity: &str,
	amount: &str,
	commodity_width: usize,
	amount_width: usize,
) -> String {
	format!(
		"{:>commodity_width$}{:>amount_width$}{:<offset_width$}",
		commodity,
		amount,
		"",
		commodity_width = commodity_width,
		amount_width = amount_width,
		offset_width = WIDTH_OFFSET * 2
	)
}

fn format_total_commodity_amount(
	commodity: &str,
	amount: &str,
	commodity_width: usize,
	amount_width: usize,
) -> String {
	format!(
		"{:>commodity_width$}{:>amount_width$}",
		commodity,
		amount,
		commodity_width = commodity_width,
		amount_width = amount_width,
	)
}

fn format_total_commodity_amount_offset(
	commodity: &str,
	amount: &str,
	header_width: usize,
	account_width: usize,
	commodity_width: usize,
	amount_width: usize,
) -> String {
	format!(
		"\n{:>offset$}{:>commodity_width$}{:>amount_width$}",
		"",
		commodity,
		amount,
		offset = header_width
			+ WIDTH_OFFSET
			+ account_width
			+ WIDTH_OFFSET
			+ commodity_width
			+ amount_width
			+ WIDTH_OFFSET * 2,
		commodity_width = commodity_width,
		amount_width = amount_width,
	)
}
