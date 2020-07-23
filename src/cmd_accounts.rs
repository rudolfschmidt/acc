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

struct Account {
	name: String,
	children: Vec<Account>,
}

pub fn print_accounts_tree(ledger: &Ledger) -> Result<(), String> {
	let accounts = ledger
		.journals
		.iter()
		.flat_map(|j| j.balanced_transactions.iter())
		.flat_map(|t| t.postings.iter())
		.map(|p| &p.account)
		.collect::<std::collections::BTreeSet<&String>>();

	let mut list: Vec<Account> = Vec::new();
	for account in accounts {
		let mut it = account.split(':').into_iter();
		build_accounts_tree(&mut list, &mut it);
	}
	print_accounts_tree_list(0, list);
	Ok(())
}

fn build_accounts_tree(list: &mut Vec<Account>, it: &mut core::str::Split<char>) {
	match it.next() {
		None => {}
		Some(token) => {
			let mut found = false;
			for item in list.iter_mut() {
				if token == item.name {
					build_accounts_tree(&mut item.children, it);
					found = true;
					break;
				}
			}
			if !found {
				let mut children = Vec::new();
				build_accounts_tree(&mut children, it);
				list.push(Account {
					name: token.to_owned(),
					children: children,
				});
			}
		}
	}
}

fn print_accounts_tree_list(indent: usize, list: Vec<Account>) {
	for item in list {
		println!("{:indent$}{}", "", item.name, indent = indent);
		print_accounts_tree_list(indent + 2, item.children);
	}
}
