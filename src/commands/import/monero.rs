//! `wallet.coin monero` import backend.
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
    let mut profile = Profile::load(conf_path)?;
    // Discover every reachable wallet-rpc by primary address, resolve our own
    // endpoint (and verify it's the right wallet).
    let endpoints = profile.discover_endpoints();
    profile.rpc_url = profile.resolve_own_url(&endpoints)?;
    // Cross-wallet transit is matched purely by TXID against the other running
    // wallets — no other conf is read, and it works even when a send cached no
    // `destinations` or went to a subaddress. A shared txid is an internal
    // transfer: their `out` list gives my receives (its sender), their `in`
    // list gives my sends (its recipient). Any wallet's leaf is
    // `<coin>-<last4 of address>`, so both legs build the same account, net to 0.
    let own = profile.wallet_address.clone();
    let (incoming, outgoing) = transit_maps(&endpoints, &own);
    profile.incoming_transits = incoming;
    profile.outgoing_transits = outgoing;
    profile.accounts = fetch_accounts(&profile.rpc_url)?;
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
    rpc_url: String,          // resolved endpoint (filled by discovery in run)
    wallet_address: String,   // primary address — identity + discovery key
    scan_host: String,        // host probed for the wallet-rpc endpoint
    scan_ports: (u16, u16),   // inclusive port range to probe
    output_file: PathBuf,
    title: String,
    account: String,   // the wallet's own account
    commodity: String, // symbol prefixing every amount (XMR)
    fee_account: String,
    rules: Vec<Rule>,
    default_account: String,
    /// (major index, label) for every wallet account, from `get_accounts`.
    /// One account → the bare `account`; several → each gets a `:label` (or
    /// `:index`) suffix. Empty until `run` fills it after the RPC.
    accounts: Vec<(u64, String)>,
    /// Transit account prefix (from `transit.self`); `Some` = transit enabled.
    transit_prefix: Option<String>,
    /// Coin name (from `wallet.coin`) — the commodity part of a transit leaf,
    /// e.g. `monero` in `monero-kcwQ`.
    coin: String,
    /// Manual own↔own transits for accounts NOT on RPC (e.g. exchanges): (exact
    /// destination address, that account's leaf).
    transit_entries: Vec<(String, String)>,
    /// txid → the SENDER's primary address (from other wallets' `out` lists).
    /// A receive whose txid appears here came from that wallet. Filled by `run`.
    incoming_transits: HashMap<String, String>,
    /// txid → the RECIPIENT's primary address (from other wallets' `in` lists).
    /// A send whose txid appears here went to that wallet — matched by txid, so
    /// it works even when the recipient is a subaddress or the sending wallet
    /// cached no `destinations`. Filled by `run`.
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
                // `transit <destination-address> <other wallet leaf>`. Note
                // `transit.self` keeps its dot, so it falls through to the
                // directive branch below.
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
            Error::from("import: monero-rpc profile needs a 'fee => <account>' rule")
        })?;

        // Transit: prefix from `transit.self` (the whole value now — each
        // wallet's leaf is derived from its address, `<coin>-<last4>`). Manual
        // `transit <address> <leaf>` entries stay for accounts NOT on RPC.
        let transit_prefix = directives.get("transit.self").cloned();
        if !raw_transits.is_empty() && transit_prefix.is_none() {
            return Err(Error::from(
                "import: transit mappings need a 'transit.self' directive",
            ));
        }

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

        // Endpoint discovery: acc finds this wallet's wallet-rpc by scanning
        // `wallet.host` across `wallet.ports` and matching the primary address,
        // so the port is free to change and acc verifies it hit the right
        // wallet. All three are required — nothing is defaulted.
        let coin = get("wallet.coin")?;
        let wallet_address = get("wallet.address")?;
        let scan_host = get("wallet.host")?;
        let scan_ports = parse_ports(&get("wallet.ports")?)?;

        Ok(Profile {
            rpc_url: String::new(), // resolved by discovery in run()
            wallet_address,
            scan_host,
            scan_ports,
            output_file: expand(&get("output.file")?),
            title: get("output.title")?,
            account: get("output.account")?,
            commodity: get("output.commodity")?,
            fee_account,
            rules,
            default_account,
            accounts: Vec::new(),
            transit_prefix,
            coin,
            transit_entries: raw_transits,
            incoming_transits: HashMap::new(),
            outgoing_transits: HashMap::new(),
        })
    }

    /// Probe the scan range and map each reachable wallet-rpc's primary
    /// address to its URL.
    fn discover_endpoints(&self) -> HashMap<String, String> {
        let mut map = HashMap::new();
        for port in self.scan_ports.0..=self.scan_ports.1 {
            let url = format!("http://{}:{}/json_rpc", self.scan_host, port);
            if let Some(addr) = probe_address(&url) {
                map.insert(addr, url);
            }
        }
        map
    }

    /// The URL serving this profile's wallet: looked up by `wallet.address` —
    /// an error if not running, which also guarantees acc talks to the right
    /// wallet.
    fn resolve_own_url(&self, endpoints: &HashMap<String, String>) -> Result<String, Error> {
        endpoints.get(&self.wallet_address).cloned().ok_or_else(|| {
            Error::from(format!(
                "import: wallet {} not reachable on {}:{}-{} — is its wallet-rpc running?",
                self.wallet_address, self.scan_host, self.scan_ports.0, self.scan_ports.1
            ))
        })
    }

    fn render(&self, g: &Group) -> String {
        match (&g.incoming, &g.outgoing) {
            // In and out on the SAME account → a self-sweep/churn: the amount
            // returns to that account, only the fee is a real cost.
            (Some(inc), Some(out)) if inc.major == out.major => self.render_self(out),
            // In and out on DIFFERENT accounts → a move between two of this
            // wallet's own accounts: it leaves one, the fee is booked, it
            // lands in the other.
            (Some(inc), Some(out)) => self.render_transfer(inc, out),
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
        let counter = self.incoming_counter(t);
        let wallet = self.wallet_account(t.major);
        let mut s = self.header(t);
        s.push_str(&format!("\t{}  {}{}\n", wallet, self.commodity, xmr(t.amount)));
        s.push_str(&format!("\t{}", counter));
        s
    }

    /// Counter for a receive: if the txid matches a send from another of my
    /// wallets, book the SAME directional transit account (so both legs net) —
    /// the sender leaf comes from its address. Otherwise normal categorization.
    fn incoming_counter(&self, t: &Transfer) -> String {
        if let Some(sender) = self.incoming_transits.get(&t.txid)
            && let Some(acct) = self.transit_account(&self.transit_leaf(sender), false)
        {
            return acct;
        }
        self.categorize(t)
    }

    /// Send: the wallet loses `amount + fee`, the fee is its own posting, and
    /// the categorized counter gains `amount` — always the LAST posting.
    /// Three explicit postings, none inferred.
    fn render_out(&self, t: &Transfer) -> String {
        let counter = self.categorize(t);
        let wallet = self.wallet_account(t.major);
        let mut s = self.header(t);
        s.push_str(&format!(
            "\t{}  {}-{}\n",
            wallet,
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
        let wallet = self.wallet_account(t.major);
        let mut s = self.header(t);
        s.push_str(&format!(
            "\t{}  {}-{}\n",
            wallet,
            self.commodity,
            xmr(t.amount + t.fee)
        ));
        s.push_str(&format!("\t{}  {}{}\n", self.fee_account, self.commodity, xmr(t.fee)));
        s.push_str(&format!("\t{}  {}{}", wallet, self.commodity, xmr(t.amount)));
        s
    }

    /// A move between two of this wallet's own accounts: `amount + fee` leaves
    /// the `out` account, the fee is its own posting, and `amount` lands in the
    /// `in` account — always the LAST posting. Three explicit postings.
    fn render_transfer(&self, inc: &Transfer, out: &Transfer) -> String {
        let from = self.wallet_account(out.major);
        let to = self.wallet_account(inc.major);
        let mut s = self.header(out);
        s.push_str(&format!("\t{}  {}-{}\n", from, self.commodity, xmr(out.amount + out.fee)));
        s.push_str(&format!("\t{}  {}{}\n", self.fee_account, self.commodity, xmr(out.fee)));
        s.push_str(&format!("\t{}  {}{}", to, self.commodity, xmr(out.amount)));
        s
    }

    /// The ledger account for a wallet account (major index). A single-account
    /// wallet books to the bare `output.account`; a multi-account wallet
    /// appends `:label` — or `:index` when that account has no label — so each
    /// Monero account maps to its own ledger sub-account.
    fn wallet_account(&self, major: u64) -> String {
        if self.accounts.len() <= 1 {
            return self.account.clone();
        }
        let suffix = self
            .accounts
            .iter()
            .find(|(m, _)| *m == major)
            .map(|(_, label)| label.as_str())
            .filter(|l| !l.is_empty())
            .map(slug)
            .unwrap_or_else(|| major.to_string());
        format!("{}:{}", self.account, suffix)
    }

    /// The transit leaf for a wallet: `<coin>-<last 4 of its address>`, e.g.
    /// `monero-kcwQ`. Derivable from the address alone, so both legs of an
    /// internal transfer name the same account without reading another conf.
    fn transit_leaf(&self, address: &str) -> String {
        format!("{}-{}", self.coin, last4(address))
    }

    /// The directional transit account for a transfer to `other` (a leaf) —
    /// `None` when transit isn't configured (no `transit.self`).
    fn transit_account(&self, other: &str, outgoing: bool) -> Option<String> {
        self.transit_prefix.as_ref().map(|p| {
            super::directional_account(p, &self.transit_leaf(&self.wallet_address), other, outgoing)
        })
    }

    /// Counter account for a send: an internal transfer to another of my
    /// wallets — matched by TXID (its recipient), leaf derived from that
    /// wallet's address — or a manually-mapped non-RPC account (exchange)
    /// matched by destination address; then the first matching rule; else the
    /// default. Receives are matched by txid in `incoming_counter` instead.
    fn categorize(&self, t: &Transfer) -> String {
        if t.field("type") == "out" {
            let other = self
                .outgoing_transits
                .get(&t.txid)
                .map(|recipient| self.transit_leaf(recipient))
                .or_else(|| {
                    let dest = t.field("address");
                    self.transit_entries
                        .iter()
                        .find(|(a, _)| a == &dest)
                        .map(|(_, leaf)| leaf.clone())
                });
            if let Some(other) = other
                && let Some(acct) = self.transit_account(&other, true)
            {
                return acct;
            }
        }
        let tmpl = super::match_account(&self.rules, |f| t.field(f))
            .unwrap_or(self.default_account.as_str());
        self.template(tmpl, t)
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
    /// The wallet account (major index) this leg belongs to.
    major: u64,
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

/// One JSON-RPC round-trip to the wallet; returns the `result` object or an
/// error for transport, bad JSON, or an RPC-level `error`.
fn rpc_call(url: &str, method: &str, params: Value) -> Result<Value, Error> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(30))
        .build();
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": "0", "method": method, "params": params
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
    resp.get("result")
        .cloned()
        .ok_or_else(|| Error::from("import: monero-rpc: response has no result"))
}

/// Build the two transit maps from every OTHER reachable wallet's transfers,
/// keyed by txid (a shared txid is one and the same on-chain transaction, so
/// it uniquely links my wallets' views of an internal transfer):
///   - their `out` list → `incoming` (txid → SENDER address, for my receives),
///   - their `in`  list → `outgoing` (txid → RECIPIENT address, for my sends).
/// Purely RPC discovery; no other conf is read.
fn transit_maps(
    endpoints: &HashMap<String, String>,
    own: &str,
) -> (HashMap<String, String>, HashMap<String, String>) {
    let mut incoming = HashMap::new();
    let mut outgoing = HashMap::new();
    for (address, url) in endpoints {
        if address == own {
            continue;
        }
        let Ok(result) = rpc_call(
            url,
            "get_transfers",
            serde_json::json!({
                "in": true, "out": true, "pending": false, "failed": false,
                "pool": false, "all_accounts": true
            }),
        ) else {
            continue;
        };
        for (list, map) in [("out", &mut incoming), ("in", &mut outgoing)] {
            if let Some(arr) = result.get(list).and_then(|v| v.as_array()) {
                for obj in arr {
                    if let Some(txid) = obj.get("txid").and_then(|v| v.as_str()) {
                        map.entry(txid.to_string()).or_insert_with(|| address.clone());
                    }
                }
            }
        }
    }
    (incoming, outgoing)
}

/// The last 4 characters of an address — its short, human-readable tail (the
/// transit leaf name). Monero addresses are ASCII base58, so byte-slicing the
/// tail keeps UTF-8 valid.
fn last4(address: &str) -> &str {
    &address[address.len().saturating_sub(4)..]
}

/// Parse a `wallet.ports` directive `<start>-<end>` into `(start, end)`.
fn parse_ports(raw: &str) -> Result<(u16, u16), Error> {
    let err = || Error::from(format!("import: wallet.ports '{}' must be <start>-<end>", raw));
    let (start, end) = raw.split_once('-').ok_or_else(err)?;
    let start: u16 = start.trim().parse().map_err(|_| err())?;
    let end: u16 = end.trim().parse().map_err(|_| err())?;
    if start > end {
        return Err(err());
    }
    Ok((start, end))
}

/// Probe a wallet-rpc for the primary address of its open wallet (account 0).
/// Short timeout; `None` on any error so scanning closed ports stays cheap.
fn probe_address(url: &str) -> Option<String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(2))
        .build();
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": "0", "method": "get_address",
        "params": { "account_index": 0 }
    })
    .to_string();
    let text = agent
        .post(url)
        .set("Content-Type", "application/json")
        .send_string(&body)
        .ok()?
        .into_string()
        .ok()?;
    let resp: Value = serde_json::from_str(&text).ok()?;
    resp.get("result")?
        .get("address")?
        .as_str()
        .map(String::from)
}

