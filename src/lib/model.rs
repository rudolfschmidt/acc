#[derive(Debug)]
pub enum Item {
	Comment {
		line: usize,
		comment: String,
	},
	Transaction {
		line: usize,
		date: String,
		state: State,
		code: Option<String>,
		description: String,
		comments: Vec<Comment>,
		postings: Vec<Posting>,
	},
}

#[derive(Debug)]
pub enum Posting {
	UnbalancedPosting {
		line: usize,
		account: String,
		comments: Vec<Comment>,
		unbalanced_amount: Option<MixedAmount>,
		balance_assertion: Option<MixedAmount>,
		costs: Option<Costs>,
	},
	BalancedPosting {
		line: usize,
		account: String,
		comments: Vec<Comment>,
		balanced_amount: MixedAmount,
		balance_assertion: Option<MixedAmount>,
		costs: Option<Costs>,
	},
	EquityPosting {
		account: String,
		amount: MixedAmount,
	},
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
