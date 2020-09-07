extern crate num;

use super::format_amount;
use super::model::Costs;
use super::model::Item;
use super::model::MixedAmount;
use super::model::Posting;

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::ops::Neg;
use std::path::PathBuf;

type Rational = num::rational::Rational64;

pub fn balance(item: Item) -> Result<Item, String> {
	match balance_item(item) {
		Ok(item) => Ok(item),
		Err(err) => match std::fs::read_to_string(&err.file) {
			Err(message) => Err(format!(
				"While parsing file \"{}\"\n{}",
				err.file.display(),
				message
			)),
			Ok(content) => {
				let mut message = String::new();
				let lines = content.lines().collect::<Vec<&str>>();
				for i in err.range_start..err.range_end {
					message.push_str(&format!("> {} : {}\n", i + 1, lines.get(i).unwrap()));
				}
				message.push_str(&err.message);
				Err(format!(
					"While parsing file \"{}\" at line {}:\n{}",
					err.file.display(),
					err.range_start + 1,
					message
				))
			}
		},
	}
}

struct Error {
	file: PathBuf,
	range_start: usize,
	range_end: usize,
	message: String,
}

fn balance_item(item: Item) -> Result<Item, Error> {
	match item {
		Item::Transaction {
			file,
			line,
			postings,
			..
		} if postings.iter().filter(is_empty_post).count() > 1 => {
			return Err(build_error(
				file,
				line,
				postings,
				String::from("Only one posting with null amount allowed per transaction"),
			));
		}
		Item::Transaction {
			file,
			line,
			date,
			state,
			code,
			description,
			comments,
			postings,
		} if has_empty_unbalanced_posts(&postings) => Ok(Item::Transaction {
			file: file.to_owned(),
			line,
			date,
			state,
			code,
			description,
			comments,
			postings: balance_empty_posts(file, line, postings)?,
		}),
		Item::Transaction {
			file,
			line,
			date,
			state,
			code,
			description,
			comments,
			postings,
		} if has_no_empty_unbalanced_posts(&postings) => Ok(Item::Transaction {
			file: file.to_owned(),
			line,
			date,
			state,
			code,
			description,
			comments,
			postings: balance_non_empty_posts(file, line, postings)?,
		}),
		_ => Ok(item),
	}
}

fn balance_empty_posts(
	file: PathBuf,
	line: usize,
	postings: Vec<Posting>,
) -> Result<Vec<Posting>, Error> {
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
		return Err(build_error(file, line, postings, message));
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
				lot_price,
				costs,
				balance_assertion,
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
						unbalanced_amount,
						balanced_amount,
						lot_price,
						costs,
						balance_assertion,
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
						let balanced_amount = MixedAmount {
							commodity: unbalanced_amount.commodity.to_owned(),
							value: unbalanced_amount.value,
						};
						let balanced_posting = Posting::BalancedPosting {
							line,
							account,
							comments,
							unbalanced_amount: Some(unbalanced_amount),
							balanced_amount,
							lot_price,
							costs,
							balance_assertion,
						};
						balanced_postings.push(balanced_posting);
					}
					Some(_) => {
						let equity_amount = MixedAmount {
							commodity: unbalanced_amount.commodity.to_owned(),
							value: unbalanced_amount.value.neg(),
						};
						let balanced_amount = MixedAmount {
							commodity: unbalanced_amount.commodity.to_owned(),
							value: unbalanced_amount.value,
						};
						let balanced_posting = Posting::BalancedPosting {
							line,
							account,
							comments,
							unbalanced_amount: Some(unbalanced_amount),
							balanced_amount,
							lot_price,
							costs,
							balance_assertion,
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

fn balance_non_empty_posts(
	file: PathBuf,
	line: usize,
	postings: Vec<Posting>,
) -> Result<Vec<Posting>, Error> {
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
					lot_price,
					costs,
				} => match unbalanced_amount {
					None => {}
					Some(unbalanced_amount) => {
						let balanced_amount = MixedAmount {
							commodity: unbalanced_amount.commodity.to_owned(),
							value: unbalanced_amount.value,
						};
						let equity_amount = MixedAmount {
							commodity: unbalanced_amount.commodity.to_owned(),
							value: unbalanced_amount.value.neg(),
						};
						let balanced_posting = Posting::BalancedPosting {
							line,
							account,
							comments,
							unbalanced_amount: Some(unbalanced_amount),
							balanced_amount,
							lot_price,
							costs,
							balance_assertion,
						};
						let equity_posting = Posting::EquityPosting {
							account: "equity".to_owned(),
							amount: equity_amount,
						};
						balanced_postings.push(balanced_posting);
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
					unbalanced_amount,
					lot_price,
					costs,
					balance_assertion,
				} => {
					if let Some(unbalanced_amount) = unbalanced_amount {
						let balanced_amount = MixedAmount {
							commodity: unbalanced_amount.commodity.to_owned(),
							value: unbalanced_amount.value,
						};
						let balanced_posting = Posting::BalancedPosting {
							line,
							account,
							comments,
							unbalanced_amount: Some(unbalanced_amount),
							balanced_amount,
							lot_price,
							costs,
							balance_assertion,
						};
						balanced_postings.push(balanced_posting);
					}
				}
				_ => {}
			}
		}
	}

	balanced_postings = disallow_unbalanced_amounts(file.to_owned(), line, balanced_postings)?;
	balanced_postings = disallow_unbalanced_costs(file.to_owned(), line, balanced_postings)?;
	Ok(balanced_postings)
}

