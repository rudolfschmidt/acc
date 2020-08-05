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
	pub unbalanced_postings: Vec<UnbalancedPosting>,
	pub balanced_postings: Vec<BalancedPosting>,
}

#[derive(Debug, Clone)]
pub enum State {
	Cleared,
	Uncleared,
	Pending,
}

#[derive(Debug)]
pub struct UnbalancedPosting {
	pub line: usize,
	pub account: String,
	pub comments: Vec<Comment>,
	pub unbalanced_amount: Option<MixedAmount>,
	pub balance_assertion: Option<MixedAmount>,
	pub virtual_posting: bool,
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

#[derive(Debug)]
pub struct BalancedPosting {
	pub unbalanced_posting: UnbalancedPosting,
	pub balanced_amount: MixedAmount,
}
