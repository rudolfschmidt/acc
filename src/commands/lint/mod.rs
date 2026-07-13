//! `lint` command — run a suite of lints over the journal and report
//! which ran, what they scanned, and any issues found as warnings.

use std::path::{Component, Path, PathBuf};

use colored::Colorize;

use super::util::shorten_home;
use crate::Error;
use crate::loader::Journal;
use crate::parser::located::Located;
use crate::parser::transaction::Transaction;

pub fn run(
    journal: &Journal,
    base: Option<&str>,
    categories: &[String],
    rules: &[String],
    fix: bool,
    execute: bool,
) -> Result<(), Error> {
    let txs = &journal.transactions;
    let tx_count = txs.len();
    let posting_count: usize = txs.iter().map(|tx| tx.value.postings.len()).sum();

    let mut lints: Vec<Lint> = vec![
        lint_commodity_casing(txs),
        lint_leaf_accounts(txs),
        lint_unresolved_role_refs(txs),
        // dir-category always appears so it can be named and fixed; without a
        // `--base` it has no root to resolve the folder against, so it reports
        // as skipped rather than silently vanishing from the list.
        match base {
            Some(b) => lint_dir_category(txs, b, categories),
            None => Lint {
                name: "dir-category",
                description: "postings must categorise into their source directory",
                issues: Vec::new(),
                fixes: Vec::new(),
                skipped: Some("needs --base"),
            },
        },
    ];

    // Positional rule filter: run only the named checks; none given → all.
    if !rules.is_empty() {
        lints.retain(|l| rules.iter().any(|r| r.as_str() == l.name));
    }

    // Shared opening for both modes: what was scanned, then the checklist.
    println!(
        "Scanned {} transactions, {} postings.\n",
        tx_count.to_string().bold(),
        posting_count.to_string().bold(),
    );
    print_checklist(&lints);

    if fix {
        return run_fix(&lints, execute);
    }

    let total_issues: usize = lints.iter().map(|c| c.issues.len()).sum();
    if total_issues == 0 {
        println!("\n{}", "No issues found.".green().bold());
        return Ok(());
    }

    println!("\n{} issue(s) found:", total_issues.to_string().red().bold());
    for lint in &lints {
        if lint.issues.is_empty() {
            continue;
        }
        println!("\n{}", format!("{}:", lint.name).red().bold());
        for issue in &lint.issues {
            println!("  {}", issue);
        }
    }
    Ok(())
}

/// The `Checks:` overview — one line per check with its ✓ / ✗ / ! mark.
/// Shared by the report and `--fix` modes so both open the same way.
fn print_checklist(lints: &[Lint]) {
    println!("{}", "Checks:".bold());
    let name_width = lints.iter().map(|l| l.name.len()).max().unwrap_or(0);
    for lint in lints {
        let mark = if lint.skipped.is_some() {
            "!".yellow()
        } else if lint.issues.is_empty() {
            "✓".green()
        } else {
            "✗".red()
        };
        let head = format!(
            "  {} {} — {}",
            mark,
            format!("{:<name_width$}", lint.name).bold(),
            lint.description,
        );
        match lint.skipped {
            Some(reason) => println!("{} {}", head, format!("({reason})").yellow()),
            None => println!("{}", head),
        }
    }
}

/// `--fix` path: after the shared header + checklist, show each check's
/// rewrites — grouped under its name like the report's findings, each with
/// an action verb (`rename …`) so it reads plainly what happens — then a
/// fix tally. With `-e` the rewrites are applied atomically per file. A
/// check that flags issues but has no fixer still shows them, marked, so
/// nothing is silently left behind.
fn run_fix(lints: &[Lint], execute: bool) -> Result<(), Error> {
    let fixes: Vec<&Fix> = lints.iter().flat_map(|l| l.fixes.iter()).collect();

    for lint in lints {
        if !lint.fixes.is_empty() {
            println!("\n{}", format!("{}:", lint.name).bold());
            for f in &lint.fixes {
                println!(
                    "  {} {} → {}",
                    loc(&f.file, f.line),
                    f.old.red(),
                    f.new.green(),
                );
            }
        } else if !lint.issues.is_empty() {
            println!(
                "\n{} {}",
                format!("{}:", lint.name).red().bold(),
                "(no automatic fix)".dimmed(),
            );
            for issue in &lint.issues {
                println!("  {}", issue);
            }
        }
    }

    if execute && !fixes.is_empty() {
        apply_fixes(&fixes)?;
    }

    let n = fixes.len();
    let files = fixes
        .iter()
        .map(|f| f.file.as_str())
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let unfixable: usize = lints
        .iter()
        .filter(|l| l.fixes.is_empty())
        .map(|l| l.issues.len())
        .sum();

    println!();
    if n == 0 {
        if unfixable == 0 {
            println!("{} Nothing to fix.", "✓".green());
        } else {
            let verb = if unfixable == 1 { "issue has" } else { "issues have" };
            println!("{} {} {} no automatic fix.", "!".yellow(), unfixable, verb);
        }
    } else {
        let fixes_word = if n == 1 { "fix" } else { "fixes" };
        let files_word = if files == 1 { "file" } else { "files" };
        if execute {
            println!("{} Applied {} {} in {} {}.", "✓".green(), n, fixes_word, files, files_word);
        } else {
            println!(
                "{} {} {} in {} {} — re-run with {} to apply.",
                "!".yellow(),
                n,
                fixes_word,
                files,
                files_word,
                "-e".bold(),
            );
        }
    }
    Ok(())
}

