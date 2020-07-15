use super::model::State;
use super::model::Token;

struct Lexer<'a> {
	tokens: Vec<Token>,
	line_str: &'a str,
	line_chars: Vec<char>,
	line_index: usize,
	line_pos: usize,
}

struct Error {}

pub fn read_lines<'a>(file: &str, content: &str) -> Result<Vec<Token>, String> {
	let mut lexer = Lexer {
		tokens: Vec::new(),
		line_chars: Vec::new(),
		line_index: 0,
		line_str: "",
		line_pos: 0,
	};
	match lexer.create_tokens(content) {
		Ok(()) => Ok(lexer.tokens),
		Err(_) => {
			let mut msg = String::new();

			msg.push_str(&format!("Lexer Error in {:?}\n", file));
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

			Err(msg)
		}
	}
}

impl<'a> Lexer<'a> {
	fn create_tokens(&mut self, content: &'a str) -> Result<(), Error> {
		for (line_index, line_str) in content.lines().enumerate() {
			self.line_str = line_str;
			self.line_chars = line_str.chars().collect();
			self.line_index = line_index;
			self.line_pos = 0;
			self.toknize_transaction_header()?;
		}
		Ok(())
	}

	fn toknize_transaction_header(&mut self) -> Result<(), Error> {
		match self.line_chars.get(self.line_pos) {
			None => {
				return Ok(());
			}
			Some(c) => {
				if c.is_whitespace() {
					self.toknize_transaction_posting()?;
				} else {
					self.toknize_transaction_date()?;
					self.toknize_transaction_state()?;
					self.toknize_transaction_description()?;
				}
			}
		}
		Ok(())
	}

	fn toknize_transaction_date(&mut self) -> Result<(), Error> {
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

	fn is_numeric(&mut self, value: &mut String) -> Result<(), Error> {
		match self.line_chars.get(self.line_pos) {
			None => return Err(Error {}),
			Some(c) => {
				if !c.is_numeric() {
					return Err(Error {});
				} else {
					value.push(*c);
					self.line_pos += 1;
				}
			}
		}
		Ok(())
	}

	fn is_dash(&mut self, value: &mut String) -> Result<(), Error> {
		match self.line_chars.get(self.line_pos) {
			None => {
				return Err(Error {});
			}
			Some(c) => {
				if '-' != *c {
					return Err(Error {});
				} else {
					value.push(*c);
					self.line_pos += 1;
				}
			}
		}
		Ok(())
	}

	fn toknize_transaction_state(&mut self) -> Result<(), Error> {
		match self.line_chars.get(self.line_pos) {
			None => {
				return Err(Error {});
			}
			Some(c) => {
				if !c.is_whitespace() {
					return Err(Error {});
				} else {
					self.line_pos += 1;
				}
			}
		}
		match self.line_chars.get(self.line_pos) {
			None => {
				return Err(Error {});
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

	fn toknize_transaction_description(&mut self) -> Result<(), Error> {
		match self.line_chars.get(self.line_pos) {
			None => {
				return Err(Error {});
			}
			Some(_) => {
				self.consume_whitespaces();
			}
		}
		let mut value = String::new();
		while self.line_pos < self.line_chars.len() {
			value.push(self.line_chars[self.line_pos]);
			self.line_pos += 1;
		}
		self
			.tokens
			.push(Token::TransactionDescription(self.line_index, value));
		Ok(())
	}

	fn consume_whitespaces(&mut self) {
		while self.line_pos < self.line_chars.len() {
			if !self.line_chars[self.line_pos].is_whitespace() {
				break;
			}
			self.line_pos += 1;
		}
	}

	fn toknize_transaction_posting(&mut self) -> Result<(), Error> {
		match self.line_chars.get(self.line_pos) {
			None => return Ok(()),
			Some(c) if !c.is_whitespace() => return Err(Error {}),
			Some(_) => {}
		}
		while self.line_pos < self.line_chars.len() {
			if !self.line_chars[self.line_pos].is_whitespace() {
				break;
			}
			self.line_pos += 1;
		}
		match self.line_chars.get(self.line_pos) {
			None => {}
			Some(c) if *c == ';' => {
				self.line_pos += 1;
				while self.line_pos < self.line_chars.len() {
					if !self.line_chars[self.line_pos].is_whitespace() {
						break;
					}
					self.line_pos += 1;
				}
				let mut value = String::new();
				while self.line_pos < self.line_chars.len() {
					value.push(self.line_chars[self.line_pos]);
					self.line_pos += 1;
				}
				self
					.tokens
					.push(Token::TransactionComment(self.line_index, value));
				return Ok(());
			}
			Some(_) => {}
		}
		let mut value = String::new();
		while self.line_pos < self.line_chars.len() {
			if self.is_tab(self.line_pos)
				|| (self.is_space(self.line_pos) && self.is_space(self.line_pos + 1))
			{
				self
					.tokens
					.push(Token::PostingAccount(self.line_index, value));
				return self.posting_mixed_amounts();
			}
			value.push(self.line_chars[self.line_pos]);
			self.line_pos += 1;
		}
		self
			.tokens
			.push(Token::PostingAccount(self.line_index, value));
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

	fn posting_mixed_amounts(&mut self) -> Result<(), Error> {
		while self.line_pos < self.line_chars.len() {
			if !self.line_chars[self.line_pos].is_whitespace() {
				break;
			}
			self.line_pos += 1;
		}
		match self.line_chars.get(self.line_pos) {
			None => Ok(()),
			Some(_) => {
				self.posting_commodity()?;
				self.posting_amount()?;
				Ok(())
			}
		}
	}

	fn posting_commodity(&mut self) -> Result<(), Error> {
		let mut value = String::new();
		while self.line_pos < self.line_chars.len() {
			if self.line_chars[self.line_pos].is_numeric() || '-' == self.line_chars[self.line_pos] {
				break;
			}
			value.push(self.line_chars[self.line_pos]);
			self.line_pos += 1;
		}
		self
			.tokens
			.push(Token::PostingCommodity(self.line_index, value));
		Ok(())
	}

	fn posting_amount(&mut self) -> Result<(), Error> {
		if self.line_chars.get(self.line_pos).is_none() {
			return Err(Error {});
		}
		while self.line_pos < self.line_chars.len() {
			if !self.line_chars[self.line_pos].is_whitespace() {
				break;
			}
			self.line_pos += 1;
		}
		let mut value = String::new();
		if let Some(c) = self.line_chars.get(self.line_pos) {
			if *c == '-' {
				value.push(self.line_chars[self.line_pos]);
				self.line_pos += 1;
			}
		}
		while self.line_pos < self.line_chars.len() {
			match self.line_chars[self.line_pos] {
				c if c.is_numeric() => value.push(c),
				c if c == '.' => match self.line_chars.get(self.line_pos + 1) {
					None => return Err(Error {}),
					Some(c) if !c.is_numeric() => return Err(Error {}),
					Some(_) => {
						value.push(c);
						self.line_pos += 1;
						break;
					}
				},
				_ => return Err(Error {}),
			}
			self.line_pos += 1;
		}
		while self.line_pos < self.line_chars.len() {
			if !self.line_chars[self.line_pos].is_numeric() {
				return Err(Error {});
			}
			value.push(self.line_chars[self.line_pos]);
			self.line_pos += 1;
		}
		self
			.tokens
			.push(Token::PostingAmount(self.line_index, value));
		Ok(())
	}
}
