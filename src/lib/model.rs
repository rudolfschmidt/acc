#[derive(Debug)]
pub enum Item<T> {
	Comment(Comment),
	Transaction(Transaction<T>),
}

#[derive(Debug)]
pub struct Transaction<T> {
	pub header: TransactionHead,
	pub postings: Vec<T>,
}

#[derive(Debug)]
pub struct TransactionHead {
	pub line: usize,
	pub date: String,
	pub state: State,
	pub code: Option<String>,
	pub description: String,
	pub comments: Vec<Comment>,
}

#[derive(Debug)]
pub struct UnbalancedPosting {
	pub header: PostingHead,
	pub amount: Option<MixedAmount>,
}

#[derive(Debug)]
pub struct BalancedPosting {
	pub head: PostingHead,
	pub balanced_amount: MixedAmount,
	pub empty_posting: bool,
}

#[derive(Debug)]
pub struct PostingHead {
	pub line: usize,
	pub account: String,
	pub comments: Vec<Comment>,
	pub balance_assertion: Option<MixedAmount>,
	pub virtual_posting: bool,
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
