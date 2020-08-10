mod chars;
mod comment;
mod directives;
mod include;
mod mixed_amount;
mod posting;
mod transaction;

use super::super::model::Item;
// use super::super::model::Posting;
use super::Error;
use std::path::Path;

struct Tokenizer<'a> {
	file: &'a Path,
	items: &'a mut Vec<Item>,
	line_string: &'a str,
	line_characters: Vec<char>,
	line_index: usize,
	line_position: usize,
}

pub fn tokenize(file: &Path, content: &str, items: &mut Vec<Item>) -> Result<(), Error> {
	let mut tokenizer = Tokenizer {
		file,
		items,
		line_characters: Vec::new(),
		line_index: 0,
		line_string: "",
		line_position: 0,
	};
	match tokenizer.create_tokens(&content) {
		Ok(()) => Ok(()),
		Err(err) => match err {
			Error::LexerError(err) => {
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
				if !err.is_empty() {
					message.push_str(&format!("\n{}", err));
				}
				Err(Error::ParseError {
					line: tokenizer.line_index + 1,
					message,
				})
			}
			Error::BalanceError {
				range_start,
				range_end,
				message,
			} => {
				let mut err = String::new();
				let lines = content.lines().collect::<Vec<&str>>();
				for i in range_start..range_end {
					err.push_str(&format!("> {} : {}\n", i + 1, lines.get(i).unwrap()));
				}
				err.push_str(&message);
				Err(Error::ParseError {
					line: range_start + 1,
					message: err,
				})
			}
			Error::ParseError { line, message } => Err(Error::ParseError { line, message }),
		},
	}
}

impl<'a> Tokenizer<'a> {
	fn create_tokens(&mut self, content: &'a str) -> Result<(), Error> {
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

	fn tokenize(&mut self) -> Result<(), Error> {
		if chars::try_consume_char(self, |c| c == '\t') || chars::try_consume_string(self, "  ") {
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
			return Err(Error::LexerError(format!("unexpected character \"{}\"", c)));
		}
		Ok(())
	}
}

fn balance_last_transaction(tokenizer: &mut Tokenizer) -> Result<(), Error> {
	let mut balanced_transactions = Vec::new();
	while let Some(item) = tokenizer.items.pop() {
		if let Item::Transaction { .. } = item {
			let balanced_item = super::balancer::balance(item)?;
			balanced_transactions.insert(0, balanced_item);
			break;
		} else {
			balanced_transactions.insert(0, item);
		}
	}
	tokenizer.items.append(&mut balanced_transactions);
	Ok(())
}

// fn print_transactions(transactions: &[Transaction]) {
// 	for transaction in transactions {
// 		print_transaction(transaction);
// 	}
// }
// pub fn print_transaction(transaction: &Transaction) {
// 	println!("{}", transaction.description);
// 	for posting in &transaction.postings {
// 		println!(
// 			"posting : {} {:?}",
// 			posting.account, posting.unbalanced_amount
// 		);
// 	}
// }
