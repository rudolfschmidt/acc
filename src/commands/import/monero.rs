//! `source monero-rpc` import backend.
//!
//! Pulls the wallet's transfers straight from a local `monero-wallet-rpc`
//! (`get_transfers` over HTTP JSON-RPC) and turns each into a ledger
//! transaction. Every entry carries its COMPLETE RPC object as a `; rpc: {…}`
//! comment right after the header; dedup reads the `txid` back out of it.
//! Amounts are atomic piconero, written at full 12-decimal length.
//!
//! Categorization, dedup, preview and append are shared with the CSV path;
//! only the source (RPC instead of a file) and the rendering differ.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use colored::Colorize;
use serde_json::Value;

use crate::error::Error;

use super::{append, expand, read, slug, Match, Rule};

/// The transfer fields a categorization rule may match on.
const FIELDS: &[&str] = &["type", "txid", "address", "subaddr", "payment_id", "note"];
/// Piconero per XMR (10^12).
const ATOMIC: i128 = 1_000_000_000_000;

pub fn run(conf_path: &str, write: bool) -> Result<(), Error> {
    let profile = Profile::load(conf_path)?;
    let groups = fetch(&profile.rpc_url)?;

    let existing = std::fs::read_to_string(&profile.output_file).unwrap_or_default();
    let seen = existing_txids(&existing);

    let mut blocks = Vec::new();
    let mut skipped = 0usize;
    for g in &groups {
        if seen.contains(&g.txid) {
            skipped += 1;
            continue;
        }
        blocks.push(profile.render(g));
    }

    if blocks.is_empty() {
        println!(
            "{} import: {} transfers read, all already present — nothing new.",
            "!".yellow(),
            groups.len()
        );
        return Ok(());
    }

    let added = crate::commands::format::format_source(&blocks.join("\n\n"), false)?;
    if write {
        append(&profile.output_file, &added)?;
    }
    super::render::diff_preview(
        &existing,
        &added,
        blocks.len(),
        &profile.output_file,
        skipped,
        write,
    );
    Ok(())
}

// ---------------------------------------------------------------------
// profile
// ---------------------------------------------------------------------

struct Profile {
    rpc_url: String,
    output_file: PathBuf,
    title: String,
    account: String,   // the wallet's own account
    commodity: String, // symbol prefixing every amount (XMR)
    fee_account: String,
    rules: Vec<Rule>,
    default_account: String,
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

        let fee_account = fee_account.ok_or_else(|| {
            Error::from("import: monero-rpc profile needs a 'fee => <account>' rule")
        })?;

