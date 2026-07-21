//! `fiat` import source — a bank's CSV export into ledger transactions.
//!
//! Driven by a per-bank profile: column mapping, output target, an `identity`
//! for dedup, and categorization rules. Each imported row keeps its source as
//! a `; csv:` comment; re-runs skip rows already present.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::error::Error;

use super::{expand, read, slug, Match, Rule, Transit};

pub(super) fn run(csv_path: &str, conf_path: &str, write: bool) -> Result<(), Error> {
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

    super::emit(&new_blocks, rows.len(), "rows", &existing, &profile.output_file, skipped, write)
}

// ---------------------------------------------------------------------
// profile
// ---------------------------------------------------------------------


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
    /// Own↔own transits (shared `Transit`): each entry maps a partner-IBAN
    /// substring to the other account's leaf; a match books the counter to a
    /// directional in-transit account, ordered by the money flow (amount
    /// sign), so both legs net to 0. `transit_field` names the CSV column that
    /// carries the partner IBAN.
    transit: Transit,
    transit_field: String,
}

impl Profile {
    fn load(path: &str) -> Result<Profile, Error> {
        let src = read(path)?;
        let mut directives: HashMap<String, String> = HashMap::new();
        let mut raw_rules: Vec<(String, String)> = Vec::new(); // (lhs, account)
        let mut raw_transits: Vec<(String, String)> = Vec::new(); // (iban, other leaf)
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
            } else if let Some(rest) = line.strip_prefix("transit ") {
                // `transit <counterparty-iban> <other account leaf>`. Note
                // `transit.prefix` keeps its dot, so it does not match here
                // and falls through to the directive branch below.
                let rest = rest.trim();
                let (iban, name) = rest.split_once(char::is_whitespace).ok_or_else(|| {
                    Error::from(format!("import: transit '{}' is not <iban> <account>", rest))
                })?;
                raw_transits.push((iban.trim().to_string(), name.trim().to_string()));
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

        // Own↔own transits (shared parser). fiat additionally needs a
        // `transit.field`: the CSV column carrying the partner IBAN differs by
        // bank (one calls it `counterparty`, another `iban`), so require it
        // whenever transit is used.
        let transit_field = directives.get("transit.field").cloned();
        let transit = Transit::parse(&directives, raw_transits)?;
        if !transit.entries.is_empty() && transit_field.is_none() {
            return Err(Error::from(
                "import: transit mappings need a 'transit.field' directive",
            ));
        }
        let transit_field = transit_field.unwrap_or_default();

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
            transit,
            transit_field,
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

    /// The counter account for a row: an own↔own transit first (a
    /// directional in-transit account), then the first matching rule, else
    /// the slugified-partner default.
    fn categorize(&self, row: &[String]) -> String {
        let cp = self.field_val(row, &self.transit_field);
        if let Some((_, other)) = self
            .transit
            .entries
            .iter()
            .find(|(iban, _)| !iban.is_empty() && cp.contains(iban.as_str()))
        {
            let out = self.field_val(row, "amount").trim_start().starts_with('-');
            if let Some(acct) = self.transit.account(other, out) {
                return acct;
            }
        }
        let tmpl = super::match_account(&self.rules, |f| self.field_val(row, f))
            .unwrap_or(self.default_account.as_str());
        self.apply_template(tmpl, row)
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


#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn fmt_amount_pads_to_precision() {
        assert_eq!(fmt_amount("-3", 2), "-3.00");
        assert_eq!(fmt_amount("-64.60", 2), "-64.60");
        assert_eq!(fmt_amount("0.25", 2), "0.25");
        assert_eq!(fmt_amount("2407.5", 2), "2407.50");
        assert_eq!(fmt_amount("100", 0), "100");
        // Thousands separators (some banks group integers) are stripped — acc's
        // decimal parser rejects them, so they must never reach the ledger.
        assert_eq!(fmt_amount("1,190.00", 2), "1190.00");
        assert_eq!(fmt_amount("-1,190.00", 2), "-1190.00");
        assert_eq!(fmt_amount("1,234,567.8", 2), "1234567.80");
    }
    
    #[test]
    fn to_iso_converts_dmy_and_passes_iso() {
        assert_eq!(to_iso("28-06-2026", Some("DD-MM-YYYY")), "2026-06-28");
        assert_eq!(to_iso("1-2-2026", Some("DD-MM-YYYY")), "2026-02-01"); // zero-padded
        assert_eq!(to_iso("2026-06-28", None), "2026-06-28"); // already ISO, passthrough
        assert_eq!(to_iso("garbage", Some("DD-MM-YYYY")), "garbage"); // shape mismatch left as-is
    }
    
    #[test]
    fn field_val_takes_first_non_empty_column() {
        let dir = std::env::temp_dir().join(format!("acc-import-multi-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let com = write(&dir, "com.ledger", "commodity €\n    precision 2\n");
        let conf = write(
            &dir,
            "bank.conf",
            &format!(
                "field.date 0\nfield.amount 1\nfield.payee 3 2\n\
                 commodities {}\noutput.file /tmp/x.ledger\noutput.title t\n\
                 output.account a:b\noutput.commodity €\nidentity date\ndefault => exp:{{payee}}\n",
                com.display()
            ),
        );
        let p = Profile::load(conf.to_str().unwrap()).unwrap();
        // field.payee 3 2 → column 3 first, then column 2.
        let mk = |c2: &str, c3: &str| -> Vec<String> {
            vec!["2025-01-01", "-1", c2, c3].into_iter().map(String::from).collect()
        };
        assert_eq!(p.field_val(&mk("foo", ""), "payee"), "foo"); // col 3 empty → col 2
        assert_eq!(p.field_val(&mk("foo", "bar"), "payee"), "bar"); // col 3 wins
        std::fs::remove_dir_all(&dir).ok();
    }
    
    #[test]
    fn slug_lowercases_and_dashes() {
        assert_eq!(slug("Foo Bar & Baz"), "foo-bar-&-baz");
    }
    
    #[test]
    fn parse_csv_handles_quoted_commas() {
        let rows = parse_csv("a,\"x, y\",c\n1,2,3\n");
        assert_eq!(rows[0], vec!["a", "x, y", "c"]);
        assert_eq!(rows[1], vec!["1", "2", "3"]);
    }
    
    fn write(dir: &std::path::Path, name: &str, content: &str) -> std::path::PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, content).unwrap();
        p
    }
    
    /// Column order mirrors a typical bank layout (date,_,payee,_,_,ref,_,amount,_,fxcur,_).
    fn row(date: &str, payee: &str, amount: &str, fxcur: &str) -> Vec<String> {
        vec![date, "", payee, "", "", "ref", "", amount, "", fxcur, ""]
            .into_iter()
            .map(String::from)
            .collect()
    }
    
    fn test_profile(dir: &std::path::Path) -> Profile {
        let com = write(dir, "com.ledger", "commodity €\n    alias EUR\n    precision 2\n");
        let conf = write(
            dir,
            "bank.conf",
            &format!(
                "field.date 0\nfield.payee 2\nfield.reference 5\nfield.amount 7\nfield.fx-currency 9\n\
                 commodities {}\noutput.file /tmp/x.ledger\noutput.title bank | me\n\
                 output.account a:bank\noutput.commodity €\n\
                 identity date amount payee\n\
                 default => exp:{{payee}}\n\
                 payee foo => exp:foo\n",
                com.display()
            ),
        );
        Profile::load(conf.to_str().unwrap()).unwrap()
    }
    
    #[test]
    fn rule_then_slug_default() {
        let dir = std::env::temp_dir().join(format!("acc-import-cat-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = test_profile(&dir);
        assert_eq!(p.precision, 2);
        assert_eq!(p.categorize(&row("2025-11-01", "Foo Shop", "-12.5", "EUR")), "exp:foo");
        assert_eq!(p.categorize(&row("2025-11-01", "Bar Baz", "-1", "")), "exp:bar-baz");
        std::fs::remove_dir_all(&dir).ok();
    }
    
    #[test]
    fn semicolon_conditions_are_anded() {
        let dir = std::env::temp_dir().join(format!("acc-import-and-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let com = write(&dir, "com.ledger", "commodity €\n    precision 2\n");
        let conf = write(
            &dir,
            "bank.conf",
            &format!(
                "field.date 0\nfield.payee 2\nfield.type 4\nfield.reference 5\nfield.amount 7\nfield.fx-currency 9\n\
                 commodities {}\noutput.file /tmp/x.ledger\noutput.title t | t\n\
                 output.account a:bank\noutput.commodity €\n\
                 identity date amount payee\n\
                 default => exp:{{payee}}\n\
                 payee foo; type bar => special:foobar\n",
                com.display()
            ),
        );
        let p = Profile::load(conf.to_str().unwrap()).unwrap();
        // columns: 0 date, 2 payee, 4 type, 5 ref, 7 amount, 9 fxcur
        let mk = |payee: &str, ty: &str| -> Vec<String> {
            vec!["2025-01-01", "", payee, "", ty, "ref", "", "-1", "", "", ""]
                .into_iter()
                .map(String::from)
                .collect()
        };
        // both conditions hold (AND) → the special account
        assert_eq!(p.categorize(&mk("Foo Inc", "bar type")), "special:foobar");
        // only one holds → falls through to the slug default
        assert_eq!(p.categorize(&mk("Foo Inc", "other")), "exp:foo-inc");
        std::fs::remove_dir_all(&dir).ok();
    }
    
    #[test]
    fn rule_value_anchors() {
        let dir = std::env::temp_dir().join(format!("acc-import-anchor-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let com = write(&dir, "com.ledger", "commodity €\n    precision 2\n");
        let conf = write(
            &dir,
            "bank.conf",
            &format!(
                "field.date 0\nfield.payee 2\nfield.reference 5\nfield.amount 7\nfield.fx-currency 9\n\
                 commodities {}\noutput.file /tmp/x.ledger\noutput.title t | t\n\
                 output.account a:bank\noutput.commodity €\n\
                 identity date amount payee\n\
                 default => exp:{{payee}}\n\
                 payee ^foo => exp:starts\n\
                 payee bar$ => exp:ends\n\
                 payee ^exact$ => exp:whole\n",
                com.display()
            ),
        );
        let p = Profile::load(conf.to_str().unwrap()).unwrap();
        // `^foo` — start anchor (case-insensitive), not a mid-string match.
        assert_eq!(p.categorize(&row("2025-01-01", "Foo Shop", "-1", "")), "exp:starts");
        assert_eq!(p.categorize(&row("2025-01-01", "A Foo", "-1", "")), "exp:a-foo");
        // `bar$` — end anchor.
        assert_eq!(p.categorize(&row("2025-01-01", "The Bar", "-1", "")), "exp:ends");
        assert_eq!(p.categorize(&row("2025-01-01", "Bar None", "-1", "")), "exp:bar-none");
        // `^exact$` — whole field only.
        assert_eq!(p.categorize(&row("2025-01-01", "Exact", "-1", "")), "exp:whole");
        assert_eq!(p.categorize(&row("2025-01-01", "Exactly", "-1", "")), "exp:exactly");
        std::fs::remove_dir_all(&dir).ok();
    }
    
    #[test]
    fn render_uses_symbol_precision_and_bare_counter() {
        let dir = std::env::temp_dir().join(format!("acc-import-render-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = test_profile(&dir);
        let block = p.render_transaction(&row("2025-11-01", "Foo Shop", "-12.5", "EUR"));
        assert!(block.contains("2025-11-01 * bank | me"));
        assert!(block.contains("; csv:"));
        assert!(block.contains("€-12.50")); // padded to precision
        assert!(block.contains("a:bank"));
        assert!(block.contains("exp:foo"));
        // domestic row → counter posting is bare (no amount)
        assert!(block.trim_end().ends_with("exp:foo"));
        std::fs::remove_dir_all(&dir).ok();
    }
    
    #[test]
    fn dedup_skips_rows_already_in_ledger() {
        let dir = std::env::temp_dir().join(format!("acc-import-dedup-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = test_profile(&dir);
        let dup = row("2025-10-01", "Foo Shop", "-12.5", "EUR");
        // An existing entry carrying that exact row in its ; csv: comment.
        let existing = format!("2025-10-01 * bank | me\n\t; csv: {}\n\ta:bank\t€-12.50\n\texp:foo\n",
            dup.iter().map(|f| format!("\"{}\"", f)).collect::<Vec<_>>().join(","));
        let seen = existing_identities(&existing, &p, 11);
        assert!(seen.contains_key(&p.identity_key(&dup)));
        // A different row is not present.
        let fresh = row("2025-11-05", "Bar Baz", "-9.99", "");
        assert!(!seen.contains_key(&p.identity_key(&fresh)));
        std::fs::remove_dir_all(&dir).ok();
    }
}
