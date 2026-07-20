//! Bitcoin Core-family import backend (`wallet.coin bitcoin` / `litecoin`).
//!
//! Pulls a wallet's transactions from a Bitcoin Core-style daemon over JSON-RPC
//! (`listtransactions`, HTTP Basic auth). bitcoind, litecoind and other forks
//! speak the identical protocol, so one backend serves them all — only the
//! port, cookie path, commodity and coin name differ, and those all come from
//! the profile. One daemon serves every wallet by URL path (`…/wallet/<name>`)
//! — no separate wallet daemon and no port discovery. Own↔own transit is
//! matched by shared `txid` against the daemon's other loaded wallets, each
//! wallet's leaf being `<coin>-<wallet name>`. Amounts are the coin's base
//! units (10^8 per coin), written at full 8-decimal length.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use colored::Colorize;
use serde_json::Value;

use crate::error::Error;

use super::{append, expand, read, slug, Match, Rule};

/// The transaction fields a categorization rule may match on.
const FIELDS: &[&str] = &["category", "address", "label", "txid"];
/// Satoshis per BTC (10^8).
const SATS: i128 = 100_000_000;

pub fn run(conf_path: &str, write: bool) -> Result<(), Error> {
    let mut profile = Profile::load(conf_path)?;
    // Own↔own transit, matched by txid against the daemon's OTHER wallets.
    let (incoming, outgoing) = transit_maps(&profile);
    profile.incoming_transits = incoming;
    profile.outgoing_transits = outgoing;
    let groups = fetch(&profile.wallet_url(), &profile.auth)?;

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
            "{} import: {} transactions read, all already present — nothing new.",
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
    rpc: String,     // daemon base URL, e.g. http://127.0.0.1:8332
    name: String,    // wallet name — the URL path AND the transit leaf
    auth: String,    // ready-made `Basic <base64>` Authorization header
    coin: String,    // from `wallet.coin` — the transit leaf's commodity part
    output_file: PathBuf,
    title: String,
    account: String,
    commodity: String,
    fee_account: String,
    rules: Vec<Rule>,
    default_account: String,
    transit_prefix: Option<String>,
    /// Manual own↔own transits for accounts NOT on this daemon (exchanges):
    /// (exact destination address, that account's leaf).
    transit_entries: Vec<(String, String)>,
    /// txid → SENDER wallet name (from other wallets' `send` entries); a
    /// receive whose txid is here came from that wallet.
    incoming_transits: HashMap<String, String>,
    /// txid → RECIPIENT wallet name (from other wallets' `receive` entries); a
    /// send whose txid is here went to that wallet.
    outgoing_transits: HashMap<String, String>,
}

