//! `rename` command — rename an account (and everything under it) across
//! journal files by prefix.
//!
//! Preview by default: prints every posting whose account would change
//! and writes nothing. `--execute` (`-e`) applies the change in place.
//!
//! The match is structural, not textual: each file is parsed so we know
//! exactly which lines are postings and what each posting's account is.
//! An account matches when it *starts with* OLD, so `rename foo:5 foo:4`
//! renames `foo:5`, `foo:50`, `foo:5:cash`, … — OLD need not be a whole
//! segment, which lets one command renumber a whole block at once. The
//! match is anchored to the start, so `bar:foo:5` is left alone. Only the
//! account token on a matched *posting* line is rewritten — `account`
//! directives, auto-rule patterns, comments and descriptions are left
//! untouched — and the rest of the file stays byte-for-byte identical. A
//! file that fails to parse is reported on stderr and skipped, never
//! edited.

use std::fs;
use std::path::{Path, PathBuf};

use colored::Colorize;

use crate::Error;
use crate::parser;
use crate::parser::entry::Entry;

/// One pending rename located in a file: the posting's line (1-based)
/// and the full account before / after.
struct Hit {
    line: usize,
    old: String,
    new: String,
}

pub fn run(paths: &[PathBuf], old: &str, new: &str, execute: bool) -> Result<(), Error> {
    if old.is_empty() || new.is_empty() {
        return Err(Error::from("rename: OLD and NEW must both be non-empty"));
    }
    if old == new {
        return Err(Error::from("rename: OLD and NEW are identical"));
    }

    // Dedup so an overlapping `-f dir -f dir/sub` never edits a file twice.
    let mut files = paths.to_vec();
    files.sort();
    files.dedup();

    let mut postings = 0usize;
    let mut changed_files = 0usize;

    for path in &files {
        let source = match fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{} {}: {}", "skip".yellow(), path.display(), e);
                continue;
            }
        };
        let entries = match parser::parse(&source) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("{} {}: parse error: {}", "skip".yellow(), path.display(), e);
                continue;
            }
        };

        let hits = collect_hits(&entries, old, new);
        if hits.is_empty() {
            continue;
        }

        if execute {
            let rewritten = apply(&source, &hits);
            write_atomic(path, &rewritten)?;
        }

        for h in &hits {
            println!("{}:{}  {} → {}", path.display(), h.line, h.old.red(), h.new.green());
        }
        postings += hits.len();
        changed_files += 1;
    }

    print_summary(postings, changed_files, execute);
    Ok(())
}

/// Every posting whose account matches OLD, with its line and renamed
/// account. Only `Entry::Transaction` postings are considered.
fn collect_hits(entries: &[crate::parser::located::Located<Entry>], old: &str, new: &str) -> Vec<Hit> {
    let mut hits = Vec::new();
    for e in entries {
        let Entry::Transaction(tx) = &e.value else { continue };
        for lp in &tx.postings {
            if let Some(renamed) = rename_account(&lp.value.account, old, new) {
                hits.push(Hit { line: lp.line, old: lp.value.account.clone(), new: renamed });
            }
        }
    }
    hits
}

/// The renamed account when `account` starts with OLD, else `None`. OLD
/// is a plain prefix (need not be a whole segment), so `foo:5` matches
/// `foo:5`, `foo:50` and `foo:5:cash`; only that OLD prefix is swapped for
/// NEW and the tail is preserved. Anchored at the start, so an OLD buried
/// mid-account (`bar:foo:5`) does not match.
fn rename_account(account: &str, old: &str, new: &str) -> Option<String> {
    account.strip_prefix(old).map(|rest| format!("{new}{rest}"))
}

/// Rewrite the matched posting lines in `source`, leaving every other
/// byte untouched. Splitting on `\n` and re-joining reconstructs the
/// file exactly (including a trailing newline and any `\r`), so only the
/// account token on each hit line changes. The account is the leading
/// token of a posting line, so replacing its first occurrence never
/// touches a later comment / amount on the same line.
fn apply(source: &str, hits: &[Hit]) -> String {
    let mut lines: Vec<String> = source.split('\n').map(String::from).collect();
    for h in hits {
        if let Some(line) = lines.get_mut(h.line - 1) {
            *line = line.replacen(&h.old, &h.new, 1);
        }
    }
    lines.join("\n")
}

/// Write `contents` to `path` via a temp file + rename, so a crash
/// mid-write never leaves a half-written journal.
fn write_atomic(path: &Path, contents: &str) -> Result<(), Error> {
    let tmp = path.with_extension("rename-tmp");
    fs::write(&tmp, contents)
        .map_err(|e| Error::from(format!("write {}: {}", tmp.display(), e)))?;
    fs::rename(&tmp, path)
        .map_err(|e| Error::from(format!("rename {} -> {}: {}", tmp.display(), path.display(), e)))
}

fn print_summary(postings: usize, files: usize, execute: bool) {
    if postings == 0 {
        println!("No matching accounts found.");
        return;
    }
    let p = if postings == 1 { "posting" } else { "postings" };
    let f = if files == 1 { "file" } else { "files" };
    println!();
    if execute {
        println!("Renamed {} {} in {} {}.", postings, p, files, f);
    } else {
        println!(
            "{} {} in {} {} would be renamed. Re-run with {} to apply.",
            postings,
            p,
            files,
            f,
            "-e".bold()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{Hit, apply, rename_account};

    #[test]
    fn matches_by_prefix_anchored_at_the_start() {
        let r = |a: &str| rename_account(a, "a:5", "a:4");
        assert_eq!(r("a:5"), Some("a:4".to_string())); // exact
        assert_eq!(r("a:50"), Some("a:40".to_string())); // prefix within the segment
        assert_eq!(r("a:5:cash"), Some("a:4:cash".to_string())); // sub-account
        assert_eq!(r("x:a:5"), None); // not at the start
        assert_eq!(r("a:6"), None); // different prefix
    }

    #[test]
    fn apply_replaces_only_the_leftmost_account_token() {
        // The account is the leading token, so the same text appearing
        // later in a comment on the line must survive untouched, and other
        // lines (and the trailing newline) stay byte-identical.
        let src = "2024-01-01 * x\n    a:11:cash   €5  ; note a:11:cash\n    equity   €-5\n";
        let hits = vec![Hit {
            line: 2,
            old: "a:11:cash".to_string(),
            new: "a:12:cash".to_string(),
        }];
        assert_eq!(
            apply(src, &hits),
            "2024-01-01 * x\n    a:12:cash   €5  ; note a:11:cash\n    equity   €-5\n"
        );
    }
}
