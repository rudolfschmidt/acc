pub enum Command {
	Print,
	Balance,
	Register,
	Debug,
	Accounts,
	Codes,
}

#[derive(PartialEq)]
pub enum Argument {
	Flat,
	Tree,
	Raw,
	Explicit,
	DebugLexer,
	DebugUnbalancedTransactions,
	DebugBalancedTransactions,
}

pub struct Ledger {
	pub journals: Vec<Journal>,
	pub command: Command,
	pub arguments: Vec<Argument>,
}

pub struct Journal {
	pub file: String,
	pub content: String,
	pub lexer_tokens: Vec<Token>,
	pub transactions: Vec<Transaction>,
}

#[derive(Debug)]
pub enum Token {
	TransactionDate(usize, String),
	TransactionState(usize, State),
	TransactionCode(usize, String),
	TransactionDescription(usize, String),
	Comment(usize, String),
	PostingAccount(usize, String),
	PostingCommodity(usize, String),
	PostingAmount(usize, String),
	BalanceAssertion(usize),
	Include(usize, String),
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
	pub comments: Vec<Comment>,
	pub account: String,
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
