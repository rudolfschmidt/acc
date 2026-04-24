//! `diff` command — compare two ledger files or directory trees at
//! the **source line** level, ignoring whitespace differences (like
//! `diff -w`). Each line is normalised by collapsing all runs of
//! whitespace into a single space and trimming both ends before the
//! comparison; only genuine character differences show up.
//!
//! Why not parse first? Because the parser canonicalises expressions
//! like `(€1/212)` into a `Decimal`, which hides exactly the kind of
//! formatter-induced damage this command needs to surface.
//!
//! Output follows `git diff` conventions: file header (`--- a/...`,
//! `+++ b/...`), hunk header (`@@ -old,n +new,m @@`) and unified-style
//! body lines prefixed with ` ` (context), `-` (removed) or `+`
//! (added). Hunks get up to 3 lines of surrounding context.
//!
//! Exits 0 when everything matches, 1 when at least one difference
//! or missing counterpart file is found.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use colored::Colorize;

use crate::error::Error;

const CONTEXT_LINES: usize = 3;

pub fn run(snapshot: Option<&str>, paths: &[String]) -> Result<(), Error> {
    // Path-count validation lives at the clap layer in `main.rs` —
    // this entry point trusts the caller to have passed exactly two
    // paths in the no-snapshot case, or any number with --snapshot.
    let pairs = match snapshot {
        Some(snap) => build_pairs_via_snapshot(Path::new(snap), paths)?,
        None => collect_pairs(Path::new(&paths[0]), Path::new(&paths[1]))?,
    };

    let mut files_compared = 0usize;
    let mut files_with_diffs = 0usize;
    let mut any_missing = false;

    for pair in &pairs {
        match pair {
            FilePair::Both(o, n) => {
                files_compared += 1;
                let hunks = compare_files(o, n)?;
                if !hunks.is_empty() {
                    files_with_diffs += 1;
                    print_file_report(o, n, &hunks);
                }
            }
            FilePair::OnlyOld(p) => {
                println!("{}", format!("- only in OLD: {}", p.display()).red());
                any_missing = true;
            }
            FilePair::OnlyNew(p) => {
                println!("{}", format!("+ only in NEW: {}", p.display()).green());
                any_missing = true;
            }
        }
    }

    println!(
        "{} files compared, {} with differences",
        files_compared, files_with_diffs
    );

    if files_with_diffs > 0 || any_missing {
        std::process::exit(1);
    }
    Ok(())
}

enum FilePair {
    Both(PathBuf, PathBuf),
    OnlyOld(PathBuf),
    OnlyNew(PathBuf),
}

fn collect_pairs(old: &Path, new: &Path) -> Result<Vec<FilePair>, Error> {
    if old.is_file() && new.is_file() {
        return Ok(vec![FilePair::Both(old.to_path_buf(), new.to_path_buf())]);
    }
    if old.is_dir() && new.is_dir() {
        let old_files = index_dir(old);
        let new_files = index_dir(new);
        let mut all_keys: BTreeSet<PathBuf> = BTreeSet::new();
        all_keys.extend(old_files.keys().cloned());
        all_keys.extend(new_files.keys().cloned());
        let mut pairs = Vec::new();
        for key in all_keys {
            match (old_files.get(&key), new_files.get(&key)) {
                (Some(o), Some(n)) => pairs.push(FilePair::Both(o.clone(), n.clone())),
                (Some(o), None) => pairs.push(FilePair::OnlyOld(o.clone())),
                (None, Some(n)) => pairs.push(FilePair::OnlyNew(n.clone())),
                (None, None) => unreachable!(),
            }
        }
        return Ok(pairs);
    }
    Err(Error::from(format!(
        "mixed types: {} is {}, {} is {}",
        old.display(),
        describe(old),
        new.display(),
        describe(new),
    )))
}