/// Apply the collected rewrites, grouped by file and written atomically
/// (temp file + rename), so a crash mid-write never leaves a half-written
/// journal. Only the matched account token on each posting line changes.
fn apply_fixes(fixes: &[&Fix]) -> Result<(), Error> {
    use std::collections::BTreeMap;
    let mut by_file: BTreeMap<&str, Vec<&Fix>> = BTreeMap::new();
    for &f in fixes {
        by_file.entry(f.file.as_str()).or_default().push(f);
    }
    for (file, group) in by_file {
        let source = std::fs::read_to_string(file)
            .map_err(|e| Error::from(format!("lint --fix: read {file}: {e}")))?;
        let mut lines: Vec<String> = source.split('\n').map(String::from).collect();
        for f in group {
            if let Some(line) = lines.get_mut(f.line - 1) {
                *line = line.replacen(&f.old, &f.new, 1);
            }
        }
        let out = lines.join("\n");
        let tmp = format!("{file}.lint-tmp");
        std::fs::write(&tmp, &out)
            .map_err(|e| Error::from(format!("lint --fix: write {tmp}: {e}")))?;
        std::fs::rename(&tmp, file)
            .map_err(|e| Error::from(format!("lint --fix: rename {tmp} -> {file}: {e}")))?;
    }
    Ok(())
}

struct Lint {
    name: &'static str,
    description: &'static str,
    issues: Vec<String>,
    /// Structured rewrites this check proposes for `--fix`. Empty for
    /// checks with no automatic fix — only `dir-category` fills it.
    fixes: Vec<Fix>,
    /// `Some(reason)` when the check could not run (missing config) — shown
    /// as a `!` warning rather than a `✓`/`✗`.
    skipped: Option<&'static str>,
}

/// One rewrite `--fix` can apply: change the account token on a single
/// posting line.
struct Fix {
    file: String,
    line: usize,
    old: String,
    new: String,
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
        fixes: Vec::new(),
        skipped: None,
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
        fixes: Vec::new(),
        skipped: None,
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
        fixes: Vec::new(),
        skipped: None,
    }
}

