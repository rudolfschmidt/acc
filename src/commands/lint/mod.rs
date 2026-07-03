//! `lint` command — run a suite of lints over the journal and report
//! which ran, what they scanned, and any issues found as warnings.

use std::path::{Component, Path, PathBuf};

use colored::Colorize;

use super::util::shorten_home;
use crate::loader::Journal;
use crate::parser::located::Located;
use crate::parser::transaction::Transaction;

pub fn run(journal: &Journal, base: Option<&str>) {
    let txs = &journal.transactions;
    let tx_count = txs.len();
    let posting_count: usize = txs.iter().map(|tx| tx.value.postings.len()).sum();

    let mut lints: Vec<Lint> = vec![
        lint_commodity_casing(txs),
        lint_leaf_accounts(txs),
        lint_unresolved_role_refs(txs),
    ];
    // Opt-in: only when a ledger root is given (via `--base`).
    if let Some(base) = base {
        lints.push(lint_dir_category(txs, base));
    }

    println!(
        "Scanned {} transactions, {} postings.\n",
        tx_count.to_string().bold(),
        posting_count.to_string().bold(),
    );

    println!("{}", "Checks:".bold());
    let name_width = lints.iter().map(|l| l.name.len()).max().unwrap_or(0);
    for lint in &lints {
        let mark = if lint.issues.is_empty() {
            "✓".green()
        } else {
            "!".yellow()
        };
        println!(
            "  {} {} — {}",
            mark,
            format!("{:<name_width$}", lint.name).bold(),
            lint.description,
        );
    }

    let total_issues: usize = lints.iter().map(|c| c.issues.len()).sum();
    if total_issues == 0 {
        println!("\n{}", "No issues found.".green().bold());
        return;
    }

    println!(
        "\n{} issue(s) found:",
        total_issues.to_string().yellow().bold()
    );
    for lint in &lints {
        if lint.issues.is_empty() {
            continue;
        }
        println!("\n{}", format!("{}:", lint.name).yellow().bold());
        for issue in &lint.issues {
            println!("  {}", issue);
        }
    }
}

struct Lint {
    name: &'static str,
    description: &'static str,
    issues: Vec<String>,
}

/// A `file:line` issue location: the home-shortened path and the `:`
/// separator in bright blue, only the line number in blue so it stands
/// apart while the colon still reads as part of the path.
fn loc(file: &str, line: usize) -> String {
    format!("{}{}", format!("{}:", shorten_home(file)).bright_blue(), line.to_string().blue())
}

/// Multi-character commodity symbols must be all uppercase.
/// Single-character symbols (`$`, `€`, `£`) are exempt — most are
/// not ASCII letters anyway and carry no natural case.
fn lint_commodity_casing(txs: &[Located<Transaction>]) -> Lint {
    let mut issues = Vec::new();
    for tx in txs {
        for lp in &tx.value.postings {
            let p = &lp.value;
            let Some(amount) = &p.amount else { continue };
            let commodity = &amount.commodity;
            if commodity.len() > 1 && commodity.chars().any(|c| c.is_lowercase()) {
                issues.push(format!(
                    "{} expected '{}' but found '{}'",
                    loc(&lp.file, lp.line),
                    commodity.to_uppercase().green(),
                    commodity.red(),
                ));
            }
        }
    }
    Lint {
        name: "commodity-casing",
        description: "multi-char commodity symbols must be all-uppercase",
        issues,
    }
}

/// Postings should target leaf accounts. An account that has at least
/// one sub-account elsewhere in the journal is a parent; posting
/// directly to it mixes the parent's own amounts with its children's,
/// so its tree total double-counts. Every offending posting is listed.
fn lint_leaf_accounts(txs: &[Located<Transaction>]) -> Lint {
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
                    "{} '{}' is not a leaf account — '{}' exists",
                    loc(&lp.file, lp.line),
                    lp.value.account.red(),
                    sub.green(),
                ));
            }
        }
    }

    Lint {
        name: "leaf-accounts",
        description: "postings must target leaf accounts (no sub-accounts)",
        issues,
    }
}

