use super::model::Ledger;

pub fn print_accounts_flat(ledger: &Ledger) -> Result<(), String> {
	for account in ledger
		.journals
		.iter()
		.flat_map(|j| j.balanced_transactions.iter())
		.flat_map(|t| t.postings.iter())
		.map(|p| &p.account)
		.collect::<std::collections::BTreeSet<&String>>()
	{
		println!("{}", account);
	}
	Ok(())
}

// TODO
// struct Account {
// 	name: String,
// 	children: Vec<Account>,
// }

pub fn print_accounts_tree(_ledger: &Ledger) -> Result<(), String> {
	// let accounts = ledger
	// 	.journals
	// 	.iter()
	// 	.flat_map(|j| j.balanced_transactions.iter())
	// 	.flat_map(|t| t.postings.iter())
	// 	.map(|p| &p.account)
	// 	.collect::<std::collections::BTreeSet<&String>>();
	Ok(())
}
