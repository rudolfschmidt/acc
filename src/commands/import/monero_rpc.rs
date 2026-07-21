//! `wallet.coin monero` import backend.
//!
//! Pulls the wallet's transfers straight from a local `monero-wallet-rpc`
//! (`get_transfers` over HTTP JSON-RPC) and turns each into a ledger
//! transaction. Every entry carries its COMPLETE RPC object as a `; rpc: {…}`
//! comment right after the header; dedup reads the `txid` back out of it.
//! Amounts are atomic piconero, written at full 12-decimal length.
//!
//! The tx model, categorization, and rendering are the shared crypto-wallet
//! core ([`super::crypto_lib`]); this backend only supplies the monero specifics:
//! endpoint discovery, `get_transfers` parsing, and the tx-shape dispatch (a
//! self-sweep / inter-account move / fee-only churn have no Bitcoin analogue).

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;

use crate::error::Error;

use super::crypto_lib::{aggregate, existing_txids, Group, Tx, Wallet};
use super::{expand, read, Match, Rule};

/// The transfer fields a categorization rule may match on.
const FIELDS: &[&str] = &["type", "txid", "address", "subaddr", "payment_id", "note"];
/// Decimal places XMR is written at (piconero, 10^12).
const DECIMALS: u32 = 12;

pub fn run(conf_path: &str, write: bool) -> Result<(), Error> {
    let mut profile = Profile::load(conf_path)?;
    // Discover the reachable wallet-rpcs by primary address. Cross-wallet
    // transit is matched purely by TXID against the OTHER running wallets — no
    // other conf is read, and it works even when a send cached no `destinations`
    // or went to a subaddress. A shared txid is an internal transfer: their
    // `out` list gives my receives (its sender), their `in` list my sends (its
    // recipient). Any wallet's leaf is `<coin>-<last4 of address>`, so both legs
    // build the same account and net to 0.
    let endpoints = profile.discover_endpoints();
    let own = profile.wallet_address.clone();
    let (incoming, outgoing) = transit_maps(&endpoints, &own, &profile.coin, profile.login.as_deref());
    profile.wallet.incoming_transits = incoming;
    profile.wallet.outgoing_transits = outgoing;

    // Every wallet's transfers come from its wallet-rpc via get_transfers (a
    // Haveno wallet uses its internal one, reached with the digest login) — so
    // the `; rpc:` source is uniform across wallets.
    profile.rpc_url = profile.resolve_own_url(&endpoints)?;
    profile.wallet.accounts = fetch_accounts(&profile.rpc_url, profile.login.as_deref())?;
    let groups = fetch(&profile.rpc_url, profile.login.as_deref())?;

    // A `haveno.*` block additionally pulls the completed trades; each trade leg
    // (a transfer matched by txid) then renders as a reto swap booking sourced
    // wholly from the trade. Every other transfer stays a plain monero booking.
    let enricher = match &profile.haveno {
        Some(cfg) => Some(super::monero_haveno_rpc::Enricher::fetch(
            cfg,
            &profile.wallet.account,
            &profile.wallet.fee_account,
            &profile.wallet.title,
        )?),
        None => None,
    };

    let existing = std::fs::read_to_string(&profile.output_file).unwrap_or_default();
    let seen = existing_txids(&existing);

    let mut blocks = Vec::new();
    let mut skipped = 0usize;
    for g in &groups {
        if seen.contains(&g.txid) {
            skipped += 1;
            continue;
        }
        // A trade leg (deposit/payout txid) renders as its reto booking; any
        // other transfer falls through to the normal monero rendering.
        let enriched = match &enricher {
            Some(e) => e.render(g)?,
            None => None,
        };
        blocks.push(enriched.unwrap_or_else(|| profile.render(g)));
    }

    super::emit(&blocks, groups.len(), "transfers", &existing, &profile.output_file, skipped, write)
}

// ---------------------------------------------------------------------
// profile
// ---------------------------------------------------------------------

