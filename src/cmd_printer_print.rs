use super::model::MixedAmount;
use super::model::Posting;
use super::model::State;
use super::model::Transaction;

const INDENT: &str = "\t";
const OFFSET: usize = 4;

pub fn print_explicit(transactions: &[Transaction]) -> Result<(), String> {
	if transactions.iter().any(|t| t.postings.is_empty()) {
		return Ok(());
	}
	let require_amount = true;
	print(transactions, require_amount, |p| p.balanced_amount.as_ref())
}

pub fn print_raw(transactions: &[Transaction]) -> Result<(), String> {
	if transactions.iter().any(|t| t.postings.is_empty()) {
		return Ok(());
	}
	let require_amount = false;
	print(transactions, require_amount, |p| {
		p.unbalanced_amount.as_ref()
	})
}

fn print<F: Fn(&Posting) -> Option<&MixedAmount>>(
	transactions: &[Transaction],
	require_amount: bool,
	f: F,
) -> Result<(), String> {
	let account_width = transactions
		.iter()
		.flat_map(|t| t.postings.iter())
		.map(|p| p.account.chars().count())
		.max()
		.unwrap_or(0);

	let mut transaction_iter = transactions.iter().peekable();

	while let Some(transaction) = transaction_iter.next() {
		print_head(transaction);
		print_comments(transaction);

		for posting in &transaction.postings {
			print_account(posting);
			print_amount(posting, require_amount, account_width, &f)?;
		}

		if transaction_iter.peek().is_some() {
			println!();
		}
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
			.unwrap_or_else(|| String::from("")),
		transaction.description
	);
}

fn print_comments(transaction: &Transaction) {
	for comment in &transaction.comments {
		println!("{}; {}", INDENT, comment.comment);
	}
}

fn print_account(posting: &Posting) {
	print!("{}{}", INDENT, posting.account);
}

fn print_amount<F>(
	posting: &Posting,
	require_amount: bool,
	account_width: usize,
	f: F,
) -> Result<(), String>
where
	F: Fn(&Posting) -> Option<&MixedAmount>,
{
	let mixed_amount = Some(posting).and_then(|p| f(p));

	match mixed_amount {
		None => {
			if require_amount {
				return Err(String::from("Amount Required"));
			}
		}

		Some(mixed_amount) => {
			for _ in 0..(account_width + OFFSET - posting.account.chars().count()) {
				print!(" ");
			}

			let formatted_amount = super::cmd_printer::format_amount(&mixed_amount.amount);

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
	}
	println!();
	Ok(())
}
