//! Kraken exchange import from the live REST API (the private `Ledgers` endpoint).
//!
//! Kraken is a multi-asset CASH ACCOUNT: fiat is deposited, traded into crypto,
//! withdrawn onward — the account holds several commodities at once. This backend
//! pulls every ledger entry, dedups by entry id, groups a trade's two legs by
//! `refid`, and renders every movement as a booking on the one output account.
//!
//! Movement shapes:
//!   * deposit / withdrawal — a single-asset move between Kraken and an external
//!     counter; the fee (if any) is skimmed by Kraken.
//!   * trade / instant-buy  — two legs sharing a `refid` (one fiat, one crypto),
//!     an in-account conversion: the crypto leg booked `@@` the fiat magnitude.
//! A leg's fee stays in its own commodity and books to the `fee` account.
//!
//! Auth is Kraken's API-Key + API-Sign: HMAC-SHA512 over the URI path plus
//! SHA256(nonce + POST body), keyed with the base64-decoded secret; the nonce
//! must strictly increase (microsecond clock). `Ledgers` is paged over `ofs`.
//! Each entry keeps its JSON verbatim under its ledger id (`{<id>: {…}}`, as
//! Kraken keys it) as the `; api:` source, with Z/X asset codes mapped to clean
//! codes only in the postings. Read-only: the key needs "Query ledger entries".

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use sha2::{Digest, Sha256, Sha512};

use crate::error::Error;

use super::exchange_lib::{atomic, dp_of, is_zero, load_aliases, mag, neg, signed};
use super::{directive, expand, match_account, read, Match, Rule};

/// The entry fields a categorization rule may match on.
const FIELDS: &[&str] = &["type", "asset", "refid", "txid"];

/// One normalized Kraken ledger entry. `asset` is the clean code (EUR/LTC/…);
/// `amount`/`fee` are decimal strings; `raw` is the verbatim `{<id>: {…}}`
/// payload for the `; api:` comment.
struct Entry {
    id: String,     // unique ledger id — the dedup key
    refid: String,  // groups the two legs of a trade / instant-buy
    time: String,   // "YYYY-MM-DD HH:MM:SS" (sorts chronologically)
    kind: String,   // deposit | withdrawal | trade | spend | receive
    asset: String,  // clean code: EUR, LTC, XRP, BTC, USDC
    amount: String, // signed decimal string
    fee: String,    // unsigned decimal string
    raw: String,    // `{<id>: {…}}` payload for the `; api:` comment
}

impl Entry {
    fn date(&self) -> &str {
        self.time.get(..10).unwrap_or(&self.time)
    }
    fn field(&self, name: &str) -> String {
        match name {
            "type" => self.kind.clone(),
            "asset" => self.asset.clone(),
            "refid" => self.refid.clone(),
            "txid" => self.id.clone(),
            _ => String::new(),
        }
    }
    fn is_conversion(&self) -> bool {
        matches!(self.kind.as_str(), "trade" | "spend" | "receive")
    }
}

// ---------------------------------------------------------------------
// entry point
// ---------------------------------------------------------------------

pub(super) fn run(conf_path: &str, write: bool) -> Result<(), Error> {
    let conf = read(conf_path)?;
    let api = directive(&conf, "kraken.api").unwrap_or_else(|| "https://api.kraken.com".to_string());
    let key = directive(&conf, "kraken.key")
        .ok_or_else(|| Error::from("import: kraken.key missing in profile"))?;
    let secret = directive(&conf, "kraken.secret")
        .ok_or_else(|| Error::from("import: kraken.secret missing in profile"))?;
    let secret = base64_decode(&secret)
        .ok_or_else(|| Error::from("import: kraken.secret is not valid base64"))?;

    let entries = fetch_ledgers(&api, &key, &secret)?;
    render_and_emit(entries, conf_path, write)
}

