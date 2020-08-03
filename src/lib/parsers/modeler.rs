extern crate num;

use super::super::model::Comment;
use super::super::model::MixedAmount;
use super::super::model::Posting;
use super::super::model::Token;
use super::super::model::Transaction;
use super::Error;
use std::path::Path;

struct Parser<'a> {
	tokens: &'a [Token],
	transactions: &'a mut Vec<Transaction>,
	index: usize,
}

pub fn build(
	file: &Path,
	tokens: &[Token],
	transactions: &mut Vec<Transaction>,
) -> Result<(), Error> {
	let mut parser = Parser {
		tokens,
		transactions,
		index: 0,
	};
	match parser.parse() {
		Ok(()) => Ok(()),
		Err(message) => Err(Error {
			line: match parser.tokens.get(parser.index) {
				None => parser.index + 1,
				Some(token) => match token {
					Token::TransactionDateYear(line, _value) => *line,
					Token::TransactionDateMonth(line, _value) => *line,
					Token::TransactionDateDay(line, _value) => *line,
					Token::TransactionState(line, _value) => *line,
					Token::TransactionCode(line, _value) => *line,
					Token::TransactionDescription(line, _value) => *line,
					Token::Comment(line, _value) => *line,
					Token::PostingAccount(line, _value) => *line,
					Token::PostingVirtualAccount(line, _value) => *line,
					Token::PostingCommodity(line, _value) => *line,
					Token::PostingAmount(line, _value) => *line,
					Token::BalanceAssertion(line) => *line,
					Token::Alias(line, _value) => *line,
				},
			},
			message,
		}),
	}
}

impl<'a> Parser<'a> {
	fn parse(&mut self) -> Result<(), String> {
		while self.index < self.tokens.len() {
			self.journal_comment()?;
			self.parse_transaction()?;
			self.parse_posting()?;
			self.parse_virtual_posting()?;
			self.parse_balance_assertion()?;
			self.parse_alias()?;
		}
		Ok(())
	}

	fn journal_comment(&mut self) -> Result<(), String> {
		let mut comments = Vec::new();
		while let Some(Token::Comment(line, comment)) = self.tokens.get(self.index) {
			comments.push(Comment {
				line: *line,
				comment: comment.to_owned(),
			});
			self.index += 1;
		}
		Ok(())
	}

	fn parse_transaction(&mut self) -> Result<(), String> {
		if let Some(Token::TransactionDateYear(line, year)) = self.tokens.get(self.index) {
			self.index += 1;

			let month = match self.tokens.get(self.index) {
				Some(Token::TransactionDateMonth(_line, month)) => {
					self.index += 1;
					month
				}
				_ => return Err(String::from("invalid date")),
			};

			let day = match self.tokens.get(self.index) {
				Some(Token::TransactionDateDay(_line, day)) => {
					self.index += 1;
					day
				}
				_ => return Err(String::from("invalid date")),
			};

			let state = match self.tokens.get(self.index) {
				Some(Token::TransactionState(_line, state)) => {
					self.index += 1;
					state.to_owned()
				}
				_ => return Err(String::from("invalid date")),
			};

			let code = match self.tokens.get(self.index) {
				Some(Token::TransactionCode(_line, code)) => {
					self.index += 1;
					Some(code.to_owned())
				}
				_ => None,
			};

			let description = match self.tokens.get(self.index) {
				Some(Token::TransactionDescription(_line, description)) => {
					self.index += 1;
					description.to_owned()
				}
				_ => return Err(String::from("no description found")),
			};

			let mut comments = Vec::new();
			while let Some(Token::Comment(_line, comment)) = self.tokens.get(self.index) {
				comments.push(Comment {
					line: *line,
					comment: comment.to_owned(),
				});
				self.index += 1;
			}

			self.transactions.push(Transaction {
				line: *line,
				date: format!("{}-{}-{}", year, month, day),
				state,
				code,
				description,
				comments: comments,
				postings: Vec::new(),
			});
		}
		Ok(())
	}

