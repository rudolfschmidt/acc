mod chars;
mod comment;
mod directives;
mod mixed_amount;
mod transaction;

use super::super::errors::Error;
use super::super::ledger::Ledger;
use super::super::model::Token;

pub(crate) fn read_lines(ledger: &mut Ledger, content: &str) -> Result<(), Error> {
	let mut tokenizer = Tokenizer {
		ledger,
		line_chars: Vec::new(),
		line_index: 0,
		line_str: "",
		line_pos: 0,
	};
	match tokenizer.create_tokens(content) {
		Ok(()) => Ok(()),
		Err(reason) => {
			let mut msg = String::new();

			msg.push_str(&format!(
				"{} : {}\n",
				tokenizer.line_index + 1,
				tokenizer.line_str.replace('\t', " ")
			));

			let mut num = tokenizer.line_index + 1;
			while num != 0 {
				num /= 10;
				msg.push('-');
			}

			msg.push('-');
			msg.push('-');
			msg.push('-');

			for _ in 0..tokenizer.line_pos {
				msg.push('-');
			}

			msg.push('^');

			if !reason.is_empty() {
				msg.push_str(&format!("\nError : {}", reason));
			}

			Err(Error {
				line: tokenizer.line_index + 1,
				message: msg,
			})
		}
	}
}

struct Tokenizer<'a> {
	ledger: &'a mut Ledger,
	line_str: &'a str,
	line_chars: Vec<char>,
	line_index: usize,
	line_pos: usize,
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
							.ledger
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
					.ledger
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

				self
					.ledger
					.tokens
					.push(Token::BalanceAssertion(self.line_index));

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