/// Snapshot mode: for each working-side PATH, find the corresponding
/// path inside `snapshot_root` via longest-suffix match and pair
/// them up as (snapshot_path, working_path). The snapshot side is
/// treated as OLD.
///
/// Algorithm per working path:
/// 1. Resolve to absolute path.
/// 2. Walk the path components from right to left, joining them as
///    a suffix (`ccp.ledger`, `@cash/ccp.ledger`, …).
/// 3. The **longest** suffix for which `snapshot_root/suffix` exists
///    on disk wins.
/// 4. If nothing matches → error listing what was searched.
fn build_pairs_via_snapshot(
    snapshot_root: &Path,
    paths: &[String],
) -> Result<Vec<FilePair>, Error> {
    if !snapshot_root.is_dir() {
        return Err(Error::from(format!(
            "snapshot root {} is not a directory",
            snapshot_root.display(),
        )));
    }

    let working_paths: Vec<PathBuf> = if paths.is_empty() {
        vec![std::env::current_dir()
            .map_err(|e| Error::from(format!("getcwd failed: {}", e)))?]
    } else {
        paths.iter().map(PathBuf::from).collect()
    };

    let mut out = Vec::new();
    for work in &working_paths {
        let abs = work
            .canonicalize()
            .map_err(|e| Error::from(format!("resolve {}: {}", work.display(), e)))?;
        let matched = longest_suffix_match(snapshot_root, &abs).ok_or_else(|| {
            Error::from(format!(
                "no matching path under {} for {}",
                snapshot_root.display(),
                abs.display(),
            ))
        })?;
        let pair_is_dir_both = matched.is_dir() && abs.is_dir();
        let pair_is_file_both = matched.is_file() && abs.is_file();
        if !(pair_is_dir_both || pair_is_file_both) {
            return Err(Error::from(format!(
                "{} and {} are not the same kind (file vs directory)",
                matched.display(),
                abs.display(),
            )));
        }
        if matched.is_file() {
            out.push(FilePair::Both(matched, abs));
        } else {
            // Directory pair: expand to per-file pairs like collect_pairs
            // does in explicit two-dir mode, so the rest of the pipeline
            // is uniform.
            let snap_files = index_dir(&matched);
            let work_files = index_dir(&abs);
            let mut all_keys: BTreeSet<PathBuf> = BTreeSet::new();
            all_keys.extend(snap_files.keys().cloned());
            all_keys.extend(work_files.keys().cloned());
            for key in all_keys {
                match (snap_files.get(&key), work_files.get(&key)) {
                    (Some(o), Some(n)) => out.push(FilePair::Both(o.clone(), n.clone())),
                    (Some(o), None) => out.push(FilePair::OnlyOld(o.clone())),
                    (None, Some(n)) => out.push(FilePair::OnlyNew(n.clone())),
                    (None, None) => unreachable!(),
                }
            }
        }
    }
    Ok(out)
}

/// Walk `work_abs` components from right to left, returning the
/// snapshot-side path for the longest suffix that exists on disk.
/// The empty suffix is also tried (`skip == components.len()`),
/// which covers the case where `snapshot_root` itself mirrors the
/// working-tree root — common when the user invokes `acc diff
/// --snapshot DIR .` from the working-tree root and the backup
/// preserves the same top-level layout.
fn longest_suffix_match(snapshot_root: &Path, work_abs: &Path) -> Option<PathBuf> {
    let components: Vec<&std::ffi::OsStr> = work_abs
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => Some(s),
            _ => None,
        })
        .collect();

    // Try longest suffix first: start with the full component list,
    // shrink from the front, and finally try the empty suffix
    // (`snapshot_root` itself). The first existing path wins.
    for skip in 0..=components.len() {
        let mut candidate = snapshot_root.to_path_buf();
        for seg in &components[skip..] {
            candidate.push(seg);
        }
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

fn describe(p: &Path) -> &'static str {
    if p.is_file() {
        "a file"
    } else if p.is_dir() {
        "a directory"
    } else {
        "missing"
    }
}

