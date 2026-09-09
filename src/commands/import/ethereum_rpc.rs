//! Ethereum wallet import backend (`wallet.coin ethereum`) — keyless, over a
//! plain JSON-RPC endpoint. No indexer account (Etherscan etc.) is needed:
//! every ERC-20 transfer writes a `Transfer(address,address,uint256)` event
//! whose sender and recipient are indexed topics, so `eth_getLogs` filtered by
//! the wallet address enumerates the token history completely. The backend
//! scans each configured `token` contract in block chunks from `wallet.start`
//! (continuing from the highest block already imported), reads each token's
//! `decimals()` from the contract, and fetches the receipt of every own send
//! for the gas fee — which is paid in ETH, so a token booking carries two
//! commodities.
//!
//! Native ETH writes no event, so a plain endpoint cannot LIST it — but it can
//! be FOUND: the balance history is bisectable. [`Profile::discover_native`]
//! models the expected balance from the known bookings, binary-searches every
//! height where the chain disagrees (~13 `eth_getBalance` reads per movement),
//! and reads the movement's transaction out of that block's body. `ethtx
//! <hash>` lines remain as the manual override for what discovery cannot name
//! (internal contract transfers, self-masking movement pairs). A balance check
//! at the end compares the booked sums per commodity against `eth_getBalance` /
//! `balanceOf` and reports any remaining gap instead of letting it slip
//! through silently.
//!
//! Booking shapes (counter last, 2 postings leave the counter bare):
//!   token receive — wallet +TOKEN, counter (the fee was the sender's)
//!   token send    — wallet -TOKEN, wallet -ETH fee, fee account +ETH, counter +TOKEN
//!   native in     — wallet +ETH, counter
//!   native out    — wallet -(value+fee) ETH, fee account +fee, counter +value
//!   failed send   — wallet -fee ETH, fee account (the revert still burned gas)
//!
//! For the relay transport, `HTTPS_PROXY`/`ALL_PROXY` (e.g. `socks5://…`) is
//! honoured for every request of the run — this backend only, the local-daemon
//! backends must never be proxied.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;

use crate::error::Error;

use super::crypto_lib::money;
use super::{directional_account, expand, match_account, read, Match, Rule};

/// The leg fields a categorization rule may match on.
const FIELDS: &[&str] = &["type", "token", "address", "txid"];
/// keccak256("Transfer(address,address,uint256)") — topic0 of every ERC-20 transfer.
const TRANSFER_SIG: &str = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";
/// Wei per ETH is 10^18; ETH amounts are written at full 18-decimal length.
const ETH_DECIMALS: u32 = 18;
/// Blocks per `eth_getLogs` window — public endpoints cap the range, and the
/// cap differs per provider, so it is a profile knob (`wallet.chunk`).
const DEFAULT_CHUNK: u64 = 50_000;

pub fn run(conf_path: &str, write: bool) -> Result<(), Error> {
    let profile = Profile::load(conf_path)?;
    let existing = std::fs::read_to_string(&profile.output_file).unwrap_or_default();
    let (seen, cursor) = existing_keys(&existing);

    let latest = hex_u64(&profile.rpc("eth_blockNumber", serde_json::json!([]))?)?;
    let from = match cursor {
        // Re-scan the highest imported block itself — it may hold further logs;
        // dedup skips the ones already present.
        Some(top) => top.max(profile.start),
        None => profile.start,
    };

    let tokens = profile.resolve_tokens()?;
    let mut bookings = profile.scan_tokens(&tokens, from, latest, &seen)?;
    bookings.extend(profile.fetch_ethtx(&seen, &bookings)?);
    profile.discover_native(from, latest, &mut bookings, &seen)?;
    profile.stamp_times(&mut bookings)?;
    bookings.sort_by(|a, b| a.time.cmp(&b.time).then(a.txhash.cmp(&b.txhash)));

    let blocks: Vec<String> = bookings.iter().map(|b| profile.render(b)).collect();
    let read_count = bookings.len() + seen.len();

    profile.balance_check(&tokens, &existing, &bookings)?;
    super::emit(&blocks, read_count, "transfers", &existing, &profile.output_file, seen.len(), write)
}

// ---------------------------------------------------------------------
// profile
// ---------------------------------------------------------------------

/// A `token <SYMBOL> <contract>` line, with `decimals()` read from the contract.
struct Token {
    symbol: String,
    contract: String, // lowercased 0x…
    decimals: u32,
}

/// One booking: all the transfer legs sharing a tx hash, plus the gas fee when
/// this wallet sent the transaction. A native `ethtx` movement is a single
/// ETH leg. `sources` are the verbatim RPC objects for the `; rpc:` lines.
struct Booking {
    txhash: String,
    block: u64,
    time: u64,
    legs: Vec<Leg>,
    fee: i128, // wei; 0 when the fee was not ours
    failed: bool,
    sources: Vec<Value>,
}

/// One commodity movement of the wallet. `amount` is signed atomic units
/// (positive = the wallet gains). `own_side` marks a self-transfer leg, which
/// gets no counter posting (its pair nets on the wallet account).
struct Leg {
    symbol: String,
    decimals: u32,
    amount: i128,
    counterparty: String, // lowercased 0x… of the other side
    own_side: bool,
    fields: HashMap<String, String>,
}

struct Profile {
    rpc_url: String,
    proxy: Option<String>,
    address: String, // lowercased 0x…
    start: u64,
    chunk: u64,
    token_lines: Vec<(String, String)>, // (symbol, contract)
    ethtx: Vec<String>,
    title: String,
    account: String,
    fee_account: String,
    rules: Vec<Rule>,
    default_account: String,
    transit_prefix: Option<String>,
    own_leaf: String,
    transit_entries: Vec<(String, String)>, // (lowercased address, leaf)
    output_file: PathBuf,
}

