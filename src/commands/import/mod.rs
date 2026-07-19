//! `import` command — turn a bank CSV export into ledger transactions and
//! (optionally) append them to a `@cash` file.
//!
//! Driven by a per-bank profile (`ledger-import/<bank>.conf`): column
//! mapping, output target, an `identity` for dedup, and categorization
//! rules. Default is a **dry-run** — the would-be additions print as a
//! green diff with surrounding context; `--execute` appends them.
//!
//! The importer never rewrites existing entries: it only appends new,
//! deduplicated transactions. Re-running over an overlapping export is
//! safe — a row already present in the ledger (matched on its embedded
//! `; csv:` comment) is skipped.

mod monero;
mod render;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use colored::Colorize;

use crate::error::Error;

pub fn run(csv_path: Option<&str>, conf_path: &str, write: bool) -> Result<(), Error> {
    // A `source` directive routes to a non-CSV backend (e.g. a wallet RPC);
    // without it the profile is a classic CSV import.
    if let Some(source) = directive(&read(conf_path)?, "source") {
        return match source.as_str() {
            "monero-rpc" => monero::run(conf_path, write),
            other => Err(Error::from(format!("import: unknown source '{}'", other))),
        };
    }
    let csv_path = csv_path.ok_or_else(|| {
        Error::from("import: this profile reads a CSV — pass the CSV file as the argument")
    })?;
    csv_run(csv_path, conf_path, write)
}

/// Read a single-word directive's value from a profile (skips `#` comments
/// and `=>` rules). Used to peek at `source` before committing to a backend.
fn directive(src: &str, key: &str) -> Option<String> {
    for line in src.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.contains("=>") {
            continue;
        }
        if let Some((k, v)) = line.split_once(char::is_whitespace)
            && k.trim() == key
        {
            return Some(v.trim().to_string());
        }
    }
    None
}

fn csv_run(csv_path: &str, conf_path: &str, write: bool) -> Result<(), Error> {
    let profile = Profile::load(conf_path)?;
    let rows = parse_csv(&read(csv_path)?);
    let mut rows: Vec<Vec<String>> = rows.into_iter().skip(1).filter(|r| !is_blank(r)).collect();
    if rows.is_empty() {
        return Err(Error::from("import: no data rows in CSV"));
    }
    let ncols = rows[0].len();

    // Emit oldest-first (the ledger's convention) even when the bank
    // exports newest-first.
    if newest_first(&rows, &profile) {
        rows.reverse();
    }

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
            "{} import: {} rows read, all already present — nothing new.",
            "!".yellow(),
            rows.len()
        );
        return Ok(());
    }

    // Align the additions exactly like `acc format`, in memory, by reusing
    // the format command — so imported entries line up with every other
    // file instead of a fixed wide column of our own.
    let added = crate::commands::format::format_source(&new_blocks.join("\n\n"), false)?;

    // Append first (when writing) — `existing` is already snapshotted, so
    // the diff still renders against the pre-write state — then show the
    // same diff in both modes; only the header differs (Preview vs Update).
    if write {
        append(&profile.output_file, &added)?;
    }
    render::diff_preview(&existing, &added, new_blocks.len(), &profile.output_file, skipped, write);
    Ok(())
}

// ---------------------------------------------------------------------
// profile
// ---------------------------------------------------------------------

/// How a rule condition matches its CSV field (case-insensitive). A bare
/// value matches anywhere; `^` anchors the start, `$` the end, `^…$` the
/// whole field — mirroring the report filter and `rename`.
#[derive(Debug, Clone, Copy)]
enum Match {
    Contains,
    StartsWith,
    EndsWith,
    Exact,
}

impl Match {
    /// Split a raw value into its anchor mode and the core text (anchors
    /// stripped). `^` / `$` are ASCII, so byte-slicing keeps UTF-8 valid.
    fn parse(value: &str) -> (Match, &str) {
        let start = value.starts_with('^');
        let end = value.ends_with('$');
        let core = &value[start as usize..value.len() - end as usize];
        let mode = match (start, end) {
            (true, true) => Match::Exact,
            (true, false) => Match::StartsWith,
            (false, true) => Match::EndsWith,
            (false, false) => Match::Contains,
        };
        (mode, core)
    }

