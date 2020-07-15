use super::model::BalancedPosting;
use super::model::State;
use super::model::Transaction;
use super::printer::format_amount;

const INDENT: &str = "\t";
const WIDTH_OFFSET: usize = 4;

pub fn print(transactions: Vec<&Transaction<BalancedPosting>>) -> Result<(), String> {
	let account_width = transactions
		.iter()
		.flat_map(|t| t.postings.iter())
		.map(|p| p.account.chars().count())
		.max()
		.unwrap_or(0);
	for transaction in transactions {
		println!(
			"{}{}{}",
			transaction.date,
			match transaction.state {
				State::Cleared => " * ",
				State::Uncleared => " ",
				State::Pending => " ! ",
			},
			transaction.description
		);
		for posting in &transaction.postings {
			print!("{}{}", INDENT, posting.account);
			for _ in 0..(account_width + WIDTH_OFFSET - posting.account.chars().count()) {
				print!(" ");
			}
			let formatted_amount = format_amount(&posting.amount);
			print!(
				"{}{}",
				posting.commodity,
				if formatted_amount.starts_with('-') {
					formatted_amount
				} else {
					let mut buf = String::new();
					buf.push(' ');
					buf.push_str(&formatted_amount);
					buf
				}
			);
			println!();
		}
		println!();
	}
	Ok(())
}
pub fn print_raw(transactions: Vec<&Transaction<BalancedPosting>>) -> Result<(), String> {
	Ok(())
}
