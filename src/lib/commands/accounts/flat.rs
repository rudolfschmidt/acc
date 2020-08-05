use super::super::super::model::Transaction;

pub(super) fn print(transactions: Vec<Transaction>) -> Result<(), String> {
	for account in transactions
		.into_iter()
		.flat_map(|transaction| transaction.balanced_postings.into_iter())
		.map(|posting| posting.unbalanced_posting.account)
		.collect::<std::collections::BTreeSet<String>>()
	{
		println!("{}", account);
	}
	Ok(())
}