impl Profile {
    fn load(path: &str) -> Result<Profile, Error> {
        let src = read(path)?;
        let mut directives: HashMap<String, String> = HashMap::new();
        let mut raw_rules: Vec<(String, String)> = Vec::new();
        let mut transits: Vec<(String, String)> = Vec::new();
        let mut token_lines: Vec<(String, String)> = Vec::new();
        let mut ethtx: Vec<String> = Vec::new();
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
                continue;
            }
            // Key and value separate on ANY whitespace — the profiles indent
            // with tabs, so a `"token "` prefix test would silently miss them.
            let Some((key, rest)) = line.split_once(char::is_whitespace) else {
                continue;
            };
            let rest = rest.trim();
            match key {
                "transit" => {
                    let (addr, leaf) = rest.rsplit_once(char::is_whitespace).ok_or_else(|| {
                        Error::from(format!("import: transit '{}' is not <address> <account>", rest))
                    })?;
                    transits.push((addr.trim().to_lowercase(), leaf.trim().to_string()));
                }
                "token" => {
                    let (symbol, contract) = rest.split_once(char::is_whitespace).ok_or_else(|| {
                        Error::from(format!("import: token '{}' is not <SYMBOL> <contract>", rest))
                    })?;
                    token_lines.push((symbol.trim().to_string(), contract.trim().to_lowercase()));
                }
                "ethtx" => ethtx.push(rest.to_lowercase()),
                _ => {
                    directives.insert(key.to_string(), rest.to_string());
                }
            }
        }

        let get = |key: &str| -> Result<String, Error> {
            directives
                .get(key)
                .cloned()
                .ok_or_else(|| Error::from(format!("import: missing '{}' in profile", key)))
        };

        let fee_account = fee_account
            .ok_or_else(|| Error::from("import: ethereum profile needs a 'fee => <account>' rule"))?;

        let transit_prefix = directives.get("transit.self").cloned();
        if !transits.is_empty() && transit_prefix.is_none() {
            return Err(Error::from("import: transit mappings need a 'transit.self' directive"));
        }

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
                        "import: rule field '{}' is not an ethereum transfer field ({})",
                        fname,
                        FIELDS.join(", ")
                    )));
                }
                let (mode, core) = Match::parse(val.trim());
                conds.push((fname.to_string(), core.to_lowercase(), mode));
            }
            rules.push(Rule { conds, account: acc });
        }

        let address = get("wallet.address")?.to_lowercase();
        if !address.starts_with("0x") || address.len() != 42 {
            return Err(Error::from(format!(
                "import: wallet.address '{}' is not a 0x… ethereum address",
                address
            )));
        }
        let start: u64 = get("wallet.start")?
            .parse()
            .map_err(|_| Error::from("import: wallet.start must be a decimal block number"))?;
        let chunk: u64 = match directives.get("wallet.chunk") {
            Some(c) => c
                .parse()
                .map_err(|_| Error::from("import: wallet.chunk must be a decimal block count"))?,
            None => DEFAULT_CHUNK,
        };

        // The wallet's transit leaf, `ethereum-<last 4 of the address>` —
        // matching the account-name convention for multi-currency accounts.
        let own_leaf = format!("ethereum-{}", &address[address.len() - 4..]);

        // Proxy for THIS backend's requests only — the relay transport. The
        // local-daemon backends (bitcoind, monero-wallet-rpc) stay direct.
        let proxy = ["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"]
            .iter()
            .find_map(|k| std::env::var(k).ok())
            .filter(|v| !v.trim().is_empty());

        Ok(Profile {
            rpc_url: get("wallet.rpc")?,
            proxy,
            address,
            start,
            chunk,
            token_lines,
            ethtx,
            title: get("output.title")?,
            account: get("output.account")?,
            fee_account,
            rules,
            default_account,
            transit_prefix,
            own_leaf,
            transit_entries: transits,
            output_file: expand(&get("output.file")?),
        })
    }

    /// One JSON-RPC round-trip to the endpoint, over the proxy when one is set.
    /// The method is folded into any error — a provider's refusal ("archive
    /// requests require…") is useless without knowing which call it refused.
    /// A public gateway throttles bursts (a scan is nothing else), so a
    /// rate-limit answer is waited out with growing pauses, not surfaced.
    fn rpc(&self, method: &str, params: Value) -> Result<Value, Error> {
        let mut wait = 2u64;
        for attempt in 0..7 {
            let result = super::rpc_lib::call_via(
                self.proxy.as_deref(),
                &self.rpc_url,
                method,
                params.clone(),
                &super::rpc_lib::Auth::None,
                "2.0",
                Duration::from_secs(60),
            );
            match result {
                Err(e) if attempt < 6 && is_throttled(&e) => {
                    eprintln!("import: {} rate limited — retrying in {}s", method, wait);
                    std::thread::sleep(Duration::from_secs(wait));
                    wait = (wait * 2).min(60);
                }
                other => return other.map_err(|e| Error::from(format!("{} ({})", e, method))),
            }
        }
        unreachable!("the last attempt returns above");
    }

    /// Read each configured token's `decimals()` from its contract — the
    /// contract knows, so the profile can't drift from the chain.
    fn resolve_tokens(&self) -> Result<Vec<Token>, Error> {
        let mut tokens = Vec::new();
        for (symbol, contract) in &self.token_lines {
            let result = self.rpc(
                "eth_call",
                serde_json::json!([{ "to": contract, "data": "0x313ce567" }, "latest"]),
            )?;
            let decimals = hex_u64(&result).map_err(|e| {
                Error::from(format!("import: token {} decimals(): {}", symbol, e))
            })? as u32;
            tokens.push(Token { symbol: symbol.clone(), contract: contract.clone(), decimals });
        }
        Ok(tokens)
    }

    /// Scan every token contract's Transfer logs touching the wallet, in block
    /// chunks, both topic positions (as sender and as recipient), and group
    /// the new legs by tx hash. A self-transfer matches both queries but is
    /// keyed once (txhash:logIndex) and yields a -/+ leg pair.
    fn scan_tokens(
        &self,
        tokens: &[Token],
        from: u64,
        latest: u64,
        seen: &HashSet<String>,
    ) -> Result<Vec<Booking>, Error> {
        let topic_addr = pad_topic(&self.address);
        let mut logs: HashMap<String, (usize, Value)> = HashMap::new(); // key → (token idx, log)
        let mut lo = from;
        while lo <= latest {
            let hi = (lo + self.chunk - 1).min(latest);
            for (i, token) in tokens.iter().enumerate() {
                for topics in [
                    serde_json::json!([TRANSFER_SIG, topic_addr]),
                    serde_json::json!([TRANSFER_SIG, Value::Null, topic_addr]),
                ] {
                    let result = self.rpc(
                        "eth_getLogs",
                        serde_json::json!([{
                            "fromBlock": to_hex(lo), "toBlock": to_hex(hi),
                            "address": token.contract, "topics": topics,
                        }]),
                    )?;
                    for log in result.as_array().into_iter().flatten() {
                        if log.get("removed").and_then(|v| v.as_bool()).unwrap_or(false) {
                            continue;
                        }
                        let key = log_key(log)?;
                        if !seen.contains(&key) {
                            logs.entry(key).or_insert_with(|| (i, log.clone()));
                        }
                    }
                }
            }
            lo = hi + 1;
        }

        // Group by tx hash; one booking may hold several token legs (a swap).
        let mut groups: HashMap<String, Booking> = HashMap::new();
        for (_, (i, log)) in logs {
            let token = &tokens[i];
            let txhash = str_field(&log, "transactionHash").to_lowercase();
            let block = hex_u64(log.get("blockNumber").unwrap_or(&Value::Null))?;
            let from_addr = topic_address(&log, 1)?;
            let to_addr = topic_address(&log, 2)?;
            let amount = hex_amount(&str_field(&log, "data"))?;
            // A zero-value transfer moves nothing and is never ours: it is
            // address-poisoning spam — `transferFrom(victim, lookalike, 0)`
            // passes every allowance check, so anyone can plant a "sent by
            // you" event. Booking it would only add 0-pairs of noise, and
            // skipping it cannot bend any balance.
            if amount == 0 {
                continue;
            }
            let booking = groups.entry(txhash.clone()).or_insert_with(|| Booking {
                txhash,
                block,
                time: 0,
                legs: Vec::new(),
                fee: 0,
                failed: false,
                sources: Vec::new(),
            });
            booking.sources.push(log.clone());
            if from_addr == self.address && to_addr == self.address {
                booking.legs.push(self.leg(token, -amount, &to_addr, true));
                booking.legs.push(self.leg(token, amount, &from_addr, true));
            } else if from_addr == self.address {
                booking.legs.push(self.leg(token, -amount, &to_addr, false));
            } else {
                booking.legs.push(self.leg(token, amount, &from_addr, false));
            }
        }

        // The gas of an own send is ours — confirmed by the receipt's `from`,
        // not by the transfer's, because a contract may move our tokens on
        // someone else's transaction (then they paid the gas).
        let mut bookings: Vec<Booking> = groups.into_values().collect();
        for b in &mut bookings {
            if !b.legs.iter().any(|l| l.amount < 0) {
                continue;
            }
            let receipt = self.rpc("eth_getTransactionReceipt", serde_json::json!([b.txhash]))?;
            if str_field(&receipt, "from").to_lowercase() == self.address {
                b.fee = receipt_fee(&receipt)?;
                b.sources.push(strip_receipt(&receipt));
            }
        }
        Ok(bookings)
    }

    fn leg(&self, token: &Token, amount: i128, counterparty: &str, own_side: bool) -> Leg {
        let mut fields = HashMap::new();
        fields.insert("type".to_string(), if amount < 0 { "out" } else { "in" }.to_string());
        fields.insert("token".to_string(), token.symbol.clone());
        fields.insert("address".to_string(), counterparty.to_string());
        Leg {
            symbol: token.symbol.clone(),
            decimals: token.decimals,
            amount,
            counterparty: counterparty.to_string(),
            own_side,
            fields,
        }
    }

    /// Fetch each `ethtx <hash>` native movement by hash. A hash whose token
    /// legs were already booked is skipped (its fee rides on the token
    /// booking); a pending tx (no receipt yet) is skipped with a note.
    fn fetch_ethtx(&self, seen: &HashSet<String>, booked: &[Booking]) -> Result<Vec<Booking>, Error> {
        let token_hashes: HashSet<&str> = booked.iter().map(|b| b.txhash.as_str()).collect();
        let mut bookings = Vec::new();
        for hash in &self.ethtx {
            if seen.contains(hash) || token_hashes.contains(hash.as_str()) {
                continue;
            }
            if let Some(b) = self.native_booking(hash)? {
                bookings.push(b);
            }
        }
        Ok(bookings)
    }

    /// One native ETH movement, fetched by hash: value legs by direction, the
    /// fee when this wallet sent it. `None` for a still-pending tx (noted) and
    /// for a tx that neither moved anything nor cost this wallet a fee.
    fn native_booking(&self, hash: &str) -> Result<Option<Booking>, Error> {
        let tx = self.rpc("eth_getTransactionByHash", serde_json::json!([hash]))?;
        if tx.is_null() {
            return Err(Error::from(format!("import: ethtx {}: not found on this chain", hash)));
        }
        let receipt = self.rpc("eth_getTransactionReceipt", serde_json::json!([hash]))?;
        if receipt.is_null() {
            eprintln!("import: ethtx {} is still pending — skipped this run", hash);
            return Ok(None);
        }
        let from_addr = str_field(&tx, "from").to_lowercase();
        let to_addr = str_field(&tx, "to").to_lowercase();
        let value = hex_amount(&str_field(&tx, "value"))?;
        let failed = str_field(&receipt, "status") == "0x0";
        let ours = from_addr == self.address;
        let mut legs = Vec::new();
        let eth = Token {
            symbol: "ETH".to_string(),
            contract: String::new(),
            decimals: ETH_DECIMALS,
        };
        if !failed && value != 0 {
            if ours && to_addr == self.address {
                legs.push(self.leg(&eth, -value, &to_addr, true));
                legs.push(self.leg(&eth, value, &from_addr, true));
            } else if ours {
                legs.push(self.leg(&eth, -value, &to_addr, false));
            } else if to_addr == self.address {
                legs.push(self.leg(&eth, value, &from_addr, false));
            } else {
                return Err(Error::from(format!(
                    "import: ethtx {} touches neither side of this wallet",
                    hash
                )));
            }
        }
        let mut sources = vec![tx.clone()];
        let fee = if ours {
            sources.push(strip_receipt(&receipt));
            receipt_fee(&receipt)?
        } else {
            0
        };
        if legs.is_empty() && fee == 0 {
            // Not ours and nothing moved — nothing to book.
            return Ok(None);
        }
        Ok(Some(Booking {
            txhash: hash.to_string(),
            block: hex_u64(tx.get("blockNumber").unwrap_or(&Value::Null))?,
            time: 0,
            legs,
            fee,
            failed,
            sources,
        }))
    }

    /// Discover the wallet's native ETH movements WITHOUT an index: bisect on
    /// `eth_getBalance`. The expected balance at any height is the balance
    /// before the scan range plus the ETH effect of every known booking up to
    /// it; wherever the chain disagrees, a binary search (~13 balance reads)
    /// pins the block of an unknown movement, whose transactions the block
    /// body then names in the clear. Repeats until chain and model agree.
    ///
    /// Two blind spots, both reported rather than swallowed: an internal
    /// (contract-mediated) transfer is invisible in the block's transaction
    /// list — only a tracing node could name it — and movements that net to
    /// zero WITHIN the range at every probed height can mask each other. The
    /// closing balance check still guards the end state either way.
    fn discover_native(
        &self,
        from: u64,
        latest: u64,
        bookings: &mut Vec<Booking>,
        seen: &HashSet<String>,
    ) -> Result<(), Error> {
        let mut cache: HashMap<u64, i128> = HashMap::new();
        let mut deltas: Vec<(u64, i128)> = bookings.iter().map(|b| (b.block, eth_delta(b))).collect();
        let mut handled: HashSet<String> = bookings.iter().map(|b| b.txhash.clone()).collect();
        let base = self.balance_at(from.saturating_sub(1), &mut cache)?;
        let expected = |deltas: &[(u64, i128)], block: u64| -> i128 {
            base + deltas.iter().filter(|(b, _)| *b <= block).map(|(_, d)| d).sum::<i128>()
        };

        for _ in 0..50 {
            if self.balance_at(latest, &mut cache)? == expected(&deltas, latest) {
                return Ok(());
            }
            let (mut lo, mut hi) = (from.saturating_sub(1), latest);
            while hi - lo > 1 {
                let mid = lo + (hi - lo) / 2;
                if self.balance_at(mid, &mut cache)? == expected(&deltas, mid) {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            let block = self.rpc("eth_getBlockByNumber", serde_json::json!([to_hex(hi), true]))?;
            let mut found = false;
            for tx in block.get("transactions").and_then(|t| t.as_array()).into_iter().flatten() {
                let f = str_field(tx, "from").to_lowercase();
                let t = str_field(tx, "to").to_lowercase();
                if f != self.address && t != self.address {
                    continue;
                }
                let hash = str_field(tx, "hash").to_lowercase();
                if handled.contains(&hash) {
                    continue;
                }
                handled.insert(hash.clone());
                let Some(b) = self.native_booking(&hash)? else {
                    continue;
                };
                deltas.push((b.block, eth_delta(&b)));
                found = true;
                // A hash already in the ledger only corrects the model (its
                // booking exists); a new one is booked.
                if !seen.contains(&hash) {
                    bookings.push(b);
                }
            }
            if !found {
                return Err(Error::from(format!(
                    "import: block {} moved this wallet's ETH without a matching transaction — an internal (contract) transfer, which a plain endpoint cannot name; book it via an `ethtx` line of its initiating tx or leave the gap reported",
                    hi
                )));
            }
        }
        Err(Error::from(
            "import: more than 50 undeclared native movements — this wallet needs explicit `ethtx` lines or an indexer",
        ))
    }

    /// The wallet's ETH balance (wei) at a height, memoised — the bisection
    /// re-probes heights as the model grows.
    fn balance_at(&self, block: u64, cache: &mut HashMap<u64, i128>) -> Result<i128, Error> {
        if let Some(v) = cache.get(&block) {
            return Ok(*v);
        }
        let r = self.rpc("eth_getBalance", serde_json::json!([self.address, to_hex(block)]))?;
        let v = hex_amount(&as_str(&r))?;
        cache.insert(block, v);
        Ok(v)
    }

    /// Logs and transactions carry no timestamp — read each booking's block
    /// header once (cached) and stamp the times.
    fn stamp_times(&self, bookings: &mut [Booking]) -> Result<(), Error> {
        let mut cache: HashMap<u64, u64> = HashMap::new();
        for b in bookings {
            let time = match cache.get(&b.block) {
                Some(t) => *t,
                None => {
                    let header =
                        self.rpc("eth_getBlockByNumber", serde_json::json!([to_hex(b.block), false]))?;
                    let t = hex_u64(header.get("timestamp").unwrap_or(&Value::Null))?;
                    cache.insert(b.block, t);
                    t
                }
            };
            b.time = time;
        }
        Ok(())
    }

    // ---- rendering ------------------------------------------------------

    fn render(&self, b: &Booking) -> String {
        let date = crate::date::ms_to_date(b.time.saturating_mul(1000));
        let mut s = format!("{} * {}\n", date, self.title);
        for src in &b.sources {
            s.push_str(&format!("\t; rpc: {}\n", serde_json::to_string(src).unwrap_or_default()));
        }

        // A failed own send burned only its gas: wallet -fee, fee account bare.
        if b.failed {
            s.push_str(&format!("\t{}  ETH-{}\n", self.account, money(b.fee, ETH_DECIMALS)));
            s.push_str(&format!("\t{}", self.fee_account));
            return s;
        }

        // A single incoming leg without an own fee is the two-posting shape:
        // the counter stays bare. Everything else books every amount explicitly.
        if b.fee == 0 && b.legs.len() == 1 && b.legs[0].amount > 0 {
            let leg = &b.legs[0];
            s.push_str(&format!("\t{}  {}\n", self.account, signed(leg)));
            s.push_str(&format!("\t{}", self.counter(leg, &b.txhash)));
            return s;
        }

        // Wallet side: every leg, then the fee pair, then the counters (last).
        for leg in &b.legs {
            s.push_str(&format!("\t{}  {}\n", self.account, signed(leg)));
        }
        if b.fee > 0 {
            s.push_str(&format!("\t{}  ETH-{}\n", self.account, money(b.fee, ETH_DECIMALS)));
            s.push_str(&format!("\t{}  ETH{}\n", self.fee_account, money(b.fee, ETH_DECIMALS)));
        }
        let counters: Vec<String> = b
            .legs
            .iter()
            .filter(|leg| !leg.own_side)
            .map(|leg| {
                let mirror = Leg {
                    symbol: leg.symbol.clone(),
                    decimals: leg.decimals,
                    amount: -leg.amount,
                    counterparty: leg.counterparty.clone(),
                    own_side: false,
                    fields: leg.fields.clone(),
                };
                format!("\t{}  {}\n", self.counter(leg, &b.txhash), signed(&mirror))
            })
            .collect();
        for c in counters {
            s.push_str(&c);
        }
        s.trim_end().to_string()
    }

    /// Counter account for a leg: a `transit` mapping of the counterparty
    /// address (either direction — an exchange withdrawal arrives FROM its
    /// address) beats the rules; then the first matching rule; else the default.
    fn counter(&self, leg: &Leg, txhash: &str) -> String {
        if let Some((_, other_leaf)) = self
            .transit_entries
            .iter()
            .find(|(addr, _)| addr == &leg.counterparty)
            && let Some(prefix) = &self.transit_prefix
        {
            return directional_account(prefix, &self.own_leaf, other_leaf, leg.amount < 0);
        }
        let get = |f: &str| -> String {
            if f == "txid" {
                return txhash.to_string();
            }
            leg.fields.get(f).cloned().unwrap_or_default()
        };
        let tmpl = match_account(&self.rules, get).unwrap_or(self.default_account.as_str());
        let mut out = tmpl.to_string();
        if out.contains("{address4}") {
            let a = &leg.counterparty;
            out = out.replace("{address4}", a.get(a.len().saturating_sub(4)..).unwrap_or(""));
        }
        if out.contains("{token}") {
            out = out.replace("{token}", &leg.symbol.to_lowercase());
        }
        if out.contains("{type}") {
            out = out.replace("{type}", leg.fields.get("type").map(String::as_str).unwrap_or(""));
        }
        out
    }

    // ---- balance check --------------------------------------------------

    /// Sum what the file plus the new bookings hold per commodity and compare
    /// against the chain. A gap means a missing `ethtx` (or a `wallet.start`
    /// after the wallet's birth) and is reported, never swallowed.
    fn balance_check(
        &self,
        tokens: &[Token],
        existing: &str,
        bookings: &[Booking],
    ) -> Result<(), Error> {
        let mut decimals: HashMap<&str, u32> = HashMap::new();
        decimals.insert("ETH", ETH_DECIMALS);
        for t in tokens {
            decimals.insert(&t.symbol, t.decimals);
        }

        let mut booked: HashMap<String, i128> = HashMap::new();
        for (symbol, amount) in file_sums(existing, &self.account) {
            if let Some(d) = decimals.get(symbol.as_str()) {
                *booked.entry(symbol).or_insert(0) += to_atomic(&amount, *d);
            }
        }
        for b in bookings {
            for leg in &b.legs {
                *booked.entry(leg.symbol.clone()).or_insert(0) += leg.amount;
            }
            *booked.entry("ETH".to_string()).or_insert(0) -= b.fee;
        }

        let mut chain: HashMap<String, i128> = HashMap::new();
        let eth = self.rpc("eth_getBalance", serde_json::json!([self.address, "latest"]))?;
        chain.insert("ETH".to_string(), hex_amount(&as_str(&eth))?);
        for t in tokens {
            let data = format!("0x70a08231{}", pad_topic(&self.address).trim_start_matches("0x"));
            let result = self.rpc(
                "eth_call",
                serde_json::json!([{ "to": t.contract, "data": data }, "latest"]),
            )?;
            chain.insert(t.symbol.clone(), hex_amount(&as_str(&result))?);
        }

        for (symbol, on_chain) in &chain {
            let have = booked.get(symbol).copied().unwrap_or(0);
            if have != *on_chain {
                use colored::Colorize;
                let d = decimals[symbol.as_str()];
                eprintln!(
                    "{} import: {} booked {}{} but the chain holds {}{} — a movement is missing (an `ethtx` line, or history before wallet.start).",
                    "!".yellow(),
                    symbol,
                    if have < 0 { "-" } else { "" },
                    money(have, d),
                    if *on_chain < 0 { "-" } else { "" },
                    money(*on_chain, d),
                );
            }
        }
        Ok(())
    }
}

/// A booking's net effect on the wallet's ETH balance: its ETH value legs
/// minus the fee — the unit of the discovery model.
fn eth_delta(b: &Booking) -> i128 {
    let legs: i128 = b.legs.iter().filter(|l| l.symbol == "ETH").map(|l| l.amount).sum();
    legs - b.fee
}

/// A provider's throttle answer, in the shapes seen in the wild: the JSON-RPC
/// code -32005, a "rate limit"/"too many requests" text, or a bare HTTP 429.
fn is_throttled(e: &Error) -> bool {
    let s = e.to_string().to_lowercase();
    s.contains("rate limit")
        || s.contains("-32005")
        || s.contains("too many request")
        || s.contains("429")
}

/// A leg's amount with its commodity, signed: `USDC-100.000000`, `ETH0.05…`.
fn signed(leg: &Leg) -> String {
    format!(
        "{}{}{}",
        leg.symbol,
        if leg.amount < 0 { "-" } else { "" },
        money(leg.amount, leg.decimals)
    )
}

// ---------------------------------------------------------------------
// dedup + cursor
// ---------------------------------------------------------------------

/// The already-imported keys, read back from the `; rpc:` comments, plus the
/// highest block seen (the incremental scan cursor). A Transfer log keys as
/// `<txhash>:<logIndex>` (one tx can carry several transfers), a native tx as
/// its bare hash. Receipts (transactionHash but no logIndex, no hash) carry no
/// key of their own — they ride on their booking as fee provenance.
fn existing_keys(src: &str) -> (HashSet<String>, Option<u64>) {
    let mut keys = HashSet::new();
    let mut top: Option<u64> = None;
    for line in src.lines() {
        let Some(rest) = line.trim_start().strip_prefix("; rpc:") else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<Value>(rest.trim()) else {
            continue;
        };
        if let Ok(block) = hex_u64(v.get("blockNumber").unwrap_or(&Value::Null)) {
            top = Some(top.map_or(block, |t: u64| t.max(block)));
        }
        if let Ok(key) = log_key(&v) {
            keys.insert(key);
        } else if let Some(hash) = v.get("hash").and_then(|h| h.as_str()) {
            keys.insert(hash.to_lowercase());
        }
    }
    (keys, top)
}

/// A Transfer log's dedup key, `<txhash>:<logIndex>` (both as the node wrote
/// them, lowercased) — errors on anything that isn't a log.
fn log_key(log: &Value) -> Result<String, Error> {
    let hash = log
        .get("transactionHash")
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::from("import: log without transactionHash"))?;
    let index = log
        .get("logIndex")
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::from("import: log without logIndex"))?;
    Ok(format!("{}:{}", hash.to_lowercase(), index.to_lowercase()))
}

