extern crate num;

use super::super::model::Costs;
use super::super::model::Item;
use super::super::model::MixedAmount;
use super::super::model::Posting;
use super::Error;

use num::Zero;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::ops::Neg;

type Rational = num::rational::Rational64;

pub fn balance(item: Item) -> Result<Item, Error> {
	match item {
		Item::Transaction { line, postings, .. }
			if postings.iter().filter(is_empty_post).count() > 1 =>
		{
			return Err(build_error(
				line,
				postings,
				"Only one posting with null amount allowed per transaction",
			));
		}
		Item::Transaction {
			line,
			date,
			state,
			code,
			description,
			comments,
			postings,
		} if postings.iter().filter(is_empty_post).next().is_some() => Ok(Item::Transaction {
			line,
			date,
			state,
			code,
			description,
			comments,
			postings: balance_empty_posts(line, postings)?,
		}),
		Item::Transaction {
			line,
			date,
			state,
			code,
			description,
			comments,
			postings,
		} if postings.iter().filter(is_empty_post).next().is_none() => Ok(Item::Transaction {
			line,
			date,
			state,
			code,
			description,
			comments,
			postings: balance_non_empty_posts(line, postings)?,
		}),
		_ => Ok(item),
	}
}

fn is_empty_post(posting: &&Posting) -> bool {
	match posting {
		Posting::UnbalancedPosting {
			unbalanced_amount, ..
		} => unbalanced_amount.is_none(),
		_ => false,
	}
}

fn balance_empty_posts(line: usize, postings: Vec<Posting>) -> Result<Vec<Posting>, Error> {
	let mut total_amount = BTreeMap::new();
	for posting in &postings {
		match posting {
			Posting::UnbalancedPosting {
				unbalanced_amount,
				costs,
				..
			} => match unbalanced_amount {
				None => {}
				Some(unbalanced_amount) => match costs {
					None => {
						total_amount
							.entry(unbalanced_amount.commodity.to_owned())
							.and_modify(|a| *a += unbalanced_amount.value)
							.or_insert(unbalanced_amount.value);
					}
					Some(c) => match c {
						Costs::PerUnit(c) => {
							total_amount
								.entry(c.commodity.to_owned())
								.and_modify(|a| *a += unbalanced_amount.value * c.value)
								.or_insert(unbalanced_amount.value * c.value);
						}
						Costs::Total(c) => {
							total_amount
								.entry(c.commodity.to_owned())
								.and_modify(|a| *a += c.value)
								.or_insert(c.value);
						}
					},
				},
			},
			_ => {}
		}
	}

	if total_amount.len() > 1 {
		let commodities_line = format!(
			"Found commodities: {}",
			total_amount
				.keys()
				.cloned()
				.collect::<Vec<String>>()
				.join(", ")
		);
		let mut message = String::new();
		message.push_str("Multiple commodities in transaction with a null amount posting not allowed.");
		message.push('\n');
		message.push_str(&commodities_line);
		return Err(build_error(line, postings, &message));
	}

	let mut balanced_postings = Vec::new();

	let has_cost_postings = postings.iter().any(|posting| match posting {
		Posting::UnbalancedPosting { costs, .. } => costs.is_some(),
		_ => false,
	});

	for posting in postings {
		match posting {
			Posting::UnbalancedPosting {
				line,
				account,
				comments,
				unbalanced_amount,
				balance_assertion,
				costs,
			} => match unbalanced_amount {
				None => {
					let balanced_amount = total_amount
						.iter()
						.map(|(commodity, value)| MixedAmount {
							commodity: commodity.to_owned(),
							value: value.neg(),
						})
						.next()
						.unwrap();
					let equity_amount = MixedAmount {
						commodity: balanced_amount.commodity.to_owned(),
						value: balanced_amount.value.neg(),
					};
					let balanced_posting = Posting::BalancedPosting {
						line,
						account,
						comments,
						balanced_amount,
						balance_assertion,
						costs,
					};
					balanced_postings.push(balanced_posting);
					if has_cost_postings {
						let equity_posting = Posting::EquityPosting {
							account: "equity".to_owned(),
							amount: equity_amount,
						};
						balanced_postings.push(equity_posting);
					}
				}
				Some(unbalanced_amount) => match costs {
					None => {
						let balanced_posting = Posting::BalancedPosting {
							line,
							account,
							comments,
							balanced_amount: unbalanced_amount,
							balance_assertion,
							costs,
						};
						balanced_postings.push(balanced_posting);
					}
					Some(_) => {
						let equity_amount = MixedAmount {
							commodity: unbalanced_amount.commodity.to_owned(),
							value: unbalanced_amount.value.neg(),
						};
						let balanced_posting = Posting::BalancedPosting {
							line,
							account,
							comments,
							balanced_amount: unbalanced_amount,
							balance_assertion,
							costs,
						};
						balanced_postings.push(balanced_posting);
						let equity_posting = Posting::EquityPosting {
							account: "equity".to_owned(),
							amount: equity_amount,
						};
						balanced_postings.push(equity_posting);
					}
				},
			},
			_ => {}
		}
	}

	Ok(balanced_postings)
}

