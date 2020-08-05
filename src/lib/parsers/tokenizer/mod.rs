mod chars;
mod comment;
mod directives;
mod include;
mod mixed_amount;
mod posting;
mod transaction;

use super::super::model::BalancedPosting;
use super::super::model::Transaction;
use super::super::model::UnbalancedPosting;
use super::Error;
use std::path::Path;

struct Tokenizer<'a> {
	file: &'a Path,
	content: &'a str,
	transactions: &'a mut Vec<Transaction<BalancedPosting>>,
	unbalanced_transactions: Vec<Transaction<UnbalancedPosting>>,
	line_string: &'a str,
	line_characters: Vec<char>,
	line_index: usize,
	line_position: usize,
}

pub fn tokenize(
	file: &Path,
	content: &str,
	transactions: &mut Vec<Transaction<BalancedPosting>>,
) -> Result<(), Error> {
	let mut tokenizer = Tokenizer {
		file,
		content,
		transactions,
		unbalanced_transactions: Vec::new(),
		line_characters: Vec::new(),
		line_index: 0,
		line_string: "",
		line_position: 0,
	};
	match tokenizer.create_tokens(&content) {
		Ok(()) => Ok(()),
		Err(reason) => {
			let mut message = String::new();
			message.push_str(&format!(
				"{} : {}\n",
				tokenizer.line_index + 1,
				tokenizer.line_string.replace('\t', " ")
			));
			let mut num = tokenizer.line_index + 1;
			while num != 0 {
				num /= 10;
				message.push('-');
			}
			message.push('-');
			message.push('-');
			message.push('-');
			for _ in 0..tokenizer.line_position {
				message.push('-');
			}
			message.push('^');
			if !reason.is_empty() {
				message.push_str(&format!("\n{}", reason));
			}
			Err(Error {
				line: tokenizer.line_index + 1,
				message,
			})
		}
	}
}

impl<'a> Tokenizer<'a> {
	fn create_tokens(&mut self, content: &'a str) -> Result<(), String> {
		for (index, line) in content.lines().enumerate() {
			self.line_string = line;
			self.line_characters = line.chars().collect();
			self.line_index = index;
			self.line_position = 0;
			self.tokenize()?;
		}
		balance_last_transaction(self)?;
		Ok(())
	}

	fn tokenize(&mut self) -> Result<(), String> {
		if chars::consume(self, |c| c == '\t') || chars::consume_string(self, "  ") {
			chars::consume_whitespaces(self);
			comment::tokenize_indented_comment(self)?;
			posting::tokenize(self)?;
		} else {
			comment::tokenize_journal_comment(self)?;
			transaction::tokenize(self)?;
			include::tokenize(self)?;
			// directives::is_alias(self)?;
		}
		if let Some(c) = self.line_characters.get(self.line_position) {
			return Err(format!("unexpected character \"{}\"", c));
		}
		Ok(())
	}
}

fn balance_last_transaction(tokenizer: &mut Tokenizer) -> Result<(), String> {
	match tokenizer.unbalanced_transactions.pop() {
		None => Ok(()),
		Some(unbalanced_transaction) => match super::balancer::balance(unbalanced_transaction) {
			Ok(balanced_transaction) => {
				tokenizer.transactions.push(balanced_transaction);
				Ok(())
			}
			Err(err) => {
				let mut message = String::new();
				let lines: Vec<&str> = tokenizer.content.lines().collect();
				for i in err.start..err.end {
					message.push_str(&format!("> {} : {}\n", i + 1, lines.get(i).unwrap()));
				}
				message.push_str(&err.message);
				Err(message)
			}
		},
	}
}

fn print_transactions(transactions: &[Transaction<UnbalancedPosting>]) {
	for transaction in transactions {
		print_transaction(transaction);
	}
}
pub fn print_transaction(transaction: &Transaction<UnbalancedPosting>) {
	println!("{}", transaction.header.description);
	for posting in &transaction.postings {
		println!("posting : {} {:?}", posting.header.account, posting.amount);
	}
}
