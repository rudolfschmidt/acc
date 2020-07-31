mod common;
mod explicit;
mod raw;

use super::super::model::Transaction;

pub fn print_explicit(transactions: &[Transaction]) -> Result<(), String> {
	explicit::print(transactions)
}

pub fn print_raw(transactions: &[Transaction]) -> Result<(), String> {
	raw::print(transactions)
}
