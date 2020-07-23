extern crate num;

use super::errors::Error;

use super::model::Comment;
use super::model::Token;
use super::model::Transaction;
use super::model::UnbalancedPosting;

pub fn parse_unbalanced_transactions(
	tokens: &[Token],
	transactions: &mut Vec<Transaction<UnbalancedPosting>>,
) -> Result<(), Error> {
	let mut parser = Parser {
		tokens,
		transactions,
		index: 0,
	};
	match parser.parse() {
		Err(message) => Err(Error {
			line: match parser.tokens.get(parser.index) {
				None => parser.index + 1,
				Some(token) => match token {
					Token::TransactionDate(line, _value) => *line,
					Token::TransactionState(line, _value) => *line,
					Token::TransactionCode(line, _value) => *line,
					Token::TransactionDescription(line, _value) => *line,
					Token::TransactionComment(line, _value) => *line,
					Token::PostingAccount(line, _value) => *line,
					Token::PostingCommodity(line, _value) => *line,
					Token::PostingAmount(line, _value) => *line,
					Token::BalanceAssertion(line) => *line,
				},
			},
			message: format!("Parse Error : {}", message),
		}),
		Ok(()) => Ok(()),
	}
}

struct Parser<'a> {
	tokens: &'a [Token],
	transactions: &'a mut Vec<Transaction<UnbalancedPosting>>,
	index: usize,
}

impl<'a> Parser<'a> {
	fn parse(&mut self) -> Result<(), String> {
		while self.index < self.tokens.len() {
			self.parse_transaction_header()?;
			self.parse_transaction_comment()?;
			self.parse_posting()?;
			self.index += 1;
		}
		Ok(())
	}

	fn parse_transaction_header(&mut self) -> Result<(), String> {
		if let Some(token) = self.tokens.get(self.index) {
			if let Token::TransactionDate(line, date) = token {
				self.index += 1;

				let state = if let Some(token) = self.tokens.get(self.index) {
					if let Token::TransactionState(_, state) = token {
						self.index += 1;
						state.to_owned()
					} else {
						Err(format!(""))?
					}
				} else {
					Err(format!(""))?
				};

				let code = if let Some(token) = self.tokens.get(self.index) {
					if let Token::TransactionCode(_, code) = token {
						self.index += 1;
						Some(code.to_owned())
					} else {
						None
					}
				} else {
					Err(format!(""))?
				};

				let description = if let Some(token) = self.tokens.get(self.index) {
					if let Token::TransactionDescription(_, description) = token {
						self.index += 1;
						description.to_owned()
					} else {
						Err(format!(""))?
					}
				} else {
					Err(format!(""))?
				};

				self.transactions.push(Transaction {
					line: *line,
					date: date.to_owned(),
					state: state,
					code: code,
					description: description,
					comments: Vec::new(),
					postings: Vec::new(),
				});
			}
		}
		Ok(())
	}

	fn parse_transaction_comment(&mut self) -> Result<(), String> {
		if let Some(token) = self.tokens.get(self.index) {
			if let Token::TransactionComment(line, value) = token {
				self
					.transactions
					.last_mut()
					.unwrap()
					.comments
					.push(Comment {
						line: *line,
						comment: value.to_owned(),
					});
				self.index += 1;
			}
		}
		Ok(())
	}

	fn parse_posting(&mut self) -> Result<(), String> {
		if let Some(token) = self.tokens.get(self.index) {
			if let Token::PostingAccount(line, account) = token {
				self.index += 1;

				let commodity = if let Some(token) = self.tokens.get(self.index) {
					if let Token::PostingCommodity(_line, commodity) = token {
						self.index += 1;
						Some(commodity.to_owned())
					} else {
						None
					}
				} else {
					None
				};

				let amount = if let Some(token) = self.tokens.get(self.index) {
					if let Token::PostingAmount(_line, amount) = token {
						self.index += 1;
						Some(create_rational(&amount)?)
					} else {
						None
					}
				} else {
					None
				};

				self
					.transactions
					.last_mut()
					.unwrap()
					.postings
					.push(UnbalancedPosting {
						line: *line,
						account: account.to_owned(),
						commodity: commodity,
						amount: amount,
						comments: Vec::new(),
					});
			}
		}
		Ok(())
	}
}

fn create_rational(value: &str) -> Result<num::rational::Rational64, String> {
	let (_, right) = if let Some(index) = value.find('.') {
		let (left, right) = value.split_at(index);
		let right = right.chars().skip(1).collect::<String>();
		(left, right)
	} else {
		(value, "".to_string())
	};
	let exponent: usize = right.chars().count();
	let numerator: i64 = value.replace('.', "").parse().unwrap();
	let denominator: i64 = 10_usize.pow(exponent as u32) as i64;
	Ok(num::rational::Rational64::new(numerator, denominator))
}