/// A `$role:slot` reference that no `account` directive declared is left
/// verbatim by the resolver (so single-file `acc format` round-trips it).
/// In a full run every reference should resolve; a leftover `$…` account
/// means a typo'd role or a missing declaration — flag each one.
fn lint_unresolved_role_refs(txs: &[Located<Transaction>]) -> Lint {
    let mut issues = Vec::new();
    for tx in txs {
        for lp in &tx.value.postings {
            if lp.value.account.starts_with('$') {
                issues.push(format!(
                    "{} '{}' resolves to no declared account",
                    loc(&lp.file, lp.line),
                    lp.value.account.red(),
                ));
            }
        }
    }
    Lint {
        name: "role-references",
        description: "every `$role:slot` reference must resolve to a declared account",
        issues,
    }
}

/// With `--base`, every transaction whose file sits in a direct
/// sub-directory of BASE should categorise into that directory: the
/// *last* posting — the income/expense (P&L) leg — must have an account
/// that *ends with* the directory name turned into account segments
/// (`food-groceries` → `…:food:groceries`), so only the account's tail —
/// the category — has to match, not its root. The earlier legs are the
/// asset / transfer sides and are not checked. Files directly in BASE and
/// under an `@…` directory are exempt. Catches e.g. a last posting of
/// `expenses:travel` in a `food-groceries/` file.
///
/// The category is found *relative to BASE*, so it works however the
/// files were loaded — `-f .` from inside the folder, `-f food-groceries`
/// from the root, or the whole tree — by resolving each file against the
/// current directory first.
fn lint_dir_category(txs: &[Located<Transaction>], base: &str) -> Lint {
    let cwd = std::env::current_dir().unwrap_or_default();
    let base_abs = absolute(base, &cwd);
    let mut issues = Vec::new();
    for tx in txs {
        let Some(dir) = category_of(&tx.file, &cwd, &base_abs) else {
            continue;
        };
        let folder = dir.replace('-', ":"); // food-groceries -> food:groceries
        let suffix = format!(":{folder}");
        // The last posting is the category account (the income/expense side);
        // the earlier legs are assets / transfers. So the *last* posting must
        // carry the folder category.
        let last = tx.value.postings.last();
        let matched = last.is_some_and(|lp| {
            lp.value.account == folder || lp.value.account.ends_with(suffix.as_str())
        });
        if !matched {
            // The last posting's account should end in the folder category;
            // suggest the concrete fix keeping its root (`expenses:travel`
            // → `expenses:food:groceries`).
            let account = last.map_or("", |lp| lp.value.account.as_str());
            let target = match account.split_once(':') {
                Some((root, _)) => format!("{root}:{folder}"),
                None => folder.clone(),
            };
            issues.push(format!(
                "{} expected '{}' but found '{}'",
                loc(&tx.file, tx.line),
                target.green(),
                account.red(),
            ));
        }
    }
    Lint {
        name: "dir-category",
        description: "postings must categorise into their source directory",
        issues,
    }
}

/// Absolutise `p` against `cwd` — a relative `-f` is relative to where the
/// user stands.
fn absolute(p: &str, cwd: &Path) -> PathBuf {
    let p = Path::new(p);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    }
}

