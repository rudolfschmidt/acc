//! `import` command — turn a bank CSV export into ledger transactions and
//! (optionally) append them to a `@cash` file.
//!
//! Driven by a per-bank profile (`ledger-import/<bank>.conf`): column
//! mapping, output target, an `identity` for dedup, and categorization
//! rules. Default is a **dry-run** — the would-be additions print as a
//! green diff with surrounding context; `--write` appends them.
//!
//! The importer never rewrites existing entries: it only appends new,
//! deduplicated transactions. Re-running over an overlapping export is
//! safe — a row already present in the ledger (matched on its embedded
//! `; csv:` comment) is skipped.

mod render;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::error::Error;

/// Amount column: every posting amount is right-aligned so its last
/// character sits at this column (counting from after the leading tab),
/// matching the existing `@cash` files.
const ALIGN: usize = 70;

pub fn run(csv_path: &str, conf_path: &str, write: bool) -> Result<(), Error> {
    let profile = Profile::load(conf_path)?;
    let rows = parse_csv(&read(csv_path)?);
    let rows: Vec<Vec<String>> = rows.into_iter().skip(1).filter(|r| !is_blank(r)).collect();
    if rows.is_empty() {
        return Err(Error::from("import: no data rows in CSV"));
    }
    let ncols = rows[0].len();

    // What's already in the target ledger, as an identity multiset.
    let existing = std::fs::read_to_string(&profile.output_file).unwrap_or_default();
    let mut seen = existing_identities(&existing, &profile, ncols);

    // Generate ledger text for every row not already present.
    let mut new_blocks: Vec<String> = Vec::new();
    let mut skipped = 0usize;
    for row in &rows {
        let key = profile.identity_key(row);
        if let Some(c) = seen.get_mut(&key)
            && *c > 0
        {
            *c -= 1;
            skipped += 1;
            continue;
        }
        new_blocks.push(profile.render_transaction(row));
    }

    if new_blocks.is_empty() {
        println!(
            "import: {} rows read, all already present — nothing new.",
            rows.len()
        );
        return Ok(());
    }

    // Append first (when writing) — `existing` is already snapshotted, so
    // the diff still renders against the pre-write state — then show the
    // same diff in both modes; only the header differs (Preview vs Update).
    if write {
        append(&profile.output_file, &new_blocks)?;
    }
    render::diff_preview(&existing, &new_blocks, &profile.output_file, skipped, write);
    Ok(())
}

// ---------------------------------------------------------------------
// profile
// ---------------------------------------------------------------------

struct Rule {
    /// (column index, lowercased substring) — all must match.
    conds: Vec<(usize, String)>,
    account: String,
}

struct Profile {
    fields: HashMap<String, usize>, // field.NAME -> column index
    output_file: PathBuf,
    title: String,
    account: String,   // bank-side account
    commodity: String, // symbol for the booked amount (€)
    precision: usize,  // decimals for the booked amount
    sym: HashMap<String, String>, // currency code (upper) -> symbol
    identity: Vec<usize>,
    rules: Vec<Rule>,
    default_account: String, // template, may contain {payee}
}

impl Profile {
    fn load(path: &str) -> Result<Profile, Error> {
        let src = read(path)?;
        let mut directives: HashMap<String, String> = HashMap::new();
        let mut raw_rules: Vec<(String, String)> = Vec::new(); // (lhs, account)
        // Fallback when the profile omits a `default` rule; profiles
        // normally set their own (e.g. `default => expenses:{payee}`).
        let mut default_account = String::from("expenses:{payee}");

        for line in src.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((lhs, rhs)) = line.split_once("=>") {
                let lhs = lhs.trim();
                let account = rhs.trim().to_string();
                if lhs == "default" {
                    default_account = account;
                } else {
                    raw_rules.push((lhs.to_string(), account));
                }
            } else if let Some((key, val)) = line.split_once(char::is_whitespace) {
                directives.insert(key.trim().to_string(), val.trim().to_string());
            }
        }

        // Column mapping: every `field.NAME` directive.
        let mut fields = HashMap::new();
        for (k, v) in &directives {
            if let Some(name) = k.strip_prefix("field.") {
                let idx: usize = v
                    .parse()
                    .map_err(|_| Error::from(format!("import: field.{} = '{}' is not a column index", name, v)))?;
                fields.insert(name.to_string(), idx);
            }
        }

        let get = |key: &str| -> Result<String, Error> {
            directives
                .get(key)
                .cloned()
                .ok_or_else(|| Error::from(format!("import: missing '{}' in profile", key)))
        };

        let output_file = expand(&get("output.file")?);
        let title = get("output.title")?;
        let account = get("output.account")?;
        let commodity = get("output.commodity")?;

