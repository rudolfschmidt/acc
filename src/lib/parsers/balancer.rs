extern crate num;

use super::super::model::BalancedPosting;
use super::super::model::MixedAmount;
use super::super::model::Transaction;
use super::super::model::UnbalancedPosting;
use super::Error;

use num::Zero;
use std::collections::BTreeMap;
use std::ops::Neg;

pub fn balance(
	transaction: Transaction<UnbalancedPosting>,
) -> Result<Transaction<BalancedPosting>, Error> {
	let transaction = disallow_multiple_empty_posts(transaction)?;
	let transaction = disallow_multiple_commodites_with_empty_posts(transaction)?;
	let transaction = balance_empty_posts(transaction)?;
	let transaction = disallow_unbalanced_postings(transaction)?;
	Ok(transaction)
}

fn disallow_multiple_empty_posts(
	transaction: Transaction<UnbalancedPosting>,
) -> Result<Transaction<UnbalancedPosting>, Error> {
	let mut balanced_previous_posting = false;
	for _ in transaction
		.postings
		.iter()
		.filter(|posting| posting.amount.is_none())
	{
		if balanced_previous_posting {
			return Err(unbalanced_error(
				transaction,
				String::from("Only one posting with null amount allowed per transaction"),
			));
		}
		balanced_previous_posting = true;
	}
	Ok(transaction)
}

fn disallow_multiple_commodites_with_empty_posts(
	transaction: Transaction<UnbalancedPosting>,
) -> Result<Transaction<UnbalancedPosting>, Error> {
	if transaction
		.postings
		.iter()
		.find(|p| p.amount.is_none())
		.is_none()
	{
		return Ok(transaction);
	}
	let mut prev_commodity = None;
	for unbalanced_posting in &transaction.postings {
		if let Some(amount) = &unbalanced_posting.amount {
			if let Some(prev) = prev_commodity {
				if prev != amount.commodity {
					return Err(unbalanced_error(
						transaction,
						String::from(
							"Multiple commodities in transaction with a null amount posting not allowed",
						),
					));
				}
			}
			prev_commodity = Some(amount.commodity.to_owned());
		}
	}
	Ok(transaction)
}

fn balance_empty_posts(
	mut transaction: Transaction<UnbalancedPosting>,
) -> Result<Transaction<BalancedPosting>, Error> {
	match transaction
		.postings
		.iter()
		.filter(|posting| !posting.header.virtual_posting)
		.flat_map(|posting| posting.amount.as_ref())
		.map(|unbalanced_amount| unbalanced_amount.commodity.to_owned())
		.next()
	{
		None => {
			return Err(unbalanced_error(
				transaction,
				String::from("No commodities found"),
			))
		}
		Some(commodity) => {
			let transaction_total_amount = transaction
				.postings
				.iter()
				.filter(|posting| !posting.header.virtual_posting)
				.flat_map(|posting| posting.amount.as_ref())
				.map(|amount| amount.value)
				.fold(num::rational::Rational64::from_integer(0), |total, val| {
					total + val
				});

			let mut balanced_postings = Vec::new();

			while let Some(unbalanced_posting) = transaction.postings.pop() {
				if unbalanced_posting.header.virtual_posting {
					balanced_postings.insert(
						0,
						BalancedPosting {
							head: unbalanced_posting.header,
							empty_posting: unbalanced_posting.amount.is_none(),
							balanced_amount: match unbalanced_posting.amount {
								None => {
									return Err(unbalanced_error(
										transaction,
										String::from("null amount virtual postings not allowed"),
									))
								}
								Some(unbalanced_amount) => unbalanced_amount,
							},
						},
					)
				} else {
					balanced_postings.insert(
						0,
						BalancedPosting {
							head: unbalanced_posting.header,
							empty_posting: unbalanced_posting.amount.is_none(),
							balanced_amount: match unbalanced_posting.amount {
								None => MixedAmount {
									commodity: commodity.to_owned(),
									value: transaction_total_amount.neg(),
								},
								Some(unbalanced_amount) => unbalanced_amount,
							},
						},
					)
				}
			}

			let balanced_transaction = Transaction {
				header: transaction.header,
				postings: balanced_postings,
			};

			Ok(balanced_transaction)
		}
	}
}

fn disallow_unbalanced_postings(
	transaction: Transaction<BalancedPosting>,
) -> Result<Transaction<BalancedPosting>, Error> {
	let total = transaction
		.postings
		.iter()
		.filter(|posting| !posting.head.virtual_posting)
		.fold(
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
		return Err(balanced_error(
			transaction,
			String::from("Transaction does not balance"),
		));
	}
	Ok(transaction)
}

fn unbalanced_error(transaction: Transaction<UnbalancedPosting>, message: String) -> Error {
	let range_start = transaction.header.line - 1;
	let range_end = transaction
		.postings
		.iter()
		.map(|p| p.header.line - 1)
		.max()
		.unwrap_or(0)
		+ 1;
	Error::BalanceError {
		range_start,
		range_end,
		message,
	}
}

fn balanced_error(transaction: Transaction<BalancedPosting>, message: String) -> Error {
	let range_start = transaction.header.line - 1;
	let range_end = transaction
		.postings
		.iter()
		.map(|p| p.head.line - 1)
		.max()
		.unwrap_or(0)
		+ 1;
	Error::BalanceError {
		range_start,
		range_end,
		message,
	}
}
