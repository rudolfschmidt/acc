//! `rename` command — rename an account (and everything under it) across
//! journal files.
//!
//! Preview by default: prints every posting whose account would change
//! and writes nothing. `--execute` (`-e`) applies the change in place.
//!
//! OLD is matched with the same anchors as the report filter: a bare
//! pattern matches anywhere (`contains`), a leading `^` anchors it to the
//! start of the account and a trailing `$` to the end. So `rename foo:5
//! foo:4` (contains) renames every account containing `foo:5` — `foo:5`,
//! `foo:50`, `bar:foo:5:cash`, … — while `rename ^foo:5 foo:4` only
//! touches accounts that *start* with `foo:5`. The matched span is
//! swapped for NEW and the rest of the account name is preserved.
//!
//! The match is structural, not textual: each file is parsed so we know
//! exactly which lines are postings and what each posting's account is.
//! Only the account token on a matched *posting* line is rewritten —
//! `account` directives, auto-rule patterns, comments and descriptions
//! are left untouched — and the rest of the file stays byte-for-byte
//! identical. A file that fails to parse is reported on stderr and
//! skipped, never edited.

use std::fs;
use std::path::{Path, PathBuf};

use colored::Colorize;

use super::util::{render_account, shorten_home};
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

        let shown = shorten_home(&path.to_string_lossy());
        for h in &hits {
            println!("{}:{} {} → {}", shown, h.line, h.old.red(), h.new.green());
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
            let p = &lp.value;
            if let Some(renamed) = rename_account(&p.account, old, new) {
                // Show — and rewrite — the account with its virtual brackets
                // (`[a:5]` / `(a:5)`), so the preview makes clear which
                // postings are virtual and the brackets survive on write.
                let new = match (p.is_virtual, p.balanced) {
                    (true, true) => format!("[{renamed}]"),
                    (true, false) => format!("({renamed})"),
                    (false, _) => renamed,
                };
                hits.push(Hit { line: lp.line, old: render_account(p), new });
            }
        }
    }
    hits
}

/// The renamed account when `account` matches OLD, else `None`. OLD uses
/// the report filter's anchors: a leading `^` matches the start, a
/// trailing `$` the end, both together an exact account, and a bare
/// pattern matches anywhere. The matched span is replaced with NEW and
/// the rest is preserved — so `^foo:5` rewrites only a leading `foo:5`,
/// `cash$` only a trailing `cash`, and a bare `foo:5` every occurrence.
fn rename_account(account: &str, old: &str, new: &str) -> Option<String> {
    let anchored_start = old.starts_with('^');
    let anchored_end = old.ends_with('$');
    // `^` and `$` are ASCII, so slicing them off keeps the core valid UTF-8.
    let core = &old[anchored_start as usize..old.len() - anchored_end as usize];
    if core.is_empty() {
        return None;
    }
    match (anchored_start, anchored_end) {
        (true, true) => (account == core).then(|| new.to_string()),
        (true, false) => account.strip_prefix(core).map(|rest| format!("{new}{rest}")),
        (false, true) => account.strip_suffix(core).map(|head| format!("{head}{new}")),
        (false, false) => account.contains(core).then(|| account.replace(core, new)),
    }
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
        println!("{} No matching accounts found.", "!".yellow());
        return;
    }
    let p = if postings == 1 { "posting" } else { "postings" };
    let f = if files == 1 { "file" } else { "files" };
    println!();
    if execute {
        println!("{} Renamed {} {} in {} {}.", "✓".green(), postings, p, files, f);
    } else {
        // Preview only — nothing written yet, so this is a warning/info, not
        // a success.
        println!(
            "{} {} {} in {} {} would be renamed. Re-run with {} to apply.",
            "!".yellow(),
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
    fn bare_pattern_matches_anywhere() {
        let r = |a: &str| rename_account(a, "foo:5", "foo:4");
        assert_eq!(r("foo:5"), Some("foo:4".to_string())); // exact occurrence
        assert_eq!(r("foo:50"), Some("foo:40".to_string())); // within a segment
        assert_eq!(r("foo:5:cash"), Some("foo:4:cash".to_string())); // sub-account
        assert_eq!(r("bar:foo:5"), Some("bar:foo:4".to_string())); // mid-account
        assert_eq!(r("foo:6"), None); // no occurrence
    }

    #[test]
    fn caret_anchors_to_the_start() {
        let r = |a: &str| rename_account(a, "^foo:5", "foo:4");
        assert_eq!(r("foo:5:cash"), Some("foo:4:cash".to_string()));
        assert_eq!(r("bar:foo:5"), None); // not at the start
    }

    #[test]
    fn dollar_anchors_to_the_end() {
        let r = |a: &str| rename_account(a, "cash$", "bank");
        assert_eq!(r("foo:5:cash"), Some("foo:5:bank".to_string()));
        assert_eq!(r("cash:foo"), None); // not at the end
    }

    #[test]
    fn caret_and_dollar_match_exactly() {
        let r = |a: &str| rename_account(a, "^foo:5$", "foo:4");
        assert_eq!(r("foo:5"), Some("foo:4".to_string()));
        assert_eq!(r("foo:50"), None); // not exact
        assert_eq!(r("foo:5:cash"), None); // not exact
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
