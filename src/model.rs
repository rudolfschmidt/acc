pub struct Ledger {
	pub journals: Vec<Journal>,
}

pub struct Journal {
	pub file: String,
	pub content: String,
	pub lexer_tokens: Vec<Token>,
	pub unbalanced_transactions: Vec<Transaction<UnbalancedPosting>>,
	pub balanced_transactions: Vec<Transaction<BalancedPosting>>,
}

#[derive(Debug)]
pub enum Token {
	TransactionDate(usize, String),
	TransactionState(usize, State),
	TransactionCode(usize, String),
	TransactionDescription(usize, String),
	TransactionComment(usize, String),
	PostingAccount(usize, String),
	PostingCommodity(usize, String),
	PostingAmount(usize, String),
}

#[derive(Debug, Clone)]
pub enum State {
	Cleared,
	Uncleared,
	Pending,
}

#[derive(Debug)]
pub struct Transaction<T> {
	pub line: usize,
	pub date: String,
	pub state: State,
	pub code: Option<String>,
	pub description: String,
	pub comments: Vec<TransactionComment>,
	pub postings: Vec<T>,
}

#[derive(Debug)]
pub struct TransactionComment {
	pub line: usize,
	pub comment: String,
}

#[derive(Debug)]
pub struct UnbalancedPosting {
	pub line: usize,
	pub account: String,
	pub commodity: Option<String>,
	pub amount: Option<num::rational::Rational64>,
}

#[derive(Debug)]
pub struct BalancedPosting {
	pub account: String,
	pub commodity: String,
	pub amount: num::rational::Rational64,
}