/// Dedup, group and render the fetched `entries`, then emit.
fn render_and_emit(mut entries: Vec<Entry>, conf_path: &str, write: bool) -> Result<(), Error> {
    let profile = Profile::load(conf_path)?;
    entries.sort_by(|a, b| a.time.cmp(&b.time).then(a.id.cmp(&b.id)));

    let existing = std::fs::read_to_string(&profile.output_file).unwrap_or_default();
    let seen = existing_ids(&existing);
    let total = entries.len();

    // Drop entries already present, then split into single moves and the
    // conversion legs (grouped by refid).
    let fresh: Vec<Entry> = entries.into_iter().filter(|e| !seen.contains(&e.id)).collect();
    let skipped = total - fresh.len();

    let mut dated: Vec<(String, String)> = Vec::new(); // (time, block)
    let mut conv: HashMap<String, Vec<Entry>> = HashMap::new();
    for e in fresh {
        if e.is_conversion() {
            conv.entry(e.refid.clone()).or_default().push(e);
        } else {
            let block = profile.render_single(&e);
            dated.push((e.time.clone(), block));
        }
    }
    for (refid, legs) in conv {
        if legs.len() != 2 {
            return Err(Error::from(format!(
                "import: kraken trade {} has {} legs (expected one quote + one base)",
                refid,
                legs.len()
            )));
        }
        // The received (positive) leg is the base — it gets `@@`; the spent
        // (negative) leg is the cost. The sign is in the data, so no fiat/crypto
        // or base/quote guessing is needed.
        let base = legs.iter().find(|e| !e.amount.starts_with('-') && !is_zero(&e.amount));
        let cost = legs.iter().find(|e| e.amount.starts_with('-'));
        match (base, cost) {
            (Some(b), Some(c)) => dated.push((b.time.clone(), profile.render_trade(b, c))),
            _ => {
                return Err(Error::from(format!(
                    "import: kraken trade {} is not one received (+) and one spent (-) leg",
                    refid
                )));
            }
        }
    }

    dated.sort_by(|a, b| a.0.cmp(&b.0));
    let blocks: Vec<String> = dated.into_iter().map(|(_, b)| b).collect();
    super::emit(&blocks, total, "entries", &existing, &profile.output_file, skipped, write)
}

// ---------------------------------------------------------------------
// profile
// ---------------------------------------------------------------------

struct Profile {
    output_file: PathBuf,
    title: String,
    account: String,              // rud:11:kraken
    fee_account: String,
    rules: Vec<Rule>,
    default_account: String,
    /// Currency code → ledger symbol, from the `commodities` file's `alias`
    /// declarations (EUR→€, USD→$). A true synonym is folded to its symbol so
    /// the ledger source reads `€`, not `EUR`. `parity` commodities (USDC/USDT)
    /// are NOT aliases and stay as their code — parity is a report-time
    /// valuation, never a source substitution.
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

