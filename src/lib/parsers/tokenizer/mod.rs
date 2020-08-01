mod chars;
mod comment;
mod directives;
mod mixed_amount;
mod transaction;

use super::super::errors;
use super::super::model::Token;
use super::super::model::Transaction;
use std::path::Path;

struct Tokenizer<'a> {
	file: &'a Path,
	tokens: &'a mut Vec<Token>,
	transactions: &'a mut Vec<Transaction>,
	line_str: &'a str,
	line_chars: Vec<char>,
	line_index: usize,
	line_pos: usize,
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
		line_chars: Vec::new(),
		line_index: 0,
		line_str: "",
		line_pos: 0,
	};
	match tokenizer.create_tokens(content) {
		Ok(()) => Ok(()),
		Err(reason) => {
			let mut message = String::new();
			message.push_str(&format!(
				"{} : {}\n",
				tokenizer.line_index + 1,
				tokenizer.line_str.replace('\t', " ")
			));
			let mut num = tokenizer.line_index + 1;
			while num != 0 {
				num /= 10;
				message.push('-');
			}
			message.push('-');
			message.push('-');
			message.push('-');
			for _ in 0..tokenizer.line_pos {
				message.push('-');
			}
			message.push('^');
			if !reason.is_empty() {
				message.push_str(&format!("\n{}", reason));
			}
			Err(errors::Error {
				line: tokenizer.line_index + 1,
				message,
			})
		}
	}
}

impl<'a> Tokenizer<'a> {
	fn create_tokens(&mut self, content: &'a str) -> Result<(), String> {
		for (line_index, line_str) in content.lines().enumerate() {
			self.line_str = line_str;
			self.line_chars = line_str.chars().collect();
			self.line_index = line_index;
			self.line_pos = 0;
			if self.line_chars.get(self.line_pos).is_some() {
				self.parse()?;
			}
		}
		Ok(())
	}

	fn parse(&mut self) -> Result<(), String> {
		if chars::is_tab(&self.line_chars, self.line_pos)
			|| (chars::is_space(&self.line_chars, self.line_pos)
				&& chars::is_space(&self.line_chars, self.line_pos + 1))
		{
			chars::consume_whitespaces(self);
			comment::tokenize(self)?;
			self.tokenize_posting()?;
		} else {
			transaction::tokenize(self)?;
			comment::tokenize(self)?;
			directives::is_include(self)?;
			directives::is_alias(self)?;
		}
		if let Some(c) = self.line_chars.get(self.line_pos) {
			return Err(format!("Unexpected character \"{}\"", c));
		}
		Ok(())
	}

	fn tokenize_posting(&mut self) -> Result<(), String> {
		match self.line_chars.get(self.line_pos) {
			None => Ok(()),
			Some(_) => {
				let mut value = String::new();

				while let Some(&c) = self.line_chars.get(self.line_pos) {
					if chars::is_tab(&self.line_chars, self.line_pos)
						|| (chars::is_space(&self.line_chars, self.line_pos)
							&& chars::is_space(&self.line_chars, self.line_pos + 1))
					{
						self
							.tokens
							.push(Token::PostingAccount(self.line_index, value));

						chars::consume_whitespaces(self);
						mixed_amount::tokenize(self)?;

						return self.balance_assertion();
					}

					value.push(c);
					self.line_pos += 1;
				}

				self
					.tokens
					.push(Token::PostingAccount(self.line_index, value));

				Ok(())
			}
		}
	}

	fn balance_assertion(&mut self) -> Result<(), String> {
		if let Some(&c) = self.line_chars.get(self.line_pos) {
			if c == '=' {
				self.line_pos += 1;

				self.tokens.push(Token::BalanceAssertion(self.line_index));

				if self.line_chars.get(self.line_pos).is_none() {
					return Err(String::from(""));
				} else {
					chars::consume_whitespaces(self);
					return mixed_amount::tokenize(self);
				};
			}
		}
		Ok(())
	}
}
