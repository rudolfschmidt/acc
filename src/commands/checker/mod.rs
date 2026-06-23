//! `validate` command — run a suite of checks over the journal and
//! report which ran, what they scanned, and any issues found.

use colored::Colorize;

use crate::loader::Journal;
use crate::parser::located::Located;
use crate::parser::transaction::Transaction;

pub fn run(journal: &Journal) {
    let txs = &journal.transactions;
    let tx_count = txs.len();
    let posting_count: usize = txs.iter().map(|tx| tx.value.postings.len()).sum();

    let checks: Vec<Check> = vec![
        check_commodity_casing(txs),
        check_leaf_accounts(txs),
    ];

    println!(
        "{}",
        format!("Scanned {} transactions, {} postings.\n", tx_count, posting_count).dimmed()
    );

    println!("{}", "Checks:".bold());
    for check in &checks {
        let status = if check.issues.is_empty() {
            "✓".green().to_string()
        } else {
            "✗".red().to_string()
        };
        println!(
            "  {} {} — {}",
            status,
            check.name.cyan(),
            check.description.dimmed()
        );
    }

    let total_issues: usize = checks.iter().map(|c| c.issues.len()).sum();
    if total_issues == 0 {
        println!("\n{}", "No issues found.".green().bold());
        return;
    }

    println!(
        "\n{} issue(s) found:\n",
        total_issues.to_string().red().bold()
    );
    for check in &checks {
        if check.issues.is_empty() {
            continue;
        }
        println!("{}", format!("{}:", check.name).red().bold());
        for issue in &check.issues {
            println!("  {}", issue);
        }
    }
}

struct Check {
    name: &'static str,
    description: &'static str,
    issues: Vec<String>,
}

/// Multi-character commodity symbols must be all uppercase.
/// Single-character symbols (`$`, `€`, `£`) are exempt — most are
/// not ASCII letters anyway and carry no natural case.
fn check_commodity_casing(txs: &[Located<Transaction>]) -> Check {
    let mut issues = Vec::new();
    for tx in txs {
        for lp in &tx.value.postings {
            let p = &lp.value;
            let Some(amount) = &p.amount else { continue };
            let commodity = &amount.commodity;
            if commodity.len() > 1 && commodity.chars().any(|c| c.is_lowercase()) {
                issues.push(format!(
                    "{} commodity '{}' (account: {})",
                    format!("{}:{}", tx.file, tx.line).cyan(),
                    commodity.yellow(),
                    p.account.dimmed(),
                ));
            }
        }
    }
    Check {
        name: "commodity-casing",
        description: "multi-char commodity symbols must be all-uppercase",
        issues,
    }
}

/// Postings should target leaf accounts. An account that has at least
/// one sub-account elsewhere in the journal is a parent; posting
/// directly to it mixes the parent's own amounts with its children's,
/// so its tree total double-counts. Every offending posting is listed.
fn check_leaf_accounts(txs: &[Located<Transaction>]) -> Check {
    use std::collections::{BTreeMap, BTreeSet};

    // Unique posted account names.
    let accounts: BTreeSet<&str> = txs
        .iter()
        .flat_map(|tx| tx.value.postings.iter())
        .map(|lp| lp.value.account.as_str())
        .collect();

    // Map every ancestor path to one concrete posted descendant, so a
    // flagged parent can name an example sub-account.
    // `expenses:food:restaurant` registers `expenses` and
    // `expenses:food`, both pointing at the full account.
    let mut parents: BTreeMap<&str, &str> = BTreeMap::new();
    for &account in &accounts {
        for (idx, _) in account.match_indices(':') {
            parents.entry(&account[..idx]).or_insert(account);
        }
    }

    let mut issues = Vec::new();
    for tx in txs {
        for lp in &tx.value.postings {
            if let Some(&sub) = parents.get(lp.value.account.as_str()) {
                issues.push(format!(
                    "{} '{}' has sub-account '{}'",
                    format!("{}:{}", lp.file, lp.line).cyan(),
                    lp.value.account.yellow(),
                    sub.dimmed(),
                ));
            }
        }
    }

    Check {
        name: "leaf-accounts",
        description: "postings must target leaf accounts (no sub-accounts)",
        issues,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{booker, parser, resolver};

    fn setup(src: &str) -> Vec<Located<Transaction>> {
        let entries = parser::parse(src).unwrap();
        let resolved = resolver::resolve(entries).unwrap();
        booker::book(resolved.transactions).unwrap()
    }

    #[test]
    fn commodity_casing_flags_mixed_case_multichar() {
        let txs = setup(
            "2024-01-01 * x\n\
             \tassets:a   10 Usd\n\
             \tequity:o  -10 Usd\n",
        );
        let check = check_commodity_casing(&txs);
        assert_eq!(check.issues.len(), 2); // both legs use `Usd`
    }

    #[test]
    fn commodity_casing_accepts_uppercase_and_single_char() {
        let txs = setup(
            "2024-01-01 * x\n\
             \tassets:a   10 USD\n\
             \tassets:b   -5 €\n\
             \tequity:o\n",
        );
        assert!(check_commodity_casing(&txs).issues.is_empty());
    }

    #[test]
    fn leaf_accounts_flags_posting_to_a_parent() {
        // `expenses` is posted to directly, but `expenses:food` also
        // exists → `expenses` is a parent, not a leaf.
        let txs = setup(
            "2024-01-01 * x\n\
             \texpenses:food  10 USD\n\
             \tassets:cash   -10 USD\n\
             2024-01-02 * y\n\
             \texpenses        5 USD\n\
             \tassets:cash    -5 USD\n",
        );
        let check = check_leaf_accounts(&txs);
        assert_eq!(check.issues.len(), 1);
        assert!(check.issues[0].contains("expenses"));
    }

    #[test]
    fn leaf_accounts_accepts_all_leaves() {
        let txs = setup(
            "2024-01-01 * x\n\
             \texpenses:food   10 USD\n\
             \tassets:cash    -10 USD\n",
        );
        assert!(check_leaf_accounts(&txs).issues.is_empty());
    }
}