/// Map relative-path → absolute-path for every `.ledger` under `root`.
fn index_dir(root: &Path) -> BTreeMap<PathBuf, PathBuf> {
    let mut out = BTreeMap::new();
    let mut stack: Vec<PathBuf> = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("ledger") {
                if let Ok(rel) = path.strip_prefix(root) {
                    out.insert(rel.to_path_buf(), path);
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------------
// diff core — whitespace-normalised LCS on source lines
// ---------------------------------------------------------------------

/// Strip every whitespace character before comparing — matches
/// `diff -w` (`--ignore-all-space`) exactly. `DZD -20000.00` and
/// `DZD-20000.00` compare equal, so only genuine token content
/// differences surface.
fn normalise(line: &str) -> String {
    line.chars().filter(|c| !c.is_whitespace()).collect()
}

/// One edit in the LCS walk. `Keep` advances both sides; `Remove`
/// consumes one line from OLD, `Add` one from NEW.
#[derive(Clone, Copy)]
enum Op {
    Keep,
    Remove,
    Add,
}

fn compare_files(old_path: &Path, new_path: &Path) -> Result<Vec<Hunk>, Error> {
    let old_src = fs::read_to_string(old_path)
        .map_err(|e| Error::from(format!("read {}: {}", old_path.display(), e)))?;
    let new_src = fs::read_to_string(new_path)
        .map_err(|e| Error::from(format!("read {}: {}", new_path.display(), e)))?;

    // Whitespace-only files (only newlines / spaces / tabs) are
    // semantically equivalent to a 0-byte file — there's no token
    // content on either side. Treat both as identical and emit no
    // hunks. Without this, an old `\n` snapshot vs. a 0-byte working
    // file (or vice versa) would surface as a removed empty line.
    if old_src.chars().all(|c| c.is_whitespace())
        && new_src.chars().all(|c| c.is_whitespace())
    {
        return Ok(Vec::new());
    }

    let old_lines: Vec<&str> = old_src.lines().collect();
    let new_lines: Vec<&str> = new_src.lines().collect();
    let old_norm: Vec<String> = old_lines.iter().map(|l| normalise(l)).collect();
    let new_norm: Vec<String> = new_lines.iter().map(|l| normalise(l)).collect();

    let ops = lcs_walk(&old_norm, &new_norm);
    Ok(build_hunks(&old_lines, &new_lines, &ops))
}

/// LCS via standard DP, then backtrack to an `Op` sequence that
/// reconstructs BOTH sides in order. `a[i] == b[j]` → `Keep`; else
/// walk toward the larger neighbour in the DP table.
fn lcs_walk(a: &[String], b: &[String]) -> Vec<Op> {
    let n = a.len();
    let m = b.len();
    let mut dp = vec![vec![0u32; m + 1]; n + 1];
    for i in 0..n {
        for j in 0..m {
            dp[i + 1][j + 1] = if a[i] == b[j] {
                dp[i][j] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }

    let mut ops = Vec::with_capacity(n + m);
    let (mut i, mut j) = (n, m);
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && a[i - 1] == b[j - 1] {
            ops.push(Op::Keep);
            i -= 1;
            j -= 1;
        } else if j > 0 && (i == 0 || dp[i][j - 1] >= dp[i - 1][j]) {
            ops.push(Op::Add);
            j -= 1;
        } else {
            ops.push(Op::Remove);
            i -= 1;
        }
    }
    ops.reverse();
    ops
}

struct Hunk {
    old_start: usize, // 1-based line number in OLD
    old_count: usize,
    new_start: usize,
    new_count: usize,
    lines: Vec<HunkLine>,
}

enum HunkLine {
    Context(String),
    Removed(String),
    Added(String),
}

/// Turn an `Op` sequence into a list of hunks. Each hunk groups
/// adjacent changes plus up to `CONTEXT_LINES` of surrounding
/// unchanged lines on either side. Runs of `Keep`s longer than
/// `2 * CONTEXT_LINES` split the diff into separate hunks.
fn build_hunks(old: &[&str], new: &[&str], ops: &[Op]) -> Vec<Hunk> {
    let mut hunks = Vec::new();
    let mut pending: Vec<(Op, usize, usize)> = Vec::new(); // (op, old_line, new_line), 1-based
    let mut i = 0usize;
    let mut j = 0usize;

    for &op in ops {
        let (oi, nj) = (i + 1, j + 1);
        match op {
            Op::Keep => {
                pending.push((op, oi, nj));
                i += 1;
                j += 1;
            }
            Op::Remove => {
                pending.push((op, oi, 0));
                i += 1;
            }
            Op::Add => {
                pending.push((op, 0, nj));
                j += 1;
            }
        }
    }

    // Walk the pending list, carving out hunks around change runs.
    let mut idx = 0usize;
    while idx < pending.len() {
        // Skip leading Keeps.
        while idx < pending.len() && matches!(pending[idx].0, Op::Keep) {
            idx += 1;
        }
        if idx >= pending.len() {
            break;
        }

        // Back up to include up to CONTEXT_LINES of prior context.
        let mut start = idx;
        let mut back = 0;
        while start > 0 && back < CONTEXT_LINES && matches!(pending[start - 1].0, Op::Keep) {
            start -= 1;
            back += 1;
        }

        // Extend forward through changes, allowing up to
        // 2 * CONTEXT_LINES of Keeps between change runs.
        let mut end = idx;
        loop {
            // consume change(s)
            while end < pending.len() && !matches!(pending[end].0, Op::Keep) {
                end += 1;
            }
            // peek ahead: how many Keeps follow?
            let keeps_start = end;
            while end < pending.len()
                && matches!(pending[end].0, Op::Keep)
                && (end - keeps_start) < 2 * CONTEXT_LINES
            {
                end += 1;
            }
            // if we stopped because of another change, keep going;
            // if we ran out of Keeps early (≥2*CONTEXT), trim trailing
            // context back to CONTEXT_LINES and stop.
            if end < pending.len() && !matches!(pending[end].0, Op::Keep) {
                continue;
            }
            // trim trailing context to at most CONTEXT_LINES
            let mut trailing = 0;
            while end > keeps_start && trailing < CONTEXT_LINES {
                trailing += 1;
                // end stays; we just limit the final count
                if keeps_start + trailing == end {
                    break;
                }
            }
            let capped_end = keeps_start + CONTEXT_LINES.min(end - keeps_start);
            end = capped_end;
            break;
        }

        // Materialise the hunk from `pending[start..end]`.
        let slice = &pending[start..end];
        let old_start = slice
            .iter()
            .find_map(|(op, oi, _)| match op {
                Op::Add => None,
                _ => Some(*oi),
            })
            .unwrap_or(1);
        let new_start = slice
            .iter()
            .find_map(|(op, _, nj)| match op {
                Op::Remove => None,
                _ => Some(*nj),
            })
            .unwrap_or(1);
        let old_count = slice
            .iter()
            .filter(|(op, _, _)| !matches!(op, Op::Add))
            .count();
        let new_count = slice
            .iter()
            .filter(|(op, _, _)| !matches!(op, Op::Remove))
            .count();

        let mut lines = Vec::new();
        for (op, oi, nj) in slice {
            match op {
                Op::Keep => lines.push(HunkLine::Context(old[oi - 1].to_string())),
                Op::Remove => lines.push(HunkLine::Removed(old[oi - 1].to_string())),
                Op::Add => lines.push(HunkLine::Added(new[nj - 1].to_string())),
            }
        }

        hunks.push(Hunk {
            old_start,
            old_count,
            new_start,
            new_count,
            lines,
        });

        idx = end;
    }

    hunks
}

// ---------------------------------------------------------------------
// rendering — git diff style
// ---------------------------------------------------------------------

fn print_file_report(old_path: &Path, new_path: &Path, hunks: &[Hunk]) {
    println!("{}", format!("--- {}", old_path.display()).red().bold());
    println!("{}", format!("+++ {}", new_path.display()).green().bold());
    for hunk in hunks {
        println!(
            "{}",
            format!(
                "@@ -{},{} +{},{} @@",
                hunk.old_start, hunk.old_count, hunk.new_start, hunk.new_count
            )
            .cyan()
        );
        for l in &hunk.lines {
            match l {
                HunkLine::Context(s) => println!(" {}", s),
                HunkLine::Removed(s) => println!("{}", format!("-{}", s).red()),
                HunkLine::Added(s) => println!("{}", format!("+{}", s).green()),
            }
        }
    }
    println!();
}