        // Rules match on the fixed RPC field set (no `field.*` mapping).
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
                        "import: rule field '{}' is not a monero transfer field ({})",
                        fname,
                        FIELDS.join(", ")
                    )));
                }
                let (mode, core) = Match::parse(val.trim());
                conds.push((fname.to_string(), core.to_lowercase(), mode));
            }
            rules.push(Rule { conds, account: acc });
        }

        Ok(Profile {
            rpc_url: get("rpc.url")?,
            output_file: expand(&get("output.file")?),
            title: get("output.title")?,
            account: get("output.account")?,
            commodity: get("output.commodity")?,
            fee_account,
            rules,
            default_account,
        })
    }

    fn render(&self, g: &Group) -> String {
        match (&g.incoming, &g.outgoing) {
            // Same txid in both lists → you sent to your own wallet: the
            // amount returns to this account, only the fee is a real cost.
            (Some(_), Some(out)) => self.render_self(out),
            // Only outgoing → a send you created.
            (_, Some(out)) => self.render_out(out),
            // Only incoming → a genuine receive. You did NOT create it (no
            // outgoing leg, no tx key), so the `fee` shown is the sender's
            // metadata, not your cost — book amount only.
            (Some(inc), None) => self.render_in(inc),
            (None, None) => String::new(),
        }
    }

    /// Header line + the `; rpc:` comment carrying the full source object.
    fn header(&self, t: &Transfer) -> String {
        let date = crate::date::ms_to_date(t.timestamp.saturating_mul(1000));
        let rpc = serde_json::to_string(&t.raw).unwrap_or_default();
        format!("{} * {}\n\t; rpc: {}\n", date, self.title, rpc)
    }

    /// External receive: the wallet gains `amount`; the fee is the sender's,
    /// not booked. Two postings, so the counter is left bare to auto-balance.
    fn render_in(&self, t: &Transfer) -> String {
        let counter = self.categorize(t);
        let mut s = self.header(t);
        s.push_str(&format!("\t{}  {}{}\n", self.account, self.commodity, xmr(t.amount)));
        s.push_str(&format!("\t{}", counter));
        s
    }

    /// Send: the wallet loses `amount + fee`, the fee is its own posting, and
    /// the categorized counter gains `amount` — always the LAST posting.
    /// Three explicit postings, none inferred.
    fn render_out(&self, t: &Transfer) -> String {
        let counter = self.categorize(t);
        let mut s = self.header(t);
        s.push_str(&format!(
            "\t{}  {}-{}\n",
            self.account,
            self.commodity,
            xmr(t.amount + t.fee)
        ));
        s.push_str(&format!("\t{}  {}{}\n", self.fee_account, self.commodity, xmr(t.fee)));
        s.push_str(&format!("\t{}  {}{}", counter, self.commodity, xmr(t.amount)));
        s
    }

    /// Self-sweep / churn: `amount` leaves this wallet account and returns to
    /// it (11 → 11) with the fee between — the fee is the only real cost.
    /// Three explicit postings summing to zero, none inferred.
    fn render_self(&self, t: &Transfer) -> String {
        let mut s = self.header(t);
        s.push_str(&format!(
            "\t{}  {}-{}\n",
            self.account,
            self.commodity,
            xmr(t.amount + t.fee)
        ));
        s.push_str(&format!("\t{}  {}{}\n", self.fee_account, self.commodity, xmr(t.fee)));
        s.push_str(&format!("\t{}  {}{}", self.account, self.commodity, xmr(t.amount)));
        s
    }

    /// Counter account: the first matching rule, else the default.
    fn categorize(&self, t: &Transfer) -> String {
        for rule in &self.rules {
            let hit = rule
                .conds
                .iter()
                .all(|(f, needle, mode)| mode.test(&t.field(f).to_lowercase(), needle));
            if hit {
                return self.template(&rule.account, t);
            }
        }
        self.template(&self.default_account, t)
    }

    fn template(&self, tmpl: &str, t: &Transfer) -> String {
        let mut out = tmpl.to_string();
        if out.contains("{address4}") {
            let a = t.field("address");
            let tail = a.get(a.len().saturating_sub(4)..).unwrap_or("").to_string();
            out = out.replace("{address4}", &tail);
        }
        if out.contains("{note}") {
            out = out.replace("{note}", &slug(&t.field("note")));
        }
        if out.contains("{subaddr}") {
            out = out.replace("{subaddr}", &t.field("subaddr").replace(':', "-"));
        }
        if out.contains("{type}") {
            out = out.replace("{type}", &t.field("type"));
        }
        out
    }
}

// ---------------------------------------------------------------------
// transfers
// ---------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Dir {
    In,
    Out,
}

struct Transfer {
    txid: String,
    amount: i128,
    fee: i128,
    timestamp: u64,
    fields: HashMap<String, String>,
    raw: Value,
}

impl Transfer {
    fn field(&self, name: &str) -> String {
        self.fields.get(name).cloned().unwrap_or_default()
    }
}

/// All transfers sharing one txid: at most one `in` and one `out` (both set
/// only for a self-send).
struct Group {
    txid: String,
    timestamp: u64,
    incoming: Option<Transfer>,
    outgoing: Option<Transfer>,
}

