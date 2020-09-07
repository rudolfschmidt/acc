#[derive(Debug)]
pub enum Item {
	Comment {
		line: usize,
		comment: String,
	},
	Transaction {
		file: std::path::PathBuf,
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
		costs: Option<Costs>,
		lot_price: Option<MixedAmount>,
		balance_assertion: Option<MixedAmount>,
	},
	BalancedPosting {
		line: usize,
		account: String,
		comments: Vec<Comment>,
		unbalanced_amount: Option<MixedAmount>,
		balanced_amount: MixedAmount,
		costs: Option<Costs>,
		lot_price: Option<MixedAmount>,
		balance_assertion: Option<MixedAmount>,
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
