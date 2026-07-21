//! Shared core for the crypto-wallet-RPC import backends — the `monero`
//! wallet-rpc import and the Bitcoin Core family (`bitcoin`/`litecoin`), plus
//! the `haveno` reto enrichment layered on monero. All three group a wallet's
//! transactions by txid and book each as a receive, a send, a self-move, or
//! (monero) a fee-only churn, categorizing the counter through the shared
//! rule/transit vocabulary in the parent module. The CSV (`fiat`) backend uses
//! none of this, so it lives here rather than in `mod.rs`: everything in this
//! file is coin-agnostic, and a backend supplies only its specifics (amount
//! precision, how the daemon's tx object is parsed, discovery) and embeds a
//! [`Wallet`] in its `Profile`.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use super::{directional_account, match_account, slug, Rule};

/// One transaction leg, coin-agnostic. `amount` is an unsigned magnitude
/// (direction lives in the [`Group`]); `category` is the daemon's direction word
/// (monero `in`/`out`, Bitcoin Core `send`/`receive`/…). `fields` holds the
/// match keys a categorization rule can test.
pub(super) struct Tx {
    pub txid: String,
    pub category: String,
    pub amount: i128,
    pub fee: i128,
    pub time: u64,
    /// Wallet account (major index) this leg belongs to — always 0 for a
    /// single-account coin (Bitcoin Core).
    pub major: u64,
    pub fields: HashMap<String, String>,
    pub raw: Value,
}

impl Tx {
    pub fn field(&self, name: &str) -> String {
        self.fields.get(name).cloned().unwrap_or_default()
    }

    /// True when this wallet is the SENDER (we created the tx), so the transit /
    /// categorization logic may treat it as an outgoing transfer.
    pub fn is_send(&self) -> bool {
        matches!(self.category.as_str(), "out" | "send")
    }
}

/// All legs sharing one txid: at most one receive and one send (both set only
/// for a self-send or an inter-account move).
pub(super) struct Group {
    pub txid: String,
    pub time: u64,
    pub receive: Option<Tx>,
    pub send: Option<Tx>,
}

impl Group {
    /// The representative leg's raw source object — for the `; rpc:` line (the
    /// send of a spend, else the receive). Used by the haveno enrichment so a
    /// reto booking still carries its wallet tx.
    pub fn source(&self) -> Option<&Value> {
        self.send.as_ref().or(self.receive.as_ref()).map(|t| &t.raw)
    }
}

/// A fixed-point magnitude in atomic units → a decimal string at `decimals`
/// places (XMR 12, BTC/LTC 8). Full length, no truncation, so acc infers the
/// precision from the written amounts.
pub(super) fn money(atomic: i128, decimals: u32) -> String {
    let scale = 10i128.pow(decimals);
    let a = atomic.abs();
    format!("{}.{:0width$}", a / scale, a % scale, width = decimals as usize)
}