	fn parse_posting(&mut self) -> Result<(), String> {
		if let Some(Token::PostingAccount(line, account)) = self.tokens.get(self.index) {
			self.index += 1;

			let mut comments = Vec::new();
			while let Some(Token::Comment(_line, comment)) = self.tokens.get(self.index) {
				self.index += 1;
				comments.push(Comment {
					line: self.index + 1,
					comment: comment.to_owned(),
				});
			}

			let commodity = match self.tokens.get(self.index) {
				Some(Token::PostingCommodity(_line, commodity)) => {
					self.index += 1;
					commodity.to_owned()
				}
				_ => String::from("commodity expected"),
			};

			let unbalanced_amount = match self.tokens.get(self.index) {
				Some(Token::PostingAmount(_line, amount)) => {
					self.index += 1;
					Some(MixedAmount {
						commodity,
						amount: create_rational(&amount),
					})
				}
				_ => None,
			};

			while let Some(Token::Comment(_line, comment)) = self.tokens.get(self.index) {
				self.index += 1;
				comments.push(Comment {
					line: self.index + 1,
					comment: comment.to_owned(),
				});
			}

			self
				.transactions
				.last_mut()
				.unwrap()
				.postings
				.push(Posting {
					line: *line,
					account: account.to_owned(),
					unbalanced_amount,
					balanced_amount: None,
					balance_assertion: None,
					comments: comments,
				});
		}
		Ok(())
	}

	fn parse_virtual_posting(&mut self) -> Result<(), String> {
		if let Some(Token::PostingVirtualAccount(_line, _account)) = self.tokens.get(self.index) {
			self.index += 1;

			let mut comments = Vec::new();

			while let Some(Token::Comment(_line, comment)) = self.tokens.get(self.index) {
				self.index += 1;
				comments.push(Comment {
					line: self.index + 1,
					comment: comment.to_owned(),
				});
			}

			let commodity = match self.tokens.get(self.index) {
				Some(Token::PostingCommodity(_line, commodity)) => {
					self.index += 1;
					commodity.to_owned()
				}
				_ => String::from("commodity expected"),
			};

			let unbalanced_amount = match self.tokens.get(self.index) {
				Some(Token::PostingAmount(_line, amount)) => {
					self.index += 1;
					Some(MixedAmount {
						commodity,
						amount: create_rational(&amount),
					})
				}
				_ => None,
			};

			while let Some(Token::Comment(_line, comment)) = self.tokens.get(self.index) {
				self.index += 1;
				comments.push(Comment {
					line: self.index + 1,
					comment: comment.to_owned(),
				});
			}
			// handle virtual posts
		}
		Ok(())
	}

	fn parse_balance_assertion(&mut self) -> Result<(), String> {
		if let Some(Token::BalanceAssertion(_line)) = self.tokens.get(self.index) {
			self.index += 1;

			let commodity = match self.tokens.get(self.index) {
				Some(Token::PostingCommodity(_line, commodity)) => {
					self.index += 1;
					commodity.to_owned()
				}
				_ => String::from("commodity expected"),
			};

			let amount = match self.tokens.get(self.index) {
				Some(Token::PostingAmount(_line, amount)) => {
					self.index += 1;
					create_rational(&amount)
				}
				_ => return Err(String::from("amount expected")),
			};

			match self.transactions.last_mut().unwrap().postings.last_mut() {
				None => return Err(format!("posting not found")),
				Some(posting) => posting.balance_assertion = Some(MixedAmount { commodity, amount }),
			}
		}
		Ok(())
	}

	fn parse_alias(&mut self) -> Result<(), String> {
		if let Some(Token::Alias(_line, _alias)) = self.tokens.get(self.index) {
			self.index += 1;
		}
		Ok(())
	}
}

fn create_rational(value: &str) -> num::rational::Rational64 {
	let (_, right) = if let Some(index) = value.find('.') {
		let (left, right) = value.split_at(index);
		let right = right.chars().skip(1).collect::<String>();
		(left, right)
	} else {
		(value, "".to_owned())
	};
	let exponent: usize = right.chars().count();
	let numerator: i64 = value.replace('.', "").parse().unwrap();
	let denominator: i64 = 10_usize.pow(exponent as u32) as i64;
	num::rational::Rational64::new(numerator, denominator)
}