impl Profile {
    fn load(path: &str) -> Result<Profile, Error> {
        let src = read(path)?;
        let mut directives: HashMap<String, String> = HashMap::new();
        let mut raw_rules: Vec<(String, String)> = Vec::new();
        let mut raw_transits: Vec<(String, String)> = Vec::new();
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
            } else if let Some(rest) = line.strip_prefix("transit ") {
                let rest = rest.trim();
                let (addr, name) = rest.split_once(char::is_whitespace).ok_or_else(|| {
                    Error::from(format!("import: transit '{}' is not <address> <account>", rest))
                })?;
                raw_transits.push((addr.trim().to_string(), name.trim().to_string()));
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
            Error::from("import: core-rpc profile needs a 'fee => <account>' rule")
        })?;

        let transit_prefix = directives.get("transit.self").cloned();
        if !raw_transits.is_empty() && transit_prefix.is_none() {
            return Err(Error::from(
                "import: transit mappings need a 'transit.self' directive",
            ));
        }

        // Rules match on the fixed transaction field set (no `field.*` mapping).
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
                        "import: rule field '{}' is not a bitcoin transaction field ({})",
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
            rpc: get("wallet.rpc")?,
            name: get("wallet.name")?,
            auth: auth_header(&directives)?,
            coin: get("wallet.coin")?,
            output_file: expand(&get("output.file")?),
            title: get("output.title")?,
            account: get("output.account")?,
            commodity: get("output.commodity")?,
            fee_account,
            rules,
            default_account,
            transit_prefix,
            transit_entries: raw_transits,
            incoming_transits: HashMap::new(),
            outgoing_transits: HashMap::new(),
        })
    }

    /// The wallet's JSON-RPC endpoint: the daemon URL plus the wallet path.
    fn wallet_url(&self) -> String {
        format!("{}/wallet/{}", self.rpc.trim_end_matches('/'), self.name)
    }

    fn render(&self, g: &Group) -> String {
        match (&g.receive, &g.send) {
            // Both legs on the same txid → a send to one of our own addresses:
            // the amount returns to us, only the fee is a real cost.
            (Some(_), Some(send)) => self.render_self(send),
            // A send we created.
            (_, Some(send)) => self.render_out(send),
            // A genuine receive — the fee is the sender's, never ours.
            (Some(recv), None) => self.render_in(recv),
            (None, None) => String::new(),
        }
    }

    fn header(&self, t: &Tx) -> String {
        let date = crate::date::ms_to_date(t.time.saturating_mul(1000));
        let rpc = serde_json::to_string(&t.raw).unwrap_or_default();
        format!("{} * {}\n\t; rpc: {}\n", date, self.title, rpc)
    }

    /// Receive: the wallet gains `amount`; the fee is the sender's, not booked.
    /// Two postings, so the counter is left bare to auto-balance.
    fn render_in(&self, t: &Tx) -> String {
        let counter = self.incoming_counter(t);
        let mut s = self.header(t);
        s.push_str(&format!("\t{}  {}{}\n", self.account, self.commodity, btc(t.amount)));
        s.push_str(&format!("\t{}", counter));
        s
    }

    /// Send: the wallet loses `amount + fee`, the fee is its own posting, and
    /// the categorized counter gains `amount` — always the LAST posting.
    fn render_out(&self, t: &Tx) -> String {
        let counter = self.categorize(t);
        let mut s = self.header(t);
        s.push_str(&format!(
            "\t{}  {}-{}\n",
            self.account,
            self.commodity,
            btc(t.amount.abs() + t.fee)
        ));
        s.push_str(&format!("\t{}  {}{}\n", self.fee_account, self.commodity, btc(t.fee)));
        s.push_str(&format!("\t{}  {}{}", counter, self.commodity, btc(t.amount.abs())));
        s
    }

    /// Self-send: `amount` leaves the wallet and returns to it with the fee
    /// between — the fee is the only real cost.
    fn render_self(&self, t: &Tx) -> String {
        let mut s = self.header(t);
        s.push_str(&format!(
            "\t{}  {}-{}\n",
            self.account,
            self.commodity,
            btc(t.amount.abs() + t.fee)
        ));
        s.push_str(&format!("\t{}  {}{}\n", self.fee_account, self.commodity, btc(t.fee)));
        s.push_str(&format!("\t{}  {}{}", self.account, self.commodity, btc(t.amount.abs())));
        s
    }

    /// The transit leaf for a wallet: `<coin>-<wallet name>`, e.g. `bitcoin-main`.
    /// Both sides know the names via `listwallets`, so they build the same string.
    fn transit_leaf(&self, wallet_name: &str) -> String {
        format!("{}-{}", self.coin, wallet_name)
    }

    /// The directional transit account for a transfer to `other` (a leaf) —
    /// `None` when transit isn't configured (no `transit.self`).
    fn transit_account(&self, other: &str, outgoing: bool) -> Option<String> {
        self.transit_prefix.as_ref().map(|p| {
            super::directional_account(p, &self.transit_leaf(&self.name), other, outgoing)
        })
    }

    /// Counter for a receive: if the txid matches a send from another of my
    /// wallets, book the SAME directional transit account (so both legs net);
    /// the sender leaf comes from its wallet name. Otherwise categorize.
    fn incoming_counter(&self, t: &Tx) -> String {
        if let Some(sender) = self.incoming_transits.get(&t.txid)
            && let Some(acct) = self.transit_account(&self.transit_leaf(sender), false)
        {
            return acct;
        }
        self.categorize(t)
    }

    /// Counter for a send: an internal transfer to another of my wallets —
    /// matched by txid (its recipient wallet) — or a manually-mapped non-RPC
    /// account matched by destination address; then a rule; else the default.
    fn categorize(&self, t: &Tx) -> String {
        let other = self
            .outgoing_transits
            .get(&t.txid)
            .map(|recipient| self.transit_leaf(recipient))
            .or_else(|| {
                self.transit_entries
                    .iter()
                    .find(|(a, _)| a == &t.address)
                    .map(|(_, leaf)| leaf.clone())
            });
        if let Some(other) = other
            && let Some(acct) = self.transit_account(&other, true)
        {
            return acct;
        }
        let tmpl = super::match_account(&self.rules, |f| t.field(f))
            .unwrap_or(self.default_account.as_str());
        self.template(tmpl, t)
    }

    fn template(&self, tmpl: &str, t: &Tx) -> String {
        let mut out = tmpl.to_string();
        if out.contains("{address4}") {
            let a = &t.address;
            let tail = a.get(a.len().saturating_sub(4)..).unwrap_or("").to_string();
            out = out.replace("{address4}", &tail);
        }
        if out.contains("{label}") {
            out = out.replace("{label}", &slug(&t.label));
        }
        if out.contains("{type}") {
            out = out.replace("{type}", &t.category);
        }
        out
    }
}

