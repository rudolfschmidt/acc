pub mod accounts;
pub mod balance;
pub mod codes;
pub mod debug;
pub mod print;
pub mod register;

fn format_amount(amount: &num::rational::Rational64) -> String {
	let n = *amount.numer() as f64;
	let d = *amount.denom() as f64;
	format!("{:.2}", n / d)
}
