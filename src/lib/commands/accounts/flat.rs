use super::super::super::model::Item;
use super::super::super::model::Posting;

pub(super) fn print(items: Vec<Item>) -> Result<(), String> {
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
		println!("{}", account);
	}
	Ok(())
}
