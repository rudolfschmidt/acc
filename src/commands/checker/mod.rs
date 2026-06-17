//! `validate` command — run a suite of checks over the journal and
//! report which ran, what they scanned, and any issues found.

use colored::Colorize;

use crate::loader::Journal;

pub fn run(journal: &Journal) {
    let tx_count = journal.transactions.len();
    let posting_count: usize = journal
        .transactions
        .iter()
        .map(|tx| tx.value.postings.len())
        .sum();

    let checks: Vec<Check> = vec![
        check_commodity_casing(journal),
        check_leaf_accounts(journal),
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
fn check_commodity_casing(journal: &Journal) -> Check {
    let mut issues = Vec::new();
    for tx in &journal.transactions {
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
fn check_leaf_accounts(journal: &Journal) -> Check {
    use std::collections::{BTreeMap, BTreeSet};

    // Unique posted account names.
    let accounts: BTreeSet<&str> = journal
        .transactions
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
    for tx in &journal.transactions {
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