    /// Test an already-lowercased field against an already-lowercased
    /// needle under this mode.
    fn test(&self, haystack: &str, needle: &str) -> bool {
        match self {
            Match::Contains => haystack.contains(needle),
            Match::StartsWith => haystack.starts_with(needle),
            Match::EndsWith => haystack.ends_with(needle),
            Match::Exact => haystack == needle,
        }
    }
}

struct Rule {
    /// (field name, lowercased needle, anchor mode) — all must match.
    conds: Vec<(String, String, Match)>,
    account: String,
}

struct Profile {
    /// field.NAME -> one or more column indices; the first non-empty wins
    /// at read time (some banks split the counterparty across several
    /// columns, e.g. merchant / payee name / payer name).
    fields: HashMap<String, Vec<usize>>,
    /// Date column layout, e.g. `DD-MM-YYYY`. `None` = already ISO.
    date_pattern: Option<String>,
    output_file: PathBuf,
    title: String,
    account: String,   // bank-side account
    commodity: String, // symbol for the booked amount (€)
    precision: usize,  // decimals for the booked amount
    sym: HashMap<String, String>, // currency code (upper) -> symbol
    identity: Vec<String>, // field names that make a row unique
    rules: Vec<Rule>,
    default_account: String, // template, may contain {payee}
    /// Own↔own transfers: (partner-IBAN substring, the other account's
    /// leaf). A match books the counter to a directional in-transit
    /// account `<transfer_prefix>:<sender>:<receiver>`, ordered by the
    /// money flow (the amount sign), so both legs net to 0. Everything is
    /// set explicitly in the conf — transfer.self and transfer.field.
    transfers: Vec<(String, String)>,
    transfer_field: String,          // field holding the partner IBAN
    transfer_prefix: Option<String>, // prefix of transfer.self; Some = enabled
    own_leaf: String,                // own name (last segment of transfer.self)
}