/// With `--base` and `--categories`, every transaction whose file sits in
/// a direct sub-directory of BASE should categorise into that directory:
/// *each* of its *category* postings — an account starting with a
/// `--categories` prefix (income / expense) — must *end with* the directory
/// name turned into account segments (`food-groceries` → `…:food:groceries`),
/// so only the account's tail — the category — has to match, not its root.
/// Every mismatching category posting is flagged (a correct sibling no
/// longer excuses a wrong one). Postings on other accounts (asset /
/// transfer legs) are ignored, and a transaction with *no* category
/// posting (a pure transfer) is left alone.
/// Files directly in BASE and under an `@…` directory are exempt.
///
/// Without `--categories` the check can't tell a category account from a
/// transfer, so it is skipped with a warning.
///
/// The category is found *relative to BASE*, so it works however the files
/// were loaded — `-f .` from inside the folder, `-f food-groceries` from the
/// root, or the whole tree.
fn lint_dir_category(txs: &[Located<Transaction>], base: &str, categories: &[String]) -> Lint {
    let name = "dir-category";
    let description = "postings must categorise into their source directory";
    if categories.is_empty() {
        return Lint {
            name,
            description,
            issues: Vec::new(),
            fixes: Vec::new(),
            skipped: Some("needs --categories to tell category accounts from transfers"),
        };
    }
    // A leading `^` is optional — the match is a plain prefix either way.
    let prefixes: Vec<&str> = categories
        .iter()
        .map(|c| c.strip_prefix('^').unwrap_or(c))
        .collect();

    let cwd = std::env::current_dir().unwrap_or_default();
    let base_abs = absolute(base, &cwd);
    let mut issues = Vec::new();
    let mut fixes = Vec::new();
    for tx in txs {
        let Some(dir) = category_of(&tx.file, &cwd, &base_abs) else {
            continue;
        };
        let folder = dir.replace('-', ":"); // food-groceries -> food:groceries
        let suffix = format!(":{folder}");
        // Check every category posting (income/expense) independently: with
        // `--categories` each one must categorise into the folder, so a
        // single correct sibling no longer excuses a wrong one. Postings on
        // other accounts (asset / transfer legs) are ignored, and a
        // transaction with no category posting is left alone.
        for lp in &tx.value.postings {
            let account = lp.value.account.as_str();
            if !prefixes.iter().any(|p| account.starts_with(p)) {
                continue;
            }
            if account == folder || account.ends_with(suffix.as_str()) {
                continue;
            }
            // Suggest — and fix — this posting, keeping the account's root
            // (`expenses:travel` → `expenses:food:groceries`).
            let target = match account.split_once(':') {
                Some((root, _)) => format!("{root}:{folder}"),
                None => folder.clone(),
            };
            issues.push(format!(
                "{} expected '{}' but found '{}'",
                loc(&lp.file, lp.line),
                target.green(),
                account.red(),
            ));
            fixes.push(Fix {
                file: lp.file.to_string(),
                line: lp.line,
                old: account.to_string(),
                new: target,
            });
        }
    }
    Lint {
        name,
        description,
        issues,
        fixes,
        skipped: None,
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
        let cats = ["expenses:".to_string()];

        // A category (expenses:) posting that doesn't end with the folder
        // segments → flagged, wherever it sits in the transaction.
        let bad = setup_file(
            "2024-01-01 * x\n\
             \tassets:cash      -10 EUR\n\
             \texpenses:travel   10 EUR\n",
            "/ledger/food-groceries/x.ledger",
        );
        assert_eq!(lint_dir_category(&bad, base, &cats).issues.len(), 1);

        // Category account ends with the folder segments → accepted,
        // regardless of its position or root.
        let good = setup_file(
            "2024-01-01 * x\n\
             \texpenses:food:groceries   10 EUR\n\
             \tassets:cash              -10 EUR\n",
            "/ledger/food-groceries/x.ledger",
        );
        assert!(lint_dir_category(&good, base, &cats).issues.is_empty());

        // No category posting at all (pure transfer) → skipped, not flagged.
        let transfer = setup_file(
            "2024-01-01 * x\n\
             \tassets:a   -10 EUR\n\
             \tassets:b    10 EUR\n",
            "/ledger/food-groceries/x.ledger",
        );
        assert!(lint_dir_category(&transfer, base, &cats).issues.is_empty());

        // An `@…` directory is exempt.
        let cash = setup_file(
            "2024-01-01 * x\n\
             \tassets:cash      -10 EUR\n\
             \texpenses:travel   10 EUR\n",
            "/ledger/@cash/x.ledger",
        );
        assert!(lint_dir_category(&cash, base, &cats).issues.is_empty());
    }

    #[test]
    fn dir_category_produces_fix() {
        let base = "/ledger";
        let cats = ["expenses:".to_string()];
        let bad = setup_file(
            "2024-01-01 * x\n\
             \tassets:cash      -10 EUR\n\
             \texpenses:travel   10 EUR\n",
            "/ledger/food-groceries/x.ledger",
        );
        let lint = lint_dir_category(&bad, base, &cats);
        assert_eq!(lint.fixes.len(), 1);
        let fix = &lint.fixes[0];
        // Rewrite the category posting on its own line (3), keeping the root.
        assert_eq!(fix.old, "expenses:travel");
        assert_eq!(fix.new, "expenses:food:groceries");
        assert_eq!(fix.line, 3);
        assert_eq!(fix.file, "/ledger/food-groceries/x.ledger");
    }

    #[test]
    fn dir_category_checks_every_category_posting() {
        let base = "/ledger";
        let cats = ["expenses:".to_string()];
        // Two expense legs in the medical-treatment folder: one already
        // categorised, one with a hyphen typo. Only the wrong one is
        // flagged — a correct sibling no longer excuses it.
        let txs = setup_file(
            "2024-01-01 * x\n\
             \tassets:cash                 -10 EUR\n\
             \texpenses:medical:treatment    6 EUR\n\
             \texpenses:medical-treatment    4 EUR\n",
            "/ledger/medical-treatment/x.ledger",
        );
        let lint = lint_dir_category(&txs, base, &cats);
        assert_eq!(lint.issues.len(), 1);
        assert_eq!(lint.fixes.len(), 1);
        assert_eq!(lint.fixes[0].old, "expenses:medical-treatment");
        assert_eq!(lint.fixes[0].new, "expenses:medical:treatment");
    }

    #[test]
    fn dir_category_skipped_without_categories() {
        let base = "/ledger";
        let tx = setup_file(
            "2024-01-01 * x\n\
             \tassets:cash      -10 EUR\n\
             \texpenses:travel   10 EUR\n",
            "/ledger/food-groceries/x.ledger",
        );
        let lint = lint_dir_category(&tx, base, &[]);
        assert!(lint.issues.is_empty());
        assert!(lint.skipped.is_some());
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
