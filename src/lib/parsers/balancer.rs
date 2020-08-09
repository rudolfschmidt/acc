extern crate num;

use super::super::model::Costs;
use super::super::model::MixedAmount;
use super::super::model::Posting;
use super::super::model::Transaction;
use super::Error;

use num::Zero;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::ops::Neg;

type Rational = num::rational::Rational64;

pub fn balance(transaction: Transaction) -> Result<Transaction, Error> {
	let transaction = disallow_multiple_empty_posts(transaction)?;
	let transaction = disallow_multiple_commodites_with_empty_post(transaction)?;
	let transaction = balance_empty_posts(transaction)?;
	let transaction = disallow_unbalanced_postings(transaction)?;
	Ok(transaction)
}

fn disallow_multiple_empty_posts(transaction: Transaction) -> Result<Transaction, Error> {
	if empty_posts_count(&transaction) > 1 {
		return Err(from_unbalanced_transaction(
			transaction,
			"Only one posting with null amount allowed per transaction",
		));
	}
	Ok(transaction)
}

fn empty_posts_count(transaction: &Transaction) -> usize {
	transaction
		.postings
		.iter()
		.filter(|posting| posting.unbalanced_amount.is_none())
		.count()
}

fn disallow_multiple_commodites_with_empty_post(
	transaction: Transaction,
) -> Result<Transaction, Error> {
	if has_empty_posts(&transaction) {
		let commodities = transaction
			.postings
			.iter()
			.filter_map(|posting| match &posting.costs {
				None => posting
					.unbalanced_amount
					.as_ref()
					.map(|a| a.commodity.to_owned()),
				Some(c) => match c {
					Costs::PerUnit(c) => Some(c.commodity.to_owned()),
					Costs::Total(c) => Some(c.commodity.to_owned()),
				},
			})
			.collect::<BTreeSet<String>>()
			.into_iter()
			.collect::<Vec<String>>();
		if commodities.len() > 1 {
			return Err(from_unbalanced_transaction(
				transaction,
				&format!(
					"Multiple commodities in transaction with a null amount posting not allowed.\nCommodities found : {}",
					commodities.join(", ")
				),
			));
		}
	}
	Ok(transaction)
}

fn has_empty_posts(transaction: &Transaction) -> bool {
	transaction
		.postings
		.iter()
		.any(|posting| posting.unbalanced_amount.is_none())
}