impl Profile {
    fn load(path: &str) -> Result<Profile, Error> {
        let src = read(path)?;
        let mut directives: HashMap<String, String> = HashMap::new();
        let mut raw_rules: Vec<(String, String)> = Vec::new(); // (lhs, account)
        let mut raw_transfers: Vec<(String, String)> = Vec::new(); // (iban, other leaf)
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
            } else if let Some(rest) = line.strip_prefix("transfer ") {
                // `transfer <counterparty-iban> <other account leaf>`. Note
                // `transfer.prefix` keeps its dot, so it does not match here
                // and falls through to the directive branch below.
                let rest = rest.trim();
                let (iban, name) = rest.split_once(char::is_whitespace).ok_or_else(|| {
                    Error::from(format!("import: transfer '{}' is not <iban> <account>", rest))
                })?;
                raw_transfers.push((iban.trim().to_string(), name.trim().to_string()));
            } else if let Some((key, val)) = line.split_once(char::is_whitespace) {
                directives.insert(key.trim().to_string(), val.trim().to_string());
            }
        }

        // Column mapping: every `field.NAME` directive maps a name to one
        // or more column indices (first non-empty wins when read).
        let mut fields: HashMap<String, Vec<usize>> = HashMap::new();
        for (k, v) in &directives {
            if let Some(name) = k.strip_prefix("field.") {
                let cols = v
                    .split_whitespace()
                    .map(|c| c.parse::<usize>())
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| Error::from(format!("import: field.{} = '{}' is not column indices", name, v)))?;
                fields.insert(name.to_string(), cols);
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

        // identity: the field names that make a row unique.
        let identity = get("identity")?
            .split_whitespace()
            .map(|name| {
                if fields.contains_key(name) {
                    Ok(name.to_string())
                } else {
                    Err(Error::from(format!("import: identity field '{}' has no field.* mapping", name)))
                }
            })
            .collect::<Result<Vec<_>, _>>()?;

        // Rules: parse each LHS into AND-ed conditions (field name + value).
        let mut rules = Vec::new();
        for (lhs, acc) in raw_rules {
            let mut conds = Vec::new();
            for part in lhs.split(';') {
                let part = part.trim();
                let (fname, val) = part
                    .split_once(char::is_whitespace)
                    .ok_or_else(|| Error::from(format!("import: rule '{}' is not <field> <value>", part)))?;
                let fname = fname.trim();
                if !fields.contains_key(fname) {
                    return Err(Error::from(format!("import: rule field '{}' has no field.* mapping", fname)));
                }
                let (mode, core) = Match::parse(val.trim());
                conds.push((fname.to_string(), core.to_lowercase(), mode));
            }
            rules.push(Rule { conds, account: acc });
        }

        // Directional in-transit accounts for own↔own transfers. All of it
        // is set in the conf: `transfer.self` is this account's in-transit
        // identity (prefix + own name, e.g. assets:transit:checking),
        // `transfer.field` names the CSV field carrying the partner IBAN —
        // kept native to each bank's CSV (one may call it `counterparty`,
        // another `iban`). Both are required once a `transfer` line is
        // present (checked below).
        let transfer_field = directives.get("transfer.field").cloned();
        let (transfer_prefix, own_leaf) = match directives.get("transfer.self") {
            Some(s) => {
                let (prefix, own) = s.trim().rsplit_once(':').ok_or_else(|| {
                    Error::from(format!(
                        "import: transfer.self '{}' must be <prefix>:<name>, e.g. assets:transit:checking",
                        s.trim()
                    ))
                })?;
                (Some(prefix.to_string()), own.to_string())
            }
            None => (None, String::new()),
        };
        // No magic default: the partner-IBAN field name differs by bank
        // (one may call it `counterparty`, another `iban`), so state it.
        if !raw_transfers.is_empty() {
            if transfer_prefix.is_none() {
                return Err(Error::from(
                    "import: transfer mappings need a 'transfer.self' directive",
                ));
            }
            if transfer_field.is_none() {
                return Err(Error::from(
                    "import: transfer mappings need a 'transfer.field' directive",
                ));
            }
        }
        let transfer_field = transfer_field.unwrap_or_default();

        Ok(Profile {
            fields,
            date_pattern: directives.get("date.format").cloned(),
            output_file,
            title,
            account,
            commodity,
            precision,
            sym,
            identity,
            rules,
            default_account,
            transfers: raw_transfers,
            transfer_field,
            transfer_prefix,
            own_leaf,
        })
    }

    /// A field's value: the first non-empty of its mapped columns.
    fn field_val(&self, row: &[String], name: &str) -> String {
        self.fields
            .get(name)
            .into_iter()
            .flatten()
            .filter_map(|&i| row.get(i))
            .find(|v| !v.trim().is_empty())
            .cloned()
            .unwrap_or_default()
    }

    fn identity_key(&self, row: &[String]) -> String {
        self.identity
            .iter()
            .map(|name| self.field_val(row, name))
            .collect::<Vec<_>>()
            .join("\u{1}")
    }

    /// The counter account for a row: an own↔own transfer first (a
    /// directional in-transit account), then the first matching rule, else
    /// the slugified-partner default.
    fn categorize(&self, row: &[String]) -> String {
        if let Some(prefix) = &self.transfer_prefix {
            let cp = self.field_val(row, &self.transfer_field);
            if let Some((_, other)) = self
                .transfers
                .iter()
                .find(|(iban, _)| !iban.is_empty() && cp.contains(iban.as_str()))
            {
                let out = self.field_val(row, "amount").trim_start().starts_with('-');
                return directional_account(prefix, &self.own_leaf, other, out);
            }
        }
        for rule in &self.rules {
            let hit = rule
                .conds
                .iter()
                .all(|(field, val, mode)| mode.test(&self.field_val(row, field).to_lowercase(), val));
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
        let date = to_iso(&self.field_val(row, "date"), self.date_pattern.as_deref());
        let amount = fmt_amount(&self.field_val(row, "amount"), self.precision);
        let bank = format!("{}{}", self.commodity, amount);
        let counter = self.categorize(row);

        let csv = row
            .iter()
            .map(|f| format!("\"{}\"", f))
            .collect::<Vec<_>>()
            .join(",");

        // Raw, minimally-spaced postings — `format_source` (acc format,
        // in memory) does the column alignment, so nothing is hand-padded.
        let mut s = String::new();
        s.push_str(&format!("{} * {}\n", date, self.title));
        s.push_str(&format!("\t; csv: {}\n", csv));
        s.push_str(&format!("\t{}  {}\n", self.account, bank));

        // Foreign-currency leg: when the row converted into another
        // currency, the counter posting carries that foreign amount;
        // domestic rows leave it bare for auto-balancing.
        let fxc = self.field_val(row, "fx-currency");
        if !fxc.is_empty() && !fxc.eq_ignore_ascii_case("EUR") {
            let symbol = self
                .sym
                .get(&fxc.to_uppercase())
                .cloned()
                .unwrap_or_else(|| fxc.clone());
            // The foreign amount takes the sign opposite the bank posting
            // (money out of the account → into the expense). Some banks give
            // it unsigned, so derive the sign from the bank side, not from
            // the foreign value.
            let mag = self.field_val(row, "fx-amount");
            let mag = mag.trim_start_matches('-');
            let signed = if amount.starts_with('-') {
                mag.to_string()
            } else {
                format!("-{}", mag)
            };
            let fx = format!("{}{}", symbol, signed);
            s.push_str(&format!("\t{}  {}", counter, fx));
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
        *map.entry(profile.identity_key(&fields)).or_insert(0) += 1;
    }
    map
}

// ---------------------------------------------------------------------
// formatting helpers
// ---------------------------------------------------------------------

fn slug(s: &str) -> String {
    s.to_lowercase().replace(' ', "-")
}

/// True if the export is newest-first — its first data row is dated later
/// than its last — so it must be reversed to emit oldest-first. ISO dates
/// sort lexically, so a string compare suffices.
fn newest_first(rows: &[Vec<String>], profile: &Profile) -> bool {
    let iso = |r: &[String]| to_iso(&profile.field_val(r, "date"), profile.date_pattern.as_deref());
    match (rows.first(), rows.last()) {
        (Some(f), Some(l)) => iso(f) > iso(l),
        _ => false,
    }
}

/// Convert a date to ISO `YYYY-MM-DD`. With `pattern = None` the value is
/// assumed already ISO and returned unchanged. A pattern like `DD-MM-YYYY`
/// names the day/month/year order and the separator; anything that doesn't
/// fit the shape is returned as-is.
fn to_iso(value: &str, pattern: Option<&str>) -> String {
    let Some(pattern) = pattern else {
        return value.to_string();
    };
    let Some(sep) = pattern.chars().find(|c| !c.is_ascii_alphabetic()) else {
        return value.to_string();
    };
    let tokens: Vec<&str> = pattern.split(sep).collect();
    let parts: Vec<&str> = value.split(sep).collect();
    if tokens.len() != parts.len() {
        return value.to_string();
    }
    let (mut y, mut m, mut d) = ("", "", "");
    for (tok, part) in tokens.iter().zip(&parts) {
        match tok.chars().next().map(|c| c.to_ascii_uppercase()) {
            Some('Y') => y = part,
            Some('M') => m = part,
            Some('D') => d = part,
            _ => {}
        }
    }
    if y.is_empty() || m.is_empty() || d.is_empty() {
        return value.to_string();
    }
    format!("{:0>4}-{:0>2}-{:0>2}", y, m, d)
}

/// Build a directional in-transit account `prefix:sender:receiver`. The
/// order encodes the money flow: an outgoing row (`outgoing == true`) is
/// own → other, an incoming one other → own. Both legs of the same
/// transfer therefore produce the identical string and net to 0, and the
/// order is computed (never typed), so two profiles can't disagree.
fn directional_account(prefix: &str, own: &str, other: &str, outgoing: bool) -> String {
    let (from, to) = if outgoing { (own, other) } else { (other, own) };
    format!("{}:{}:{}", prefix, from, to)
}

/// Format a decimal string to exactly `precision` fractional digits.
/// `-3` → `-3.00`, `-64.60` → `-64.60`, `0.25` → `0.25`,
/// `-1,190.00` → `-1190.00`.
///
/// A comma is always a thousands separator here (some banks group
/// integers, e.g. `1,190.00`); the importer treats `.` as the only
/// decimal point, so commas are stripped before formatting. acc's own
/// decimal parser rejects thousand separators, so they must not survive.
fn fmt_amount(s: &str, precision: usize) -> String {
    let s = s.replace(',', "");
    let (sign, body) = match s.strip_prefix('-') {
        Some(r) => ("-", r),
        None => ("", s.as_str()),
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

/// Append the already-aligned additions to the ledger. A blank line
/// separates them from existing content, but a fresh (empty) file starts
/// straight at the first transaction — no leading blank.
fn append(path: &Path, added: &str) -> Result<(), Error> {
    use std::io::Write as _;
    let has_content = std::fs::metadata(path).map(|m| m.len() > 0).unwrap_or(false);
    let lead = if has_content { "\n" } else { "" };
    let body = format!("{}{}\n", lead, added.trim_end());
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