        // Symbols + precision come from the referenced commodities file.
        let (sym, precision) = match directives.get("commodities") {
            Some(p) => load_commodities(&expand(p), &commodity)?,
            None => (HashMap::new(), 2),
        };

        // identity: field names -> column indices.
        let identity = get("identity")?
            .split_whitespace()
            .map(|name| {
                fields
                    .get(name)
                    .copied()
                    .ok_or_else(|| Error::from(format!("import: identity field '{}' has no field.* mapping", name)))
            })
            .collect::<Result<Vec<_>, _>>()?;

        // Rules: parse each LHS into AND-ed conditions.
        let mut rules = Vec::new();
        for (lhs, acc) in raw_rules {
            let mut conds = Vec::new();
            for part in lhs.split(';') {
                let part = part.trim();
                let (fname, val) = part
                    .split_once(char::is_whitespace)
                    .ok_or_else(|| Error::from(format!("import: rule '{}' is not <field> <value>", part)))?;
                let idx = *fields
                    .get(fname.trim())
                    .ok_or_else(|| Error::from(format!("import: rule field '{}' has no field.* mapping", fname.trim())))?;
                conds.push((idx, val.trim().to_lowercase()));
            }
            rules.push(Rule { conds, account: acc });
        }

        Ok(Profile {
            fields,
            output_file,
            title,
            account,
            commodity,
            precision,
            sym,
            identity,
            rules,
            default_account,
        })
    }

    fn field_val(&self, row: &[String], name: &str) -> String {
        self.fields
            .get(name)
            .and_then(|&i| row.get(i))
            .cloned()
            .unwrap_or_default()
    }

    fn identity_key(&self, row: &[String]) -> String {
        self.identity
            .iter()
            .map(|&i| row.get(i).cloned().unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\u{1}")
    }

    /// The counter account for a row: first matching rule wins, else the
    /// slugified-partner default.
    fn categorize(&self, row: &[String]) -> String {
        for rule in &self.rules {
            let hit = rule.conds.iter().all(|(idx, val)| {
                row.get(*idx)
                    .map(|f| f.to_lowercase().contains(val))
                    .unwrap_or(false)
            });
            if hit {
                return self.apply_template(&rule.account, row);
            }
        }
        self.apply_template(&self.default_account, row)
    }

    fn apply_template(&self, tmpl: &str, row: &[String]) -> String {
        if tmpl.contains("{payee}") {
            let slug = slug(&self.field_val(row, "payee"));
            tmpl.replace("{payee}", &slug)
        } else {
            tmpl.to_string()
        }
    }

    /// Render one row as a ledger transaction block (no trailing newline).
    fn render_transaction(&self, row: &[String]) -> String {
        let date = self.field_val(row, "date");
        let amount = fmt_amount(&self.field_val(row, "amount"), self.precision);
        let bank = format!("{}{}", self.commodity, amount);
        let counter = self.categorize(row);

        let csv = row
            .iter()
            .map(|f| format!("\"{}\"", f))
            .collect::<Vec<_>>()
            .join(",");

        let mut s = String::new();
        s.push_str(&format!("{} * {}\n", date, self.title));
        s.push_str(&format!("\t; csv: {}\n", csv));
        s.push_str(&format!("\t{}\n", pad_amount(&self.account, &bank)));

        // Foreign-currency leg: counter posting carries the original amount,
        // sign flipped; domestic rows leave it bare for auto-balancing.
        let fxc = self.field_val(row, "fx-currency");
        if !fxc.is_empty() && !fxc.eq_ignore_ascii_case("EUR") {
            let symbol = self
                .sym
                .get(&fxc.to_uppercase())
                .cloned()
                .unwrap_or_else(|| fxc.clone());
            let fx = format!("{}{}", symbol, negate(&self.field_val(row, "fx-amount")));
            s.push_str(&format!("\t{}", pad_amount(&counter, &fx)));
        } else {
            s.push_str(&format!("\t{}", counter));
        }
        s
    }
}

// ---------------------------------------------------------------------
// dedup
// ---------------------------------------------------------------------