struct Profile {
    /// The shared crypto-wallet core: config + render/categorize/transit.
    wallet: Wallet,
    rpc_url: String,        // resolved endpoint (filled by discovery in run)
    wallet_address: String, // primary address — identity + discovery key
    scan_host: String,      // host probed for the wallet-rpc endpoint
    scan_ports: (u16, u16), // inclusive port range to probe
    output_file: PathBuf,
    /// Coin name (from `wallet.coin`) — the commodity part of a transit leaf,
    /// e.g. `monero` in `monero-a1b2`. Used to key the transit maps in `run`.
    coin: String,
    /// Haveno reto enrichment, present only when the profile carries a
    /// `haveno.*` block. Trade-leg transfers then render as reto swap bookings
    /// instead of plain transfers; everything else is unaffected.
    haveno: Option<super::monero_haveno_rpc::Config>,
    /// `user:pass` for wallet-rpcs behind an HTTP digest login (Haveno's
    /// internal one). Sent only on a `401`, so login-less wallets are unaffected.
    login: Option<String>,
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
        let haveno = super::monero_haveno_rpc::Config::parse(&directives)?;
        let login = directives.get("wallet.login").cloned();

        let wallet = Wallet {
            title: get("output.title")?,
            account: get("output.account")?,
            commodity: get("output.commodity")?,
            decimals: DECIMALS,
            fee_account,
            rules,
            default_account,
            transit_prefix,
            // This wallet's own transit leaf, derived from its address once so
            // the shared code needs no coin-specific derivation.
            own_leaf: format!("{}-{}", coin, last4(&wallet_address)),
            transit_entries: raw_transits,
            accounts: Vec::new(),
            incoming_transits: HashMap::new(),
            outgoing_transits: HashMap::new(),
        };

