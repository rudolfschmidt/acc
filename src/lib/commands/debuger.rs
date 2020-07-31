use super::super::model::Token;
use super::super::model::Transaction;

pub fn print_tokens(tokens: &[Token]) {
	for token in tokens {
		match token {
			Token::TransactionDateYear(_line, value) => println!("TransactionDateYear({:?})", value),
			Token::TransactionDateMonth(_line, value) => println!("TransactionDateMonth({:?})", value),
			Token::TransactionDateDay(_line, value) => println!("TransactionDateDay({:?})", value),
			Token::TransactionState(_line, value) => println!("TransactionState({:?})", value),
			Token::TransactionCode(_line, value) => println!("TransactionCode({:?})", value),
			Token::TransactionDescription(_line, value) => {
				println!("TransactionDescription({:?})", value)
			}
			Token::Comment(_line, value) => println!("Comment({:?})", value),
			Token::PostingAccount(_line, value) => println!("PostingAccount({:?})", value),
			Token::PostingCommodity(_line, value) => println!("PostingCommodity({:?})", value),
			Token::PostingAmount(_line, value) => println!("PostingAmount({:?})", value),
			Token::BalanceAssertion(_line) => println!("BalanceAssertion"),
			Token::Alias(_line, value) => println!("Alias({:?})", value),
		}
	}
}

pub fn print_transactions(transactions: &[Transaction]) {
	transactions.iter().for_each(|t| println!("{:?}", t));
}
