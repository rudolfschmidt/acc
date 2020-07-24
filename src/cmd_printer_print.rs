use super::model::Ledger;
use super::model::State;
use super::model::Transaction;

const INDENT: &str = "\t";
const WIDTH_OFFSET: usize = 4;

pub fn print(ledger: &Ledger) -> Result<(), String> {
	let account_width = ledger
		.journals
		.iter()
		.flat_map(|j| j.transactions.iter())
		.flat_map(|t| t.postings.iter())
		.map(|p| p.account.chars().count())
		.max()
		.unwrap_or(0);
	for transaction in ledger
		.journals
		.iter()
		.flat_map(|j| j.transactions.iter())
		.collect::<Vec<&Transaction>>()
	{
		print_head(&transaction);
		print_comments(&transaction);
		for posting in &transaction.postings {
			print_posting_amount(account_width, &posting.account);
			let formatted_amount =
				super::cmd_printer::format_amount(&posting.balanced_amount.as_ref().unwrap().amount);
			print!(
				"{}{}",
				posting.balanced_amount.as_ref().unwrap().commodity,
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
		.flat_map(|j| j.transactions.iter())
		.flat_map(|t| t.postings.iter())
		.map(|p| p.account.chars().count())
		.max()
		.unwrap_or(0);
	for transaction in ledger
		.journals
		.iter()
		.flat_map(|j| j.transactions.iter())
		.collect::<Vec<&Transaction>>()
	{
		print_head(&transaction);
		print_comments(&transaction);
		for posting in &transaction.postings {
			print_posting_amount(account_width, &posting.account);
			if let Some(unbalanced_amount) = &posting.unbalanced_amount {
				let formatted_amount = super::cmd_printer::format_amount(&unbalanced_amount.amount);
				print!(
					"{}{}",
					unbalanced_amount.commodity,
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

fn print_head(transaction: &Transaction) {
	println!(
		"{}{}{}{}",
		transaction.date,
		match transaction.state {
			State::Cleared => " * ",
			State::Uncleared => " ",
			State::Pending => " ! ",
		},
		transaction
			.code
			.as_ref()
			.and_then(|c| {
				let mut ret = String::new();
				ret.push('(');
				ret.push_str(c);
				ret.push(')');
				ret.push(' ');
				Some(ret)
			})
			.unwrap_or("".to_owned()),
		transaction.description
	);
}

fn print_comments(transaction: &Transaction) {
	for comment in &transaction.comments {
		println!("{}; {}", INDENT, comment.comment);
	}
}

fn print_posting_amount(account_width: usize, account: &str) {
	print!("{}{}", INDENT, account);
	for _ in 0..(account_width + WIDTH_OFFSET - account.chars().count()) {
		print!(" ");
	}
}
