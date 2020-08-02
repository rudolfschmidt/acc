#[derive(Debug)]
pub enum Token {
	TransactionDateYear(usize, String),
	TransactionDateMonth(usize, String),
	TransactionDateDay(usize, String),
	TransactionState(usize, State),
	TransactionCode(usize, String),
	TransactionDescription(usize, String),
	Comment(usize, String),
	PostingAccount(usize, String),
	PostingVirtualAccount(usize, String),
	PostingCommodity(usize, String),
	PostingAmount(usize, String),
	BalanceAssertion(usize),
	Alias(usize, String),
}

#[derive(Debug, Clone)]
pub enum State {
	Cleared,
	Uncleared,
	Pending,
}

#[derive(Debug)]
pub struct Transaction {
	pub line: usize,
	pub date: String,
	pub state: State,
	pub code: Option<String>,
	pub description: String,
	pub comments: Vec<Comment>,
	pub postings: Vec<Posting>,
}

#[derive(Debug)]
pub struct Posting {
	pub line: usize,
	pub account: String,
	pub comments: Vec<Comment>,
	pub unbalanced_amount: Option<MixedAmount>,
	pub balanced_amount: Option<MixedAmount>,
	pub balance_assertion: Option<MixedAmount>,
}

#[derive(Debug)]
pub struct MixedAmount {
	pub commodity: String,
	pub amount: num::rational::Rational64,
}

#[derive(Debug)]
pub struct Comment {
	pub line: usize,
	pub comment: String,
}
