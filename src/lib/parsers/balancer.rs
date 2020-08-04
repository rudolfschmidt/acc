extern crate num;

use super::super::model::MixedAmount;
use super::super::model::Transaction;
use super::Error;

use num::Zero;
use std::collections::BTreeMap;
use std::ops::Neg;

pub fn balance(transaction: &mut Transaction) -> Result<(), Error> {
	disallow_multiple_empty_posts(transaction)?;
	disallow_multiple_commodites_with_empty_posts(transaction)?;
	balance_empty_posts(transaction);
	disallow_unbalanced_transaction(transaction)?;
	Ok(())
}

fn disallow_multiple_empty_posts(transaction: &Transaction) -> Result<(), Error> {
	let mut balanced_previous_posting = false;
	for _ in transaction
		.postings
		.iter()
		.filter(|p| p.unbalanced_amount.is_none())
	{
		if balanced_previous_posting {
			return Err(Error {
				line: transaction.line,
				message: String::from("Only one posting with null amount allowed per transaction"),
			});
		}
		balanced_previous_posting = true;
	}
	Ok(())
}

fn disallow_multiple_commodites_with_empty_posts(transaction: &Transaction) -> Result<(), Error> {
	if transaction
		.postings
		.iter()
		.find(|p| p.unbalanced_amount.is_none())
		.is_none()
	{
		return Ok(());
	}
	let mut prev_commodity = None;
	for posting in &transaction.postings {
		if let Some(ma) = &posting.unbalanced_amount {
			if let Some(prev) = prev_commodity {
				if prev != ma.commodity {
					return Err(Error {
						line: transaction.line,
						message: String::from(
							"Multiple commodities in transaction with a null amount posting not allowed",
						),
					});
				}
			}
			prev_commodity = Some(ma.commodity.to_owned());
		}
	}
	Ok(())
}

fn balance_empty_posts(transaction: &mut Transaction) {
	if let Some(commodity) = transaction
		.postings
		.iter()
		.filter(|p| !p.virtual_posting)
		.flat_map(|p| p.unbalanced_amount.as_ref())
		.map(|a| a.commodity.to_owned())
		.next()
	{
		let transaction_total_amount = transaction
			.postings
			.iter()
			.filter(|p| !p.virtual_posting)
			.flat_map(|p| p.unbalanced_amount.as_ref())
			.map(|ma| ma.amount)
			.fold(num::rational::Rational64::from_integer(0), |acc, val| {
				acc + val
			});

		for posting in transaction
			.postings
			.iter_mut()
			.filter(|p| !p.virtual_posting)
		{
			posting.balanced_amount = match &posting.unbalanced_amount {
				None => Some(MixedAmount {
					commodity: commodity.to_owned(),
					amount: transaction_total_amount.neg(),
				}),
				Some(unbalanced_amount) => Some(MixedAmount {
					commodity: unbalanced_amount.commodity.to_owned(),
					amount: unbalanced_amount.amount,
				}),
			}
		}
	}
}

fn disallow_unbalanced_transaction(transaction: &Transaction) -> Result<(), Error> {
	let total = transaction
		.postings
		.iter()
		.filter(|p| !p.virtual_posting)
		.fold(
			BTreeMap::<String, num::rational::Rational64>::new(),
			|mut total, posting| {
				total
					.entry(
						posting
							.balanced_amount
							.as_ref()
							.expect("null commodity not allowed")
							.commodity
							.to_owned(),
					)
					.and_modify(|a| {
						*a += posting
							.balanced_amount
							.as_ref()
							.expect("null amount not allowed")
							.amount
					})
					.or_insert(
						posting
							.balanced_amount
							.as_ref()
							.expect("null amount not allowed")
							.amount,
					);
				total
			},
		);

	if !total.iter().all(|(_, a)| a.is_zero()) {
		return Err(Error {
			line: transaction.line,
			message: String::from("Transaction does not balance"),
		});
	}

	Ok(())
}
