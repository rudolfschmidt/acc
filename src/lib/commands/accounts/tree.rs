use super::super::super::model::Item;
use super::super::super::model::Posting;

pub(super) fn print(items: Vec<Item>) -> Result<(), String> {
	let mut list: Vec<Account> = Vec::new();
	for account in items
		.into_iter()
		.filter_map(|item| match item {
			Item::Transaction { postings, .. } => Some(postings),
			_ => None,
		})
		.flat_map(|postings| postings.into_iter())
		.flat_map(|posting| match posting {
			Posting::UnbalancedPosting { .. } => None,
			Posting::BalancedPosting { account, .. } => Some(account),
			Posting::EquityPosting { account, .. } => Some(account),
		})
		.collect::<std::collections::BTreeSet<String>>()
	{
		let mut it = account.split(':');
		build_accounts_tree(&mut list, &mut it);
	}
	print_accounts_tree_list(0, list);
	Ok(())
}

struct Account {
	name: String,
	children: Vec<Account>,
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
					children,
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
