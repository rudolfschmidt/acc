use super::super::super::model::Transaction;
use super::super::format_amount;
use super::common::group_postings_by_account;
use super::common::print_commodity_amount;

use colored::Colorize;
use num::Zero;
use std::collections::BTreeMap;
use std::collections::HashMap;

#[derive(Debug, Clone)]
struct BalanceAccount {
	account: String,
	amounts: BTreeMap<String, num::rational::Rational64>,
	children: Vec<BalanceAccount>,
}

pub(super) fn print(transactions: Vec<Transaction>) -> Result<(), String> {
	if transactions.iter().any(|t| t.balanced_postings.is_empty()) {
		return Ok(());
	}
	let grouped_postings = group_postings_by_account(transactions)?;
	let accounts = make_balance_accounts(&grouped_postings);
	let root_account = make_root_balance_account(&accounts);
	let amount_width = calculate_amount_width(&root_account.children);
	print_balance_accounts(root_account, amount_width);
	Ok(())
}

fn calculate_amount_width(list: &[BalanceAccount]) -> usize {
	std::cmp::max(
		list
			.iter()
			.flat_map(|a| a.amounts.iter())
			.map(|(k, v)| k.chars().count() + format_amount(&v).chars().count())
			.max()
			.unwrap_or(0),
		list
			.iter()
			.map(|a| calculate_amount_width(&a.children))
			.max()
			.unwrap_or(0),
	)
}

fn make_balance_accounts(
	posts: &BTreeMap<String, BTreeMap<String, num::rational::Rational64>>,
) -> Vec<BalanceAccount> {
	let mut balance_accounts = Vec::<BalanceAccount>::new();
	for (account, amounts) in posts {
		let accounts = account.split(':').map(|s| s).collect::<Vec<&str>>();
		balance_accounts.push(make_balance_account(amounts, &accounts, 0));
	}
	balance_accounts
}

fn make_balance_account(
	amounts: &BTreeMap<String, num::rational::Rational64>,
	accounts: &[&str],
	index: usize,
) -> BalanceAccount {
	if index + 1 >= accounts.len() {
		return BalanceAccount {
			account: accounts.get(index).unwrap().to_string(),
			amounts: amounts.clone(),
			children: Vec::new(),
		};
	}
	let made_balance_account = make_balance_account(amounts, &accounts, index + 1);
	let mut balance_account = BalanceAccount {
		account: accounts.get(index).unwrap().to_string(),
		amounts: BTreeMap::new(),
		children: Vec::new(),
	};
	balance_account.children.push(made_balance_account);
	balance_account
}

fn make_root_balance_account(structs: &[BalanceAccount]) -> BalanceAccount {
	let mut children = Vec::new();
	let mut by_account = HashMap::<String, Vec<&BalanceAccount>>::new();
	structs.iter().for_each(|s| {
		by_account
			.entry(s.account.to_string())
			.and_modify(|sp| sp.push(s))
			.or_insert_with(|| vec![s]);
	});
	for (account, mut list) in by_account {
		let same_account_posts = list
			.iter()
			.flat_map(|sp| sp.children.iter().cloned())
			.collect::<Vec<BalanceAccount>>();
		let calculated = make_root_balance_account(&same_account_posts);
		let mut amounts = BTreeMap::new();
		list
			.iter_mut()
			.flat_map(|p| p.amounts.iter())
			.for_each(|(commodity, amount)| {
				amounts
					.entry(commodity.to_string())
					.and_modify(|a| *a += amount)
					.or_insert(*amount);
			});
		calculated
			.children
			.iter()
			.flat_map(|p| p.amounts.iter())
			.for_each(|(commodity, amount)| {
				amounts
					.entry(commodity.to_string())
					.and_modify(|a| *a += amount)
					.or_insert(*amount);
			});
		children.push(BalanceAccount {
			account,
			amounts,
			children: calculated.children,
		});
	}

	children.sort_by(|a, b| a.account.cmp(&b.account));

	let mut root_amounts = BTreeMap::new();
	children
		.iter()
		.flat_map(|a| a.amounts.iter())
		.for_each(|(commodity, amount)| {
			root_amounts
				.entry(commodity.to_string())
				.and_modify(|a| *a += amount)
				.or_insert(*amount);
		});

	BalanceAccount {
		account: "".to_string(),
		amounts: root_amounts,
		children,
	}
}

fn print_balance_accounts(account: BalanceAccount, amount_width: usize) {
	for child in &account.children {
		print_balance_account("", child, amount_width);
	}
	for _ in 0..amount_width {
		print!("-");
	}
	if account.children.is_empty() {
		return;
	}
	println!();
	if account.amounts.iter().all(|(_, a)| a.is_zero()) {
		println!("{:>w$} ", 0, w = amount_width);
		return;
	}
	account.amounts.iter().for_each(|(commodity, amount)| {
		print_commodity_amount(commodity, amount, amount_width);
		println!();
	});
}

fn print_balance_account(indent: &str, post: &BalanceAccount, amount_width: usize) {
	let mut it = post.amounts.iter().peekable();
	while let Some((commodity, amount)) = it.next() {
		print_commodity_amount(commodity, amount, amount_width);
		if it.peek().is_some() {
			println!();
		}
	}
	println!("{}{}", indent, post.account.blue());
	for child in &post.children {
		print_balance_account(&format!("{}  ", indent), child, amount_width);
	}
}