/// The txids already imported, read back from the `; rpc:` comments (the
/// complete source object each booking carries). Dedup compares them and skips
/// transactions already present. Shared by every wallet-RPC backend; the CSV
/// path dedups by whole row instead.
pub(super) fn existing_txids(src: &str) -> HashSet<String> {
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

/// The shared config + render/categorize/transit for a crypto-wallet import.
/// The backend's `Profile` embeds one; the backend keeps only the connection,
/// discovery and per-coin tx parsing.
pub(super) struct Wallet {
    pub title: String,
    pub account: String,
    pub commodity: String,
    pub decimals: u32,
    pub fee_account: String,
    pub rules: Vec<Rule>,
    pub default_account: String,
    /// Transit account prefix (from `transit.self`); `Some` = transit enabled.
    pub transit_prefix: Option<String>,
    /// This wallet's own transit leaf (`<coin>-<tail>`), pre-computed by the
    /// backend so the shared code needs no coin-specific leaf derivation.
    pub own_leaf: String,
    /// Manual own↔own transits for accounts NOT on RPC (exchanges): (exact
    /// destination address, that account's full leaf).
    pub transit_entries: Vec<(String, String)>,
    /// (major index, label) per wallet account, from the daemon. ≤1 entry books
    /// to the bare account; several give each its own `:label` / `:index`.
    pub accounts: Vec<(u64, String)>,
    /// txid → the SENDER's full transit leaf (from other wallets' receives).
    pub incoming_transits: HashMap<String, String>,
    /// txid → the RECIPIENT's full transit leaf (from other wallets' sends).
    pub outgoing_transits: HashMap<String, String>,
}

impl Wallet {
    fn money(&self, atomic: i128) -> String {
        money(atomic, self.decimals)
    }

    /// Header line + the `; rpc:` comment carrying the full source object.
    fn header(&self, t: &Tx) -> String {
        let date = crate::date::ms_to_date(t.time.saturating_mul(1000));
        let rpc = serde_json::to_string(&t.raw).unwrap_or_default();
        format!("{} * {}\n\t; rpc: {}\n", date, self.title, rpc)
    }

    /// External receive: the wallet gains `amount`; the fee is the sender's, not
    /// booked. Two postings, so the counter is left bare to auto-balance.
    pub fn render_in(&self, t: &Tx) -> String {
        let counter = self.incoming_counter(t);
        let wallet = self.wallet_account(t.major);
        let mut s = self.header(t);
        s.push_str(&format!("\t{}  {}{}\n", wallet, self.commodity, self.money(t.amount)));
        s.push_str(&format!("\t{}", counter));
        s
    }

    /// Send: the wallet loses `amount + fee`, the fee is its own posting, and the
    /// categorized counter gains `amount` — always the LAST posting. Three
    /// explicit postings, none inferred.
    pub fn render_out(&self, t: &Tx) -> String {
        let counter = self.categorize(t);
        let wallet = self.wallet_account(t.major);
        let mut s = self.header(t);
        s.push_str(&format!("\t{}  {}-{}\n", wallet, self.commodity, self.money(t.amount + t.fee)));
        s.push_str(&format!("\t{}  {}{}\n", self.fee_account, self.commodity, self.money(t.fee)));
        s.push_str(&format!("\t{}  {}{}", counter, self.commodity, self.money(t.amount)));
        s
    }

    /// Self-sweep / churn: `amount` leaves this wallet account and returns to it
    /// with the fee between — the fee is the only real cost. Three explicit
    /// postings summing to zero.
    pub fn render_self(&self, t: &Tx) -> String {
        let wallet = self.wallet_account(t.major);
        let mut s = self.header(t);
        s.push_str(&format!("\t{}  {}-{}\n", wallet, self.commodity, self.money(t.amount + t.fee)));
        s.push_str(&format!("\t{}  {}{}\n", self.fee_account, self.commodity, self.money(t.fee)));
        s.push_str(&format!("\t{}  {}{}", wallet, self.commodity, self.money(t.amount)));
        s
    }

    /// Fee-only booking for a tx that moved nothing to others (`amount` 0) — a
    /// churn/multisig tx whose only real cost is the network fee. Two postings:
    /// the wallet loses the fee, the fee account is left bare to auto-balance.
    pub fn render_fee_only(&self, t: &Tx) -> String {
        let wallet = self.wallet_account(t.major);
        let mut s = self.header(t);
        s.push_str(&format!("\t{}  {}-{}\n", wallet, self.commodity, self.money(t.fee)));
        s.push_str(&format!("\t{}", self.fee_account));
        s
    }

    /// A move between two of this wallet's own accounts: `amount + fee` leaves
    /// the `out` account, the fee is its own posting, and `amount` lands in the
    /// `in` account — always the LAST posting. Three explicit postings.
    pub fn render_transfer(&self, inc: &Tx, out: &Tx) -> String {
        let from = self.wallet_account(out.major);
        let to = self.wallet_account(inc.major);
        let mut s = self.header(out);
        s.push_str(&format!("\t{}  {}-{}\n", from, self.commodity, self.money(out.amount + out.fee)));
        s.push_str(&format!("\t{}  {}{}\n", self.fee_account, self.commodity, self.money(out.fee)));
        s.push_str(&format!("\t{}  {}{}", to, self.commodity, self.money(out.amount)));
        s
    }

    /// The ledger account for a wallet account (major index). A single-account
    /// wallet books to the bare `account`; a multi-account wallet appends
    /// `:label` — or `:index` when that account has no label.
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

    /// Counter for a receive: if the txid matches a send from another of my
    /// wallets, book the SAME directional transit account (so both legs net).
    /// Otherwise normal categorization.
    fn incoming_counter(&self, t: &Tx) -> String {
        if let Some(sender_leaf) = self.incoming_transits.get(&t.txid)
            && let Some(acct) = self.transit_account(sender_leaf, false)
        {
            return acct;
        }
        self.categorize(t)
    }

    /// The directional transit account for a transfer to `other_leaf` — `None`
    /// when transit isn't configured (no `transit.self`).
    fn transit_account(&self, other_leaf: &str, outgoing: bool) -> Option<String> {
        self.transit_prefix
            .as_ref()
            .map(|p| directional_account(p, &self.own_leaf, other_leaf, outgoing))
    }

    /// Counter account for a send: an internal transfer to another of my wallets
    /// — matched by TXID (its recipient leaf) — or a manually-mapped non-RPC
    /// account (exchange) matched by destination address; then the first
    /// matching rule; else the default. Receives match by txid in
    /// `incoming_counter` instead.
    fn categorize(&self, t: &Tx) -> String {
        if t.is_send() {
            let other = self
                .outgoing_transits
                .get(&t.txid)
                .cloned()
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
        let tmpl = match_account(&self.rules, |f| t.field(f)).unwrap_or(self.default_account.as_str());
        self.template(tmpl, t)
    }

    /// Expand a target template's placeholders from the tx's fields. `{address4}`
    /// and `{type}` are coin-agnostic; `{note}`/`{subaddr}` (monero) and
    /// `{label}` (Bitcoin Core) resolve from whichever the coin populated.
    fn template(&self, tmpl: &str, t: &Tx) -> String {
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
        if out.contains("{label}") {
            out = out.replace("{label}", &slug(&t.field("label")));
        }
        if out.contains("{type}") {
            out = out.replace("{type}", &t.field("type"));
        }
        out
    }
}

/// Merge the legs of one txid+direction: sum the amounts, keep the
/// largest-magnitude leg as the representative (its raw + fields carry the
/// meaning; a 0-value padding output is noise), and keep the largest fee seen.
pub(super) fn aggregate(mut legs: Vec<Tx>) -> Tx {
    legs.sort_by_key(|t| std::cmp::Reverse(t.amount.abs()));
    let total: i128 = legs.iter().map(|t| t.amount).sum();
    let fee = legs.iter().map(|t| t.fee).max().unwrap_or(0);
    let mut repr = legs.into_iter().next().expect("aggregate: non-empty legs");
    repr.amount = total;
    repr.fee = fee;
    repr
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn money_writes_full_precision_per_coin() {
        // XMR at 12 decimals, BTC/LTC at 8 — full length, no truncation.
        assert_eq!(money(200_000_000_000, 12), "0.200000000000");
        assert_eq!(money(130_364_401_518, 12), "0.130364401518");
        assert_eq!(money(2_778_253, 8), "0.02778253");
        assert_eq!(money(100_000_000, 8), "1.00000000");
        assert_eq!(money(-10_000, 8), "0.00010000"); // magnitude
    }

    #[test]
    fn aggregate_sums_outputs_keeps_real_leg_and_max_fee() {
        // A real output + a 0-value padding output sharing a txid → summed, the
        // representative is the non-zero leg, the fee taken once (largest).
        let leg = |amount, fee| Tx {
            txid: "t".to_string(),
            category: "in".to_string(),
            amount,
            fee,
            time: 0,
            major: 0,
            fields: HashMap::new(),
            raw: Value::Null,
        };
        let agg = aggregate(vec![leg(130_364_401_518, 1_022_340_000), leg(0, 1_022_340_000)]);
        assert_eq!(agg.amount, 130_364_401_518);
        assert_eq!(agg.fee, 1_022_340_000);
    }

    #[test]
    fn existing_txids_read_from_rpc_comments() {
        let src = "2025-07-02 * x\n\t; rpc: {\"txid\":\"aaa\",\"type\":\"in\"}\n\tassets:xmr XMR1\n\
                   \n2025-07-03 * y\n\t; rpc: {\"txid\":\"bbb\"}\n";
        let set = existing_txids(src);
        assert!(set.contains("aaa") && set.contains("bbb"));
        assert_eq!(set.len(), 2);
    }
}
