use super::errors::Error;
use super::model::Journal;
use super::model::Ledger;
use super::model::State;
use super::model::Token;

pub fn read_lines(
	ledger: &mut Ledger,
	content: &str,
	tokens: &mut Vec<Token>,
) -> Result<(), Error> {
	let mut lexer = Lexer {
		ledger,
		tokens,
		content,
		line_chars: Vec::new(),
		line_index: 0,
		line_str: "",
		line_pos: 0,
	};
	match lexer.create_tokens() {
		Ok(()) => Ok(()),
		Err(reason) => {
			let mut msg = String::new();

			msg.push_str(&format!(
				"{} : {}\n",
				lexer.line_index + 1,
				lexer.line_str.replace('\t', " ")
			));

			let mut num = lexer.line_index + 1;
			while num != 0 {
				num /= 10;
				msg.push('-');
			}

			msg.push('-');
			msg.push('-');
			msg.push('-');

			for _ in 0..lexer.line_pos {
				msg.push('-');
			}

			msg.push('^');

			if !reason.is_empty() {
				msg.push_str(&format!("\nError : {}", reason));
			}

			Err(Error {
				line: lexer.line_index + 1,
				message: msg,
			})
		}
	}
}

struct Lexer<'a> {
	ledger: &'a mut Ledger,
	tokens: &'a mut Vec<Token>,
	content: &'a str,
	line_str: &'a str,
	line_chars: Vec<char>,
	line_index: usize,
	line_pos: usize,
}