        let fee_account =
            fee_account.ok_or_else(|| Error::from("import: kraken profile needs a 'fee => <account>' rule"))?;

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
                        "import: rule field '{}' is not a kraken entry field ({})",
                        fname,
                        FIELDS.join(", ")
                    )));
                }
                let (mode, core) = Match::parse(val.trim());
                conds.push((fname.to_string(), core.to_lowercase(), mode));
            }
            rules.push(Rule { conds, account: acc });
        }

        // Currency aliases (EUR→€, USD→$) from the optional `commodities` file.
        // Best-effort: absent/unreadable → empty map, codes pass through verbatim.
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

    /// The ledger commodity for a Kraken asset code: its alias canonical form
    /// (EUR→€, USD→$) when the commodities file declares one, else the code
    /// itself. `parity` commodities (USDC/USDT) are not aliases, so they keep
    /// their code — parity is a valuation relationship, applied at report time,
    /// never folded into the source.
    fn commodity<'a>(&'a self, asset: &'a str) -> &'a str {
        self.aliases.get(asset).map(String::as_str).unwrap_or(asset)
    }

    /// Counter account for a deposit / withdrawal (a trade's counter is the
    /// output account itself). First matching rule, else the default.
    fn categorize(&self, e: &Entry) -> String {
        match_account(&self.rules, |f| e.field(f))
            .unwrap_or(self.default_account.as_str())
            .to_string()
    }

    /// A deposit or withdrawal: gross `amount` moves between the account and the
    /// external counter, the fee (if any) skimmed by Kraken. Three postings when
    /// there is a fee (account gets `amount - fee`, the fee its own posting, the
    /// counter `-amount`); two with the counter left bare when there is none.
    fn render_single(&self, e: &Entry) -> String {
        let sym = self.commodity(&e.asset); // EUR→€ etc.; parity codes stay verbatim
        let counter = self.categorize(e);
        let mut s = format!("{} * {}\n\t; api: {}\n", e.date(), self.title, e.raw);
        if is_zero(&e.fee) {
            // Two postings: the gross amount verbatim, the counter bare to balance.
            s.push_str(&format!("\t{}  {}{}\n", self.account, sym, e.amount));
            s.push_str(&format!("\t{}", counter));
        } else {
            // The account nets `amount - fee`; the fee its own posting; the
            // counter carries the gross `-amount`. Amount and fee share the
            // source's own precision, so the subtraction is exact.
            let dp = dp_of(&e.amount).max(dp_of(&e.fee));
            let net = signed(atomic(&e.amount, dp) - atomic(&e.fee, dp), dp);
            s.push_str(&format!("\t{}  {}{}\n", self.account, sym, net));
            s.push_str(&format!("\t{}  {}{}\n", self.fee_account, sym, e.fee));
            s.push_str(&format!("\t{}  {}{}", counter, sym, neg(&e.amount)));
        }
        s
    }

    /// A trade / instant-buy: the received (`base`, positive) leg is booked
    /// gross `@@` the spent (`cost`, negative) leg's magnitude; the cost leg
    /// carries its own signed amount. Which leg is the base comes from the sign
    /// in the data, never a fiat/crypto guess. Each leg's fee (if any) leaves
    /// the account to the fee account in that leg's commodity. Amounts verbatim.
    fn render_trade(&self, base: &Entry, cost: &Entry) -> String {
        let bsym = self.commodity(&base.asset);
        let csym = self.commodity(&cost.asset);
        let mut s = format!("{} * {}\n", base.date(), self.title);
        s.push_str(&format!("\t; api: {}\n", base.raw));
        s.push_str(&format!("\t; api: {}\n", cost.raw));
        // base gross @@ cost magnitude, then the cost leg (verbatim, signed).
        s.push_str(&format!("\t{}  {}{} @@ {}{}\n", self.account, bsym, base.amount, csym, mag(&cost.amount)));
        s.push_str(&format!("\t{}  {}{}", self.account, csym, cost.amount));
        for leg in [base, cost] {
            if !is_zero(&leg.fee) {
                let sym = self.commodity(&leg.asset);
                s.push_str(&format!("\n\t{}  {}{}", self.fee_account, sym, leg.fee));
                s.push_str(&format!("\n\t{}  {}{}", self.account, sym, neg(&leg.fee)));
            }
        }
        s
    }
}

// ---------------------------------------------------------------------
// dedup
// ---------------------------------------------------------------------

/// The entry ids already imported, read back from the `; api:` comments each
/// booking carries. Each is `{<txid>: {…}}` (Kraken keys every ledger entry by
/// its id), so the txid is the single top-level key.
fn existing_ids(src: &str) -> HashSet<String> {
    let mut set = HashSet::new();
    for line in src.lines() {
        let Some(rest) = line.trim_start().strip_prefix("; api:") else {
            continue;
        };
        if let Ok(v) = serde_json::from_str::<Value>(rest.trim())
            && let Some(id) = v.as_object().and_then(|o| o.keys().next())
        {
            set.insert(id.clone());
        }
    }
    set
}

// ---------------------------------------------------------------------
// fetch + normalize
// ---------------------------------------------------------------------

/// Every ledger entry, walking `Ledgers` in pages of `ofs` until `count` reached.
fn fetch_ledgers(api: &str, key: &str, secret: &[u8]) -> Result<Vec<Entry>, Error> {
    let mut entries = Vec::new();
    let mut ofs = 0usize;
    loop {
        let result = private_call(api, key, secret, "Ledgers", &format!("ofs={}", ofs))?;
        let count = result.get("count").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let Some(ledger) = result.get("ledger").and_then(|v| v.as_object()) else {
            break;
        };
        if ledger.is_empty() {
            break;
        }
        for (id, obj) in ledger {
            entries.push(entry_from(id, obj));
        }
        ofs += ledger.len();
        if entries.len() >= count {
            break;
        }
    }
    Ok(entries)
}

