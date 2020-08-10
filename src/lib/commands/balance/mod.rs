mod common;
mod flat;
mod tree;

use super::super::model::Item;

pub fn print_flat(items: Vec<Item>) -> Result<(), String> {
	flat::print(items)
}

pub fn print_tree(items: Vec<Item>) -> Result<(), String> {
	tree::print(items)
}
