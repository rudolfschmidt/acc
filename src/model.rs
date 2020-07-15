pub struct Ledger<'a> {
	pub journals: Vec<Journal<'a>>,
}

pub struct Journal<'a> {
	pub file: String,
	pub content: String,
	pub lexer_tokens: Vec<Token>,
	pub unbalanced_transactions: Vec<Transaction<'a, UnbalancedPosting<'a>>>,
	pub balanced_transactions: Vec<Transaction<'a, BalancedPosting<'a>>>,
}

#[derive(Debug)]
pub enum Token {
	TransactionDate(usize, String),
	TransactionState(usize, State),
	TransactionDescription(usize, String),
	TransactionComment(usize, String),
	PostingAccount(usize, String),
	PostingCommodity(usize, String),
	PostingAmount(usize, String),
}

#[derive(Debug)]
pub enum State {
	Cleared,
	Uncleared,
	Pending,
}

#[derive(Debug)]
pub struct Transaction<'a, T> {
	pub line: usize,
	pub date: &'a str,
	pub state: &'a State,
	pub description: &'a str,
	pub comments: Vec<TransactionComment<'a>>,
	pub postings: Vec<T>,
}

#[derive(Debug)]
pub struct TransactionComment<'a> {
	pub line: usize,
	pub comment: &'a str,
}

#[derive(Debug)]
pub struct UnbalancedPosting<'a> {
	pub line: usize,
	pub account: &'a str,
	pub commodity: Option<&'a str>,
	pub amount: Option<num::rational::Rational64>,
}

#[derive(Debug)]
pub struct BalancedPosting<'a> {
	pub account: &'a str,
	pub commodity: &'a str,
	pub amount: num::rational::Rational64,
}
