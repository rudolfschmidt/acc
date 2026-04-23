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

    let checks: Vec<Check> = vec![check_commodity_casing(journal)];

    println!(
        "Scanned {} transactions, {} postings.\n",
        tx_count, posting_count
    );

    println!("Checks:");
    for check in &checks {
        let status = if check.issues.is_empty() {
            "✓".green().to_string()
        } else {
            "✗".red().to_string()
        };
        println!("  {} {} — {}", status, check.name, check.description);
    }

    let total_issues: usize = checks.iter().map(|c| c.issues.len()).sum();
    if total_issues == 0 {
        println!("\n{}", "No issues found.".green());
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
        println!("{}:", check.name.bold());
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
                    "{}:{} commodity '{}' (account: {})",
                    tx.file, tx.line, commodity, p.account,
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