/// Call `get_transfers` and group the in/out lists by txid (oldest first).
fn fetch(url: &str) -> Result<Vec<Group>, Error> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(30))
        .build();
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": "0", "method": "get_transfers",
        "params": { "in": true, "out": true, "pending": false, "failed": false, "pool": false }
    })
    .to_string();
    let text = agent
        .post(url)
        .set("Content-Type", "application/json")
        .send_string(&body)
        .map_err(|e| Error::from(format!("import: monero-rpc {}: {}", url, e)))?
        .into_string()
        .map_err(|e| Error::from(format!("import: monero-rpc read {}: {}", url, e)))?;
    let resp: Value = serde_json::from_str(&text)
        .map_err(|e| Error::from(format!("import: monero-rpc bad JSON: {}", e)))?;
    if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
        return Err(Error::from(format!("import: monero-rpc error: {}", err)));
    }
    let result = resp
        .get("result")
        .ok_or_else(|| Error::from("import: monero-rpc: response has no result"))?;

    // `get_transfers` lists one entry per OUTPUT, so a transaction with
    // several outputs to this wallet appears as several entries sharing a
    // txid (e.g. a real amount + a 0-value padding output). Collect the legs
    // per txid+direction and aggregate them.
    let mut ins: HashMap<String, Vec<Transfer>> = HashMap::new();
    let mut outs: HashMap<String, Vec<Transfer>> = HashMap::new();
    if let Some(arr) = result.get("in").and_then(|v| v.as_array()) {
        for obj in arr {
            let t = parse_transfer(obj, Dir::In)?;
            ins.entry(t.txid.clone()).or_default().push(t);
        }
    }
    if let Some(arr) = result.get("out").and_then(|v| v.as_array()) {
        for obj in arr {
            let t = parse_transfer(obj, Dir::Out)?;
            outs.entry(t.txid.clone()).or_default().push(t);
        }
    }

    let mut txids: Vec<String> = ins.keys().chain(outs.keys()).cloned().collect();
    txids.sort();
    txids.dedup();
    let mut groups = Vec::new();
    for txid in txids {
        let incoming = ins.remove(&txid).map(aggregate);
        let outgoing = outs.remove(&txid).map(aggregate);
        let timestamp = incoming
            .as_ref()
            .or(outgoing.as_ref())
            .map(|t| t.timestamp)
            .unwrap_or(0);
        groups.push(Group {
            txid,
            timestamp,
            incoming,
            outgoing,
        });
    }
    groups.sort_by(|a, b| a.timestamp.cmp(&b.timestamp).then(a.txid.cmp(&b.txid)));
    Ok(groups)
}

/// Merge the legs of one txid+direction: sum the amounts, and keep the
/// largest-amount leg as the representative (its raw object + fields carry
/// the meaningful data; the padding 0-output is noise).
fn aggregate(mut legs: Vec<Transfer>) -> Transfer {
    legs.sort_by_key(|t| std::cmp::Reverse(t.amount));
    let total: i128 = legs.iter().map(|t| t.amount).sum();
    let mut repr = legs.into_iter().next().expect("aggregate: non-empty legs");
    repr.amount = total;
    repr
}

fn parse_transfer(obj: &Value, dir: Dir) -> Result<Transfer, Error> {
    let str_of = |k: &str| obj.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let txid = str_of("txid");
    if txid.is_empty() {
        return Err(Error::from("import: monero-rpc: transfer without txid"));
    }
    let subaddr = obj
        .get("subaddr_index")
        .map(|si| {
            format!(
                "{}:{}",
                si.get("major").and_then(|v| v.as_u64()).unwrap_or(0),
                si.get("minor").and_then(|v| v.as_u64()).unwrap_or(0)
            )
        })
        .unwrap_or_default();
    // The address to categorize on: for a send, the recipient (first
    // destination); for a receive, the subaddress that received it.
    let address = match dir {
        Dir::Out => obj
            .get("destinations")
            .and_then(|d| d.as_array())
            .and_then(|a| a.first())
            .and_then(|d| d.get("address"))
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_else(|| str_of("address")),
        Dir::In => str_of("address"),
    };

    let mut fields = HashMap::new();
    fields.insert(
        "type".to_string(),
        match dir {
            Dir::In => "in",
            Dir::Out => "out",
        }
        .to_string(),
    );
    fields.insert("txid".to_string(), txid.clone());
    fields.insert("address".to_string(), address);
    fields.insert("subaddr".to_string(), subaddr);
    fields.insert("payment_id".to_string(), str_of("payment_id"));
    fields.insert("note".to_string(), str_of("note"));

    Ok(Transfer {
        txid,
        amount: atomic(obj.get("amount")),
        fee: atomic(obj.get("fee")),
        timestamp: obj.get("timestamp").and_then(|v| v.as_u64()).unwrap_or(0),
        fields,
        raw: obj.clone(),
    })
}

/// A JSON number as atomic units (piconero). Handles serde_json's
/// arbitrary-precision numbers by falling back to the literal string.
fn atomic(v: Option<&Value>) -> i128 {
    let Some(v) = v else { return 0 };
    if let Some(u) = v.as_u64() {
        return u as i128;
    }
    v.to_string().trim_matches('"').parse().unwrap_or(0)
}

/// Atomic piconero → an XMR decimal string at full 12-digit precision.
fn xmr(atomic: i128) -> String {
    let a = atomic.abs();
    format!("{}.{:012}", a / ATOMIC, a % ATOMIC)
}

