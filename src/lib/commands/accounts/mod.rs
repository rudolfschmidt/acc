mod flat;
mod tree;

use super::super::model::Transaction;

pub(crate) fn print_flat(transactions: &[Transaction]) -> Result<(), String> {
	flat::print(transactions)
}

pub(crate) fn print_tree(transactions: &[Transaction]) -> Result<(), String> {
	tree::print(transactions)
}
