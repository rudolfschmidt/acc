//! `crypto` (crypto.com) import from its "journals" statement CSVs.
//!
//! crypto.com is a multi-asset CASH ACCOUNT like kraken: USD is deposited,
//! traded into crypto (LTC), withdrawn onward, and crypto (BTC) is deposited
//! and withdrawn — the account holds several commodities at once. The statement
//! rows carry a `journal_type`:
//!   * `FIAT_DEPOSIT`                 — fiat onto the account
//!   * `TRADING` (two rows, one per   — an in-account conversion, the crypto leg
//!      instrument, same `trade_id`)    booked `@@` the fiat magnitude
//!   * `TRADE_FEE` (same `trade_id`)  — the trade's fee, in its own commodity
//!   * `ONCHAIN_DEPOSIT` / `_WITHDRAWAL` — crypto moving in / out on-chain
//!
//! Columns are addressed BY HEADER NAME, so the two export layouts (one adds
//! leading `isolation_id`,`isolation_type` columns) both parse. Each row is kept
//! verbatim as a `; csv:` comment; a re-run dedups by `journal_id` / `trade_id`.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::error::Error;

use super::exchange_lib::{is_zero, load_aliases, mag};
use super::fiat_csv::parse_record;
use super::{expand, match_account, read, Match, Rule};

/// The row fields a categorization rule may match on.
const FIELDS: &[&str] = &["type", "asset", "side"];

/// One normalized statement row.
struct Row {
    time: String,       // "YYYY-MM-DD HH:MM:SS …" (sorts chronologically)
    asset: String,      // clean code as crypto.com writes it: USD, LTC, BTC
    journal_id: String, // unique per row — the single-movement dedup key
    kind: String,       // FIAT_DEPOSIT | TRADING | TRADE_FEE | ONCHAIN_DEPOSIT | ONCHAIN_WITHDRAWAL
    side: String,       // BUY | SELL | "" (only TRADING carries it)
    qty: String,        // signed decimal string (transaction_qty)
    trade_id: String,   // groups a trade's legs; "" for non-trade rows
    raw: String,        // the verbatim CSV line, for the `; csv:` comment
}

impl Row {
    fn date(&self) -> &str {
        self.time.get(..10).unwrap_or(&self.time)
    }
    fn field(&self, name: &str) -> String {
        match name {
            "type" => self.kind.clone(),
            "asset" => self.asset.clone(),
            "side" => self.side.clone(),
            _ => String::new(),
        }
    }
    fn is_trade_part(&self) -> bool {
        matches!(self.kind.as_str(), "TRADING" | "TRADE_FEE")
    }
}

// ---------------------------------------------------------------------
// entry point
// ---------------------------------------------------------------------

pub(super) fn run(csvs: &[String], conf_path: &str, write: bool) -> Result<(), Error> {
    let profile = Profile::load(conf_path)?;
    let rows = read_rows(csvs)?;
    // Drop exact-duplicate rows by journal_id before anything else: the same
    // file listed twice, or a directory plus a file already inside it, would
    // otherwise book the movement twice (and double a trade's fee legs).
    let mut seen_ids = HashSet::new();
    let rows: Vec<Row> = rows
        .into_iter()
        .filter(|r| seen_ids.insert(r.journal_id.clone()))
        .collect();
    if rows.is_empty() {
        return Err(Error::from("import: no data rows in crypto statements"));
    }
    let total = rows.len();

    let existing = std::fs::read_to_string(&profile.output_file).unwrap_or_default();

    // Group a trade's legs by trade_id; every other row is its own booking.
    let mut trades: HashMap<String, Vec<Row>> = HashMap::new();
    let mut singles: Vec<Row> = Vec::new();
    for r in rows {
        if r.is_trade_part() {
            trades.entry(r.trade_id.clone()).or_default().push(r);
        } else {
            singles.push(r);
        }
    }

    // A booking is already present when its identifying id (trade_id for a
    // trade, journal_id for a single) is in the ledger — the ids are unique
    // 19-digit numbers, so a substring test never false-matches.
    let mut dated: Vec<(String, String)> = Vec::new(); // (time, block)
    let mut skipped = 0usize;

    for (trade_id, legs) in trades {
        if existing.contains(&trade_id) {
            skipped += legs.len();
            continue;
        }
        let (time, block) = profile.render_trade(&trade_id, &legs)?;
        dated.push((time, block));
    }
    for r in singles {
        if existing.contains(&r.journal_id) {
            skipped += 1;
            continue;
        }
        dated.push((r.time.clone(), profile.render_single(&r)));
    }

    dated.sort_by(|a, b| a.0.cmp(&b.0));
    let blocks: Vec<String> = dated.into_iter().map(|(_, b)| b).collect();
    super::emit(&blocks, total, "rows", &existing, &profile.output_file, skipped, write)
}

