mod chars;
mod comment;
mod directives;
mod mixed_amount;
mod posting;
mod transaction;

use super::super::errors;
use super::super::model::Token;
use super::super::model::Transaction;
use std::path::Path;

struct Tokenizer<'a> {
	file: &'a Path,
	tokens: &'a mut Vec<Token>,
	transactions: &'a mut Vec<Transaction>,
	line: &'a str,
	chars: Vec<char>,
	index: usize,
	pos: usize,
}

pub fn tokenize(
	file: &Path,
	content: &str,
	tokens: &mut Vec<Token>,
	transactions: &mut Vec<Transaction>,
) -> Result<(), errors::Error> {
	let mut tokenizer = Tokenizer {
		file,
		tokens,
		transactions,
		chars: Vec::new(),
		index: 0,
		line: "",
		pos: 0,
	};
	match tokenizer.create_tokens(content) {
		Ok(()) => Ok(()),
		Err(reason) => {
			let mut message = String::new();
			message.push_str(&format!(
				"{} : {}\n",
				tokenizer.index + 1,
				tokenizer.line.replace('\t', " ")
			));
			let mut num = tokenizer.index + 1;
			while num != 0 {
				num /= 10;
				message.push('-');
			}
			message.push('-');
			message.push('-');
			message.push('-');
			for _ in 0..tokenizer.pos {
				message.push('-');
			}
			message.push('^');
			if !reason.is_empty() {
				message.push_str(&format!("\n{}", reason));
			}
			Err(errors::Error {
				line: tokenizer.index + 1,
				message,
			})
		}
	}
}

impl<'a> Tokenizer<'a> {
	fn create_tokens(&mut self, content: &'a str) -> Result<(), String> {
		for (index, line) in content.lines().enumerate() {
			self.line = line;
			self.chars = line.chars().collect();
			self.index = index;
			self.pos = 0;
			if self.chars.get(self.pos).is_some() {
				self.parse()?;
			}
		}
		Ok(())
	}

	fn parse(&mut self) -> Result<(), String> {
		if chars::is_char(self, '\t')
			|| (chars::is_char_pos(&self.chars, self.pos, ' ')
				&& chars::is_char_pos(&self.chars, self.pos + 1, ' '))
		{
			chars::consume_whitespaces(self);
			comment::tokenize(self)?;
			posting::tokenize(self)?;
		} else {
			transaction::tokenize(self)?;
			comment::tokenize(self)?;
			directives::is_include(self)?;
			directives::is_alias(self)?;
		}
		if let Some(c) = self.chars.get(self.pos) {
			return Err(format!("Unexpected character \"{}\"", c));
		}
		Ok(())
	}
}
