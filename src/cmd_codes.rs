use super::model::Transaction;

pub fn print_codes(transactions: &[Transaction]) -> Result<(), String> {
	for code in transactions
		.iter()
		.filter(|t| t.code.is_some())
		.map(|t| t.code.as_ref().unwrap())
		.collect::<std::collections::BTreeSet<&String>>()
	{
		println!("{}", code);
	}
	Ok(())
}