/// Read the given CSV inputs — each one a file or a directory (its `*.csv`) —
/// into normalized rows. crypto.com splits its statements across several files,
/// so any number can be listed in any order. Columns are located by header name
/// so both export layouts parse.
fn read_rows(csvs: &[String]) -> Result<Vec<Row>, Error> {
    let mut files: Vec<PathBuf> = Vec::new();
    for csv in csvs {
        let path = expand(csv);
        if path.is_dir() {
            let mut entries: Vec<PathBuf> = std::fs::read_dir(&path)
                .map_err(|e| Error::from(format!("import: read dir {}: {}", path.display(), e)))?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|x| x == "csv"))
                .collect();
            entries.sort();
            files.extend(entries);
        } else {
            files.push(path);
        }
    }

    let mut rows = Vec::new();
    for file in &files {
        let src = std::fs::read_to_string(file)
            .map_err(|e| Error::from(format!("import: read {}: {}", file.display(), e)))?;
        rows.extend(rows_from_csv(&src)?);
    }
    Ok(rows)
}

/// Parse one statement CSV: map header names → column indices, then build a
/// `Row` per data line (its verbatim source kept for the `; csv:` comment).
fn rows_from_csv(src: &str) -> Result<Vec<Row>, Error> {
    let mut lines = src.lines();
    let Some(header_line) = lines.find(|l| !l.trim().is_empty()) else {
        return Ok(Vec::new());
    };
    let header = parse_record(header_line);
    let col = |name: &str| header.iter().position(|h| h == name);
    let (Some(c_time), Some(c_asset), Some(c_jid), Some(c_type), Some(c_qty)) = (
        col("event_time_display"),
        col("instrument_name"),
        col("journal_id"),
        col("journal_type"),
        col("transaction_qty"),
    ) else {
        return Err(Error::from(
            "import: crypto statement CSV missing an expected column (event_time_display/instrument_name/journal_id/journal_type/transaction_qty)",
        ));
    };
    let c_side = col("side");
    let c_trade = col("trade_id");

    let mut rows = Vec::new();
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        let f = parse_record(line);
        let at = |i: Option<usize>| i.and_then(|i| f.get(i)).cloned().unwrap_or_default();
        let get = |i: usize| f.get(i).cloned().unwrap_or_default();
        rows.push(Row {
            time: get(c_time),
            asset: get(c_asset),
            journal_id: get(c_jid),
            kind: get(c_type),
            side: at(c_side),
            qty: get(c_qty),
            trade_id: at(c_trade),
            raw: line.to_string(),
        });
    }
    Ok(rows)
}

// ---------------------------------------------------------------------
// profile
// ---------------------------------------------------------------------

struct Profile {
    output_file: PathBuf,
    title: String,
    account: String, // usmec:11:crypto
    fee_account: String,
    rules: Vec<Rule>,
    default_account: String,
    /// Currency code → ledger symbol from the `commodities` file's `alias`
    /// lines (USD→$). `parity` codes (USDC/USDT) stay verbatim.
    aliases: HashMap<String, String>,
}

impl Profile {
    fn load(path: &str) -> Result<Profile, Error> {
        let src = read(path)?;
        let mut directives: HashMap<String, String> = HashMap::new();
        let mut raw_rules: Vec<(String, String)> = Vec::new();
        let mut default_account = String::from("expenses:unknown");
        let mut fee_account: Option<String> = None;

        for line in src.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((lhs, rhs)) = line.split_once("=>") {
                let lhs = lhs.trim();
                let account = rhs.trim().to_string();
                match lhs {
                    "default" => default_account = account,
                    "fee" => fee_account = Some(account),
                    _ => raw_rules.push((lhs.to_string(), account)),
                }
            } else if let Some((key, val)) = line.split_once(char::is_whitespace) {
                directives.insert(key.trim().to_string(), val.trim().to_string());
            }
        }

        let get = |key: &str| -> Result<String, Error> {
            directives
                .get(key)
                .cloned()
                .ok_or_else(|| Error::from(format!("import: missing '{}' in profile", key)))
        };

