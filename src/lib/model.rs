#[derive(Debug)]
pub enum Item {
	Comment(Comment),
	Transaction(Transaction),
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
	pub costs: Option<Costs>,
	pub virtual_posting: bool,
}

#[derive(Debug, Clone)]
pub enum Costs {
	Total(MixedAmount),
	PerUnit(MixedAmount),
}

#[derive(Debug, Clone)]
pub enum State {
	Cleared,
	Uncleared,
	Pending,
}

#[derive(Debug, Clone)]
pub struct MixedAmount {
	pub commodity: String,
	pub value: num::rational::Rational64,
}

#[derive(Debug)]
pub struct Comment {
	pub line: usize,
	pub comment: String,
}
