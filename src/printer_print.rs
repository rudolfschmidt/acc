use super::model::BalancedPosting;
use super::model::Ledger;
use super::model::State;
use super::model::Transaction;
use super::model::UnbalancedPosting;

const INDENT: &str = "\t";
const WIDTH_OFFSET: usize = 4;

pub fn print(ledger: &Ledger) -> Result<(), String> {
	let account_width = ledger
		.journals
		.iter()
		.flat_map(|j| j.balanced_transactions.iter())
		.flat_map(|t| t.postings.iter())
		.map(|p| p.account.chars().count())
		.max()
		.unwrap_or(0);
	for transaction in ledger
		.journals
		.iter()
		.flat_map(|j| j.balanced_transactions.iter())
		.collect::<Vec<&Transaction<BalancedPosting>>>()
	{
		print_head(&transaction);
		print_comments(&transaction);
		for posting in &transaction.postings {
			print_posting_amount(account_width, &posting.account);
			let formatted_amount = super::printer::format_amount(&posting.amount);
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

pub fn print_raw(ledger: &Ledger) -> Result<(), String> {
	let account_width = ledger
		.journals
		.iter()
		.flat_map(|j| j.unbalanced_transactions.iter())
		.flat_map(|t| t.postings.iter())
		.map(|p| p.account.chars().count())
		.max()
		.unwrap_or(0);
	for transaction in ledger
		.journals
		.iter()
		.flat_map(|j| j.unbalanced_transactions.iter())
		.collect::<Vec<&Transaction<UnbalancedPosting>>>()
	{
		print_head(&transaction);
		print_comments(&transaction);
		for posting in &transaction.postings {
			print_posting_amount(account_width, &posting.account);
			if let Some(amount) = posting.amount {
				let formatted_amount = super::printer::format_amount(&amount);
				print!(
					"{}{}",
					posting.commodity.as_ref().unwrap(),
					if formatted_amount.starts_with('-') {
						formatted_amount
					} else {
						let mut buf = String::new();
						buf.push(' ');
						buf.push_str(&formatted_amount);
						buf
					}
				);
			}
			println!();
		}
		println!();
	}
	Ok(())
}

fn print_head<T>(transaction: &Transaction<T>) {
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
}

fn print_comments<T>(transaction: &Transaction<T>) {
	for comment in &transaction.comments {
		println!("{}{}", INDENT, comment.comment);
	}
}

fn print_posting_amount(account_width: usize, account: &str) {
	print!("{}{}", INDENT, account);
	for _ in 0..(account_width + WIDTH_OFFSET - account.chars().count()) {
		print!(" ");
	}
}
