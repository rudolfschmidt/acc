use super::super::super::model::BalancedPosting;
use super::super::super::model::Transaction;

pub(super) fn print(transactions: Vec<Transaction<BalancedPosting>>) -> Result<(), String> {
	for account in transactions
		.into_iter()
		.flat_map(|transaction| transaction.postings.into_iter())
		.map(|posting| posting.head.account)
		.collect::<std::collections::BTreeSet<String>>()
	{
		println!("{}", account);
	}
	Ok(())
}