/// A receipt for the `; rpc:` line: the original object minus `logs` (each
/// booked transfer already carries its own log verbatim) and `logsBloom`
/// (256 bytes of filter bits, pure ballast). Original field names throughout.
fn strip_receipt(receipt: &Value) -> Value {
    let mut r = receipt.clone();
    if let Some(obj) = r.as_object_mut() {
        obj.remove("logs");
        obj.remove("logsBloom");
    }
    r
}

/// The gas actually paid: `gasUsed × effectiveGasPrice`, from the receipt.
fn receipt_fee(receipt: &Value) -> Result<i128, Error> {
    let gas = hex_amount(&str_field(receipt, "gasUsed"))?;
    let price = hex_amount(&str_field(receipt, "effectiveGasPrice"))?;
    gas.checked_mul(price)
        .ok_or_else(|| Error::from("import: receipt fee overflows"))
}

// ---------------------------------------------------------------------
// hex + parsing helpers
// ---------------------------------------------------------------------

fn str_field(v: &Value, key: &str) -> String {
    v.get(key).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

fn as_str(v: &Value) -> String {
    v.as_str().unwrap_or("").to_string()
}

fn to_hex(n: u64) -> String {
    format!("0x{:x}", n)
}

/// A `0x…` quantity as u64 (block numbers, timestamps, decimals).
fn hex_u64(v: &Value) -> Result<u64, Error> {
    let s = v.as_str().ok_or_else(|| Error::from("import: expected a hex quantity"))?;
    u64::from_str_radix(s.trim_start_matches("0x"), 16)
        .map_err(|e| Error::from(format!("import: bad hex quantity '{}': {}", s, e)))
}

/// A `0x…` amount (log data, wei values) as i128 — errors when a value
/// genuinely exceeds 128 bits rather than truncating it.
fn hex_amount(s: &str) -> Result<i128, Error> {
    let h = s.trim().trim_start_matches("0x").trim_start_matches('0');
    if h.is_empty() {
        return Ok(0);
    }
    u128::from_str_radix(h, 16)
        .ok()
        .and_then(|v| i128::try_from(v).ok())
        .ok_or_else(|| Error::from(format!("import: amount '{}' exceeds 128 bits", s)))
}

/// An address as a 32-byte topic: `0x` + 24 zeroes + the 40 address digits.
fn pad_topic(address: &str) -> String {
    format!("0x{:0>64}", address.trim_start_matches("0x"))
}

/// The address packed into a log topic (the last 40 of its 64 hex digits).
fn topic_address(log: &Value, position: usize) -> Result<String, Error> {
    let topic = log
        .get("topics")
        .and_then(|t| t.as_array())
        .and_then(|t| t.get(position))
        .and_then(|t| t.as_str())
        .ok_or_else(|| Error::from("import: transfer log without its address topics"))?;
    let h = topic.trim_start_matches("0x");
    if h.len() != 64 {
        return Err(Error::from(format!("import: topic '{}' is not 32 bytes", topic)));
    }
    Ok(format!("0x{}", h[24..].to_lowercase()))
}

/// A decimal string as atomic units at the given precision (fraction padded,
/// excess digits truncated) — the reverse of `money`, for the balance check.
fn to_atomic(s: &str, decimals: u32) -> i128 {
    let s = s.trim();
    let (neg, s) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s),
    };
    let (int, frac) = s.split_once('.').unwrap_or((s, ""));
    let mut frac = frac.to_string();
    frac.truncate(decimals as usize);
    while frac.len() < decimals as usize {
        frac.push('0');
    }
    let int: i128 = int.parse().unwrap_or(0);
    let frac: i128 = frac.parse().unwrap_or(0);
    let total = int * 10i128.pow(decimals) + frac;
    if neg { -total } else { total }
}

