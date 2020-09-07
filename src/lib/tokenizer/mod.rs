mod chars;
mod comment;
mod directives;
mod include;
mod mixed_amount;
mod posting;
mod transaction;

use super::model::Item;
use std::path::Path;

pub fn tokenize_file(file: &Path, items: &mut Vec<Item>) -> Result<(), String> {
	match std::fs::read_to_string(file) {
		Err(err) => Err(format!(
			"While parsing file \"{}\"\n{}",
			file.display(),
			err
		)),
		Ok(content) => tokenize(file, &content, items),
	}
}

struct Tokenizer<'a> {
	file: &'a Path,
	items: &'a mut Vec<Item>,
	line_string: &'a str,
	line_characters: Vec<char>,
	line_index: usize,
	line_position: usize,
}

fn tokenize(file: &Path, content: &str, items: &mut Vec<Item>) -> Result<(), String> {
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
		Err(err) => {
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
			Err(format!(
				"While parsing file \"{}\" at line {}:\n{}",
				file.display(),
				tokenizer.line_index + 1,
				message
			))
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
		Ok(())
	}

	fn tokenize(&mut self) -> Result<(), String> {
		if chars::try_consume_char(self, |c| c == '\t') || chars::try_consume_string(self, "  ") {
			chars::consume_whitespaces(self);
			comment::tokenize_indented_comment(self);
			posting::tokenize(self)?;
		} else {
			comment::tokenize_journal_comment(self);
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