/// One ledger entry object → a normalized `Entry`. Kraken returns amounts as
/// strings and `time` as a unix float. The `; api:` payload keeps the entry
/// object VERBATIM (nothing invented) under its ledger id, exactly the
/// `{<id>: {…}}` shape Kraken keys it by — so the id is the map key, where dedup
/// reads it.
fn entry_from(id: &str, o: &Value) -> Entry {
    let s = |k: &str| o.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let num = |k: &str| match o.get(k) {
        Some(Value::String(v)) => v.clone(),
        Some(v) => v.to_string(),
        None => "0".to_string(),
    };
    let mut wrapped = serde_json::Map::new();
    wrapped.insert(id.to_string(), o.clone());
    Entry {
        id: id.to_string(),
        refid: s("refid"),
        time: unix_to_datetime(o.get("time").and_then(|v| v.as_f64()).unwrap_or(0.0)),
        kind: s("type"),
        asset: normalize_asset(&s("asset")),
        amount: num("amount"),
        fee: num("fee"),
        raw: serde_json::to_string(&Value::Object(wrapped)).unwrap_or_default(),
    }
}

/// Map Kraken's internal asset code to a clean code for the postings. Legacy
/// assets carry an `X` (crypto) or `Z` (fiat) prefix on a 4-char code; bitcoin
/// is `XBT` there but `BTC` everywhere else. Unprefixed codes (USDC, USDT, …)
/// pass through.
fn normalize_asset(code: &str) -> String {
    let stripped = if code.len() == 4 && (code.starts_with('X') || code.starts_with('Z')) {
        &code[1..]
    } else {
        code
    };
    match stripped {
        "XBT" => "BTC".to_string(),
        other => other.to_string(),
    }
}

/// A unix timestamp (seconds, possibly fractional) → `YYYY-MM-DD HH:MM:SS` (UTC),
/// which sorts chronologically and gives the booking its date.
fn unix_to_datetime(t: f64) -> String {
    let secs = t as u64;
    let date = crate::date::ms_to_date(secs.saturating_mul(1000));
    let sod = secs % 86_400;
    format!("{} {:02}:{:02}:{:02}", date, sod / 3600, (sod % 3600) / 60, sod % 60)
}

// ---------------------------------------------------------------------
// signed request
// ---------------------------------------------------------------------

/// One signed POST to a private endpoint, returning its `result`. `params` is the
/// urlencoded query without the nonce (e.g. `ofs=50`); the nonce is prepended.
fn private_call(api: &str, key: &str, secret: &[u8], method: &str, params: &str) -> Result<Value, Error> {
    let path = format!("/0/private/{}", method);
    let nonce = nonce();
    let postdata = if params.is_empty() {
        format!("nonce={}", nonce)
    } else {
        format!("nonce={}&{}", nonce, params)
    };
    let sign = signature(&path, nonce, &postdata, secret);
    let url = format!("{}{}", api.trim_end_matches('/'), path);

    let connector = native_tls::TlsConnector::new()
        .map_err(|e| Error::from(format!("import: native-tls init: {}", e)))?;
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(30))
        .tls_connector(std::sync::Arc::new(connector))
        .build();
    let resp = agent
        .post(&url)
        .set("API-Key", key)
        .set("API-Sign", &sign)
        .set("Content-Type", "application/x-www-form-urlencoded")
        .set("User-Agent", "acc")
        .send_string(&postdata);
    let text = match resp {
        Ok(r) => r.into_string(),
        // Kraken returns API errors as a non-2xx status with a JSON body too.
        Err(ureq::Error::Status(_, r)) => r.into_string(),
        Err(e) => return Err(Error::from(format!("import: kraken api {}: {}", url, e))),
    }
    .map_err(|e| Error::from(format!("import: kraken api read: {}", e)))?;

    let v: Value =
        serde_json::from_str(&text).map_err(|e| Error::from(format!("import: kraken api bad JSON: {}", e)))?;
    if let Some(err) = v.get("error").and_then(|e| e.as_array()).filter(|a| !a.is_empty()) {
        return Err(Error::from(format!("import: kraken api error: {:?}", err)));
    }
    v.get("result")
        .cloned()
        .ok_or_else(|| Error::from("import: kraken api: response has no result"))
}

/// A strictly-increasing nonce (microseconds since the epoch).
fn nonce() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0)
}

