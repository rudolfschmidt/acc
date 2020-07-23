use super::model::BalancedPosting;
use super::model::Token;
use super::model::Transaction;
use super::model::UnbalancedPosting;

pub fn print_tokens(tokens: &[Token]) {
	for token in tokens {
		match token {
			Token::TransactionDate(_line, value) => println!("TransactionDate({:?})", value),
			Token::TransactionState(_line, value) => println!("TransactionState({:?})", value),
			Token::TransactionCode(_line, value) => println!("TransactionCode({:?})", value),
			Token::TransactionDescription(_line, value) => {
				println!("TransactionDescription({:?})", value)
			}
			Token::TransactionComment(_line, value) => println!("Comment({:?})", value),
			Token::PostingAccount(_line, value) => println!("PostingAccount({:?})", value),
			Token::PostingCommodity(_line, value) => println!("PostingCommodity({:?})", value),
			Token::PostingAmount(_line, value) => println!("PostingAmount({:?})", value),
			Token::BalanceAssertion(_line) => println!("BalanceAssertion"),
		}
	}
}

pub fn print_unbalanced_transactions(transactions: &[Transaction<UnbalancedPosting>]) {
	transactions.iter().for_each(|t| println!("{:?}", t));
}

pub fn print_balanced_transactions(transactions: &[Transaction<BalancedPosting>]) {
	transactions.iter().for_each(|t| println!("{:?}", t));
}
