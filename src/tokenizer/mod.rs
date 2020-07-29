mod mixed_amount;
mod transaction;

use super::errors::Error;
use super::ledger::Ledger;
use super::model::Token;

pub fn read_lines(ledger: &mut Ledger, content: &str) -> Result<(), Error> {
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
				if self.is_tab(self.line_pos)
					|| (self.is_space(self.line_pos) && self.is_space(self.line_pos + 1))
				{
					self.consume_whitespaces();
					self.tokenize_comment()?;
					self.tokenize_posting()?;
				} else {
					transaction::tokenize(self)?;
					self.tokenize_comment()?;
					self.tokenize_directive_include()?;
				}
				if let Some(c) = self.line_chars.get(self.line_pos) {
					return Err(format!("Unexpected character \"{}\"", c));
				}
			}
		}
		Ok(())
	}

	fn is_space(&mut self, pos: usize) -> bool {
		match self.line_chars.get(pos) {
			None => false,
			Some(c) if *c == ' ' => true,
			Some(_) => false,
		}
	}

	fn is_tab(&mut self, pos: usize) -> bool {
		match self.line_chars.get(pos) {
			None => false,
			Some(c) if *c == '\t' => true,
			Some(_) => false,
		}
	}

	fn consume_whitespaces(&mut self) {
		while let Some(c) = self.line_chars.get(self.line_pos) {
			if !c.is_whitespace() {
				break;
			}
			self.line_pos += 1;
		}
	}

	fn tokenize_comment(&mut self) -> Result<(), String> {
		match self.line_chars.get(self.line_pos) {
			None => Ok(()),
			Some(&c) => {
				if c == ';' {
					self.line_pos += 1;

					self.consume_whitespaces();

					let mut value = String::new();

					while let Some(&c) = self.line_chars.get(self.line_pos) {
						value.push(c);
						self.line_pos += 1;
					}

					self
						.ledger
						.tokens
						.push(Token::Comment(self.line_index, value));
				}
				Ok(())
			}
		}
	}

	fn tokenize_directive_include(&mut self) -> Result<(), String> {
		match self.line_chars.get(self.line_pos) {
			None => Ok(()),
			Some(_) => {
				let directive = "include";
				let directive_len = directive.chars().count();
				if self
					.line_chars
					.iter()
					.collect::<String>()
					.starts_with(directive)
				{
					let file = self
						.line_chars
						.iter()
						.skip(directive_len + 1)
						.collect::<String>();

					match self.ledger.read_tokens(&file) {
						Err(err) => return Err(err),
						Ok(()) => self.line_pos += directive_len + 1 + file.chars().count(),
					}
				}

				Ok(())
			}
		}
	}

	fn tokenize_posting(&mut self) -> Result<(), String> {
		match self.line_chars.get(self.line_pos) {
			None => Ok(()),
			Some(_) => {
				let mut value = String::new();

				while let Some(&c) = self.line_chars.get(self.line_pos) {
					if self.is_tab(self.line_pos)
						|| (self.is_space(self.line_pos) && self.is_space(self.line_pos + 1))
					{
						self
							.ledger
							.tokens
							.push(Token::PostingAccount(self.line_index, value));

						self.consume_whitespaces();
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
					self.consume_whitespaces();
					return mixed_amount::tokenize(self);
				};
			}
		}
		Ok(())
	}
}