/// Every wallet account as `(major index, label)`, via `get_accounts`. The
/// count decides whether accounts get a name suffix; the label supplies it.
fn fetch_accounts(url: &str) -> Result<Vec<(u64, String)>, Error> {
    let result = rpc_call(url, "get_accounts", serde_json::json!({}))?;
    let arr = result
        .get("subaddress_accounts")
        .and_then(|v| v.as_array())
        .ok_or_else(|| Error::from("import: monero-rpc get_accounts: no subaddress_accounts"))?;
    Ok(arr
        .iter()
        .map(|a| {
            let major = a.get("account_index").and_then(|v| v.as_u64()).unwrap_or(0);
            let label = a.get("label").and_then(|v| v.as_str()).unwrap_or("").to_string();
            (major, label)
        })
        .collect())
}

/// Call `get_transfers` and group the in/out lists by txid (oldest first).
fn fetch(url: &str) -> Result<Vec<Group>, Error> {
    // `all_accounts` pulls transfers from every wallet account (major index),
    // not just account 0 — without it a multi-account wallet's other accounts
    // are silently skipped.
    let result = rpc_call(
        url,
        "get_transfers",
        serde_json::json!({
            "in": true, "out": true, "pending": false, "failed": false,
            "pool": false, "all_accounts": true
        }),
    )?;

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
    let major = obj
        .get("subaddr_index")
        .and_then(|si| si.get("major"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let subaddr = obj
        .get("subaddr_index")
        .map(|si| {
            format!(
                "{}:{}",
                major,
                si.get("minor").and_then(|v| v.as_u64()).unwrap_or(0)
            )
        })
        .unwrap_or_default();
    // The address to categorize on: for a send, the RECIPIENT (first
    // destination); for a receive, the subaddress that received it. A restored
    // wallet may have no cached destinations for a send — then the recipient
    // is UNKNOWN (empty), NOT the wallet's own sending address (which would be
    // mistaken for a transfer to ourselves).
    let address = match dir {
        Dir::Out => obj
            .get("destinations")
            .and_then(|d| d.as_array())
            .and_then(|a| a.first())
            .and_then(|d| d.get("address"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
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
        major,
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
            wallet_address: String::new(),
            scan_host: "127.0.0.1".to_string(),
            scan_ports: (18082, 18099),
            output_file: PathBuf::new(),
            title: "monero".to_string(),
            account: "assets:xmr".to_string(),
            commodity: "XMR".to_string(),
            fee_account: "expenses:fees".to_string(),
            rules: Vec::new(),
            default_account: "expenses:unsorted".to_string(),
            accounts: Vec::new(),
            transit_prefix: None,
            coin: "monero".to_string(),
            transit_entries: Vec::new(),
            incoming_transits: HashMap::new(),
            outgoing_transits: HashMap::new(),
        }
    }

    // A wallet with three accounts: two labelled, one bare.
    fn multi() -> Profile {
        let mut p = profile();
        p.accounts = vec![
            (0, "Cold".to_string()),
            (1, "Hot".to_string()),
            (2, String::new()),
        ];
        p
    }

    // Amount `x` piconero, fee `f`, dated 2025-07-02, on account 0.
    fn transfer(x: i128, f: i128) -> Transfer {
        Transfer {
            txid: "t".to_string(),
            amount: x,
            fee: f,
            major: 0,
            timestamp: 1_751_495_027,
            fields: HashMap::new(),
            raw: serde_json::json!({ "txid": "t", "amount": x as i64 }),
        }
    }

    // As `transfer`, but on a specific wallet account (major index).
    fn at_major(x: i128, f: i128, major: u64) -> Transfer {
        let mut t = transfer(x, f);
        t.major = major;
        t
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

    #[test]
    fn single_account_wallet_has_no_suffix() {
        // One account → bare name, even if that account carries a label.
        let mut p = profile();
        p.accounts = vec![(0, "Cold".to_string())];
        assert_eq!(p.wallet_account(0), "assets:xmr");
    }

    #[test]
    fn multi_account_uses_label_then_index() {
        let p = multi();
        assert_eq!(p.wallet_account(0), "assets:xmr:cold"); // label
        assert_eq!(p.wallet_account(1), "assets:xmr:hot"); // label
        assert_eq!(p.wallet_account(2), "assets:xmr:2"); // unlabelled → index
    }

    #[test]
    fn receive_into_second_account_books_to_its_subaccount() {
        let s = multi().render(&group(Some(at_major(130_364_401_518, 0, 1)), None));
        assert!(s.contains("assets:xmr:hot  XMR0.130364401518"));
    }

    #[test]
    fn move_between_accounts_leaves_one_lands_in_the_other() {
        // in on major 1, out on major 0, same txid → an inter-account move,
        // not a self-sweep: it leaves account 0 and lands in account 1.
        let inc = at_major(16_340_000_000, 0, 1);
        let out = at_major(16_340_000_000, 30_580_000, 0);
        let s = multi().render(&group(Some(inc), Some(out)));
        assert!(s.contains("assets:xmr:cold  XMR-0.016370580000")); // out of account 0
        assert!(s.contains("expenses:fees  XMR0.000030580000")); // fee
        assert!(s.trim_end().ends_with("assets:xmr:hot  XMR0.016340000000")); // into account 1
    }

    // Own wallet address ends in "cold"; a sibling RPC wallet ends in "warm";
    // plus a manual (non-RPC) exchange address. Leaf = coin + last4.
    fn transit_profile() -> Profile {
        let mut p = profile();
        p.wallet_address = "OWNADDR_cold".to_string();
        p.transit_prefix = Some("assets:transit".to_string());
        p.transit_entries = vec![("EXCHANGE_ADDR".to_string(), "extern".to_string())];
        p
    }

    fn out_to(address: &str) -> Transfer {
        let mut t = transfer(16_340_000_000, 30_580_000);
        t.fields.insert("type".to_string(), "out".to_string());
        t.fields.insert("address".to_string(), address.to_string());
        t
    }

    #[test]
    fn last4_takes_the_address_tail() {
        assert_eq!(last4("xxxxxxxxkcwQ"), "kcwQ");
        assert_eq!(last4("ab"), "ab"); // shorter than 4 → whole string
    }

    #[test]
    fn send_matched_by_txid_books_directional_transit() {
        // txid in outgoing_transits (recipient = another of my wallets) →
        // transit, leaf from the recipient's address. The destination need NOT
        // be known — matched by txid, not address (the whole point).
        let mut p = transit_profile();
        p.outgoing_transits.insert("t".to_string(), "SIBADDR_warm".to_string());
        let s = p.render(&group(None, Some(out_to(""))));
        assert!(s.trim_end().ends_with("assets:transit:monero-cold:monero-warm  XMR0.016340000000"));
        assert!(s.contains("assets:xmr  XMR-0.016370580000")); // wallet out
        assert!(s.contains("expenses:fees  XMR0.000030580000")); // fee still booked
    }

    #[test]
    fn send_to_manual_exchange_books_transit() {
        // No txid match, but the destination matches a manual `transit` entry
        // (a non-RPC account) → its leaf is used as-is.
        let s = transit_profile().render(&group(None, Some(out_to("EXCHANGE_ADDR"))));
        assert!(s.trim_end().ends_with("assets:transit:monero-cold:extern  XMR0.016340000000"));
    }

    #[test]
    fn send_without_txid_or_entry_is_not_transit() {
        // No txid match and no manual entry (destination unknown) → default.
        // This is the case that used to be mis-booked as own:own.
        let s = transit_profile().render(&group(None, Some(out_to(""))));
        assert!(!s.contains("transit"));
        assert!(s.trim_end().ends_with("expenses:unsorted  XMR0.016340000000"));
    }

    #[test]
    fn receive_matched_by_txid_books_directional_transit() {
        // A receive whose txid is in the map came from that sender wallet →
        // leaf from the sender's address, incoming → other:own, matching the
        // send leg (monero-warm:monero-cold) so both net to 0.
        let mut p = transit_profile();
        p.incoming_transits.insert("t".to_string(), "SIBADDR_warm".to_string());
        let s = p.render(&group(Some(transfer(130_364_401_518, 0)), None));
        assert!(s.contains("assets:xmr  XMR0.130364401518"));
        assert!(s.trim_end().ends_with("assets:transit:monero-warm:monero-cold"));
    }

    #[test]
    fn receive_without_txid_match_is_unsorted() {
        // No matching send for this txid → normal categorization (default).
        let s = transit_profile().render(&group(Some(transfer(130_364_401_518, 0)), None));
        assert!(!s.contains("transit"));
        assert!(s.trim_end().ends_with("expenses:unsorted"));
    }

    #[test]
    fn parse_ports_parses_and_rejects() {
        assert_eq!(parse_ports("18082-18099").unwrap(), (18082, 18099));
        assert!(parse_ports("nonsense").is_err());
        assert!(parse_ports("18099-18082").is_err()); // reversed
        assert!(parse_ports("18082").is_err()); // no range
    }

    #[test]
    fn resolve_own_url_by_address() {
        let mut p = profile();
        p.wallet_address = "ADDR".to_string();
        let mut eps = HashMap::new();
        eps.insert("ADDR".to_string(), "http://127.0.0.1:18085/json_rpc".to_string());
        assert_eq!(p.resolve_own_url(&eps).unwrap(), "http://127.0.0.1:18085/json_rpc");
        // not discovered (wallet not running) → error
        assert!(p.resolve_own_url(&HashMap::new()).is_err());
    }
}