impl<'a> Lexer<'a> {
	fn create_tokens(&mut self) -> Result<(), String> {
		for (line_index, line_str) in self.content.lines().enumerate() {
			self.line_str = line_str;
			self.line_chars = line_str.chars().collect();
			self.line_index = line_index;
			self.line_pos = 0;
			match self.line_chars.get(self.line_pos) {
				None => {
					return Ok(());
				}
				Some(_) => {
					if self.is_tab(self.line_pos)
						|| (self.is_space(self.line_pos) && self.is_space(self.line_pos + 1))
					{
						self.consume_whitespaces();
						self.toknize_comment()?;
						self.toknize_posting()?;
					} else {
						self.toknize_transaction_head()?;
						self.toknize_comment()?;
						self.toknize_directive_include()?;
					}
					if let Some(c) = self.line_chars.get(self.line_pos) {
						return Err(format!("Unexpected character \"{}\"", c));
					}
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

	fn toknize_transaction_head(&mut self) -> Result<(), String> {
		if let Some(&c) = self.line_chars.get(self.line_pos) {
			if c.is_numeric() {
				let mut value = String::new();

				self.parse_number(&mut value)?;
				self.parse_number(&mut value)?;
				self.parse_number(&mut value)?;
				self.parse_number(&mut value)?;
				self.parse_dash(&mut value)?;
				self.parse_number(&mut value)?;
				self.parse_number(&mut value)?;
				self.parse_dash(&mut value)?;
				self.parse_number(&mut value)?;
				self.parse_number(&mut value)?;
				self.expect_whitespace()?;

				self
					.tokens
					.push(Token::TransactionDate(self.line_index, value));

				self.toknize_transaction_state()?;
				self.toknize_transaction_code()?;
				self.toknize_transaction_description()?;
			}
		}

		Ok(())
	}

	fn parse_number(&mut self, value: &mut String) -> Result<(), String> {
		match self.line_chars.get(self.line_pos) {
			None => Err(format!("Unexpected end of line. Expected number instead")),
			Some(c) => {
				if !c.is_numeric() {
					Err(format!(
						"Unexpected character \"{}\". Expected number instead",
						c
					))
				} else {
					value.push(*c);
					self.line_pos += 1;
					Ok(())
				}
			}
		}
	}

	fn parse_dash(&mut self, value: &mut String) -> Result<(), String> {
		match self.line_chars.get(self.line_pos) {
			None => Err(format!("Unexpected end of line. Expected \"-\" instead")),
			Some(c) => {
				if '-' != *c {
					Err(format!(
						"Unexpected character \"{}\". Expected \"-\" instead",
						c
					))
				} else {
					value.push(*c);
					self.line_pos += 1;
					Ok(())
				}
			}
		}
	}

	fn expect_whitespace(&mut self) -> Result<(), String> {
		match self.line_chars.get(self.line_pos) {
			None => Err(format!("Unexpected end of line. Expected \"-\" instead")),
			Some(&c) => {
				if !c.is_whitespace() {
					return Err(format!(
						"Unexpected character \"{}\". Expected \"-\" instead",
						c
					));
				}
				self.line_pos += 1;
				Ok(())
			}
		}
	}

	fn toknize_transaction_state(&mut self) -> Result<(), String> {
		match self.line_chars.get(self.line_pos) {
			None => Err(format!("Unexpected end of line")),
			Some(&c) => {
				self.consume_whitespaces();
				match c {
					'*' => {
						self
							.tokens
							.push(Token::TransactionState(self.line_index, State::Cleared));
						self.line_pos += 1;
					}
					'!' => {
						self
							.tokens
							.push(Token::TransactionState(self.line_index, State::Pending));
						self.line_pos += 1;
					}
					_ => {
						self
							.tokens
							.push(Token::TransactionState(self.line_index, State::Uncleared));
					}
				}
				Ok(())
			}
		}
	}

	fn toknize_transaction_code(&mut self) -> Result<(), String> {
		match self.line_chars.get(self.line_pos) {
			None => Ok(()),
			Some(&c) => {
				self.consume_whitespaces();
				if c == '(' {
					self.line_pos += 1;

					let mut value = String::new();

					match self.line_chars.get(self.line_pos) {
						None => {
							return Err(String::from(""));
						}
						Some(&c) => {
							value.push(c);
							self.line_pos += 1;
						}
					}

					while let Some(&c) = self.line_chars.get(self.line_pos) {
						if c == ')' {
							self.line_pos += 1;
							break;
						}
						value.push(c);
						self.line_pos += 1;
					}

					self
						.tokens
						.push(Token::TransactionCode(self.line_index, value));
				}
				Ok(())
			}
		}
	}

	fn toknize_transaction_description(&mut self) -> Result<(), String> {
		match self.line_chars.get(self.line_pos) {
			None => Err(format!("Unexpected end of line")),
			Some(_) => {
				self.consume_whitespaces();

				let mut value = String::new();

				while let Some(&c) = self.line_chars.get(self.line_pos) {
					value.push(c);
					self.line_pos += 1;
				}

				self
					.tokens
					.push(Token::TransactionDescription(self.line_index, value));
				Ok(())
			}
		}
	}

	fn toknize_comment(&mut self) -> Result<(), String> {
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

					self.tokens.push(Token::Comment(self.line_index, value));
				}
				Ok(())
			}
		}
	}

	fn toknize_directive_include(&mut self) -> Result<(), String> {
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
					match super::read(file, self.ledger) {
						Err(err) => return Err(err),
						Ok(journal) => {
							self.line_pos += directive_len + 1 + journal.file.chars().count();
							self.ledger.journals.insert(0, journal);
						}
					}
				}

				Ok(())
			}
		}
	}

	fn toknize_posting(&mut self) -> Result<(), String> {
		match self.line_chars.get(self.line_pos) {
			None => Ok(()),
			Some(_) => {
				let mut value = String::new();

				while let Some(&c) = self.line_chars.get(self.line_pos) {
					if self.is_tab(self.line_pos)
						|| (self.is_space(self.line_pos) && self.is_space(self.line_pos + 1))
					{
						self
							.tokens
							.push(Token::PostingAccount(self.line_index, value));

						self.consume_whitespaces();
						self.posting_commodity()?;

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
					self.consume_whitespaces();
					return self.posting_commodity();
				};
			}
		}
		Ok(())
	}

	fn posting_commodity(&mut self) -> Result<(), String> {
		match self.line_chars.get(self.line_pos) {
			None => Ok(()),
			Some(_) => {
				let mut value = String::new();

				while let Some(&c) = self.line_chars.get(self.line_pos) {
					if c == '-' || c.is_numeric() {
						break;
					}

					if c.is_whitespace() {
						self.line_pos += 1;
						continue;
					}

					value.push(c);
					self.line_pos += 1;
				}

				if !value.is_empty() {
					self
						.tokens
						.push(Token::PostingCommodity(self.line_index, value));
				}

				self.posting_amount()
			}
		}
	}

	fn posting_amount(&mut self) -> Result<(), String> {
		match self.line_chars.get(self.line_pos) {
			None => Err(format!("Unexpected end of line")),
			Some(_) => {
				let mut value = String::new();

				if let Some(&c) = self.line_chars.get(self.line_pos) {
					if c == '-' {
						value.push(c);
						self.line_pos += 1;
					}
				}

				match self.line_chars.get(self.line_pos) {
					None => return Err(format!("Unexpected end of line")),
					Some(c) if !c.is_numeric() => return Err(format!("Expected number, got {}", c)),
					Some(&c) => {
						value.push(c);
						self.line_pos += 1;
					}
				}

				while let Some(&c) = self.line_chars.get(self.line_pos) {
					if !c.is_numeric() && c != '.' {
						break;
					}
					value.push(c);
					self.line_pos += 1;
				}

				while let Some(&c) = self.line_chars.get(self.line_pos) {
					if c == '=' {
						break;
					} else if c.is_whitespace() {
						self.line_pos += 1;
						continue;
					} else if !c.is_numeric() {
						return Err(format!("Expected number, got {}", c));
					}
					value.push(c);
					self.line_pos += 1;
				}

				self
					.tokens
					.push(Token::PostingAmount(self.line_index, value));

				Ok(())
			}
		}
	}
}
