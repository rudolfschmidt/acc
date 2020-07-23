use super::errors::Error;
use super::model::State;
use super::model::Token;

pub fn read_lines(content: &str, tokens: &mut Vec<Token>) -> Result<(), Error> {
	let mut lexer = Lexer {
		tokens,
		content,
		line_chars: Vec::new(),
		line_index: 0,
		line_str: "",
		line_pos: 0,
	};
	match lexer.create_tokens() {
		Ok(()) => Ok(()),
		Err(_) => {
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

			Err(Error {
				line: lexer.line_index + 1,
				message: msg,
			})
		}
	}
}

struct Lexer<'a> {
	tokens: &'a mut Vec<Token>,
	content: &'a str,
	line_str: &'a str,
	line_chars: Vec<char>,
	line_index: usize,
	line_pos: usize,
}

impl<'a> Lexer<'a> {
	fn create_tokens(&mut self) -> Result<(), ()> {
		for (line_index, line_str) in self.content.lines().enumerate() {
			self.line_str = line_str;
			self.line_chars = line_str.chars().collect();
			self.line_index = line_index;
			self.line_pos = 0;
			self.toknize_transaction_header()?;
		}
		Ok(())
	}

	fn toknize_transaction_header(&mut self) -> Result<(), ()> {
		match self.line_chars.get(self.line_pos) {
			None => {
				return Ok(());
			}
			Some(_) => {
				if self.is_tab(self.line_pos)
					|| (self.is_space(self.line_pos) && self.is_space(self.line_pos + 1))
				{
					self.consume_whitespaces();
					self.toknize_transaction_comment()?;
					self.toknize_transaction_posting()?;
				} else {
					self.toknize_transaction_date()?;
					self.toknize_transaction_state()?;
					self.toknize_transaction_code()?;
					self.toknize_transaction_description()?;
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

	fn toknize_transaction_date(&mut self) -> Result<(), ()> {
		let mut value = String::new();
		self.is_numeric(&mut value)?;
		self.is_numeric(&mut value)?;
		self.is_numeric(&mut value)?;
		self.is_numeric(&mut value)?;
		self.is_dash(&mut value)?;
		self.is_numeric(&mut value)?;
		self.is_numeric(&mut value)?;
		self.is_dash(&mut value)?;
		self.is_numeric(&mut value)?;
		self.is_numeric(&mut value)?;
		self
			.tokens
			.push(Token::TransactionDate(self.line_index, value));
		Ok(())
	}

	fn is_numeric(&mut self, value: &mut String) -> Result<(), ()> {
		match self.line_chars.get(self.line_pos) {
			None => return Err(()),
			Some(c) => {
				if !c.is_numeric() {
					return Err(());
				} else {
					value.push(*c);
					self.line_pos += 1;
				}
			}
		}
		Ok(())
	}

	fn is_dash(&mut self, value: &mut String) -> Result<(), ()> {
		match self.line_chars.get(self.line_pos) {
			None => {
				return Err(());
			}
			Some(c) => {
				if '-' != *c {
					return Err(());
				} else {
					value.push(*c);
					self.line_pos += 1;
				}
			}
		}
		Ok(())
	}

	fn toknize_transaction_state(&mut self) -> Result<(), ()> {
		match self.line_chars.get(self.line_pos) {
			None => {
				return Err(());
			}
			Some(c) => {
				if !c.is_whitespace() {
					return Err(());
				} else {
					self.line_pos += 1;
				}
			}
		}
		match self.line_chars.get(self.line_pos) {
			None => {
				return Err(());
			}
			Some(c) => match c {
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
			},
		}
		Ok(())
	}

	fn toknize_transaction_code(&mut self) -> Result<(), ()> {
		match self.line_chars.get(self.line_pos) {
			None => {
				return Ok(());
			}
			Some(_) => {
				self.consume_whitespaces();
			}
		}
		if let Some(&c) = self.line_chars.get(self.line_pos) {
			if c == '(' {
				self.line_pos += 1;
				let mut value = String::new();
				match self.line_chars.get(self.line_pos) {
					None => {
						return Err(());
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
		}
		Ok(())
	}

	fn toknize_transaction_description(&mut self) -> Result<(), ()> {
		match self.line_chars.get(self.line_pos) {
			None => {
				return Err(());
			}
			Some(_) => {
				self.consume_whitespaces();
			}
		}
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

	fn toknize_transaction_comment(&mut self) -> Result<(), ()> {
		if let Some(&c) = self.line_chars.get(self.line_pos) {
			if c == ';' {
				self.line_pos += 1;
				self.consume_whitespaces();
				let mut value = String::new();
				while let Some(&c) = self.line_chars.get(self.line_pos) {
					value.push(c);
					self.line_pos += 1;
				}
				self
					.tokens
					.push(Token::TransactionComment(self.line_index, value));
			}
		}
		Ok(())
	}

	fn toknize_transaction_posting(&mut self) -> Result<(), ()> {
		if self.line_chars.get(self.line_pos).is_none() {
			return Ok(());
		}

		let mut value = String::new();

		while let Some(&c) = self.line_chars.get(self.line_pos) {
			if self.is_tab(self.line_pos)
				|| (self.is_space(self.line_pos) && self.is_space(self.line_pos + 1))
			{
				self
					.tokens
					.push(Token::PostingAccount(self.line_index, value));
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

	fn balance_assertion(&mut self) -> Result<(), ()> {
		if let Some(&c) = self.line_chars.get(self.line_pos) {
			if c == '=' {
				self.line_pos += 1;
				self.tokens.push(Token::BalanceAssertion(self.line_index));
				return if self.line_chars.get(self.line_pos).is_none() {
					Err(())
				} else {
					self.posting_commodity()
				};
			}
		}
		Ok(())
	}

	fn posting_commodity(&mut self) -> Result<(), ()> {
		if self.line_chars.get(self.line_pos).is_none() {
			return Ok(());
		}

		self.consume_whitespaces();

		let mut value = String::new();

		while let Some(&c) = self.line_chars.get(self.line_pos) {
			if c == '-' || c.is_numeric() {
				break;
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

	fn posting_amount(&mut self) -> Result<(), ()> {
		if self.line_chars.get(self.line_pos).is_none() {
			return Err(());
		}

		let mut value = String::new();

		if let Some(&c) = self.line_chars.get(self.line_pos) {
			if c == '-' {
				value.push(c);
				self.line_pos += 1;
			}
		}

		match self.line_chars.get(self.line_pos) {
			None => return Err(()),
			Some(&c) if !c.is_numeric() => return Err(()),
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
				return Err(());
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
