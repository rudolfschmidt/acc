use super::super::super::model::Transaction;

pub(super) fn print(transactions: &[Transaction]) -> Result<(), String> {
	for account in transactions
		.iter()
		.flat_map(|t| t.postings.iter())
		.map(|p| &p.account)
		.collect::<std::collections::BTreeSet<&String>>()
	{
		println!("{}", account);
	}
	Ok(())
}