fn total_commodities(postings: &[Posting]) -> usize {
	postings
		.iter()
		.filter_map(|posting| match posting {
			Posting::UnbalancedPosting {
				unbalanced_amount, ..
			} => unbalanced_amount.as_ref(),
			_ => None,
		})
		.map(|amount| amount.commodity.as_str())
		.collect::<BTreeSet<&str>>()
		.len()
}

fn balance_non_empty_posts(line: usize, postings: Vec<Posting>) -> Result<Vec<Posting>, Error> {
	let mut balanced_postings = Vec::new();
	if total_commodities(&postings) > 1 {
		for posting in postings {
			match posting {
				Posting::UnbalancedPosting {
					line,
					account,
					comments,
					unbalanced_amount,
					balance_assertion,
					costs,
				} => match unbalanced_amount {
					None => {}
					Some(unbalanced_amount) => {
						let balanced_amount = MixedAmount {
							commodity: unbalanced_amount.commodity.to_owned(),
							value: unbalanced_amount.value,
						};
						let balanced_posting = Posting::BalancedPosting {
							line,
							account,
							comments,
							balance_assertion,
							costs,
							balanced_amount,
						};
						balanced_postings.push(balanced_posting);
						let equity_amount = MixedAmount {
							commodity: unbalanced_amount.commodity.to_owned(),
							value: unbalanced_amount.value.neg(),
						};
						let equity_posting = Posting::EquityPosting {
							account: "equity".to_owned(),
							amount: equity_amount,
						};
						balanced_postings.push(equity_posting);
					}
				},
				Posting::BalancedPosting { .. } => {}
				Posting::EquityPosting { .. } => {}
			}
		}
	} else {
		for posting in postings {
			match posting {
				Posting::UnbalancedPosting {
					line,
					account,
					comments,
					balance_assertion,
					costs,
					unbalanced_amount,
				} => {
					let unbalanced_amount_expected = unbalanced_amount.expect("non null amount");
					let balanced_posting = Posting::BalancedPosting {
						line,
						account,
						comments,
						balance_assertion,
						costs,
						balanced_amount: MixedAmount {
							commodity: unbalanced_amount_expected.commodity.to_owned(),
							value: unbalanced_amount_expected.value,
						},
					};
					balanced_postings.push(balanced_posting);
				}
				_ => {}
			}
		}
	}
	balanced_postings = disallow_unbalanced_amounts(line, balanced_postings)?;
	balanced_postings = disallow_unbalanced_costs(line, balanced_postings)?;
	Ok(balanced_postings)
}

fn disallow_unbalanced_costs(line: usize, postings: Vec<Posting>) -> Result<Vec<Posting>, Error> {
	if !postings.iter().any(|posting| match posting {
		Posting::BalancedPosting { costs, .. } => costs.is_some(),
		_ => false,
	}) {
		return Ok(postings);
	}
	let total = postings
		.iter()
		.fold(BTreeMap::<String, Rational>::new(), |mut total, posting| {
			match posting {
				Posting::BalancedPosting {
					balanced_amount,
					costs,
					..
				} => match costs {
					None => {
						total
							.entry(balanced_amount.commodity.to_owned())
							.and_modify(|a| *a += balanced_amount.value)
							.or_insert(balanced_amount.value);
					}
					Some(c) => match c {
						Costs::PerUnit(c) => {
							total
								.entry(c.commodity.to_owned())
								.and_modify(|a| *a += balanced_amount.value * c.value)
								.or_insert(balanced_amount.value * c.value);
						}
						Costs::Total(c) => {
							total
								.entry(c.commodity.to_owned())
								.and_modify(|a| *a += c.value)
								.or_insert(c.value);
						}
					},
				},
				_ => {}
			}
			total
		});

	if !total.iter().all(|(_, a)| a.is_zero()) {
		return Err(build_error(line, postings, "Transaction does not balance"));
	}

	Ok(postings)
}

fn disallow_unbalanced_amounts(line: usize, postings: Vec<Posting>) -> Result<Vec<Posting>, Error> {
	let total = postings
		.iter()
		.fold(BTreeMap::<String, Rational>::new(), |mut total, posting| {
			match posting {
				Posting::BalancedPosting {
					balanced_amount, ..
				} => {
					total
						.entry(balanced_amount.commodity.to_owned())
						.and_modify(|a| *a += balanced_amount.value)
						.or_insert(balanced_amount.value);
				}
				Posting::EquityPosting { amount, .. } => {
					total
						.entry(amount.commodity.to_owned())
						.and_modify(|a| *a += amount.value)
						.or_insert(amount.value);
				}
				_ => {}
			}
			total
		});

	if !total.iter().all(|(_, a)| a.is_zero()) {
		return Err(build_error(line, postings, "Transaction does not balance"));
	}

	Ok(postings)
}

fn build_error(line: usize, postings: Vec<Posting>, message: &str) -> Error {
	let range_start = line - 1;
	let range_end = postings
		.iter()
		.map(|posting| match posting {
			Posting::UnbalancedPosting { line, .. } => line - 1,
			Posting::BalancedPosting { line, .. } => line - 1,
			Posting::EquityPosting { .. } => 0,
		})
		.max()
		.unwrap_or(0)
		+ 1;
	Error::BalanceError {
		range_start,
		range_end,
		message: message.to_owned(),
	}
}