fn balance_empty_posts(mut transaction: Transaction) -> Result<Transaction, Error> {
	let total_commodities = transaction
		.postings
		.iter()
		.filter_map(|posting| {
			posting
				.unbalanced_amount
				.as_ref()
				.map(|unbalanced_amount| match posting.costs.as_ref() {
					None => unbalanced_amount.commodity.to_owned(),
					Some(costs) => match costs {
						Costs::PerUnit(costs) => costs.commodity.to_owned(),
						Costs::Total(costs) => costs.commodity.to_owned(),
					},
				})
		})
		.collect::<BTreeSet<String>>();

	let transaction_total_amount = transaction
		.postings
		.iter()
		.filter(|posting| !posting.virtual_posting)
		.flat_map(|posting| {
			posting
				.unbalanced_amount
				.as_ref()
				.map(|unbalanced_amount| match posting.costs.as_ref() {
					None => unbalanced_amount.value,
					Some(costs) => match costs {
						Costs::PerUnit(costs) => unbalanced_amount.value * costs.value,
						Costs::Total(costs) => costs.value,
					},
				})
		})
		.fold(Rational::from_integer(0), |total, val| total + val);

	let mut balanced_postings = Vec::new();
	let mut handled_costs = false;

	for unbalanced_posting in transaction.postings {
		match unbalanced_posting.costs {
			None => match unbalanced_posting.unbalanced_amount {
				None => {
					balanced_postings.push(Posting {
						line: unbalanced_posting.line,
						account: unbalanced_posting.account,
						comments: unbalanced_posting.comments,
						balance_assertion: unbalanced_posting.balance_assertion,
						costs: unbalanced_posting.costs,
						virtual_posting: unbalanced_posting.virtual_posting,
						unbalanced_amount: match &unbalanced_posting.unbalanced_amount {
							None => None,
							Some(unbalanced_amount) => Some(MixedAmount {
								commodity: unbalanced_amount.commodity.to_owned(),
								value: unbalanced_amount.value,
							}),
						},
						balanced_amount: Some(MixedAmount {
							commodity: total_commodities
								.iter()
								.nth(0)
								.expect("commodity expected")
								.to_owned(),
							value: transaction_total_amount.neg(),
						}),
					});
					if handled_costs {
						balanced_postings.push(Posting {
							line: unbalanced_posting.line,
							account: "equity".to_owned(),
							comments: Vec::new(),
							balance_assertion: None,
							costs: None,
							virtual_posting: false,
							unbalanced_amount: match &unbalanced_posting.unbalanced_amount {
								None => None,
								Some(unbalanced_amount) => Some(MixedAmount {
									commodity: unbalanced_amount.commodity.to_owned(),
									value: unbalanced_amount.value,
								}),
							},
							balanced_amount: Some(MixedAmount {
								commodity: total_commodities
									.iter()
									.nth(0)
									.expect("commodity expected")
									.to_owned(),
								value: transaction_total_amount,
							}),
						});
						handled_costs = false;
					}
				}
				Some(unbalanced_amount) => {
					let equity_posting = Posting {
						line: unbalanced_posting.line,
						account: "equity".to_owned(),
						comments: Vec::new(),
						balance_assertion: None,
						costs: None,
						virtual_posting: false,
						unbalanced_amount: Some(MixedAmount {
							commodity: unbalanced_amount.commodity.to_owned(),
							value: unbalanced_amount.value,
						}),
						balanced_amount: Some(MixedAmount {
							commodity: unbalanced_amount.commodity.to_owned(),
							value: unbalanced_amount.value.neg(),
						}),
					};
					let balanced_posting = Posting {
						line: unbalanced_posting.line,
						account: unbalanced_posting.account,
						comments: unbalanced_posting.comments,
						balance_assertion: unbalanced_posting.balance_assertion,
						costs: unbalanced_posting.costs,
						virtual_posting: unbalanced_posting.virtual_posting,
						unbalanced_amount: Some(MixedAmount {
							commodity: unbalanced_amount.commodity.to_owned(),
							value: unbalanced_amount.value,
						}),
						balanced_amount: Some(unbalanced_amount),
					};
					balanced_postings.push(balanced_posting);
					if handled_costs {
						balanced_postings.push(equity_posting);
						handled_costs = false;
					}
				}
			},
			Some(costs) => {
				let unbalanced_amount = unbalanced_posting
					.unbalanced_amount
					.expect("costs have to have an amount");
				let equity_posting = Posting {
					line: unbalanced_posting.line,
					account: "equity".to_owned(),
					comments: Vec::new(),
					balance_assertion: None,
					costs: None,
					virtual_posting: false,
					unbalanced_amount: Some(MixedAmount {
						commodity: unbalanced_amount.commodity.to_owned(),
						value: unbalanced_amount.value,
					}),
					balanced_amount: Some(MixedAmount {
						commodity: unbalanced_amount.commodity.to_owned(),
						value: unbalanced_amount.value.neg(),
					}),
				};
				match costs {
					Costs::PerUnit(costs_per_unit) => {
						balanced_postings.push(Posting {
							line: unbalanced_posting.line,
							account: unbalanced_posting.account,
							comments: unbalanced_posting.comments,
							balance_assertion: unbalanced_posting.balance_assertion,
							costs: Some(Costs::PerUnit(costs_per_unit)),
							virtual_posting: unbalanced_posting.virtual_posting,
							unbalanced_amount: Some(MixedAmount {
								commodity: unbalanced_amount.commodity.to_owned(),
								value: unbalanced_amount.value,
							}),
							balanced_amount: Some(unbalanced_amount),
						});
					}
					Costs::Total(costs_total) => {
						balanced_postings.push(Posting {
							line: unbalanced_posting.line,
							account: unbalanced_posting.account,
							comments: unbalanced_posting.comments,
							balance_assertion: unbalanced_posting.balance_assertion,
							costs: Some(Costs::Total(costs_total)),
							virtual_posting: unbalanced_posting.virtual_posting,
							unbalanced_amount: Some(MixedAmount {
								commodity: unbalanced_amount.commodity.to_owned(),
								value: unbalanced_amount.value,
							}),
							balanced_amount: Some(unbalanced_amount),
						});
					}
				}
				balanced_postings.push(equity_posting);
				handled_costs = true;
			}
		}
	}

	// println!("{:?}", total_commodities);
	// println!("{:?}", transaction_total_amount);
	// for post in &balanced_postings {
	// 	println!("{:?}\n", post);
	// }

	let balanced_transaction = Transaction {
		line: transaction.line,
		date: transaction.date,
		state: transaction.state,
		code: transaction.code,
		description: transaction.description,
		comments: transaction.comments,
		postings: balanced_postings,
	};

	Ok(balanced_transaction)
}

fn disallow_unbalanced_postings(transaction: Transaction) -> Result<Transaction, Error> {
	// println!("commodity : {}", commodity);
	// println!("transaction_total_amount : {}", transaction_total_amount);

	// for posting in transaction.postings{

	// }

	let total = transaction
		.postings
		.iter()
		.filter(|posting| !posting.virtual_posting)
		.fold(BTreeMap::<String, Rational>::new(), |mut total, posting| {
			total
				.entry(
					posting
						.balanced_amount
						.as_ref()
						.expect("balanced amount not found")
						.commodity
						.to_owned(),
				)
				.and_modify(|a| {
					*a += posting
						.balanced_amount
						.as_ref()
						.expect("balanced amount not found")
						.value
				})
				.or_insert(
					posting
						.balanced_amount
						.as_ref()
						.expect("balanced amount not found")
						.value,
				);
			total
		});

	if !total.iter().all(|(_, a)| a.is_zero()) {
		return Err(from_balanced_transaction(
			transaction,
			"Transaction does not balance",
		));
	}
	Ok(transaction)
}

fn from_unbalanced_transaction(transaction: Transaction, message: &str) -> Error {
	let range_start = transaction.line - 1;
	let range_end = transaction
		.postings
		.iter()
		.map(|p| p.line - 1)
		.max()
		.unwrap_or(0)
		+ 1;
	Error::BalanceError {
		range_start,
		range_end,
		message: message.to_owned(),
	}
}

fn from_balanced_transaction(transaction: Transaction, message: &str) -> Error {
	let range_start = transaction.line - 1;
	let range_end = transaction
		.postings
		.iter()
		.map(|p| p.line - 1)
		.max()
		.unwrap_or(0)
		+ 1;
	Error::BalanceError {
		range_start,
		range_end,
		message: message.to_owned(),
	}
}
