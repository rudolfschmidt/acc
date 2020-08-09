use super::super::model::MixedAmount;
use super::super::model::Posting;
use super::super::model::State;
use super::super::model::Transaction;
use super::format_amount;

const INDENT: &str = "\t";
const OFFSET: usize = 4;

pub fn print_explicit(transactions: Vec<Transaction>) -> Result<(), String> {
	if transactions
		.iter()
		.any(|transaction| transaction.postings.is_empty())
	{
		return Ok(());
	}
	print(transactions, false)
}

pub fn print_raw(transactions: Vec<Transaction>) -> Result<(), String> {
	if transactions
		.iter()
		.any(|transaction| transaction.postings.is_empty())
	{
		return Ok(());
	}
	print(transactions, true)
}

fn print(transactions: Vec<Transaction>, natural: bool) -> Result<(), String> {
	let account_width = transactions
		.iter()
		.flat_map(|transaction| transaction.postings.iter())
		.map(|balanced_posting| {
			if balanced_posting.virtual_posting {
				balanced_posting.account.chars().count() + 2
			} else {
				balanced_posting.account.chars().count()
			}
		})
		.max()
		.unwrap_or(0);

	let mut transaction_iter = transactions.into_iter().peekable();

	while let Some(transaction) = transaction_iter.next() {
		print_transaction_head(&transaction);
		print_transaction_comments(&transaction);

		for posting in transaction.postings {
			print_account(&posting);
			print_amount(&posting, account_width, natural)?;
			print_posting_comments(&posting);
		}

		if transaction_iter.peek().is_some() {
			println!();
		}
	}
	Ok(())
}

fn print_transaction_head(transaction: &Transaction) {
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
			.map(|c| format!("({}) ", c))
			.unwrap_or_else(|| String::from("")),
		transaction.description
	);
}

fn print_transaction_comments(transaction: &Transaction) {
	for comment in &transaction.comments {
		println!("{}; {}", INDENT, comment.comment);
	}
}

fn print_posting_comments(posting: &Posting) {
	for comment in &posting.comments {
		println!("{}; {}", INDENT, comment.comment);
	}
}

fn print_account(posting: &Posting) {
	if posting.virtual_posting {
		print!("{}{}", INDENT, format!("({})", posting.account));
	} else {
		print!("{}{}", INDENT, posting.account);
	}
}

fn print_amount(posting: &Posting, account_width: usize, natural: bool) -> Result<(), String> {
	if !natural {
		print_mixed_amount(
			posting,
			account_width,
			&posting.balanced_amount.as_ref().expect("unbalanced amount"),
		);
	}
	if let Some(ref balanced_amount) = posting.balanced_amount {
		print_mixed_amount(posting, account_width, balanced_amount);
	}
	println!();
	Ok(())
}

fn print_mixed_amount(posting: &Posting, account_width: usize, mixed_amount: &MixedAmount) {
	for _ in 0..(account_width + OFFSET
		- if posting.virtual_posting {
			posting.account.chars().count() + 2
		} else {
			posting.account.chars().count()
		}) {
		print!(" ");
	}

	let formatted_amount = format_amount(&mixed_amount.value);

	print!(
		"{}{}",
		mixed_amount.commodity,
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