/// Every `<account>  <COMMODITY><amount>` posting of the file booked on the
/// given account, as (commodity, decimal string) pairs — the file side of the
/// balance check. The file is this backend's own output, so the shape is known.
fn file_sums(src: &str, account: &str) -> Vec<(String, String)> {
    let mut sums = Vec::new();
    for line in src.lines() {
        let t = line.trim();
        if t.starts_with(';') || t.is_empty() || !line.starts_with(['\t', ' ']) {
            continue;
        }
        let mut parts = t.split_whitespace();
        let (Some(acct), Some(amount)) = (parts.next(), parts.next()) else {
            continue;
        };
        if acct != account {
            continue;
        }
        let split = amount.find(|c: char| c == '-' || c.is_ascii_digit()).unwrap_or(amount.len());
        let (commodity, number) = amount.split_at(split);
        if commodity.is_empty() || number.is_empty() {
            continue;
        }
        sums.push((commodity.to_string(), number.to_string()));
    }
    sums
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> Profile {
        Profile {
            rpc_url: "http://127.0.0.1:1".to_string(),
            proxy: None,
            address: "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaab12".to_string(),
            start: 0,
            chunk: DEFAULT_CHUNK,
            token_lines: Vec::new(),
            ethtx: Vec::new(),
            title: "ethereum-ab12".to_string(),
            account: "assets:eth".to_string(),
            fee_account: "expenses:fees".to_string(),
            rules: Vec::new(),
            default_account: "expenses:unsorted".to_string(),
            transit_prefix: None,
            own_leaf: "ethereum-ab12".to_string(),
            transit_entries: Vec::new(),
            output_file: PathBuf::new(),
        }
    }

    fn usdc() -> Token {
        Token { symbol: "USDC".to_string(), contract: "0xc".to_string(), decimals: 6 }
    }

    fn booking(legs: Vec<Leg>, fee: i128, failed: bool) -> Booking {
        Booking {
            txhash: "0xt".to_string(),
            block: 1,
            time: 1_700_000_000,
            legs,
            fee,
            failed,
            sources: vec![serde_json::json!({"transactionHash": "0xt", "logIndex": "0x1"})],
        }
    }

    #[test]
    fn hex_amount_parses_padded_log_data() {
        // 100 USDC in a 32-byte data word.
        let data = format!("0x{:0>64}", "5f5e100");
        assert_eq!(hex_amount(&data).unwrap(), 100_000_000);
        assert_eq!(hex_amount("0x0").unwrap(), 0);
        assert_eq!(hex_amount("0x").unwrap(), 0);
        // A full 256-bit value must error, not truncate.
        assert!(hex_amount(&format!("0x{}", "f".repeat(64))).is_err());
    }

    #[test]
    fn topic_carries_the_address_in_its_tail() {
        let addr = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaab12";
        let topic = pad_topic(addr);
        assert_eq!(topic.len(), 66);
        let log = serde_json::json!({ "topics": ["0xsig", topic, topic] });
        assert_eq!(topic_address(&log, 1).unwrap(), addr);
    }

    #[test]
    fn existing_keys_reads_logs_txs_and_cursor_but_not_receipts() {
        let src = "2026-01-01 * x\n\
                   \t; rpc: {\"transactionHash\":\"0xA\",\"logIndex\":\"0x2\",\"blockNumber\":\"0x10\"}\n\
                   \t; rpc: {\"transactionHash\":\"0xA\",\"gasUsed\":\"0x5208\",\"blockNumber\":\"0x10\"}\n\
                   \t; rpc: {\"hash\":\"0xB\",\"blockNumber\":\"0x20\"}\n";
        let (keys, top) = existing_keys(src);
        assert!(keys.contains("0xa:0x2"));
        assert!(keys.contains("0xb"));
        assert_eq!(keys.len(), 2); // the receipt contributes no key
        assert_eq!(top, Some(0x20)); // …but its block still feeds the cursor
    }

    #[test]
    fn strip_receipt_drops_only_the_ballast() {
        let r = serde_json::json!({
            "transactionHash": "0xt", "gasUsed": "0x5208",
            "effectiveGasPrice": "0x3b9aca00", "status": "0x1",
            "logs": [{"a": 1}], "logsBloom": "0x00",
        });
        let s = strip_receipt(&r);
        assert!(s.get("logs").is_none() && s.get("logsBloom").is_none());
        assert_eq!(s.get("gasUsed").unwrap(), "0x5208"); // original names stay
    }

    #[test]
    fn receipt_fee_is_gas_times_price() {
        let r = serde_json::json!({ "gasUsed": "0x5208", "effectiveGasPrice": "0x3b9aca00" });
        assert_eq!(receipt_fee(&r).unwrap(), 21_000i128 * 1_000_000_000);
    }

    #[test]
    fn receive_is_two_postings_counter_bare() {
        let p = profile();
        let leg = p.leg(&usdc(), 100_000_000, "0xother", false);
        let s = p.render(&booking(vec![leg], 0, false));
        assert!(s.contains("assets:eth  USDC100.000000"));
        assert!(s.trim_end().ends_with("expenses:unsorted"));
        assert!(!s.contains("ETH"));
    }

    #[test]
    fn send_books_token_fee_and_counter_explicitly() {
        let p = profile();
        let leg = p.leg(&usdc(), -100_000_000, "0xother", false);
        let s = p.render(&booking(vec![leg], 21_000_000_000_000, false));
        assert!(s.contains("assets:eth  USDC-100.000000"));
        assert!(s.contains("assets:eth  ETH-0.000021000000000000"));
        assert!(s.contains("expenses:fees  ETH0.000021000000000000"));
        assert!(s.trim_end().ends_with("expenses:unsorted  USDC100.000000"));
    }

    #[test]
    fn self_transfer_books_no_counter() {
        let p = profile();
        let a = p.leg(&usdc(), -100_000_000, &p.address.clone(), true);
        let b = p.leg(&usdc(), 100_000_000, &p.address.clone(), true);
        let s = p.render(&booking(vec![a, b], 21_000_000_000_000, false));
        assert!(!s.contains("expenses:unsorted"));
        assert!(s.contains("assets:eth  USDC-100.000000"));
        assert!(s.contains("assets:eth  USDC100.000000"));
        assert!(s.contains("expenses:fees  ETH0.000021000000000000"));
    }

    #[test]
    fn failed_send_books_the_burned_gas_only() {
        let p = profile();
        let s = p.render(&booking(Vec::new(), 21_000_000_000_000, true));
        assert!(s.contains("assets:eth  ETH-0.000021000000000000"));
        assert!(s.trim_end().ends_with("expenses:fees"));
        assert!(!s.contains("USDC"));
    }

    #[test]
    fn transit_mapping_beats_rules_in_both_directions() {
        let mut p = profile();
        p.transit_prefix = Some("assets:transit".to_string());
        p.transit_entries = vec![("0xdeadbeef".to_string(), "exchange".to_string())];
        let out = p.leg(&usdc(), -100_000_000, "0xdeadbeef", false);
        assert_eq!(p.counter(&out, "0xt"), "assets:transit:ethereum-ab12:exchange");
        let inc = p.leg(&usdc(), 100_000_000, "0xdeadbeef", false);
        assert_eq!(p.counter(&inc, "0xt"), "assets:transit:exchange:ethereum-ab12");
    }

    #[test]
    fn counter_templates_expand_token_and_address() {
        let mut p = profile();
        p.default_account = "expenses:{token}:{address4}".to_string();
        let leg = p.leg(&usdc(), -1, "0xdeadbeef", false);
        assert_eq!(p.counter(&leg, "0xt"), "expenses:usdc:beef");
    }

    #[test]
    fn eth_delta_is_value_legs_minus_fee() {
        let p = profile();
        let eth = Token { symbol: "ETH".to_string(), contract: String::new(), decimals: ETH_DECIMALS };
        // Incoming funding: +value, no fee.
        let inc = booking(vec![p.leg(&eth, 5_365_520_000_000_000, "0xother", false)], 0, false);
        assert_eq!(eth_delta(&inc), 5_365_520_000_000_000);
        // Own send: -value and the fee both leave the wallet.
        let out = booking(vec![p.leg(&eth, -1_000, "0xother", false)], 21, false);
        assert_eq!(eth_delta(&out), -1_021);
        // A token send moves no ETH legs — only its fee.
        let tok = booking(vec![p.leg(&usdc(), -100_000_000, "0xother", false)], 21, false);
        assert_eq!(eth_delta(&tok), -21);
    }

    #[test]
    fn file_sums_reads_only_the_wallet_account() {
        let src = "2026-01-01 * x\n\
                   \t; rpc: {}\n\
                   \tassets:eth        USDC100.000000\n\
                   \tassets:eth        ETH-0.000021000000000000\n\
                   \texpenses:fees     ETH0.000021000000000000\n\
                   \texpenses:unsorted USDC-100.000000\n";
        let sums = file_sums(src, "assets:eth");
        assert_eq!(sums.len(), 2);
        assert_eq!(sums[0], ("USDC".to_string(), "100.000000".to_string()));
        assert_eq!(sums[1], ("ETH".to_string(), "-0.000021000000000000".to_string()));
    }

    #[test]
    fn to_atomic_reverses_money() {
        assert_eq!(to_atomic("100.000000", 6), 100_000_000);
        assert_eq!(to_atomic("-0.000021000000000000", 18), -21_000_000_000_000);
        assert_eq!(to_atomic("1", 6), 1_000_000);
    }
}
