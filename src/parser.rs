extern crate num;

use super::model::Token;
use super::model::Transaction;
use super::model::TransactionComment;
use super::model::UnbalancedPosting;

#[derive(Debug)]
struct Error {
	message: String,
}

pub fn parse_unbalanced_transactions<'a>(
	tokens: &'a [Token],
	transactions: &'a mut Vec<Transaction<'a, UnbalancedPosting<'a>>>,
) -> Result<(), String> {
	match parse(tokens, transactions) {
		Err(err) => Err(format!("Parse Error : {}", err.message)),
		Ok(()) => Ok(()),
	}
}

fn parse<'a>(
	tokens: &'a [Token],
	transactions: &'a mut Vec<Transaction<'a, UnbalancedPosting<'a>>>,
) -> Result<(), Error> {
	let mut parser = Parser {
		tokens: tokens,
		transactions: transactions,
		index: 0,
	};

	while parser.index < tokens.len() {
		parser.parse_transaction_header()?;
		parser.parse_transaction_comment()?;
		parser.parse_posting()?;
	}

	Ok(())
}

struct Parser<'a> {
	tokens: &'a [Token],
	transactions: &'a mut Vec<Transaction<'a, UnbalancedPosting<'a>>>,
	index: usize,
}

impl<'a> Parser<'a> {
	fn parse_transaction_header(&mut self) -> Result<(), Error> {
		let date;
		let line;

		match self.tokens.get(self.index).unwrap() {
			Token::TransactionDate(file_line, value) => {
				self.index += 1;
				date = value;
				line = file_line;
			}
			_ => return Ok(()),
		}

		let state;
		match self.tokens.get(self.index).unwrap() {
			Token::TransactionState(_, value) => {
				self.index += 1;
				state = value;
			}
			_ => panic!("transaction state expected"),
		}

		let description;
		match self.tokens.get(self.index).unwrap() {
			Token::TransactionDescription(_, value) => {
				self.index += 1;
				description = value;
			}
			_ => panic!("transaction description expected"),
		}

		self.transactions.push(Transaction {
			line: *line,
			date: date,
			state: state,
			description: description,
			comments: Vec::new(),
			postings: Vec::new(),
		});

		Ok(())
	}

	fn parse_transaction_comment(&mut self) -> Result<(), Error> {
		if let Some(token) = self.tokens.get(self.index) {
			if let Token::TransactionComment(line, value) = token {
				self
					.transactions
					.last_mut()
					.unwrap()
					.comments
					.push(TransactionComment {
						line: *line,
						comment: value,
					});
				self.index += 1;
			}
		}
		Ok(())
	}

	fn parse_posting(&mut self) -> Result<(), Error> {
		let line;
		let account;

		match self.tokens.get(self.index) {
			None => return Ok(()),
			Some(token) => match token {
				Token::PostingAccount(file_line, value) => {
					self.index += 1;
					line = file_line;
					account = value;
				}
				_ => return Ok(()),
			},
		};

		match self.tokens.get(self.index) {
			None => {
				self
					.transactions
					.last_mut()
					.unwrap()
					.postings
					.push(UnbalancedPosting {
						line: *line,
						account: account,
						commodity: None,
						amount: None,
					});
				return Ok(());
			}
			Some(token) => match token {
				Token::PostingCommodity(_value, _line) => {}
				_ => {
					self
						.transactions
						.last_mut()
						.unwrap()
						.postings
						.push(UnbalancedPosting {
							line: *line,
							account: account,
							commodity: None,
							amount: None,
						});
					return Ok(());
				}
			},
		}

		let commodity = match self.tokens.get(self.index) {
			None => panic!("posting commodity not found"),
			Some(token) => match token {
				Token::PostingCommodity(_line, value) => {
					self.index += 1;
					value
				}
				_ => panic!("not a posting commodity"),
			},
		};

		let amount = match self.tokens.get(self.index) {
			None => panic!("posting amount not found"),
			Some(token) => match token {
				Token::PostingAmount(_line, value) => {
					self.index += 1;
					value
				}
				_ => panic!("not a posting amount"),
			},
		};

		self
			.transactions
			.last_mut()
			.unwrap()
			.postings
			.push(UnbalancedPosting {
				line: *line,
				account: account,
				commodity: Some(commodity),
				amount: Some(create_rational(&amount)?),
			});

		Ok(())
	}
}

fn create_rational(value: &str) -> Result<num::rational::Rational64, Error> {
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
