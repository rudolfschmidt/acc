use super::super::model::BalancedPosting;
use super::super::model::Transaction;

pub fn print(transactions: Vec<Transaction<BalancedPosting>>) -> Result<(), String> {
	for code in transactions
		.into_iter()
		.filter_map(|transaction| transaction.header.code)
		.collect::<std::collections::BTreeSet<String>>()
	{
		println!("{}", code);
	}
	Ok(())
}