/// The `API-Sign` value: base64(HMAC-SHA512(path ‖ SHA256(nonce ‖ postdata))),
/// keyed with the raw secret.
fn signature(path: &str, nonce: u64, postdata: &str, secret: &[u8]) -> String {
    let mut sha = Sha256::new();
    sha.update(nonce.to_string().as_bytes());
    sha.update(postdata.as_bytes());
    let inner = sha.finalize();

    let mut msg = Vec::with_capacity(path.len() + inner.len());
    msg.extend_from_slice(path.as_bytes());
    msg.extend_from_slice(&inner);
    base64_encode(&hmac_sha512(secret, &msg))
}

/// HMAC-SHA512 (block size 128 bytes).
fn hmac_sha512(key: &[u8], msg: &[u8]) -> [u8; 64] {
    let mut k = key.to_vec();
    if k.len() > 128 {
        k = Sha512::digest(&k).to_vec();
    }
    k.resize(128, 0);
    let ipad: Vec<u8> = k.iter().map(|b| b ^ 0x36).collect();
    let opad: Vec<u8> = k.iter().map(|b| b ^ 0x5c).collect();
    let mut inner = Sha512::new();
    inner.update(&ipad);
    inner.update(msg);
    let ih = inner.finalize();
    let mut outer = Sha512::new();
    outer.update(&opad);
    outer.update(&ih);
    let mut out = [0u8; 64];
    out.copy_from_slice(&outer.finalize());
    out
}

// ---------------------------------------------------------------------
// base64
// ---------------------------------------------------------------------

