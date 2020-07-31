use super::super::super::model::Transaction;
use super::common;

pub(super) fn print(transactions: &[Transaction]) -> Result<(), String> {
	if transactions.iter().any(|t| t.postings.is_empty()) {
		return Ok(());
	}
	let require_amount = false;
	common::print(transactions, require_amount, |p| {
		p.unbalanced_amount.as_ref()
	})
}