/// The category sub-directory of `file`, resolving a relative `file`
/// against `cwd`. `None` when the file is not under `base`, sits directly
/// in it, or its sub-directory is an `@…` one.
fn category_of(file: &str, cwd: &Path, base: &Path) -> Option<String> {
    let abs = absolute(file, cwd);
    let rest = abs.strip_prefix(base).ok()?;
    let mut names = rest.components().filter_map(|c| match c {
        Component::Normal(s) => s.to_str(),
        _ => None,
    });
    let dir = names.next()?; // first segment under base = the sub-directory
    names.next()?; // a file must follow it, else this is a base-level file
    (!dir.starts_with('@')).then(|| dir.to_string())
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

    fn setup_file(src: &str, file: &str) -> Vec<Located<Transaction>> {
        let entries = parser::parse_with_file(src, std::sync::Arc::from(file)).unwrap();
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
        let lint = lint_commodity_casing(&txs);
        assert_eq!(lint.issues.len(), 2); // both legs use `Usd`
    }

    #[test]
    fn commodity_casing_accepts_uppercase_and_single_char() {
        let txs = setup(
            "2024-01-01 * x\n\
             \tassets:a   10 USD\n\
             \tassets:b   -5 €\n\
             \tequity:o\n",
        );
        assert!(lint_commodity_casing(&txs).issues.is_empty());
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
        let lint = lint_leaf_accounts(&txs);
        assert_eq!(lint.issues.len(), 1);
        assert!(lint.issues[0].contains("expenses"));
    }

    #[test]
    fn leaf_accounts_accepts_all_leaves() {
        let txs = setup(
            "2024-01-01 * x\n\
             \texpenses:food   10 USD\n\
             \tassets:cash    -10 USD\n",
        );
        assert!(lint_leaf_accounts(&txs).issues.is_empty());
    }

    #[test]
    fn role_refs_flags_unresolved() {
        // No `capital gain` declared → `$capital:gain` survives verbatim.
        let txs = setup(
            "2024-01-01 * x\n\
             \tassets:a       -1 EUR\n\
             \t$capital:gain   1 EUR\n",
        );
        let lint = lint_unresolved_role_refs(&txs);
        assert_eq!(lint.issues.len(), 1);
        assert!(lint.issues[0].contains("$capital:gain"));
    }

    #[test]
    fn role_refs_accept_resolved() {
        // Declared → the reference resolves before lint ever sees it.
        let txs = setup(
            "account income:cap\n    capital gain\n\
             2024-01-01 * x\n\
             \tassets:a       -1 EUR\n\
             \t$capital:gain   1 EUR\n",
        );
        assert!(lint_unresolved_role_refs(&txs).issues.is_empty());
    }

    #[test]
    fn dir_category_flags_mismatch_and_accepts_tail_match() {
        let base = "/ledger";
        // Last posting is expenses:travel in a food-groceries/ file → its
        // account doesn't end with food:groceries → flagged.
        let bad = setup_file(
            "2024-01-01 * x\n\
             \tassets:cash      -10 EUR\n\
             \texpenses:travel   10 EUR\n",
            "/ledger/food-groceries/x.ledger",
        );
        assert_eq!(lint_dir_category(&bad, base).issues.len(), 1);

        // The last posting ends with the folder segments → accepted,
        // regardless of its root.
        let good = setup_file(
            "2024-01-01 * x\n\
             \tassets:cash              -10 EUR\n\
             \texpenses:food:groceries   10 EUR\n",
            "/ledger/food-groceries/x.ledger",
        );
        assert!(lint_dir_category(&good, base).issues.is_empty());

        // The category account is present but not last (asset is last) →
        // flagged: only the last posting is checked.
        let not_last = setup_file(
            "2024-01-01 * x\n\
             \texpenses:food:groceries   10 EUR\n\
             \tassets:cash              -10 EUR\n",
            "/ledger/food-groceries/x.ledger",
        );
        assert_eq!(lint_dir_category(&not_last, base).issues.len(), 1);

        // An `@…` directory is exempt.
        let cash = setup_file(
            "2024-01-01 * x\n\
             \texpenses:travel   10 EUR\n\
             \tassets:cash      -10 EUR\n",
            "/ledger/@cash/x.ledger",
        );
        assert!(lint_dir_category(&cash, base).issues.is_empty());
    }

    #[test]
    fn category_of_resolves_relative_to_base() {
        let base = Path::new("/ledger");
        let dir = |file, cwd| category_of(file, Path::new(cwd), base);

        // `-f food-groceries` from the ledger root.
        assert_eq!(dir("food-groceries/x.ledger", "/ledger"), Some("food-groceries".into()));
        // `-f .` (and `-f ./x`) from inside the folder.
        assert_eq!(dir("x.ledger", "/ledger/food-groceries"), Some("food-groceries".into()));
        assert_eq!(dir("./x.ledger", "/ledger/food-groceries"), Some("food-groceries".into()));
        // Absolute path under BASE, regardless of cwd.
        assert_eq!(dir("/ledger/food-groceries/x.ledger", "/somewhere"), Some("food-groceries".into()));
        // Exemptions: `@…` directory, a base-level file, and paths outside
        // BASE (e.g. a config file, or `base-x/` matching `base` by string).
        assert_eq!(dir("/ledger/@cash/x.ledger", "/x"), None);
        assert_eq!(dir("/ledger/x.ledger", "/x"), None);
        assert_eq!(dir("/ledger-config/x.ledger", "/x"), None);
    }
}