        Ok(Profile {
            wallet,
            rpc_url: String::new(), // resolved by discovery in run()
            wallet_address,
            scan_host,
            scan_ports,
            output_file: expand(&get("output.file")?),
            coin,
            haveno,
            login,
        })
    }

    /// Probe the scan range and map each reachable wallet-rpc's primary
    /// address to its URL.
    fn discover_endpoints(&self) -> HashMap<String, String> {
        let mut map = HashMap::new();
        for port in self.scan_ports.0..=self.scan_ports.1 {
            let url = format!("http://{}:{}/json_rpc", self.scan_host, port);
            if let Some(addr) = probe_address(&url, self.login.as_deref()) {
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

    /// Dispatch one grouped transaction to the shared renderer by its shape.
    /// The zero-amount cases (a churn/multisig tx that moved nothing to others)
    /// book fee-only; every other shape is a monero specific the shared core
    /// draws for us.
    fn render(&self, g: &Group) -> String {
        let w = &self.wallet;
        match (&g.receive, &g.send) {
            // In and out on the SAME account → a self-sweep/churn: the amount
            // returns to that account, only the fee is a real cost.
            (Some(inc), Some(out)) if inc.major == out.major => {
                if out.amount == 0 { w.render_fee_only(out) } else { w.render_self(out) }
            }
            // In and out on DIFFERENT accounts → a move between two of this
            // wallet's own accounts: it leaves one, the fee is booked, it
            // lands in the other.
            (Some(inc), Some(out)) => w.render_transfer(inc, out),
            // Only outgoing → a send you created. Nothing moved to others
            // (amount 0) → a churn whose only cost is the network fee.
            (_, Some(out)) => {
                if out.amount == 0 { w.render_fee_only(out) } else { w.render_out(out) }
            }
            // Only incoming → a genuine receive. You did NOT create it (no
            // outgoing leg, no tx key), so the `fee` shown is the sender's
            // metadata, not your cost — book amount only.
            (Some(inc), None) => w.render_in(inc),
            (None, None) => String::new(),
        }
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

/// Build the two transit maps from every OTHER reachable wallet's transfers,
/// keyed by txid (a shared txid is one and the same on-chain transaction, so
/// it uniquely links my wallets' views of an internal transfer):
///   - their `out` list → `incoming` (txid → SENDER leaf, for my receives),
///   - their `in`  list → `outgoing` (txid → RECIPIENT leaf, for my sends).
/// Each leaf is `<coin>-<last4 of that wallet's address>`, computed here so the
/// shared code needs no coin knowledge. Purely RPC discovery; no conf is read.
fn transit_maps(
    endpoints: &HashMap<String, String>,
    own: &str,
    coin: &str,
    login: Option<&str>,
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
            login,
        ) else {
            continue;
        };
        let leaf = format!("{}-{}", coin, last4(address));
        for (list, map) in [("out", &mut incoming), ("in", &mut outgoing)] {
            if let Some(arr) = result.get(list).and_then(|v| v.as_array()) {
                for obj in arr {
                    if let Some(txid) = obj.get("txid").and_then(|v| v.as_str()) {
                        map.entry(txid.to_string()).or_insert_with(|| leaf.clone());
                    }
                }
            }
        }
    }
    (incoming, outgoing)
}

/// One JSON-RPC round-trip to a monero-wallet-rpc, via the shared client.
/// `login` (`user:pass`) drives the HTTP digest handshake for a login-protected
/// wallet-rpc (Haveno's internal one); login-less wallets ignore it.
fn rpc_call(url: &str, method: &str, params: Value, login: Option<&str>) -> Result<Value, Error> {
    let auth = login.map(super::rpc::Auth::Digest).unwrap_or(super::rpc::Auth::None);
    super::rpc::call(url, method, params, &auth, "2.0", Duration::from_secs(30))
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
fn probe_address(url: &str, login: Option<&str>) -> Option<String> {
    let auth = login.map(super::rpc::Auth::Digest).unwrap_or(super::rpc::Auth::None);
    let result = super::rpc::call(
        url,
        "get_address",
        serde_json::json!({ "account_index": 0 }),
        &auth,
        "2.0",
        Duration::from_secs(2),
    )
    .ok()?;
    result.get("address")?.as_str().map(String::from)
}

/// Every wallet account as `(major index, label)`, via `get_accounts`. The
/// count decides whether accounts get a name suffix; the label supplies it.
fn fetch_accounts(url: &str, login: Option<&str>) -> Result<Vec<(u64, String)>, Error> {
    let result = rpc_call(url, "get_accounts", serde_json::json!({}), login)?;
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
fn fetch(url: &str, login: Option<&str>) -> Result<Vec<Group>, Error> {
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
        login,
    )?;

    // `get_transfers` lists one entry per OUTPUT, so a transaction with
    // several outputs to this wallet appears as several entries sharing a
    // txid (e.g. a real amount + a 0-value padding output). Collect the legs
    // per txid+direction and aggregate them.
    let mut ins: HashMap<String, Vec<Tx>> = HashMap::new();
    let mut outs: HashMap<String, Vec<Tx>> = HashMap::new();
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
        let receive = ins.remove(&txid).map(aggregate);
        let send = outs.remove(&txid).map(aggregate);
        let time = receive
            .as_ref()
            .or(send.as_ref())
            .map(|t| t.time)
            .unwrap_or(0);
        groups.push(Group { txid, time, receive, send });
    }
    groups.sort_by(|a, b| a.time.cmp(&b.time).then(a.txid.cmp(&b.txid)));
    Ok(groups)
}

fn parse_transfer(obj: &Value, dir: Dir) -> Result<Tx, Error> {
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

    let category = match dir {
        Dir::In => "in",
        Dir::Out => "out",
    }
    .to_string();
    let mut fields = HashMap::new();
    fields.insert("type".to_string(), category.clone());
    fields.insert("txid".to_string(), txid.clone());
    fields.insert("address".to_string(), address);
    fields.insert("subaddr".to_string(), subaddr);
    fields.insert("payment_id".to_string(), str_of("payment_id"));
    fields.insert("note".to_string(), str_of("note"));

    Ok(Tx {
        txid,
        category,
        amount: atomic(obj.get("amount")),
        fee: atomic(obj.get("fee")),
        major,
        time: obj.get("timestamp").and_then(|v| v.as_u64()).unwrap_or(0),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> Profile {
        Profile {
            wallet: Wallet {
                title: "monero".to_string(),
                account: "assets:xmr".to_string(),
                commodity: "XMR".to_string(),
                decimals: DECIMALS,
                fee_account: "expenses:fees".to_string(),
                rules: Vec::new(),
                default_account: "expenses:unsorted".to_string(),
                transit_prefix: None,
                own_leaf: String::new(),
                transit_entries: Vec::new(),
                accounts: Vec::new(),
                incoming_transits: HashMap::new(),
                outgoing_transits: HashMap::new(),
            },
            rpc_url: String::new(),
            wallet_address: String::new(),
            scan_host: "127.0.0.1".to_string(),
            scan_ports: (18082, 18099),
            output_file: PathBuf::new(),
            coin: "monero".to_string(),
            haveno: None,
            login: None,
        }
    }

    // A wallet with three accounts: two labelled, one bare.
    fn multi() -> Profile {
        let mut p = profile();
        p.wallet.accounts = vec![
            (0, "Cold".to_string()),
            (1, "Hot".to_string()),
            (2, String::new()),
        ];
        p
    }

    // Amount `x` piconero, fee `f`, dated 2025-07-02, on account 0, incoming.
    fn transfer(x: i128, f: i128) -> Tx {
        Tx {
            txid: "t".to_string(),
            category: "in".to_string(),
            amount: x,
            fee: f,
            major: 0,
            time: 1_751_495_027,
            fields: HashMap::new(),
            raw: serde_json::json!({ "txid": "t", "amount": x as i64 }),
        }
    }

    // As `transfer`, but on a specific wallet account (major index).
    fn at_major(x: i128, f: i128, major: u64) -> Tx {
        let mut t = transfer(x, f);
        t.major = major;
        t
    }

    #[test]
    fn atomic_reads_json_number() {
        let v = serde_json::json!({ "amount": 130_364_401_518_u64 });
        assert_eq!(atomic(v.get("amount")), 130_364_401_518);
        assert_eq!(atomic(None), 0);
    }

    fn group(inc: Option<Tx>, out: Option<Tx>) -> Group {
        Group { txid: "t".to_string(), time: 1_751_495_027, receive: inc, send: out }
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
        let mut out = transfer(16_340_000_000, 30_580_000);
        out.category = "out".to_string();
        let s = profile().render(&group(None, Some(out)));
        assert!(s.contains("assets:xmr  XMR-0.016370580000")); // amount + fee out
        assert!(s.contains("expenses:fees  XMR0.000030580000"));
        assert!(s.trim_end().ends_with("expenses:unsorted  XMR0.016340000000")); // counter last, filled
    }

    #[test]
    fn zero_amount_out_is_fee_only() {
        // A churn/multisig tx (amount 0, only a fee) → two postings: the wallet
        // loses the fee, the fee account is left bare to auto-balance.
        let s = profile().render(&group(None, Some(transfer(0, 30_740_000))));
        assert!(s.contains("assets:xmr  XMR-0.000030740000")); // wallet loses the fee
        assert!(s.trim_end().ends_with("expenses:fees")); // fee account bare, last posting
        assert!(!s.contains("expenses:unsorted")); // no counter
        assert!(!s.contains("expenses:fees  XMR")); // fee posting carries no amount
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
        p.wallet.accounts = vec![(0, "Cold".to_string())];
        // Rendered through a receive so we exercise the shared wallet_account.
        let s = p.render(&group(Some(transfer(1, 0)), None));
        assert!(s.contains("assets:xmr  XMR0.000000000001"));
        assert!(!s.contains("assets:xmr:"));
    }

    #[test]
    fn multi_account_uses_label_then_index() {
        let p = multi();
        assert!(p.render(&group(Some(at_major(1, 0, 0)), None)).contains("assets:xmr:cold  XMR")); // label
        assert!(p.render(&group(Some(at_major(1, 0, 1)), None)).contains("assets:xmr:hot  XMR")); // label
        assert!(p.render(&group(Some(at_major(1, 0, 2)), None)).contains("assets:xmr:2  XMR")); // unlabelled → index
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

    // Own wallet leaf is `monero-cold`; a sibling RPC wallet is `monero-warm`;
    // plus a manual (non-RPC) exchange leaf `extern`.
    fn transit_profile() -> Profile {
        let mut p = profile();
        p.wallet.own_leaf = "monero-cold".to_string();
        p.wallet.transit_prefix = Some("assets:transit".to_string());
        p.wallet.transit_entries = vec![("EXCHANGE_ADDR".to_string(), "extern".to_string())];
        p
    }

    fn out_to(address: &str) -> Tx {
        let mut t = transfer(16_340_000_000, 30_580_000);
        t.category = "out".to_string();
        t.fields.insert("type".to_string(), "out".to_string());
        t.fields.insert("address".to_string(), address.to_string());
        t
    }

    #[test]
    fn last4_takes_the_address_tail() {
        assert_eq!(last4("xxxxxxxxa1b2"), "a1b2");
        assert_eq!(last4("ab"), "ab"); // shorter than 4 → whole string
    }

    #[test]
    fn send_matched_by_txid_books_directional_transit() {
        // txid in outgoing_transits (recipient = another of my wallets) →
        // transit, leaf from the recipient's wallet. The destination need NOT
        // be known — matched by txid, not address (the whole point).
        let mut p = transit_profile();
        p.wallet.outgoing_transits.insert("t".to_string(), "monero-warm".to_string());
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
        // incoming → other:own, matching the send leg (monero-warm:monero-cold)
        // so both net to 0.
        let mut p = transit_profile();
        p.wallet.incoming_transits.insert("t".to_string(), "monero-warm".to_string());
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
