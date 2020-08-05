extern crate num;

use super::super::model::BalancedPosting;
use super::super::model::MixedAmount;
use super::super::model::Transaction;

use num::Zero;
use std::collections::BTreeMap;
use std::ops::Neg;

pub struct Error {
	pub transaction: Transaction,
	pub message: String,
}

pub fn balance(transaction: Transaction) -> Result<Transaction, Error> {
	let transaction = disallow_multiple_empty_posts(transaction)?;
	let transaction = disallow_multiple_commodites_with_empty_posts(transaction)?;
	let transaction = balance_empty_posts(transaction)?;
	let transaction = disallow_unbalanced_postings(transaction)?;
	Ok(transaction)
}

fn disallow_multiple_empty_posts(transaction: Transaction) -> Result<Transaction, Error> {
	let mut balanced_previous_posting = false;
	for _ in transaction
		.unbalanced_postings
		.iter()
		.filter(|p| p.unbalanced_amount.is_none())
	{
		if balanced_previous_posting {
			return Err(Error {
				transaction: transaction,
				message: String::from("Only one posting with null amount allowed per transaction"),
			});
		}
		balanced_previous_posting = true;
	}
	Ok(transaction)
}

fn disallow_multiple_commodites_with_empty_posts(
	transaction: Transaction,
) -> Result<Transaction, Error> {
	if transaction
		.unbalanced_postings
		.iter()
		.find(|p| p.unbalanced_amount.is_none())
		.is_none()
	{
		return Ok(transaction);
	}
	let mut prev_commodity = None;
	for unbalanced_posting in &transaction.unbalanced_postings {
		if let Some(ma) = &unbalanced_posting.unbalanced_amount {
			if let Some(prev) = prev_commodity {
				if prev != ma.commodity {
					return Err(Error {
						transaction: transaction,
						message: String::from(
							"Multiple commodities in transaction with a null amount posting not allowed",
						),
					});
				}
			}
			prev_commodity = Some(ma.commodity.to_owned());
		}
	}
	Ok(transaction)
}

fn balance_empty_posts(mut transaction: Transaction) -> Result<Transaction, Error> {
	if let Some(commodity) = transaction
		.unbalanced_postings
		.iter()
		.filter(|posting| !posting.virtual_posting)
		.flat_map(|posting| posting.unbalanced_amount.as_ref())
		.map(|unbalanced_amount| unbalanced_amount.commodity.to_owned())
		.next()
	{
		let transaction_total_amount = transaction
			.unbalanced_postings
			.iter()
			.filter(|posting| !posting.virtual_posting)
			.flat_map(|posting| posting.unbalanced_amount.as_ref())
			.map(|unbalanced_amount| unbalanced_amount.value)
			.fold(num::rational::Rational64::from_integer(0), |acc, val| {
				acc + val
			});

		while let Some(unbalanced_posting) = transaction.unbalanced_postings.pop() {
			if unbalanced_posting.virtual_posting {
				transaction.balanced_postings.push(BalancedPosting {
					balanced_amount: match &unbalanced_posting.unbalanced_amount {
						None => {
							return Err(Error {
								transaction: transaction,
								message: String::from("null amount virtual postings not allowed"),
							})
						}
						Some(unbalanced_amount) => unbalanced_amount.clone(),
					},
					unbalanced_posting: unbalanced_posting,
				})
			} else {
				transaction.balanced_postings.push(BalancedPosting {
					balanced_amount: match &unbalanced_posting.unbalanced_amount {
						None => MixedAmount {
							commodity: commodity.to_owned(),
							value: transaction_total_amount.neg(),
						},
						Some(unbalanced_amount) => MixedAmount {
							commodity: unbalanced_amount.commodity.to_owned(),
							value: unbalanced_amount.value,
						},
					},
					unbalanced_posting: unbalanced_posting,
				})
			}
		}
	}
	Ok(transaction)
}

fn disallow_unbalanced_postings(transaction: Transaction) -> Result<Transaction, Error> {
	let total = transaction.balanced_postings.iter().fold(
		BTreeMap::<String, num::rational::Rational64>::new(),
		|mut total, posting| {
			total
				.entry(posting.balanced_amount.commodity.to_owned())
				.and_modify(|a| *a += posting.balanced_amount.value)
				.or_insert(posting.balanced_amount.value);
			total
		},
	);

	if !total.iter().all(|(_, a)| a.is_zero()) {
		return Err(Error {
			transaction: transaction,
			message: String::from("Transaction does not balance"),
		});
	}
	Ok(transaction)
}
