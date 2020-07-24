use super::model::Ledger;

pub fn print_codes(ledger: &Ledger) -> Result<(), String> {
	for code in ledger
		.journals
		.iter()
		.flat_map(|j| j.transactions.iter())
		.filter(|t| t.code.is_some())
		.map(|t| t.code.as_ref().unwrap())
		.collect::<std::collections::BTreeSet<&String>>()
	{
		println!("{}", code);
	}
	Ok(())
}