// ---------------------------------------------------------------------
// transactions
// ---------------------------------------------------------------------

struct Tx {
    txid: String,
    category: String,
    amount: i128, // satoshis, signed (negative for a send)
    fee: i128,    // satoshis, magnitude (0 on receives)
    time: u64,
    address: String,
    label: String,
    raw: Value,
}

impl Tx {
    fn field(&self, name: &str) -> String {
        match name {
            "category" => self.category.clone(),
            "address" => self.address.clone(),
            "label" => self.label.clone(),
            "txid" => self.txid.clone(),
            _ => String::new(),
        }
    }
}

/// All entries sharing one txid: a `send` leg, a `receive` leg, or both (a
/// send to one of our own addresses).
struct Group {
    txid: String,
    time: u64,
    send: Option<Tx>,
    receive: Option<Tx>,
}

// ---------------------------------------------------------------------
// rpc
// ---------------------------------------------------------------------

/// One JSON-RPC round-trip to `bitcoind`; returns the `result` object or an
/// error. Bitcoin Core answers RPC errors with a non-2xx status and a JSON
/// error body, so the body is read on both the ok and status-error paths.
fn rpc_call(url: &str, auth: &str, method: &str, params: Value) -> Result<Value, Error> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(30))
        .build();
    let body = serde_json::json!({
        "jsonrpc": "1.0", "id": "acc", "method": method, "params": params
    })
    .to_string();
    let text = match agent
        .post(url)
        .set("Content-Type", "application/json")
        .set("Authorization", auth)
        .send_string(&body)
    {
        Ok(r) => r.into_string(),
        Err(ureq::Error::Status(_, r)) => r.into_string(),
        Err(e) => return Err(Error::from(format!("import: core-rpc {}: {}", url, e))),
    }
    .map_err(|e| Error::from(format!("import: core-rpc read {}: {}", url, e)))?;

    let resp: Value = serde_json::from_str(&text)
        .map_err(|e| Error::from(format!("import: core-rpc bad JSON: {}", e)))?;
    if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
        return Err(Error::from(format!("import: core-rpc error: {}", err)));
    }
    resp.get("result")
        .cloned()
        .ok_or_else(|| Error::from("import: core-rpc: response has no result"))
}

/// Every wallet transaction, walking `listtransactions` in pages of 1000.
fn list_transactions(url: &str, auth: &str) -> Result<Vec<Value>, Error> {
    let mut all = Vec::new();
    let batch = 1000i64;
    let mut skip = 0i64;
    loop {
        let result = rpc_call(url, auth, "listtransactions", serde_json::json!(["*", batch, skip]))?;
        let arr = result.as_array().cloned().unwrap_or_default();
        let n = arr.len() as i64;
        all.extend(arr);
        if n < batch {
            break;
        }
        skip += batch;
    }
    Ok(all)
}

