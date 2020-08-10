use super::super::model::Comment;
use super::super::model::Item;
use super::super::model::MixedAmount;
use super::super::model::Posting;
use super::super::model::State;
use super::format_amount;

use num::Signed;

const INDENT: &str = "\t";
const OFFSET: usize = 4;

pub fn print_explicit(items: Vec<Item>) -> Result<(), String> {
	print(items, false)
}

pub fn print_raw(items: Vec<Item>) -> Result<(), String> {
	print(items, true)
}

fn print(items: Vec<Item>, natural: bool) -> Result<(), String> {
	if items.iter().any(|item| match item {
		Item::Transaction { postings, .. } => postings.is_empty(),
		_ => true,
	}) {
		return Ok(());
	}
	let account_max_width = items
		.iter()
		.filter_map(|item| match item {
			Item::Transaction { postings, .. } => Some(postings),
			_ => None,
		})
		.flat_map(|postings| postings.iter())
		.filter_map(|posting| match posting {
			Posting::BalancedPosting { account, .. } => Some(account),
			_ => None,
		})
		.map(|account| account.chars().count())
		.max()
		.unwrap_or(0);

	let mut iter = items.into_iter().peekable();

	while let Some(item) = iter.next() {
		match item {
			Item::Comment { .. } => {}
			Item::Transaction {
				date,
				state,
				code,
				description,
				comments,
				postings,
				..
			} => {
				println!(
					"{}{}{}{}",
					date,
					match state {
						State::Cleared => " * ",
						State::Uncleared => " ",
						State::Pending => " ! ",
					},
					code
						.map(|c| format!("({}) ", c))
						.unwrap_or_else(|| String::from("")),
					description
				);
				for comment in comments {
					println!("{}; {}", INDENT, comment.comment);
				}
				for posting in postings {
					match posting {
						Posting::BalancedPosting {
							account,
							comments,
							balanced_amount,
							..
						} => {
							let account_width = account.chars().count();
							print!("{}{}", INDENT, account);
							print_mixed_amount(balanced_amount, account_width, account_max_width);
							for comment in comments {
								println!("{}; {}", INDENT, comment.comment);
							}
						}
						_ => {}
					}
				}
				if iter.peek().is_some() {
					println!();
				}
			}
		}
	}
	Ok(())
}

fn print_mixed_amount(
	balanced_amount: MixedAmount,
	account_width: usize,
	account_max_width: usize,
) {
	for _ in 0..(account_max_width + OFFSET - account_width) {
		print!(" ");
	}
	println!(
		"{}{}",
		balanced_amount.commodity,
		if balanced_amount.value.is_negative() {
			format_amount(&balanced_amount.value)
		} else {
			let mut buf = String::new();
			buf.push(' ');
			buf.push_str(&format_amount(&balanced_amount.value));
			buf
		}
	);
}
