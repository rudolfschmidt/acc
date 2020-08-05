mod flat;
mod tree;

use super::super::model::BalancedPosting;
use super::super::model::Transaction;

pub fn print_flat(transactions: Vec<Transaction<BalancedPosting>>) -> Result<(), String> {
	flat::print(transactions)
}

pub fn print_tree(transactions: Vec<Transaction<BalancedPosting>>) -> Result<(), String> {
	tree::print(transactions)
}
