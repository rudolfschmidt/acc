use super::super::model::Transaction;

pub fn print(transactions: Vec<Transaction>) -> Result<(), String> {
	for code in transactions
		.into_iter()
		.filter_map(|transaction| transaction.code)
		.collect::<std::collections::BTreeSet<String>>()
	{
		println!("{}", code);
	}
	Ok(())
}