fn disallow_unbalanced_amounts(
	file: PathBuf,
	line: usize,
	postings: Vec<Posting>,
) -> Result<Vec<Posting>, Error> {
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

	for (commodity, amount) in total {
		if num::abs(amount) >= Rational::new(1, 100) {
			return Err(build_error(
				file,
				line,
				postings,
				format!(
					"Transaction is not balanced. Unbalanced amount : {}{}",
					commodity,
					format_amount(&amount)
				),
			));
		}
	}

	Ok(postings)
}

fn disallow_unbalanced_costs(
	file: PathBuf,
	line: usize,
	postings: Vec<Posting>,
) -> Result<Vec<Posting>, Error> {
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
					lot_price,
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
							match lot_price {
								None => {}
								Some(lot_price) => {
									let value = balanced_amount.value * c.value;
									let value = value - balanced_amount.value * lot_price.value;
									let value = value.neg();
									total
										.entry(lot_price.commodity.to_owned())
										.and_modify(|a| *a += value)
										.or_insert(value);
								}
							}
						}
						Costs::Total(c) => {
							total
								.entry(c.commodity.to_owned())
								.and_modify(|a| *a += c.value)
								.or_insert(c.value);
							match lot_price {
								None => {}
								Some(lot_price) => {
									let value = balanced_amount.value * c.value;
									let value = value - balanced_amount.value * lot_price.value;
									let value = value.neg();
									total
										.entry(lot_price.commodity.to_owned())
										.and_modify(|a| *a += value)
										.or_insert(value);
								}
							}
						}
					},
				},
				_ => {}
			}
			total
		});

	for (commodity, amount) in total {
		if num::abs(amount) >= Rational::new(1, 100) {
			return Err(build_error(
				file,
				line,
				postings,
				format!(
					"Transaction not balanced.\nUnbalanced amount: {}{} ({}{})",
					commodity,
					format_amount(&amount),
					commodity,
					amount
				),
			));
		}
	}

	Ok(postings)
}

fn build_error(file: PathBuf, line: usize, postings: Vec<Posting>, message: String) -> Error {
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
	Error {
		file,
		range_start,
		range_end,
		message,
	}
}

fn has_no_empty_unbalanced_posts(postings: &[Posting]) -> bool {
	!has_empty_unbalanced_posts(postings)
}

fn has_empty_unbalanced_posts(postings: &[Posting]) -> bool {
	postings.iter().filter(is_empty_post).next().is_some()
}

fn is_empty_post(posting: &&Posting) -> bool {
	match posting {
		Posting::UnbalancedPosting {
			unbalanced_amount, ..
		} => unbalanced_amount.is_none(),
		_ => false,
	}
}
