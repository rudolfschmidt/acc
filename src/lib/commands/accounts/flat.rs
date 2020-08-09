use super::super::super::model::Transaction;

pub(super) fn print(transactions: Vec<Transaction>) -> Result<(), String> {
	for account in transactions
		.into_iter()
		.flat_map(|transaction| transaction.postings.into_iter())
		.map(|posting| posting.account)
		.collect::<std::collections::BTreeSet<String>>()
	{
		println!("{}", account);
	}
	Ok(())
}