        let fee_account = fee_account
            .ok_or_else(|| Error::from("import: crypto profile needs a 'fee => <account>' rule"))?;

        let mut rules = Vec::new();
        for (lhs, acc) in raw_rules {
            let mut conds = Vec::new();
            for part in lhs.split(';') {
                let part = part.trim();
                let (fname, val) = part.split_once(char::is_whitespace).ok_or_else(|| {
                    Error::from(format!("import: rule '{}' is not <field> <value>", part))
                })?;
                let fname = fname.trim();
                if !FIELDS.contains(&fname) {
                    return Err(Error::from(format!(
                        "import: rule field '{}' is not a crypto row field ({})",
                        fname,
                        FIELDS.join(", ")
                    )));
                }
                let (mode, core) = Match::parse(val.trim());
                conds.push((fname.to_string(), core.to_lowercase(), mode));
            }
            rules.push(Rule { conds, account: acc });
        }

        let aliases = match directives.get("commodities") {
            Some(p) => load_aliases(&expand(p)),
            None => HashMap::new(),
        };

        Ok(Profile {
            output_file: expand(&get("output.file")?),
            title: get("output.title")?,
            account: get("output.account")?,
            fee_account,
            rules,
            default_account,
            aliases,
        })
    }

    /// The ledger commodity for a crypto.com asset code: its alias canonical
    /// form (USD→$) when declared, else the code itself (LTC/BTC stay put; a
    /// parity stablecoin keeps its own symbol).
    fn commodity<'a>(&'a self, asset: &'a str) -> &'a str {
        self.aliases.get(asset).map(String::as_str).unwrap_or(asset)
    }

    /// Counter account for a single movement: first matching rule, else default.
    fn categorize(&self, r: &Row) -> String {
        match_account(&self.rules, |f| r.field(f))
            .unwrap_or(self.default_account.as_str())
            .to_string()
    }

    /// A deposit or withdrawal: one asset moves between the account and a
    /// counter. Two postings — the signed amount verbatim, the counter bare
    /// to auto-balance.
    fn render_single(&self, r: &Row) -> String {
        let sym = self.commodity(&r.asset);
        let counter = self.categorize(r);
        let mut s = format!("{} * {}\n\t; csv: {}\n", r.date(), self.title, r.raw);
        s.push_str(&format!("\t{}  {}{}\n", self.account, sym, r.qty));
        s.push_str(&format!("\t{}", counter));
        s
    }

    /// A trade: the received (positive) leg is booked gross `@@` the spent
    /// (negative) leg's magnitude; the spent leg carries its own signed amount.
    /// The fee (a `TRADE_FEE` row, negative, in its own commodity) leaves the
    /// account to the fee account. Which leg is the base comes from the sign in
    /// the data. Amounts verbatim, so the fee nets at its own precision.
    fn render_trade(&self, trade_id: &str, legs: &[Row]) -> Result<(String, String), Error> {
        let base = legs
            .iter()
            .find(|r| r.kind == "TRADING" && !r.qty.starts_with('-') && !is_zero(&r.qty));
        let cost = legs
            .iter()
            .find(|r| r.kind == "TRADING" && r.qty.starts_with('-'));
        let (Some(base), Some(cost)) = (base, cost) else {
            return Err(Error::from(format!(
                "import: crypto trade {} is not one received (+) and one spent (-) leg",
                trade_id
            )));
        };
        let bsym = self.commodity(&base.asset);
        let csym = self.commodity(&cost.asset);

        let mut s = format!("{} * {}\n", base.date(), self.title);
        // Source rows in booking order: the two trade legs, then the fee(s) —
        // so the `; csv:` audit trail mirrors the postings below (fee last).
        s.push_str(&format!("\t; csv: {}\n", base.raw));
        s.push_str(&format!("\t; csv: {}\n", cost.raw));
        for fee in legs.iter().filter(|r| r.kind == "TRADE_FEE") {
            s.push_str(&format!("\t; csv: {}\n", fee.raw));
        }
        s.push_str(&format!("\t{}  {}{} @@ {}{}\n", self.account, bsym, base.qty, csym, mag(&cost.qty)));
        s.push_str(&format!("\t{}  {}{}", self.account, csym, cost.qty));
        for fee in legs.iter().filter(|r| r.kind == "TRADE_FEE" && !is_zero(&r.qty)) {
            let fsym = self.commodity(&fee.asset);
            // fee qty is negative (it left the account): book its magnitude to
            // the fee account (second-to-last), then its negation out of the
            // account (last).
            s.push_str(&format!("\n\t{}  {}{}", self.fee_account, fsym, mag(&fee.qty)));
            s.push_str(&format!("\n\t{}  {}{}", self.account, fsym, fee.qty));
        }
        Ok((base.time.clone(), s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> Profile {
        let mut aliases = HashMap::new();
        aliases.insert("USD".to_string(), "$".to_string());
        Profile {
            output_file: PathBuf::new(),
            title: "crypto | me".to_string(),
            account: "usmec:11:crypto".to_string(),
            fee_account: "usmec:12:crypto-fee".to_string(),
            rules: Vec::new(),
            default_account: "usmec:12:crypto-uncategorized".to_string(),
            aliases,
        }
    }

    fn row(kind: &str, asset: &str, side: &str, qty: &str, trade_id: &str, jid: &str) -> Row {
        Row {
            time: "2025-08-25 16:19:50 +00:00".to_string(),
            asset: asset.to_string(),
            journal_id: jid.to_string(),
            kind: kind.to_string(),
            side: side.to_string(),
            qty: qty.to_string(),
            trade_id: trade_id.to_string(),
            raw: format!("row-{}", jid),
        }
    }

    #[test]
    fn header_indexes_both_layouts() {
        // Normal header.
        let normal = "\"event_time_display\",\"instrument_name\",\"journal_id\",\"journal_type\",\"order_id\",\"side\",\"taker_side\",\"transaction_qty\",\"transaction_cost\",\"trade_id\",\"trade_match_id\",\"client_oid\",\"realized_pnl\",\"event_timestamp_ns\"\n2025-08-25 15:20:22 +00:00,USD,111,FIAT_DEPOSIT,,,,500,500,,,\"oid\",0,1\n";
        let rows = rows_from_csv(normal).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].asset, "USD");
        assert_eq!(rows[0].kind, "FIAT_DEPOSIT");
        assert_eq!(rows[0].qty, "500");
        assert_eq!(rows[0].journal_id, "111");

        // Isolation layout: two leading columns; names still locate the fields.
        let iso = "\"isolation_id\",\"isolation_type\",\"event_time_display\",\"instrument_name\",\"journal_id\",\"journal_type\",\"order_id\",\"side\",\"taker_side\",\"transaction_qty\",\"transaction_cost\",\"trade_id\",\"trade_match_id\",\"client_oid\",\"realized_pnl\",\"event_timestamp_ns\"\n,,2026-03-03 22:21:01 +00:00,LTC,222,TRADING,ord,BUY,MAKER,24.59,1345,tid,mid,\"oid\",0,1\n";
        let rows = rows_from_csv(iso).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].asset, "LTC");
        assert_eq!(rows[0].kind, "TRADING");
        assert_eq!(rows[0].side, "BUY");
        assert_eq!(rows[0].qty, "24.59");
        assert_eq!(rows[0].trade_id, "tid");
    }

    #[test]
    fn single_deposit_two_postings_bare_counter() {
        let s = profile().render_single(&row("FIAT_DEPOSIT", "USD", "", "500", "", "111"));
        assert!(s.contains("usmec:11:crypto  $500"));
        assert!(s.trim_end().ends_with("usmec:12:crypto-uncategorized"));
    }

    #[test]
    fn withdrawal_keeps_sign_and_symbol() {
        let s = profile().render_single(&row("ONCHAIN_WITHDRAWAL", "BTC", "", "-0.02818253", "", "222"));
        assert!(s.contains("usmec:11:crypto  BTC-0.02818253"));
    }

    #[test]
    fn trade_books_received_leg_at_spent_cost_with_fee() {
        let legs = vec![
            row("TRADING", "LTC", "BUY", "0.091", "T1", "1"),
            row("TRADING", "USD", "SELL", "-10.21475", "T1", "2"),
            row("TRADE_FEE", "LTC", "", "-0.0002275", "T1", "3"),
        ];
        let (_, s) = profile().render_trade("T1", &legs).unwrap();
        assert!(s.contains("usmec:11:crypto  LTC0.091 @@ $10.21475"));
        assert!(s.contains("usmec:11:crypto  $-10.21475"));
        assert!(s.contains("usmec:12:crypto-fee  LTC0.0002275"));
        assert!(s.contains("usmec:11:crypto  LTC-0.0002275"));
    }
}
