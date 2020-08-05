mod flat;
mod tree;

use super::super::model::Transaction;

pub fn print_flat(transactions: Vec<Transaction>) -> Result<(), String> {
	flat::print(transactions)
}

pub fn print_tree(transactions: Vec<Transaction>) -> Result<(), String> {
	tree::print(transactions)
}