/// Build the identity multiset from a ledger's embedded `; csv:` comments.
/// Only comments with the same column count as the current CSV are used,
/// so an older differently-shaped export format can't false-match.
fn existing_identities(src: &str, profile: &Profile, ncols: usize) -> HashMap<String, usize> {
    let mut map = HashMap::new();
    for line in src.lines() {
        let t = line.trim_start();
        let Some(rest) = t.strip_prefix("; csv:") else {
            continue;
        };
        let fields = parse_record(rest.trim());
        if fields.len() != ncols {
            continue;
        }
        let key = profile
            .identity
            .iter()
            .map(|&i| fields.get(i).cloned().unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\u{1}");
        *map.entry(key).or_insert(0) += 1;
    }
    map
}

// ---------------------------------------------------------------------
// formatting helpers
// ---------------------------------------------------------------------

fn slug(s: &str) -> String {
    s.to_lowercase().replace(' ', "-")
}

/// Pad an amount string to right-align it at [`ALIGN`].
fn pad_amount(account: &str, amount: &str) -> String {
    let used = account.chars().count() + amount.chars().count();
    let pad = ALIGN.saturating_sub(used).max(1);
    format!("{}{}{}", account, " ".repeat(pad), amount)
}

/// Format a decimal string to exactly `precision` fractional digits.
/// `-3` → `-3.00`, `-64.60` → `-64.60`, `0.25` → `0.25`.
fn fmt_amount(s: &str, precision: usize) -> String {
    let (sign, body) = match s.strip_prefix('-') {
        Some(r) => ("-", r),
        None => ("", s),
    };
    let (int, frac) = body.split_once('.').unwrap_or((body, ""));
    if precision == 0 {
        return format!("{}{}", sign, int);
    }
    let frac = if frac.len() >= precision {
        frac[..precision].to_string()
    } else {
        format!("{}{}", frac, "0".repeat(precision - frac.len()))
    };
    format!("{}{}.{}", sign, int, frac)
}

fn negate(s: &str) -> String {
    match s.strip_prefix('-') {
        Some(r) => r.to_string(),
        None => format!("-{}", s),
    }
}

// ---------------------------------------------------------------------
// commodities / csv / io
// ---------------------------------------------------------------------

/// Parse a `commodities.ledger`: return (code→symbol, precision-of-target).
fn load_commodities(path: &Path, target: &str) -> Result<(HashMap<String, String>, usize), Error> {
    let src = std::fs::read_to_string(path)
        .map_err(|e| Error::from(format!("import: read {}: {}", path.display(), e)))?;
    let mut sym = HashMap::new();
    let mut precision = 2usize;
    let mut current = String::new();
    for line in src.lines() {
        let t = line.trim();
        if let Some(s) = t.strip_prefix("commodity ") {
            current = s.trim().to_string();
            sym.insert(current.to_uppercase(), current.clone());
        } else if let Some(a) = t.strip_prefix("alias ") {
            sym.insert(a.trim().to_uppercase(), current.clone());
        } else if let Some(p) = t.strip_prefix("precision ")
            && current == target
            && let Ok(n) = p.trim().parse::<usize>()
        {
            precision = n;
        }
    }
    Ok((sym, precision))
}

/// Minimal RFC-4180 parser: handles quoted fields with embedded commas,
/// doubled `""` escapes, and `\r\n`. Returns every record (no header skip).
fn parse_csv(src: &str) -> Vec<Vec<String>> {
    let mut records = Vec::new();
    let mut record = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = src.chars().peekable();
    while let Some(c) = chars.next() {
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(c);
            }
        } else {
            match c {
                '"' => in_quotes = true,
                ',' => record.push(std::mem::take(&mut field)),
                '\n' => {
                    record.push(std::mem::take(&mut field));
                    records.push(std::mem::take(&mut record));
                }
                '\r' => {}
                _ => field.push(c),
            }
        }
    }
    if !field.is_empty() || !record.is_empty() {
        record.push(field);
        records.push(record);
    }
    records
}

/// Parse a single CSV record (one line, no trailing newline).
fn parse_record(line: &str) -> Vec<String> {
    parse_csv(line).into_iter().next().unwrap_or_default()
}

fn is_blank(record: &[String]) -> bool {
    record.iter().all(|f| f.trim().is_empty())
}

fn read(path: &str) -> Result<String, Error> {
    std::fs::read_to_string(expand(path))
        .map_err(|e| Error::from(format!("import: read {}: {}", path, e)))
}

/// Append new transaction blocks to the ledger, separated by blank lines.
fn append(path: &Path, blocks: &[String]) -> Result<(), Error> {
    use std::io::Write as _;
    let body = format!("\n{}\n", blocks.join("\n\n"));
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| Error::from(format!("import: open {}: {}", path.display(), e)))?;
    f.write_all(body.as_bytes())
        .map_err(|e| Error::from(format!("import: write {}: {}", path.display(), e)))
}

/// Expand a leading `~/` to `$HOME`.
fn expand(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/")
        && let Ok(home) = std::env::var("HOME")
    {
        return Path::new(&home).join(rest);
    }
    PathBuf::from(path)
}

#[cfg(test)]
mod tests;