/// Build the two transit maps from the daemon's OTHER loaded wallets (found by
/// `listwallets`), keyed by txid: their `send` entries → `incoming` (sender
/// wallet name, for my receives); their `receive` entries → `outgoing`
/// (recipient wallet name, for my sends). No other conf is read.
fn transit_maps(p: &Profile) -> (HashMap<String, String>, HashMap<String, String>) {
    let mut incoming = HashMap::new();
    let mut outgoing = HashMap::new();
    let Ok(result) = rpc_call(&p.rpc, &p.auth, "listwallets", serde_json::json!([])) else {
        return (incoming, outgoing);
    };
    let names: Vec<String> = result
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str())
        .map(String::from)
        .collect();
    for name in names {
        if name == p.name {
            continue;
        }
        let url = format!("{}/wallet/{}", p.rpc.trim_end_matches('/'), name);
        let Ok(txs) = list_transactions(&url, &p.auth) else {
            continue;
        };
        for t in &txs {
            if is_dead(t) {
                continue;
            }
            let Some(txid) = t.get("txid").and_then(|v| v.as_str()) else {
                continue;
            };
            match t.get("category").and_then(|v| v.as_str()) {
                Some("send") => {
                    incoming.entry(txid.to_string()).or_insert_with(|| name.clone());
                }
                Some("receive") => {
                    outgoing.entry(txid.to_string()).or_insert_with(|| name.clone());
                }
                _ => {}
            }
        }
    }
    (incoming, outgoing)
}

/// Call `listtransactions` and group the entries by txid (oldest first). A
/// txid with several outputs appears as several entries — summed per direction.
fn fetch(url: &str, auth: &str) -> Result<Vec<Group>, Error> {
    let txs = list_transactions(url, auth)?;
    let mut sends: HashMap<String, Vec<Tx>> = HashMap::new();
    let mut recvs: HashMap<String, Vec<Tx>> = HashMap::new();
    for obj in &txs {
        if is_dead(obj) {
            continue;
        }
        let t = parse_tx(obj)?;
        // `generate`/`immature` (mining) count as receives; skip the rest.
        match t.category.as_str() {
            "send" => sends.entry(t.txid.clone()).or_default().push(t),
            "receive" | "generate" | "immature" => recvs.entry(t.txid.clone()).or_default().push(t),
            _ => {}
        }
    }

    let mut txids: Vec<String> = sends.keys().chain(recvs.keys()).cloned().collect();
    txids.sort();
    txids.dedup();
    let mut groups = Vec::new();
    for txid in txids {
        let send = sends.remove(&txid).map(aggregate);
        let receive = recvs.remove(&txid).map(aggregate);
        let time = send.as_ref().or(receive.as_ref()).map(|t| t.time).unwrap_or(0);
        groups.push(Group { txid, time, send, receive });
    }
    groups.sort_by(|a, b| a.time.cmp(&b.time).then(a.txid.cmp(&b.txid)));
    Ok(groups)
}

/// Merge the entries of one txid+direction: sum the amounts and keep the
/// largest as representative (its raw object + fields carry the meaningful
/// data). The fee is a per-transaction value, so it is taken once, not summed.
fn aggregate(mut legs: Vec<Tx>) -> Tx {
    legs.sort_by_key(|t| std::cmp::Reverse(t.amount.abs()));
    let total: i128 = legs.iter().map(|t| t.amount).sum();
    let fee = legs.iter().map(|t| t.fee).max().unwrap_or(0);
    let mut repr = legs.into_iter().next().expect("aggregate: non-empty legs");
    repr.amount = total;
    repr.fee = fee;
    repr
}

/// A transaction Bitcoin Core reports as conflicted or replaced (negative
/// confirmations) or manually abandoned never settled on-chain — booking it
/// would double-count against its replacement, so it is skipped.
fn is_dead(obj: &Value) -> bool {
    obj.get("confirmations").and_then(|v| v.as_i64()).unwrap_or(0) < 0
        || obj.get("abandoned").and_then(|v| v.as_bool()).unwrap_or(false)
}