/// The txids already imported, read back from the `; rpc:` comments.
fn existing_txids(src: &str) -> HashSet<String> {
    let mut set = HashSet::new();
    for line in src.lines() {
        let Some(rest) = line.trim_start().strip_prefix("; rpc:") else {
            continue;
        };
        if let Ok(v) = serde_json::from_str::<Value>(rest.trim())
            && let Some(txid) = v.get("txid").and_then(|x| x.as_str())
        {
            set.insert(txid.to_string());
        }
    }
    set
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> Profile {
        Profile {
            rpc_url: String::new(),
            output_file: PathBuf::new(),
            title: "monero".to_string(),
            account: "assets:xmr".to_string(),
            commodity: "XMR".to_string(),
            fee_account: "expenses:fees".to_string(),
            rules: Vec::new(),
            default_account: "expenses:unsorted".to_string(),
        }
    }

    // Amount `x` piconero, fee `f`, dated 2025-07-02.
    fn transfer(x: i128, f: i128) -> Transfer {
        Transfer {
            txid: "t".to_string(),
            amount: x,
            fee: f,
            timestamp: 1_751_495_027,
            fields: HashMap::new(),
            raw: serde_json::json!({ "txid": "t", "amount": x as i64 }),
        }
    }

    #[test]
    fn xmr_writes_full_twelve_decimals() {
        assert_eq!(xmr(200_000_000_000), "0.200000000000");
        assert_eq!(xmr(1_000_000_000_000), "1.000000000000");
        assert_eq!(xmr(130_364_401_518), "0.130364401518");
        assert_eq!(xmr(47_232_749_000_000), "47.232749000000");
    }

    #[test]
    fn atomic_reads_json_number() {
        let v = serde_json::json!({ "amount": 130_364_401_518_u64 });
        assert_eq!(atomic(v.get("amount")), 130_364_401_518);
        assert_eq!(atomic(None), 0);
    }

    #[test]
    fn existing_txids_read_from_rpc_comments() {
        let src = "2025-07-02 * x\n\t; rpc: {\"txid\":\"aaa\",\"type\":\"in\"}\n\tassets:xmr XMR1\n\
                   \n2025-07-03 * y\n\t; rpc: {\"txid\":\"bbb\"}\n";
        let set = existing_txids(src);
        assert!(set.contains("aaa") && set.contains("bbb"));
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn aggregate_sums_outputs_and_keeps_the_real_leg() {
        // real 0.130364401518 + a 0-value padding output → summed, representative
        // is the non-zero leg.
        let agg = aggregate(vec![transfer(130_364_401_518, 1_022_340_000), transfer(0, 1_022_340_000)]);
        assert_eq!(agg.amount, 130_364_401_518);
        assert_eq!(agg.fee, 1_022_340_000);
    }

    fn group(inc: Option<Transfer>, out: Option<Transfer>) -> Group {
        Group { txid: "t".to_string(), timestamp: 1_751_495_027, incoming: inc, outgoing: out }
    }

    #[test]
    fn receive_is_two_postings_fee_ignored_last_bare() {
        // in-only → receive: wallet +amount, counter bare; the fee is the
        // sender's, never booked.
        let s = profile().render(&group(Some(transfer(130_364_401_518, 1_022_340_000)), None));
        assert!(s.contains("assets:xmr  XMR0.130364401518"));
        assert!(s.trim_end().ends_with("expenses:unsorted"));
        assert!(!s.contains("expenses:fees"));
    }

    #[test]
    fn send_is_three_postings_counter_last() {
        let s = profile().render(&group(None, Some(transfer(16_340_000_000, 30_580_000))));
        assert!(s.contains("assets:xmr  XMR-0.016370580000")); // amount + fee out
        assert!(s.contains("expenses:fees  XMR0.000030580000"));
        assert!(s.trim_end().ends_with("expenses:unsorted  XMR0.016340000000")); // counter last, filled
    }

    #[test]
    fn self_sweep_is_eleven_to_eleven_with_fee_between() {
        let t = || transfer(130_364_401_518, 1_022_340_000);
        let s = profile().render(&group(Some(t()), Some(t())));
        assert!(s.contains("assets:xmr  XMR-0.131386741518")); // X + F out
        assert!(s.contains("expenses:fees  XMR0.001022340000")); // fee between
        assert!(s.trim_end().ends_with("assets:xmr  XMR0.130364401518")); // X back into 11
    }
}
