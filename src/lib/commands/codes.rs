use super::super::model::Item;

pub fn print(transactions: Vec<Item>) -> Result<(), String> {
	for code in transactions
		.into_iter()
		.filter_map(|item| match item {
			Item::Transaction { code, .. } => code,
			_ => None,
		})
		.collect::<std::collections::BTreeSet<String>>()
	{
		println!("{}", code);
	}
	Ok(())
}