fn parse_tx(obj: &Value) -> Result<Tx, Error> {
    let str_of = |k: &str| obj.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let txid = str_of("txid");
    if txid.is_empty() {
        return Err(Error::from("import: core-rpc: transaction without txid"));
    }
    Ok(Tx {
        txid,
        category: str_of("category"),
        amount: sats(obj.get("amount")),
        fee: sats(obj.get("fee")).abs(),
        time: obj.get("time").and_then(|v| v.as_u64()).unwrap_or(0),
        address: str_of("address"),
        label: str_of("label"),
        raw: obj.clone(),
    })
}

/// A JSON amount in BTC (e.g. `0.02778253`, `-0.00010000`) as signed satoshis.
/// Read as a literal string so serde_json's arbitrary-precision numbers keep
/// full exactness (no float rounding).
fn sats(v: Option<&Value>) -> i128 {
    let Some(v) = v else { return 0 };
    to_sats(&v.to_string())
}

/// Parse a decimal BTC string into signed satoshis (8-decimal fixed point).
fn to_sats(s: &str) -> i128 {
    let s = s.trim().trim_matches('"');
    let (neg, s) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s),
    };
    let (int, frac) = s.split_once('.').unwrap_or((s, ""));
    let mut frac = frac.to_string();
    frac.truncate(8);
    while frac.len() < 8 {
        frac.push('0');
    }
    let int: i128 = int.parse().unwrap_or(0);
    let frac: i128 = frac.parse().unwrap_or(0);
    let total = int * SATS + frac;
    if neg { -total } else { total }
}

