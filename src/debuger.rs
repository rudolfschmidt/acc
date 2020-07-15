use super::model::BalancedPosting;
use super::model::Token;
use super::model::Transaction;
use super::model::UnbalancedPosting;

pub fn print_tokens(tokens: &[Token]) {
	for token in tokens {
		match token {
			Token::TransactionDate(value, _line) => println!("TransactionDate({:?})", value),
			Token::TransactionState(value, _line) => println!("TransactionState({:?})", value),
			Token::TransactionDescription(value, _line) => {
				println!("TransactionDescription({:?})", value)
			}
			Token::TransactionComment(value, _line) => println!("TransactionComment({:?})", value),
			Token::PostingAccount(value, _line) => println!("PostingAccount({:?})", value),
			Token::PostingCommodity(value, _line) => println!("PostingCommodity({:?})", value),
			Token::PostingAmount(value, _line) => println!("PostingAmount({:?})", value),
		}
	}
}

pub fn print_unbalanced_transactions(transactions: &[Transaction<UnbalancedPosting>]) {
	transactions.iter().for_each(|t| println!("{:?}", t));
}

pub fn print_balanced_transactions(transactions: &[Transaction<BalancedPosting>]) {
	transactions.iter().for_each(|t| println!("{:?}", t));
}
