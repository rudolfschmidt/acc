extern crate num;

use super::super::errors::Error;
use super::super::model::Comment;
use super::super::model::MixedAmount;
use super::super::model::Posting;
use super::super::model::Token;
use super::super::model::Transaction;

pub fn build(tokens: &[Token], transactions: &mut Vec<Transaction>) -> Result<(), Error> {
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
					Token::TransactionDateYear(line, _value) => *line,
					Token::TransactionDateMonth(line, _value) => *line,
					Token::TransactionDateDay(line, _value) => *line,
					Token::TransactionState(line, _value) => *line,
					Token::TransactionCode(line, _value) => *line,
					Token::TransactionDescription(line, _value) => *line,
					Token::Comment(line, _value) => *line,
					Token::PostingAccount(line, _value) => *line,
					Token::PostingCommodity(line, _value) => *line,
					Token::PostingAmount(line, _value) => *line,
					Token::BalanceAssertion(line) => *line,
					Token::Alias(line, _value) => *line,
				},
			},
			message: format!("Parse Error : {}", message),
		}),
		Ok(()) => Ok(()),
	}
}

struct Parser<'a> {
	tokens: &'a [Token],
	transactions: &'a mut Vec<Transaction>,
	index: usize,
}

impl<'a> Parser<'a> {
	fn parse(&mut self) -> Result<(), String> {
		while self.index < self.tokens.len() {
			self.parse_transaction()?;
			self.parse_comment()?;
			self.parse_posting()?;
			self.parse_balance_assertion()?;
			self.parse_alias()?;
		}
		Ok(())
	}

	fn parse_transaction(&mut self) -> Result<(), String> {
		if let Some(token) = self.tokens.get(self.index) {
			if let Token::TransactionDateYear(line, year) = token {
				self.index += 1;

				let month = match self.tokens.get(self.index) {
					None => return Err(format!("")),
					Some(token) => match token {
						Token::TransactionDateMonth(_, month) => {
							self.index += 1;
							month.to_owned()
						}
						_ => return Err(format!("")),
					},
				};

				let day = match self.tokens.get(self.index) {
					None => return Err(format!("")),
					Some(token) => match token {
						Token::TransactionDateDay(_, day) => {
							self.index += 1;
							day.to_owned()
						}
						_ => return Err(format!("")),
					},
				};

				let state = match self.tokens.get(self.index) {
					None => return Err(format!("")),
					Some(token) => match token {
						Token::TransactionState(_, state) => {
							self.index += 1;
							state.to_owned()
						}
						_ => return Err(format!("")),
					},
				};

				let code = match self.tokens.get(self.index) {
					None => return Err(format!("")),
					Some(token) => match token {
						Token::TransactionCode(_, code) => {
							self.index += 1;
							Some(code.to_owned())
						}
						_ => None,
					},
				};

				let description = match self.tokens.get(self.index) {
					None => return Err(format!("")),
					Some(token) => match token {
						Token::TransactionDescription(_, description) => {
							self.index += 1;
							description.to_owned()
						}
						_ => return Err(format!("")),
					},
				};

				self.transactions.push(Transaction {
					line: *line,
					date: format!(
						"{}-{}-{}",
						year.to_owned(),
						month.to_owned(),
						day.to_owned()
					),
					state,
					code,
					description,
					comments: Vec::new(),
					postings: Vec::new(),
				});
			}
		}
		Ok(())
	}

	fn parse_comment(&mut self) -> Result<(), String> {
		if let Some(token) = self.tokens.get(self.index) {
			if let Token::Comment(line, value) = token {
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

				let commodity = match self.tokens.get(self.index) {
					None => format!(""),
					Some(token) => match token {
						Token::PostingCommodity(_, commodity) => {
							self.index += 1;
							commodity.to_owned()
						}
						_ => format!(""),
					},
				};

				let unbalanced_amount = match self.tokens.get(self.index) {
					None => None,
					Some(token) => match token {
						Token::PostingAmount(_, amount) => {
							self.index += 1;
							Some(MixedAmount {
								commodity,
								amount: create_rational(&amount),
							})
						}
						_ => None,
					},
				};

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
						comments: Vec::new(),
					});
			}
		}
		Ok(())
	}

	fn parse_balance_assertion(&mut self) -> Result<(), String> {
		if let Some(token) = self.tokens.get(self.index) {
			if let Token::BalanceAssertion(_line) = token {
				self.index += 1;

				let commodity = match self.tokens.get(self.index) {
					None => format!(""),
					Some(token) => match token {
						Token::PostingCommodity(_, commodity) => {
							self.index += 1;
							commodity.to_owned()
						}
						_ => format!(""),
					},
				};

				let amount = match self.tokens.get(self.index) {
					None => return Err(format!("")),
					Some(token) => match token {
						Token::PostingAmount(_, amount) => {
							self.index += 1;
							create_rational(&amount)
						}
						_ => return Err(format!("")),
					},
				};

				match self.transactions.last_mut().unwrap().postings.last_mut() {
					None => return Err(format!("")),
					Some(posting) => posting.balance_assertion = Some(MixedAmount { commodity, amount }),
				}
			}
		}
		Ok(())
	}

	fn parse_alias(&mut self) -> Result<(), String> {
		if let Some(token) = self.tokens.get(self.index) {
			if let Token::Alias(_line, _alias) = token {
				self.index += 1;
			}
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