/// Satoshis → a BTC decimal string at full 8-digit precision.
fn btc(sats: i128) -> String {
    let a = sats.abs();
    format!("{}.{:08}", a / SATS, a % SATS)
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

// ---------------------------------------------------------------------
// auth
// ---------------------------------------------------------------------

/// Build the `Basic <base64>` Authorization header. `wallet.user` +
/// `wallet.pass` win; otherwise the cookie file at `wallet.cookie`, whose
/// content is already `user:password`. The cookie path is coin-specific
/// (`~/.bitcoin/.cookie`, `~/.litecoin/.cookie`, …), so it must be given —
/// there is no default that would be right for every coin.
fn auth_header(directives: &HashMap<String, String>) -> Result<String, Error> {
    let creds = match (directives.get("wallet.user"), directives.get("wallet.pass")) {
        (Some(u), Some(p)) => format!("{}:{}", u, p),
        _ => {
            let path = directives.get("wallet.cookie").ok_or_else(|| {
                Error::from("import: need wallet.cookie (or wallet.user + wallet.pass) for auth")
            })?;
            std::fs::read_to_string(expand(path))
                .map_err(|e| Error::from(format!("import: read cookie {}: {}", path, e)))?
                .trim()
                .to_string()
        }
    };
    Ok(format!("Basic {}", base64(creds.as_bytes())))
}

/// Standard base64 (no line breaks) — enough for the small auth string.
fn base64(input: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 { T[((n >> 6) & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[(n & 63) as usize] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> Profile {
        Profile {
            rpc: "http://127.0.0.1:8332".to_string(),
            name: "main".to_string(),
            auth: String::new(),
            coin: "bitcoin".to_string(),
            output_file: PathBuf::new(),
            title: "bitcoin".to_string(),
            account: "assets:btc".to_string(),
            commodity: "BTC".to_string(),
            fee_account: "expenses:fees".to_string(),
            rules: Vec::new(),
            default_account: "expenses:unsorted".to_string(),
            transit_prefix: None,
            transit_entries: Vec::new(),
            incoming_transits: HashMap::new(),
            outgoing_transits: HashMap::new(),
        }
    }

    fn tx(category: &str, amount: i128, fee: i128) -> Tx {
        Tx {
            txid: "t".to_string(),
            category: category.to_string(),
            amount,
            fee,
            time: 1_700_000_000,
            address: String::new(),
            label: String::new(),
            raw: serde_json::json!({ "txid": "t", "category": category }),
        }
    }

    fn group(recv: Option<Tx>, send: Option<Tx>) -> Group {
        Group { txid: "t".to_string(), time: 1_700_000_000, receive: recv, send }
    }

    #[test]
    fn to_sats_and_btc_roundtrip() {
        assert_eq!(to_sats("0.02778253"), 2_778_253);
        assert_eq!(to_sats("-0.00010000"), -10_000);
        assert_eq!(to_sats("1"), 100_000_000);
        assert_eq!(to_sats("\"0.5\""), 50_000_000);
        assert_eq!(btc(2_778_253), "0.02778253");
        assert_eq!(btc(100_000_000), "1.00000000");
        assert_eq!(btc(-10_000), "0.00010000");
    }

    #[test]
    fn conflicted_and_abandoned_are_dead() {
        // Negative confirmations = replaced/conflicted; abandoned = dropped.
        assert!(is_dead(&serde_json::json!({ "confirmations": -3 })));
        assert!(is_dead(&serde_json::json!({ "abandoned": true })));
        // A confirmed (or still-pending, 0-conf) transaction is live.
        assert!(!is_dead(&serde_json::json!({ "confirmations": 42 })));
        assert!(!is_dead(&serde_json::json!({ "confirmations": 0 })));
    }

    #[test]
    fn base64_encodes() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"__cookie__:ab"), "X19jb29raWVfXzphYg==");
    }

    #[test]
    fn receive_is_two_postings_fee_ignored() {
        let s = profile().render(&group(Some(tx("receive", 2_778_253, 0)), None));
        assert!(s.contains("assets:btc  BTC0.02778253"));
        assert!(s.trim_end().ends_with("expenses:unsorted"));
        assert!(!s.contains("expenses:fees"));
    }

    #[test]
    fn send_is_three_postings_counter_last() {
        // amount -10000 sats (negative for a send), fee 141 sats
        let s = profile().render(&group(None, Some(tx("send", -10_000, 141))));
        assert!(s.contains("assets:btc  BTC-0.00010141")); // amount + fee out
        assert!(s.contains("expenses:fees  BTC0.00000141"));
        assert!(s.trim_end().ends_with("expenses:unsorted  BTC0.00010000")); // counter last
    }

    #[test]
    fn send_to_my_wallet_books_directional_transit() {
        let mut p = profile();
        p.transit_prefix = Some("assets:transit".to_string());
        p.outgoing_transits.insert("t".to_string(), "cold".to_string()); // recipient wallet "cold"
        let s = p.render(&group(None, Some(tx("send", -10_000, 141))));
        // leaf = coin + wallet name: own=bitcoin-main, recipient=bitcoin-cold
        assert!(s.trim_end().ends_with("assets:transit:bitcoin-main:bitcoin-cold  BTC0.00010000"));
    }

    #[test]
    fn receive_matched_by_txid_books_directional_transit() {
        let mut p = profile();
        p.transit_prefix = Some("assets:transit".to_string());
        p.incoming_transits.insert("t".to_string(), "cold".to_string()); // sender wallet "cold"
        let s = p.render(&group(Some(tx("receive", 2_778_253, 0)), None));
        // incoming → other:own = bitcoin-cold:bitcoin-main, matching the send leg
        assert!(s.trim_end().ends_with("assets:transit:bitcoin-cold:bitcoin-main"));
    }

    #[test]
    fn send_to_manual_address_books_transit() {
        let mut p = profile();
        p.transit_prefix = Some("assets:transit".to_string());
        p.transit_entries = vec![("EXCH_ADDR".to_string(), "extern".to_string())];
        let mut t = tx("send", -10_000, 141);
        t.address = "EXCH_ADDR".to_string();
        let s = p.render(&group(None, Some(t)));
        assert!(s.trim_end().ends_with("assets:transit:bitcoin-main:extern  BTC0.00010000"));
    }
}