fn base64_encode(input: &[u8]) -> String {
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

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some((c - b'A') as u32),
            b'a'..=b'z' => Some((c - b'a' + 26) as u32),
            b'0'..=b'9' => Some((c - b'0' + 52) as u32),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let clean: Vec<u8> = s.bytes().filter(|&b| b != b'=' && !b.is_ascii_whitespace()).collect();
    let mut out = Vec::with_capacity(clean.len() * 3 / 4);
    for chunk in clean.chunks(4) {
        let mut v = [0u32; 4];
        for (i, &c) in chunk.iter().enumerate() {
            v[i] = val(c)?;
        }
        let n = (v[0] << 18) | (v[1] << 12) | (v[2] << 6) | v[3];
        out.push((n >> 16) as u8);
        if chunk.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(n as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> Profile {
        Profile {
            output_file: PathBuf::new(),
            title: "kraken | me".to_string(),
            account: "assets:kraken".to_string(),
            fee_account: "expenses:kraken-fee".to_string(),
            rules: Vec::new(),
            default_account: "assets:transit:kraken".to_string(),
            aliases: HashMap::new(),
        }
    }

    fn entry(kind: &str, asset: &str, amount: &str, fee: &str) -> Entry {
        Entry {
            id: "LID".to_string(),
            refid: "R".to_string(),
            time: "2026-03-01 12:05:19".to_string(),
            kind: kind.to_string(),
            asset: asset.to_string(),
            amount: amount.to_string(),
            fee: fee.to_string(),
            raw: "{\"LID\":{}}".to_string(),
        }
    }

    #[test]
    fn deposit_without_fee_is_two_postings_bare_counter() {
        let s = profile().render_single(&entry("deposit", "EUR", "26.6500", "0.0000"));
        assert!(s.contains("assets:kraken  EUR26.6500"));
        assert!(s.trim_end().ends_with("assets:transit:kraken"));
        assert!(!s.contains("expenses:kraken-fee"));
    }

    #[test]
    fn withdrawal_with_fee_nets_to_zero() {
        // amount -23.2267780800, fee 0.0020000000 (LTC): account loses amount+fee,
        // fee its own posting, counter receives the sent amount.
        let s = profile().render_single(&entry("withdrawal", "LTC", "-23.2267780800", "0.0020000000"));
        assert!(s.contains("assets:kraken  LTC-23.2287780800")); // amount - fee, verbatim precision
        assert!(s.contains("expenses:kraken-fee  LTC0.0020000000"));
        assert!(s.trim_end().ends_with("assets:transit:kraken  LTC23.2267780800")); // -amount
    }

    #[test]
    fn trade_books_received_leg_at_spent_cost_with_fee() {
        let cost = entry("trade", "EUR", "-1063.7500", "0.0000"); // spent (−)
        let base = entry("trade", "LTC", "23.2869965000", "0.0582176000"); // received (+)
        let s = profile().render_trade(&base, &cost);
        assert!(s.contains("assets:kraken  LTC23.2869965000 @@ EUR1063.7500"));
        assert!(s.contains("assets:kraken  EUR-1063.7500"));
        assert!(s.contains("expenses:kraken-fee  LTC0.0582176000"));
        assert!(s.contains("assets:kraken  LTC-0.0582176000"));
    }

    #[test]
    fn instant_buy_spent_leg_fee_leaves_account() {
        // spend EUR -24.75 fee 0.25 (cost), receive XRP +8.86782 fee 0 (base).
        let cost = entry("spend", "EUR", "-24.7500", "0.2500");
        let base = entry("receive", "XRP", "8.86782000", "0.00000000");
        let s = profile().render_trade(&base, &cost);
        assert!(s.contains("assets:kraken  XRP8.86782000 @@ EUR24.7500"));
        assert!(s.contains("assets:kraken  EUR-24.7500"));
        assert!(s.contains("expenses:kraken-fee  EUR0.2500"));
        assert!(s.contains("assets:kraken  EUR-0.2500"));
    }

    #[test]
    fn stablecoin_stays_its_own_commodity() {
        // USDC is written as its own code, never folded to $ — aliases are
        // report-time display only, never baked into the ledger source.
        let cost = entry("trade", "EUR", "-74.0004", "0.0000");
        let base = entry("trade", "USDC", "86.70225511", "0.00000000");
        let s = profile().render_trade(&base, &cost);
        assert!(s.contains("assets:kraken  USDC86.70225511 @@ EUR74.0004"));
        assert!(s.contains("assets:kraken  EUR-74.0004"));
    }

    #[test]
    fn alias_folds_currency_symbol_but_parity_code_stays() {
        // A commodities `alias EUR→€` (a true synonym) makes the import write €;
        // USDC has no alias (it's `parity $`, report-time only), so it stays USDC.
        let mut p = profile();
        p.aliases.insert("EUR".to_string(), "€".to_string());
        let cost = entry("trade", "EUR", "-74.0004", "0.0000");
        let base = entry("trade", "USDC", "86.70225511", "0.00000000");
        let s = p.render_trade(&base, &cost);
        assert!(s.contains("assets:kraken  USDC86.70225511 @@ €74.0004"));
        assert!(s.contains("assets:kraken  €-74.0004"));
    }

    #[test]
    fn existing_ids_reads_the_api_map_key() {
        let src = "2026-03-01 * k\n\t; api: {\"LID1\":{\"type\":\"deposit\"}}\n\tassets:kraken €1\n\
                   \n2026-03-02 * k\n\t; api: {\"LID2\":{\"type\":\"withdrawal\"}}\n\tassets:kraken €1\n";
        let ids = existing_ids(src);
        assert!(ids.contains("LID1") && ids.contains("LID2"));
        assert_eq!(ids.len(), 2);
    }

    #[test]
    fn normalize_asset_maps_kraken_codes() {
        assert_eq!(normalize_asset("ZEUR"), "EUR");
        assert_eq!(normalize_asset("XLTC"), "LTC");
        assert_eq!(normalize_asset("XXRP"), "XRP");
        assert_eq!(normalize_asset("XXBT"), "BTC");
        assert_eq!(normalize_asset("USDC"), "USDC");
        assert_eq!(normalize_asset("EUR"), "EUR");
    }

    #[test]
    fn base64_roundtrip() {
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64_decode("Zm9vYmFy").unwrap(), b"foobar");
        assert_eq!(base64_decode("Zg==").unwrap(), b"f");
        assert_eq!(base64_decode("Zm8=").unwrap(), b"fo");
    }

    #[test]
    fn signature_matches_kraken_reference() {
        // Kraken's documented API-Sign example.
        let secret = base64_decode(
            "kQH5HW/8p1uGOVjbgWA7FunAmGO8lsSUXNsu3eow76sz84Q18fWxnyRzBHCd3pd5nE9qa99HAZtuZuj6F1huXg==",
        )
        .unwrap();
        let sig = signature(
            "/0/private/AddOrder",
            1616492376594,
            "nonce=1616492376594&ordertype=limit&pair=XBTUSD&price=37500&type=buy&volume=1.25",
            &secret,
        );
        assert_eq!(
            sig,
            "4/dpxb3iT4tp/ZCVEwSnEsLxx0bqyhLpdfOpc6fn7OR8+UClSV5n9E6aSS8MPtnRfp32bAb0nmbRn6H8ndwLUQ=="
        );
    }
}
