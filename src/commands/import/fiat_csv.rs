//! `fiat` import source — a bank's CSV export into ledger transactions.
//!
//! Driven by a per-bank profile: column mapping, output target, an `identity`
//! for dedup, and categorization rules. Each imported row keeps its source as
//! a `; csv:` comment; re-runs skip rows already present.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::error::Error;

use super::{expand, read, slug, Match, Rule, Transit};

pub(super) fn run(csvs: &[String], conf_path: &str, write: bool) -> Result<(), Error> {
    let profile = Profile::load(conf_path)?;

    // Any number of exports — files or directories (their `*.csv`), e.g. one
    // per year. Each file is flipped to oldest-first on its own (the ledger's
    // convention, even when the bank exports newest-first), then they
    // concatenate in argument order; dedup runs over the whole set, so
    // overlapping exports still add every row exactly once.
    let mut rows: Vec<Vec<String>> = Vec::new();
    for file in &super::csv_files(csvs)? {
        let src = std::fs::read_to_string(file)
            .map_err(|e| Error::from(format!("import: read {}: {}", file.display(), e)))?;
        let mut file_rows: Vec<Vec<String>> =
            parse_csv(&src).into_iter().skip(1).filter(|r| !is_blank(r)).collect();
        if file_rows.is_empty() {
            continue;
        }
        if let Some(first) = rows.first()
            && file_rows[0].len() != first.len()
        {
            return Err(Error::from(format!(
                "import: {} has {} columns, earlier input has {} — mixed export formats",
                file.display(),
                file_rows[0].len(),
                first.len()
            )));
        }
        if newest_first(&file_rows, &profile) {
            file_rows.reverse();
        }
        rows.append(&mut file_rows);
    }
    if rows.is_empty() {
        return Err(Error::from("import: no data rows in CSV"));
    }
    // `order.funding first`: undo the processor's payment-before-cover
    // ordering, so the account never dips negative between an instant
    // funding pair booked at the same moment.
    if profile.funding_first {
        reorder_funding_first(&mut rows, &profile);
    }
    let ncols = rows[0].len();

    // What's already in the target ledger, as an identity multiset.
    let existing = std::fs::read_to_string(&profile.output_file).unwrap_or_default();
    let mut seen = existing_identities(&existing, &profile, ncols);

    // `pair.type`: collect each currency exchange's legs — rows of the pair
    // type sharing (date, reference). A group renders as one transaction at
    // its first leg's position; the other legs leave the row-by-row flow.
    let mut group_at: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut followers: std::collections::HashSet<usize> = std::collections::HashSet::new();
    if let Some(pt) = &profile.pair_type {
        let mut groups: HashMap<(String, String), Vec<usize>> = HashMap::new();
        for (i, row) in rows.iter().enumerate() {
            let reference = profile.field_val(row, "reference");
            if !reference.is_empty() && profile.field_val(row, "type").contains(pt.as_str()) {
                groups
                    .entry((profile.field_val(row, "date"), reference))
                    .or_default()
                    .push(i);
            }
        }
        for members in groups.into_values().filter(|m| m.len() >= 2) {
            followers.extend(members.iter().skip(1).copied());
            group_at.insert(members[0], members);
        }
    }

    // Generate ledger text for every row not already present.
    let mut new_blocks: Vec<String> = Vec::new();
    let mut skipped = 0usize;
    for (i, row) in rows.iter().enumerate() {
        if followers.contains(&i) {
            continue;
        }
        if let Some(members) = group_at.get(&i) {
            // An exchange group dedups as a whole: all legs present → skip
            // them all; none → book the merged transaction. A partial state
            // means an earlier import booked some legs individually — going
            // on would double them, so stop and say which group.
            let keys: Vec<String> =
                members.iter().map(|&m| profile.identity_key(&rows[m])).collect();
            let present =
                keys.iter().filter(|k| seen.get(*k).is_some_and(|c| *c > 0)).count();
            if present == members.len() {
                for k in &keys {
                    *seen.get_mut(k).unwrap() -= 1;
                }
                skipped += members.len();
            } else if present > 0 {
                return Err(Error::from(format!(
                    "import: exchange group of {} (reference {}) is partially imported — \
                     remove the partial booking, then re-run",
                    profile.field_val(row, "date"),
                    profile.field_val(row, "reference"),
                )));
            } else {
                let legs: Vec<&Vec<String>> = members.iter().map(|&m| &rows[m]).collect();
                new_blocks.push(profile.render_exchange(&legs)?);
            }
            continue;
        }
        // A rule whose target is the reserved word `skip` drops the row
        // entirely — for statement lines that move no money (authorization
        // holds and their reversals, pending echoes of later bookings).
        // Checked before dedup so a dropped row never consumes an identity.
        if profile.categorize(row) == "skip" {
            skipped += 1;
            continue;
        }
        let key = profile.identity_key(row);
        if let Some(c) = seen.get_mut(&key)
            && *c > 0
        {
            *c -= 1;
            skipped += 1;
            continue;
        }
        new_blocks.push(profile.render_transaction(row)?);
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
    /// `amount.decimal comma`: the CSV writes `1.234,56` (comma decimals,
    /// dot thousands). Default is dot decimals with comma thousands.
    comma_decimal: bool,
    /// `order.funding first`: a row whose `reference` names the txid of an
    /// earlier same-day row is that row's funding leg (an instant bank
    /// deposit covering the payment it pairs with) — emit it before the
    /// row it references. Requires `field.txid` and `field.reference`.
    funding_first: bool,
    /// `commodity.field NAME`: a multi-currency account — each row's booked
    /// commodity comes from this column (code mapped through the commodities
    /// file, unknown codes verbatim), falling back to `output.commodity`
    /// when the cell is empty.
    commodity_field: Option<String>,
    /// `pair.type SUBSTRING`: rows of this type are a currency exchange's
    /// legs — one row per commodity, all referencing the transaction they
    /// fund. Legs sharing (date, reference) merge into ONE transaction whose
    /// postings all sit on the bank account and balance as a trade.
    /// Requires `field.reference`.
    pair_type: Option<String>,
    output_file: PathBuf,
    title: String,
    account: String,   // bank-side account
    /// `output.commodity` — the booked symbol; with `commodity.field` it is
    /// only the fallback for rows whose currency cell is empty, and may be
    /// omitted entirely (an empty cell is then an error).
    commodity: Option<String>,
    precision: usize, // fallback decimals when a symbol isn't in `precisions`
    sym: HashMap<String, String>, // currency code (upper) -> symbol
    precisions: HashMap<String, usize>, // symbol -> decimals (commodities file)
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
                // `transit <counterparty-key> <other account leaf>`. Split at
                // the LAST whitespace: the leaf is a single account segment,
                // while the key may carry spaces (a partner's display name —
                // IBANs and addresses never do, so they parse the same). Note
                // `transit.prefix` keeps its dot, so it does not match here
                // and falls through to the directive branch below.
                let rest = rest.trim();
                let (iban, name) = rest.rsplit_once(char::is_whitespace).ok_or_else(|| {
                    Error::from(format!("import: transit '{}' is not <key> <account>", rest))
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

        let commodity_field = match directives.get("commodity.field") {
            None => None,
            Some(name) => {
                if !fields.contains_key(name) {
                    return Err(Error::from(format!(
                        "import: commodity.field '{}' has no field.* mapping",
                        name
                    )));
                }
                Some(name.clone())
            }
        };
        let pair_type = match directives.get("pair.type") {
            None => None,
            Some(t) => {
                if !fields.contains_key("reference") || !fields.contains_key("type") {
                    return Err(Error::from(
                        "import: pair.type needs field.reference and field.type mappings",
                    ));
                }
                Some(t.clone())
            }
        };
        let funding_first = match directives.get("order.funding").map(String::as_str) {
            None => false,
            Some("first") => {
                if !fields.contains_key("txid") || !fields.contains_key("reference") {
                    return Err(Error::from(
                        "import: order.funding needs field.txid and field.reference mappings",
                    ));
                }
                true
            }
            Some(other) => {
                return Err(Error::from(format!(
                    "import: order.funding = '{}' (expected 'first')",
                    other
                )));
            }
        };

        let get = |key: &str| -> Result<String, Error> {
            directives
                .get(key)
                .cloned()
                .ok_or_else(|| Error::from(format!("import: missing '{}' in profile", key)))
        };

        let output_file = expand(&get("output.file")?);
        let title = get("output.title")?;
        let account = get("output.account")?;
        // With a per-row commodity column the profile-wide symbol is only a
        // fallback for empty cells and may be left out.
        let commodity = match directives.get("output.commodity") {
            Some(c) => Some(c.clone()),
            None if commodity_field.is_some() => None,
            None => return Err(Error::from("import: missing 'output.commodity' in profile")),
        };

        // Symbols + per-commodity precision come from the commodities file.
        let (sym, precisions) = match directives.get("commodities") {
            Some(p) => load_commodities(&expand(p))?,
            None => (HashMap::new(), HashMap::new()),
        };
        let precision =
            commodity.as_deref().and_then(|c| precisions.get(c)).copied().unwrap_or(2);

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
            comma_decimal: match directives.get("amount.decimal").map(String::as_str) {
                None | Some("dot") => false,
                Some("comma") => true,
                Some(other) => {
                    return Err(Error::from(format!(
                        "import: amount.decimal = '{}' (expected 'comma' or 'dot')",
                        other
                    )));
                }
            },
            funding_first,
            commodity_field,
            pair_type,
            precisions,
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

    /// The commodity symbol a row books in: the `commodity.field` column's
    /// code mapped through the commodities file (unknown codes verbatim),
    /// else the profile-wide `output.commodity`. A row with neither is an
    /// error — silently guessing a currency would corrupt the ledger.
    fn row_commodity(&self, row: &[String]) -> Result<String, Error> {
        let code = self
            .commodity_field
            .as_deref()
            .map(|f| self.field_val(row, f))
            .unwrap_or_default();
        if code.is_empty() {
            self.commodity.clone().ok_or_else(|| {
                Error::from(format!(
                    "import: row of {} has an empty currency cell and the profile \
                     sets no output.commodity fallback",
                    self.field_val(row, "date"),
                ))
            })
        } else {
            Ok(self.sym.get(&code.to_uppercase()).cloned().unwrap_or(code))
        }
    }

    /// Decimals for a symbol: its commodities-file declaration, else the
    /// profile fallback.
    fn precision_of(&self, symbol: &str) -> usize {
        self.precisions.get(symbol).copied().unwrap_or(self.precision)
    }

    /// Render a currency exchange's legs (`pair.type` rows sharing one
    /// (date, reference)) as ONE transaction: every leg books its signed
    /// amount, in its own commodity, on the bank account — the legs balance
    /// each other as a trade, no counter account. Each keeps its own
    /// `; csv:` source line, so dedup still sees every row.
    fn render_exchange(&self, legs: &[&Vec<String>]) -> Result<String, Error> {
        let date = to_iso(&self.field_val(legs[0], "date"), self.date_pattern.as_deref());
        let mut s = String::new();
        s.push_str(&format!("{} * {}\n", date, self.title));
        for row in legs {
            let csv =
                row.iter().map(|f| format!("\"{}\"", f)).collect::<Vec<_>>().join(",");
            s.push_str(&format!("\t; csv: {}\n", csv));
            let symbol = self.row_commodity(row)?;
            let amount = fmt_amount(
                &self.field_val(row, "amount"),
                self.precision_of(&symbol),
                self.comma_decimal,
            );
            s.push_str(&format!("\t{}  {}{}\n", self.account, symbol, amount));
        }
        Ok(s.trim_end().to_string())
    }

    /// Render one row as a ledger transaction block (no trailing newline).
    fn render_transaction(&self, row: &[String]) -> Result<String, Error> {
        let date = to_iso(&self.field_val(row, "date"), self.date_pattern.as_deref());
        let symbol = self.row_commodity(row)?;
        let amount = fmt_amount(
            &self.field_val(row, "amount"),
            self.precision_of(&symbol),
            self.comma_decimal,
        );
        let bank = format!("{}{}", symbol, amount);
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
            let mag = normalize_decimal(&self.field_val(row, "fx-amount"), self.comma_decimal);
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
        Ok(s)
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

/// Move every funding leg directly before the same-day row its `reference`
/// names. Processors list an instant funding deposit AFTER the payment it
/// covers (their books dip negative for a moment); the ledger reads better
/// with the cover first. Rows whose reference names nothing in the set
/// (billing-agreement codes, cross-day settlements) stay where they are.
fn reorder_funding_first(rows: &mut Vec<Vec<String>>, profile: &Profile) {
    let mut i = 0;
    while i < rows.len() {
        let reference = profile.field_val(&rows[i], "reference");
        if !reference.is_empty() {
            let date = profile.field_val(&rows[i], "date");
            let referenced = rows[..i].iter().position(|r| {
                profile.field_val(r, "txid") == reference && profile.field_val(r, "date") == date
            });
            if let Some(j) = referenced {
                let row = rows.remove(i);
                rows.insert(j, row);
            }
        }
        i += 1;
    }
}

/// Normalize a raw CSV amount to dot-decimal, no grouping. With
/// `comma_decimal` the source writes `1.234,56` (EU banks) — dots are
/// grouping and the comma is the decimal point; otherwise commas are
/// grouping (`1,190.00`) and the dot is the decimal point. acc's own
/// decimal parser rejects thousand separators, so they must not survive.
fn normalize_decimal(s: &str, comma_decimal: bool) -> String {
    if comma_decimal {
        s.replace('.', "").replace(',', ".")
    } else {
        s.replace(',', "")
    }
}

/// Format a decimal string to exactly `precision` fractional digits.
/// `-3` → `-3.00`, `-64.60` → `-64.60`, `0.25` → `0.25`,
/// `-1,190.00` → `-1190.00`; with `comma_decimal`, `-6,58` → `-6.58`.
fn fmt_amount(s: &str, precision: usize, comma_decimal: bool) -> String {
    let s = normalize_decimal(s, comma_decimal);
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

/// Parse a `commodities.toml`: return (code→symbol, symbol→precision). Each
/// `[[commodity]]`'s symbol maps to itself and every alias folds to it — keys
/// upper-cased, so a source code like `EUR` resolves regardless of case.
#[allow(clippy::type_complexity)]
fn load_commodities(
    path: &Path,
) -> Result<(HashMap<String, String>, HashMap<String, usize>), Error> {
    let src = std::fs::read_to_string(path)
        .map_err(|e| Error::from(format!("import: read {}: {}", path.display(), e)))?;
    let commodities = crate::loader::config::commodities_from_toml(&src)
        .map_err(|e| Error::from(format!("import: parse {}: {}", path.display(), e)))?;
    let mut sym = HashMap::new();
    let mut precisions = HashMap::new();
    for c in commodities {
        sym.insert(c.symbol.to_uppercase(), c.symbol.clone());
        for a in &c.aliases {
            sym.insert(a.to_uppercase(), c.symbol.clone());
        }
        precisions.insert(c.symbol.clone(), c.precision);
    }
    Ok((sym, precisions))
}

/// Minimal RFC-4180 parser: handles quoted fields with embedded commas,
/// doubled `""` escapes, and `\r\n`. Returns every record (no header skip).
/// Shared with `crypto_csv` (both parse exchange/bank CSV exports).
pub(super) fn parse_csv(src: &str) -> Vec<Vec<String>> {
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
pub(super) fn parse_record(line: &str) -> Vec<String> {
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
        assert_eq!(fmt_amount("-3", 2, false), "-3.00");
        assert_eq!(fmt_amount("-64.60", 2, false), "-64.60");
        assert_eq!(fmt_amount("0.25", 2, false), "0.25");
        assert_eq!(fmt_amount("2407.5", 2, false), "2407.50");
        assert_eq!(fmt_amount("100", 0, false), "100");
        // Thousands separators (some banks group integers) are stripped — acc's
        // decimal parser rejects them, so they must never reach the ledger.
        assert_eq!(fmt_amount("1,190.00", 2, false), "1190.00");
        assert_eq!(fmt_amount("-1,190.00", 2, false), "-1190.00");
        assert_eq!(fmt_amount("1,234,567.8", 2, false), "1234567.80");
    }

    #[test]
    fn fmt_amount_comma_decimals() {
        // `amount.decimal comma`: dot groups, comma is the decimal point.
        assert_eq!(fmt_amount("-6,58", 2, true), "-6.58");
        assert_eq!(fmt_amount("250,00", 2, true), "250.00");
        assert_eq!(fmt_amount("1.234,56", 2, true), "1234.56");
        assert_eq!(fmt_amount("-1.234.567,8", 2, true), "-1234567.80");
        assert_eq!(fmt_amount("100", 2, true), "100.00");
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
        let com = write(&dir, "com.toml", "[[commodity]]\nsymbol = \"€\"\nprecision = 2\n");
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
        let com = write(dir, "com.toml", "[[commodity]]\nsymbol = \"€\"\naliases = [\"EUR\"]\nprecision = 2\n");
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
        let com = write(&dir, "com.toml", "[[commodity]]\nsymbol = \"€\"\nprecision = 2\n");
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
        let com = write(&dir, "com.toml", "[[commodity]]\nsymbol = \"€\"\nprecision = 2\n");
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
        let block = p.render_transaction(&row("2025-11-01", "Foo Shop", "-12.5", "EUR")).unwrap();
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

    #[test]
    fn skip_rule_target_drops_the_row() {
        let dir = std::env::temp_dir().join(format!("acc-import-skip-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let com = write(&dir, "com.toml", "[[commodity]]\nsymbol = \"$\"\nprecision = 2\n");
        let conf = write(
            &dir,
            "bank.conf",
            &format!(
                "field.date 0\nfield.type 1\nfield.payee 2\nfield.amount 3\n\
                 commodities {}\noutput.file /tmp/x.ledger\noutput.title t\n\
                 output.account a:bank\noutput.commodity $\nidentity date amount\n\
                 type Authorization => skip\n\
                 default => exp:{{payee}}\n",
                com.display()
            ),
        );
        let p = Profile::load(conf.to_str().unwrap()).unwrap();
        let mk = |kind: &str| -> Vec<String> {
            vec!["2025-01-01", kind, "Foo Shop", "-5.00"].into_iter().map(String::from).collect()
        };
        // The matching row resolves to the reserved `skip` target; a
        // non-matching row books normally.
        assert_eq!(p.categorize(&mk("General Authorization")), "skip");
        assert_eq!(p.categorize(&mk("Payment")), "exp:foo-shop");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn transit_key_may_carry_spaces() {
        let dir = std::env::temp_dir().join(format!("acc-import-transit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let com = write(&dir, "com.toml", "[[commodity]]\nsymbol = \"$\"\nprecision = 2\n");
        let conf = write(
            &dir,
            "bank.conf",
            &format!(
                "field.date 0\nfield.type 1\nfield.amount 2\n\
                 commodities {}\noutput.file /tmp/x.ledger\noutput.title t\n\
                 output.account a:bank\noutput.commodity $\nidentity date amount\n\
                 transit.field type\ntransit.self a:transit:main\n\
                 transit Bank Deposit other\n\
                 default => exp:misc\n",
                com.display()
            ),
        );
        let p = Profile::load(conf.to_str().unwrap()).unwrap();
        let mk = |kind: &str, amount: &str| -> Vec<String> {
            vec!["2025-01-01", kind, amount].into_iter().map(String::from).collect()
        };
        // The multi-word key splits at the LAST whitespace, so the leaf is
        // `other` and the key `Bank Deposit`; direction follows the sign.
        assert_eq!(p.categorize(&mk("Bank Deposit to Account", "5.00")), "a:transit:other:main");
        assert_eq!(p.categorize(&mk("Payment", "-5.00")), "exp:misc");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn exchange_legs_render_as_one_multi_commodity_transaction() {
        let dir = std::env::temp_dir().join(format!("acc-import-pair-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let com = write(
            &dir,
            "com.toml",
            "[[commodity]]\nsymbol = \"$\"\naliases = [\"USD\"]\nprecision = 2\n\
             [[commodity]]\nsymbol = \"£\"\naliases = [\"GBP\"]\nprecision = 2\n",
        );
        let conf = write(
            &dir,
            "bank.conf",
            &format!(
                "field.date 0\nfield.type 1\nfield.currency 2\nfield.amount 3\n\
                 field.txid 4\nfield.reference 5\nfield.payee 6\n\
                 commodities {}\noutput.file /tmp/x.ledger\noutput.title t\n\
                 output.account a:bank\noutput.commodity $\nidentity txid\n\
                 commodity.field currency\npair.type Conversion\n\
                 default => exp:{{payee}}\n",
                com.display()
            ),
        );
        let p = Profile::load(conf.to_str().unwrap()).unwrap();
        let usd: Vec<String> = vec!["2025-01-01", "Conversion", "USD", "-11.00", "T2", "T1", ""]
            .into_iter().map(String::from).collect();
        let gbp: Vec<String> = vec!["2025-01-01", "Conversion", "GBP", "8.00", "T3", "T1", ""]
            .into_iter().map(String::from).collect();
        let block = p.render_exchange(&[&usd, &gbp]).unwrap();
        // One transaction, both legs on the bank account in their own
        // commodity, each with its own ; csv: source line.
        assert_eq!(block.matches("; csv:").count(), 2);
        assert!(block.contains("a:bank  $-11.00"));
        assert!(block.contains("a:bank  £8.00"));
        assert_eq!(block.matches("2025-01-01 * t").count(), 1);
        // Per-row commodity also drives ordinary rows.
        let single: Vec<String> = vec!["2025-01-02", "Payment", "GBP", "-5.00", "T9", "", "Foo"]
            .into_iter().map(String::from).collect();
        assert!(p.render_transaction(&single).unwrap().contains("a:bank  £-5.00"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn funding_first_moves_the_cover_before_its_payment() {
        let dir = std::env::temp_dir().join(format!("acc-import-fund-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let com = write(&dir, "com.toml", "[[commodity]]\nsymbol = \"$\"\nprecision = 2\n");
        let conf = write(
            &dir,
            "bank.conf",
            &format!(
                "field.date 0\nfield.payee 1\nfield.amount 2\nfield.txid 3\nfield.reference 4\n\
                 commodities {}\noutput.file /tmp/x.ledger\noutput.title t\n\
                 output.account a:bank\noutput.commodity $\nidentity txid\n\
                 order.funding first\n\
                 default => exp:{{payee}}\n",
                com.display()
            ),
        );
        let p = Profile::load(conf.to_str().unwrap()).unwrap();
        let mk = |date: &str, amount: &str, txid: &str, reference: &str| -> Vec<String> {
            vec![date, "Foo", amount, txid, reference].into_iter().map(String::from).collect()
        };
        let mut rows = vec![
            mk("2025-01-01", "-5.00", "TX1", ""),
            mk("2025-01-01", "5.00", "TX2", "TX1"),   // same-day cover → moves before TX1
            mk("2025-01-02", "-9.00", "TX3", "B-77"), // names nothing → stays
            mk("2025-01-03", "9.00", "TX4", "TX3"),   // cross-day → stays
        ];
        reorder_funding_first(&mut rows, &p);
        let order: Vec<&str> = rows.iter().map(|r| r[3].as_str()).collect();
        assert_eq!(order, ["TX2", "TX1", "TX3", "TX4"]);
    }
}
