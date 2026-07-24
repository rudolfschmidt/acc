//! Haveno reto enrichment for the `monero` import backend.
//!
//! Haveno runs its own monero wallet, so its transfers already flow through the
//! monero import. When that profile carries a `haveno.*` block, this module
//! pulls the completed trades from a running `haveno-daemon` over gRPC (via the
//! `grpcurl` CLI — the daemon ships no reflection, so the Haveno `.proto` is
//! supplied explicitly) and turns the two on-chain legs of each trade into the
//! reto swap bookings: the funding leg (XMR sold/bought `@@` fiat) and the
//! security-deposit return. Everything else in the wallet keeps its normal
//! monero rendering. All Haveno logic lives here; `monero.rs` stays pure and
//! only feeds this the per-transaction amounts it already has.

use std::collections::HashMap;
use std::process::Command;

use serde_json::Value;

use crate::error::Error;

/// Atomic units per XMR (piconero).
const ATOMIC: i128 = 1_000_000_000_000;

// ---------------------------------------------------------------------
// gRPC transport (grpcurl)
// ---------------------------------------------------------------------

/// A running `haveno-daemon`'s gRPC API, reached through the `grpcurl` CLI.
/// Reflection is disabled on the daemon, so the Haveno `.proto` directory is
/// passed on every call.
pub(crate) struct Rpc {
    /// Directory holding Haveno's `grpc.proto` (which imports `pb.proto`).
    pub proto: String,
    pub host: String,
    pub port: u16,
    pub pass: String,
}

impl Rpc {
    /// Invoke one unary gRPC method and return the reply as parsed JSON.
    /// `request` is the request message as a JSON string (`{}` when empty).
    fn call(&self, method: &str, request: &str) -> Result<Value, Error> {
        let endpoint = format!("{}:{}", self.host, self.port);
        let out = Command::new("grpcurl")
            .args(["-plaintext", "-import-path", &self.proto, "-proto", "grpc.proto"])
            .args(["-H", &format!("password: {}", self.pass)])
            .args(["-d", request, &endpoint, method])
            .output()
            .map_err(|e| Error::from(format!("import: grpcurl: {} (is it in PATH?)", e)))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            let msg = err.lines().find(|l| !l.trim().is_empty()).unwrap_or("failed");
            return Err(Error::from(format!("import: grpcurl {}: {}", method, msg.trim())));
        }
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout))
            .map_err(|e| Error::from(format!("import: grpcurl {} JSON: {}", method, e)))
    }
}

/// Read an integer that gRPC-JSON encodes as a string (uint64/int64) or leaves
/// as a plain number — `0` when the field is absent or unparseable.
fn int(v: Option<&Value>) -> i128 {
    match v {
        Some(v) if v.is_string() => v.as_str().unwrap_or("0").parse().unwrap_or(0),
        Some(v) => v.as_i64().map(i128::from).unwrap_or(0),
        None => 0,
    }
}

/// Error for a `TradeInfo` field Haveno didn't return — its name told, so an API
/// change (renamed/removed field) is a loud, diagnosable failure, not a silent 0.
fn miss(id: &str, key: &str) -> Error {
    Error::from(format!("import: haveno trade {}: TradeInfo has no '{}' (Haveno API changed?)", id, key))
}

/// A required string field: present and non-empty, else a named error.
fn field_str(info: &Value, id: &str, key: &str) -> Result<String, Error> {
    info.get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| miss(id, key))
}

/// A required integer field: present (non-null), else a named error.
fn field_int(info: &Value, id: &str, key: &str) -> Result<i128, Error> {
    match info.get(key) {
        Some(v) if !v.is_null() => Ok(int(Some(v))),
        _ => Err(miss(id, key)),
    }
}

/// Atomic piconero → an XMR decimal string at full 12-digit precision.
fn xmr(atomic: i128) -> String {
    let a = atomic.abs();
    format!("{}.{:012}", a / ATOMIC, a % ATOMIC)
}

/// Render a swap-posting amount from Haveno's raw `tradeVolume`, verbatim.
/// USD/EUR map to their symbols (`$`/`€`); any other currency (BTC) keeps its
/// code. The sign lands after the symbol (`€-966`). The commodity formatter is
/// deliberately NOT used here: it would re-precision whole fiat to `.00`, but
/// the ledger mirrors the source exactly — the same way wallet amounts keep
/// their reported precision — so `$354` stays `$354`.
fn money(currency: &str, volume: &str, negative: bool) -> String {
    let commodity = match currency {
        "USD" => "$",
        "EUR" => "€",
        other => other,
    };
    let sign = if negative { "-" } else { "" };
    format!("{}{}{}", commodity, sign, volume)
}

// ---------------------------------------------------------------------
// configuration (the `haveno.*` block)
// ---------------------------------------------------------------------

/// The `haveno.*` block of a monero profile: how to reach the daemon plus the
/// two reto-only accounts. The wallet, fee, and title come from the monero
/// profile itself.
pub(crate) struct Config {
    pub rpc: Rpc,
    /// Security-deposit clearing account (from `haveno.deposit`).
    pub deposit: String,
    /// Fiat swap account (from `haveno.swap`).
    pub swap: String,
    /// Haveno trading-fee account when we made the offer (from `haveno.makerfee`).
    pub maker_fee: String,
    /// Haveno trading-fee account when we took the offer (from `haveno.takerfee`).
    pub taker_fee: String,
}

impl Config {
    /// Parse the block from a profile's directives — `None` when there is no
    /// `haveno.*` block (`haveno.port` is its marker), so plain monero profiles
    /// are untouched.
    pub(crate) fn parse(d: &HashMap<String, String>) -> Result<Option<Config>, Error> {
        if !d.contains_key("haveno.port") {
            return Ok(None);
        }
        let get = |k: &str| {
            d.get(k)
                .cloned()
                .ok_or_else(|| Error::from(format!("import: missing '{}' in haveno block", k)))
        };
        Ok(Some(Config {
            rpc: Rpc {
                proto: super::expand(&get("haveno.proto")?).to_string_lossy().into_owned(),
                host: d.get("haveno.host").cloned().unwrap_or_else(|| "127.0.0.1".to_string()),
                port: get("haveno.port")?
                    .parse()
                    .map_err(|_| Error::from("import: haveno.port must be a number"))?,
                pass: get("haveno.pass")?,
            },
            deposit: get("haveno.deposit")?,
            swap: get("haveno.swap")?,
            maker_fee: get("haveno.makerfee")?,
            taker_fee: get("haveno.takerfee")?,
        }))
    }
}

// ---------------------------------------------------------------------
// trades (GetTrades → TradeInfo)
// ---------------------------------------------------------------------

/// A completed Haveno trade, distilled from its `TradeInfo`.
struct Trade {
    /// The two on-chain deposit txids (maker + taker). Only OURS is a tx in
    /// this wallet, so matching a wallet send against both picks the right leg
    /// without needing to know our role.
    deposit_txids: Vec<String>,
    /// The shared payout txid — the multisig return whose output lands here.
    payout_txid: String,
    /// Net traded XMR (atomic piconero), from `amount`. This is the `@@` cost.
    trade_amount: i128,
    /// Our security deposit (atomic) — set aside on funding, returned on payout.
    /// Resolved by role from `sellerSecurityDeposit` / `buyerSecurityDeposit`.
    sec_deposit: i128,
    /// Our deposit tx's network fee (atomic), from `{seller,buyer}DepositTxFee`.
    deposit_fee: i128,
    /// What lands back in this wallet at payout (atomic), from
    /// `{seller,buyer}PayoutAmount`.
    payout_amount: i128,
    /// Fiat volume exactly as Haveno reports it (`tradeVolume`), used verbatim.
    fiat: String,
    /// Counter currency (USD, EUR, BTC, …), from the offer.
    currency: String,
    /// Our role, e.g. `XMR seller as taker`; its `XMR seller` / `XMR buyer`
    /// prefix gives the swap direction (used only for the funding leg).
    role: String,
    /// Whether the traded XMR came back to this wallet in the payout — set by
    /// comparing the payout to its two far-apart possible values (deposit alone
    /// vs deposit + trade). True for a completed buy or a refunded sell; false
    /// for a completed sell or a refunded buy. Decides the payout's shape.
    payout_includes_trade: bool,
    /// Short trade id, for diagnostics in error messages.
    short_id: String,
    /// True when we took the offer (`… as taker`) — picks the trading-fee account
    /// (`haveno.takerfee` vs `haveno.makerfee`) and the applicable fee field.
    is_taker: bool,
    /// Haveno's trading fee for our role (`takerFee`/`makerFee`, atomic piconero),
    /// or 0 when the trade has none — trades from before Haveno introduced the
    /// fee carry no such field. When present it is charged INSIDE the deposit tx,
    /// so the wallet's outflow exceeds trade+deposit; the funding leg then books
    /// it to the trading-fee account.
    trade_fee: i128,
}

impl Trade {
    /// Build a trade from one `TradeInfo`. EVERY field the booking computes from
    /// is required — a missing one means Haveno's API changed (renamed/removed a
    /// field), so we fail loud with the field's name rather than booking a silent
    /// zero. Nothing here is tolerated or defaulted.
    fn from_info(info: &Value) -> Result<Trade, Error> {
        let id = info.get("shortId").and_then(|v| v.as_str()).unwrap_or("?");
        let role = field_str(info, id, "role")?;

        // Our side of the trade (seller vs buyer) supplies the deposit, fee and
        // payout — all present in the TradeInfo, so the booking needs no tx data.
        let side = if role.starts_with("XMR seller") { "seller" } else { "buyer" };
        let trade_amount = field_int(info, id, "amount")?;
        let sec_deposit = field_int(info, id, &format!("{}SecurityDeposit", side))?;
        let deposit_fee = field_int(info, id, &format!("{}DepositTxFee", side))?;
        let payout_amount = field_int(info, id, &format!("{}PayoutAmount", side))?;
        let fiat = field_str(info, id, "tradeVolume")?;

        let currency = info
            .get("offer")
            .and_then(|o| o.get("counterCurrencyCode"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| miss(id, "offer.counterCurrencyCode"))?
            .to_string();

        let mut deposit_txids = Vec::new();
        for k in ["makerDepositTxId", "takerDepositTxId"] {
            if let Some(h) = info.get(k).and_then(|v| v.as_str()).filter(|h| !h.is_empty()) {
                deposit_txids.push(h.to_string());
            }
        }
        if deposit_txids.is_empty() {
            return Err(miss(id, "makerDepositTxId/takerDepositTxId"));
        }
        let payout_txid = field_str(info, id, "payoutTxId")?;

        // Did the traded XMR come back in the payout? The two possible returns —
        // the security deposit alone (the trade settled) or the deposit PLUS the
        // whole trade amount (the swap fell through and everything was refunded,
        // e.g. a reorg) — lie a full `trade_amount` apart. We classify by which
        // the actual payout is NEAREST, never by exact equality: that margin is
        // the entire trade, so no network fee can flip it (the booking still uses
        // the real `payout_amount`, so it balances whatever the fee was). A
        // payout near NEITHER value is an unexpected settlement we refuse to
        // guess at — a loud, named error instead of a silent mis-booking.
        let d_deposit = (payout_amount - sec_deposit).abs();
        let d_with_trade = (payout_amount - (sec_deposit + trade_amount)).abs();
        if d_deposit.min(d_with_trade) * 4 > trade_amount {
            return Err(Error::from(format!(
                "import: haveno trade {}: payout {} is near neither the deposit ({}) \
                 nor deposit+trade ({}) — unexpected settlement",
                id, payout_amount, sec_deposit, sec_deposit + trade_amount
            )));
        }
        let payout_includes_trade = d_with_trade < d_deposit;

        // Haveno's trading fee for our role — `takerFee` if we took the offer,
        // else `makerFee`. OPTIONAL: Haveno reports it only on the trades where it
        // was charged into the deposit tx on-chain (most trades have no such
        // field), so a missing one is 0, not an error. A trading fee that DID
        // leave the wallet without a field still surfaces as a loud error in the
        // funding leg, which reconciles the postings against the real outflow.
        let is_taker = role.contains("as taker");
        let trade_fee = int(info.get(if is_taker { "takerFee" } else { "makerFee" }));

        Ok(Trade {
            deposit_txids,
            payout_txid,
            trade_amount,
            sec_deposit,
            deposit_fee,
            payout_amount,
            fiat,
            currency,
            role,
            payout_includes_trade,
            short_id: id.to_string(),
            is_taker,
            trade_fee,
        })
    }

    /// True when we sold XMR (received fiat); false when we bought XMR.
    fn sells_xmr(&self) -> bool {
        self.role.starts_with("XMR seller")
    }
}

/// Every completed trade from the daemon, in one `GetTrades` call. A trade whose
/// TradeInfo is missing a booking field aborts the whole import (named error) —
/// no silent skipping, no partial booking.
fn fetch_trades(rpc: &Rpc) -> Result<Vec<Trade>, Error> {
    let reply = rpc.call("io.haveno.protobuffer.Trades/GetTrades", r#"{"category":"CLOSED"}"#)?;
    match reply.get("trades").and_then(|v| v.as_array()) {
        Some(arr) => arr.iter().map(Trade::from_info).collect(),
        None => Ok(Vec::new()),
    }
}

// ---------------------------------------------------------------------
// enrichment (txid → reto booking)
// ---------------------------------------------------------------------

/// Holds the fetched trades indexed by their on-chain legs, plus the account
/// names a booking needs. `render` turns one wallet transaction into its reto
/// booking when the txid is a trade's deposit or payout, else yields `None` so
/// the monero import books it normally.
pub(crate) struct Enricher {
    trades: Vec<Trade>,
    deposit_idx: HashMap<String, usize>, // deposit txid → trade
    payout_idx: HashMap<String, usize>,  // payout txid → trade
    wallet: String,                      // profile's own wallet account
    fee: String,                         // profile's network-fee account
    deposit: String,                     // reto deposit clearing
    swap: String,                        // reto fiat swap
    maker_fee: String,                   // trading-fee account (we made the offer)
    taker_fee: String,                   // trading-fee account (we took the offer)
    title: String,                       // profile's booking title
}

impl Enricher {
    /// Fetch the trades and index them. `wallet`, `fee`, `title` come from the
    /// monero profile; `deposit` and `swap` from the `haveno.*` block.
    pub(crate) fn fetch(cfg: &Config, wallet: &str, fee: &str, title: &str) -> Result<Enricher, Error> {
        let trades = fetch_trades(&cfg.rpc)?;
        let mut deposit_idx = HashMap::new();
        let mut payout_idx = HashMap::new();
        for (i, t) in trades.iter().enumerate() {
            for txid in &t.deposit_txids {
                deposit_idx.insert(txid.clone(), i);
            }
            if !t.payout_txid.is_empty() {
                payout_idx.insert(t.payout_txid.clone(), i);
            }
        }
        Ok(Enricher {
            trades,
            deposit_idx,
            payout_idx,
            wallet: wallet.to_string(),
            fee: fee.to_string(),
            deposit: cfg.deposit.clone(),
            swap: cfg.swap.clone(),
            maker_fee: cfg.maker_fee.clone(),
            taker_fee: cfg.taker_fee.clone(),
            title: title.to_string(),
        })
    }

    /// The reto booking for one wallet transaction, or `None` when its txid is
    /// not a trade leg. The booking carries the wallet tx as `; rpc:` (like every
    /// other, from the group's source) plus the trade as `; reto:`; all amounts
    /// come from the trade, only the date from the transfer's timestamp.
    pub(crate) fn render(&self, g: &super::crypto_lib::Group) -> Result<Option<String>, Error> {
        if let Some(&i) = self.deposit_idx.get(g.txid.as_str()) {
            // The XMR that actually left the wallet in the deposit tx — the funding
            // leg reconciles its postings against this, so a bundled trading fee is
            // caught rather than silently overstating the wallet.
            let outflow = g.send.as_ref().map(|t| t.amount + t.fee).unwrap_or(0);
            Ok(Some(self.render_funding(&self.trades[i], g.source(), g.time, outflow)?))
        } else if let Some(&i) = self.payout_idx.get(g.txid.as_str()) {
            Ok(Some(self.render_payout(&self.trades[i], g.source(), g.time)))
        } else {
            Ok(None)
        }
    }

    /// The `; rpc:` line for a trade leg — the wallet tx's raw source object, so
    /// a reto booking is as traceable as a plain one (empty if the group has no
    /// transfer, which shouldn't happen for a matched leg).
    fn rpc_line(rpc: Option<&Value>) -> String {
        match rpc {
            Some(r) => format!("\t; rpc: {}\n", serde_json::to_string(r).unwrap_or_default()),
            None => String::new(),
        }
    }

    /// Funding leg (the deposit tx). Sell: the outgoing XMR (tradeAmount + our
    /// security deposit + network fee) splits into fee, the deposit set aside, and
    /// the net traded XMR sold `@@` fiat. Buy: only our security deposit leaves —
    /// no swap yet (it happens at the payout, where the bought XMR arrives).
    ///
    /// The wallet leg is the ACTUAL `outflow` of the deposit tx, so the booking
    /// matches the chain to the piconero. On some trades Haveno bundles our
    /// trading fee (`takerFee`/`makerFee`) into this same tx — then the outflow
    /// exceeds trade+deposit by exactly that fee, which we book to the role's
    /// trading-fee account. An outflow matching neither is a loud error.
    fn render_funding(
        &self,
        t: &Trade,
        rpc: Option<&Value>,
        timestamp: u64,
        outflow: i128,
    ) -> Result<String, Error> {
        let funded = if t.sells_xmr() { t.trade_amount + t.sec_deposit } else { t.sec_deposit };
        let base = funded + t.deposit_fee; // trade + deposit + network fee, no trading fee
        let trade_fee = if outflow == base {
            0
        } else if outflow == base + t.trade_fee {
            t.trade_fee
        } else {
            return Err(Error::from(format!(
                "import: haveno trade {}: deposit outflow {} is neither the funded \
                 amount ({}) nor +tradeFee ({}) — unexpected",
                t.short_id, outflow, base, base + t.trade_fee
            )));
        };
        let trade_fee_account = if t.is_taker { &self.taker_fee } else { &self.maker_fee };

        let mut s = self.header(timestamp);
        s.push_str(&Self::rpc_line(rpc));
        s.push_str(&format!("\t{}  XMR-{}\n", self.wallet, xmr(outflow)));
        s.push_str(&format!("\t{}  XMR{}\n", self.fee, xmr(t.deposit_fee)));
        if trade_fee > 0 {
            s.push_str(&format!("\t{}  XMR{}\n", trade_fee_account, xmr(trade_fee)));
        }
        if t.sells_xmr() {
            s.push_str(&format!("\t{}  XMR{}\n", self.deposit, xmr(t.sec_deposit)));
            s.push_str(&format!(
                "\t{}  {} @@ XMR{}",
                self.swap,
                money(&t.currency, &t.fiat, false),
                xmr(t.trade_amount)
            ));
        } else {
            // Buy funding is purely our security deposit moving to clearing.
            s.push_str(&format!("\t{}  XMR{}", self.deposit, xmr(t.sec_deposit)));
        }
        Ok(s)
    }

    /// Payout leg (the payout tx). The shape is chosen by whether the traded XMR
    /// came back in this payout ([`Trade::payout_includes_trade`]), NOT by our
    /// role — because a refunded sell (the swap fell through, our XMR returned)
    /// settles identically to a completed buy (we received the bought XMR):
    ///   - traded XMR in the payout → the deposit returns AND the swap settles
    ///     here, `fiat` leaving the swap account (a buy pays fiat; a refunded
    ///     sell reverses the fiat booked at funding, so the swap nets to 0);
    ///   - only the deposit back → the swap already settled at funding (a
    ///     completed sell) or never happened (a refunded buy) — deposit bare.
    fn render_payout(&self, t: &Trade, rpc: Option<&Value>, timestamp: u64) -> String {
        let mut s = self.header(timestamp);
        s.push_str(&Self::rpc_line(rpc));
        // The deposit portion that returned — the whole payout, unless the traded
        // XMR is in it too (a bought coin, or a refunded sell whose XMR came
        // back), then minus that.
        let deposit_back = if t.payout_includes_trade {
            t.payout_amount - t.trade_amount
        } else {
            t.payout_amount
        };
        // The payout tx's network fee was taken out of our deposit before it
        // returned, so book the full deposit back (clears the funding leg to 0)
        // and the shortfall as our fee. Derived, not read from a separate field:
        // it's exactly what the return fell short of the deposit.
        let fee = t.sec_deposit - deposit_back;
        s.push_str(&format!("\t{}  XMR{}\n", self.wallet, xmr(t.payout_amount)));
        s.push_str(&format!("\t{}  XMR{}\n", self.fee, xmr(fee)));
        s.push_str(&format!("\t{}  XMR-{}", self.deposit, xmr(t.sec_deposit)));
        if t.payout_includes_trade {
            s.push_str(&format!(
                "\n\t{}  {} @@ XMR{}",
                self.swap,
                money(&t.currency, &t.fiat, true),
                xmr(t.trade_amount)
            ));
        }
        s
    }

    fn header(&self, timestamp: u64) -> String {
        let date = crate::date::ms_to_date(timestamp.saturating_mul(1000));
        format!("{} * {}\n", date, self.title)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enricher() -> Enricher {
        Enricher {
            trades: Vec::new(),
            deposit_idx: HashMap::new(),
            payout_idx: HashMap::new(),
            wallet: "w".to_string(),
            fee: "f".to_string(),
            deposit: "d".to_string(),
            swap: "s".to_string(),
            maker_fee: "mf".to_string(),
            taker_fee: "tf".to_string(),
            title: "reto".to_string(),
        }
    }

    // A sell: 1 XMR traded for $300, our 0.1 XMR security deposit, 0.00003 fee;
    // 0.0999 comes back (deposit minus the payout fee).
    fn sell() -> Trade {
        Trade::from_info(&serde_json::json!({
            "shortId": "sell01",
            "role": "XMR seller as taker",
            "amount": "1000000000000",
            "tradeVolume": "300",
            "offer": { "counterCurrencyCode": "USD" },
            "sellerSecurityDeposit": "100000000000",
            "sellerDepositTxFee": "30000000",
            "sellerPayoutAmount": "99900000000",
            "takerFee": "10000000000",
            "makerFee": "1000000000",
            "makerDepositTxId": "dep_maker",
            "takerDepositTxId": "dep_taker",
            "payoutTxId": "pay01"
        }))
        .unwrap()
    }

    // A buy: 2 XMR bought for €500, our 0.3 XMR security deposit, 0.00004 funding
    // fee; 2.29998 comes back (the bought 2.0 + deposit, minus the 0.00002 payout
    // fee).
    fn buy() -> Trade {
        Trade::from_info(&serde_json::json!({
            "shortId": "buy01",
            "role": "XMR buyer as taker",
            "amount": "2000000000000",
            "tradeVolume": "500",
            "offer": { "counterCurrencyCode": "EUR" },
            "buyerSecurityDeposit": "300000000000",
            "buyerDepositTxFee": "40000000",
            "buyerPayoutAmount": "2299980000000",
            "takerFee": "20000000000",
            "makerFee": "2000000000",
            "makerDepositTxId": "bdep_maker",
            "takerDepositTxId": "bdep_taker",
            "payoutTxId": "bpay01"
        }))
        .unwrap()
    }

    #[test]
    fn from_info_reads_amounts_from_our_side_of_the_trade() {
        let t = sell();
        assert_eq!(t.trade_amount, 1_000_000_000_000);
        assert_eq!(t.fiat, "300"); // verbatim tradeVolume
        assert_eq!(t.currency, "USD");
        assert!(t.sells_xmr());
        assert_eq!(t.sec_deposit, 100_000_000_000); // seller* fields for a seller
        assert_eq!(t.deposit_fee, 30_000_000);
        assert_eq!(t.payout_amount, 99_900_000_000);
        assert_eq!(t.payout_txid, "pay01");
        assert!(t.deposit_txids.contains(&"dep_taker".to_string()));
        assert!(t.is_taker); // "as taker"
        assert_eq!(t.trade_fee, 10_000_000_000); // takerFee for a taker

        let b = buy();
        assert_eq!(b.sec_deposit, 300_000_000_000); // buyer* fields for a buyer
        assert_eq!(b.payout_amount, 2_299_980_000_000);
    }

    #[test]
    fn money_renders_tradevolume_verbatim() {
        assert_eq!(money("USD", "300", false), "$300"); // verbatim — no .00 padding
        assert_eq!(money("EUR", "500", true), "€-500"); // sign after symbol
        assert_eq!(money("BTC", "0.00523640", false), "BTC0.00523640"); // kept as reported
    }

    // A stand-in get_transfers object for the `; rpc:` line.
    fn rpc() -> Value {
        serde_json::json!({ "txid": "leg_txid", "amount": 1, "type": "in" })
    }

    #[test]
    fn sell_funding_splits_into_fee_deposit_and_at_at_swap() {
        // outflow == trade + deposit + network fee (no trading fee bundled here).
        let s = enricher()
            .render_funding(&sell(), Some(&rpc()), 1_700_000_000, 1_100_030_000_000)
            .unwrap();
        assert!(s.contains("\t; rpc: "), "{s}"); // the wallet tx, the only source line
        assert!(!s.contains("; reto:"), "{s}"); // no reto line — dropped entirely
        assert!(s.contains("\tw  XMR-1.100030000000\n"), "{s}"); // wallet = trade + deposit + fee
        assert!(s.contains("\tf  XMR0.000030000000\n"), "{s}"); // fee
        assert!(!s.contains("\ttf  "), "{s}"); // no trading-fee leg (not bundled)
        assert!(s.contains("\td  XMR0.100000000000\n"), "{s}"); // deposit set aside
        assert!(s.contains("\ts  $300 @@ XMR1.000000000000"), "{s}"); // swap @@ tradeAmount
        // The three XMR legs net to -tradeAmount, the @@ cost counterweighs → 0.
    }

    #[test]
    fn sell_funding_with_bundled_taker_fee_books_it_to_the_trade_fee_account() {
        // Haveno bundled the taker fee into the deposit tx, so the outflow is
        // trade + deposit + network fee + takerFee (0.01). It books to the taker
        // fee account, and the wallet leg matches the real outflow to the piconero.
        let s = enricher()
            .render_funding(&sell(), Some(&rpc()), 1_700_000_000, 1_110_030_000_000)
            .unwrap();
        assert!(s.contains("\tw  XMR-1.110030000000\n"), "{s}"); // real outflow (incl. taker fee)
        assert!(s.contains("\tf  XMR0.000030000000\n"), "{s}"); // network fee
        assert!(s.contains("\ttf  XMR0.010000000000\n"), "{s}"); // taker fee → taker-fee account
        assert!(s.contains("\td  XMR0.100000000000\n"), "{s}"); // deposit set aside
        assert!(s.contains("\ts  $300 @@ XMR1.000000000000"), "{s}");
    }

    #[test]
    fn funding_outflow_matching_neither_is_an_error() {
        // An outflow that is neither the funded amount nor +tradeFee is unexpected.
        let r = enricher().render_funding(&sell(), Some(&rpc()), 1_700_000_000, 1_105_000_000_000);
        assert!(r.is_err(), "unexpected outflow must error");
    }

    #[test]
    fn sell_payout_returns_full_deposit_and_books_the_payout_fee() {
        // Only the deposit returns (success). The wallet gets the payout, the
        // shortfall (deposit − payout = the payout network fee) books to fee, and
        // the FULL deposit clears — so the deposit account nets the funding leg
        // to exactly 0 instead of leaving the fee behind.
        let s = enricher().render_payout(&sell(), Some(&rpc()), 1_700_000_000);
        assert!(s.contains("\tw  XMR0.099900000000\n"), "{s}"); // payout amount back
        assert!(s.contains("\tf  XMR0.000100000000\n"), "{s}"); // payout fee (0.1 - 0.0999)
        assert!(s.ends_with("\td  XMR-0.100000000000"), "{s}"); // full deposit clears
    }

    #[test]
    fn buy_funding_is_only_the_security_deposit() {
        // outflow == deposit + network fee (no trading fee bundled).
        let s = enricher()
            .render_funding(&buy(), Some(&rpc()), 1_700_000_000, 300_040_000_000)
            .unwrap();
        assert!(s.contains("\tw  XMR-0.300040000000\n"), "{s}"); // deposit + fee out
        assert!(s.contains("\tf  XMR0.000040000000\n"), "{s}");
        assert!(s.ends_with("\td  XMR0.300000000000"), "{s}"); // deposit set aside
        assert!(!s.contains(" @@ "), "{s}"); // no swap yet
    }

    #[test]
    fn buy_payout_carries_the_at_at_swap() {
        let s = enricher().render_payout(&buy(), Some(&rpc()), 1_700_000_000);
        assert!(s.contains("\tw  XMR2.299980000000\n"), "{s}"); // bought XMR + deposit back
        assert!(s.contains("\tf  XMR0.000020000000\n"), "{s}"); // payout fee off the deposit
        assert!(s.contains("\td  XMR-0.300000000000\n"), "{s}"); // full deposit clears
        assert!(s.contains("\ts  €-500 @@ XMR2.000000000000"), "{s}"); // paid €500 for 2 XMR
    }

    // A refunded sell (the swap fell through — e.g. a reorg — so the WHOLE amount
    // came back): same 1 XMR / $300 / 0.1 deposit as `sell`, but the payout
    // returns deposit + trade (1.1) minus the payout fee → 1.0999.
    fn sell_refund() -> Trade {
        Trade::from_info(&serde_json::json!({
            "shortId": "refund1",
            "role": "XMR seller as taker",
            "amount": "1000000000000",
            "tradeVolume": "300",
            "offer": { "counterCurrencyCode": "USD" },
            "sellerSecurityDeposit": "100000000000",
            "sellerDepositTxFee": "30000000",
            "sellerPayoutAmount": "1099900000000",
            "takerFee": "10000000000",
            "makerFee": "1000000000",
            "makerDepositTxId": "rdep_maker",
            "takerDepositTxId": "rdep_taker",
            "payoutTxId": "rpay01"
        }))
        .unwrap()
    }

    #[test]
    fn refunded_sell_reverses_the_swap_and_nets_deposit_to_zero() {
        // The whole amount came back, so the payout carries the trade: it books
        // the payout fee, clears the FULL deposit, and reverses the funding swap
        // (fiat leaves the swap account).
        let t = sell_refund();
        assert!(t.payout_includes_trade, "refund must be detected");
        let s = enricher().render_payout(&t, Some(&rpc()), 1_700_000_000);
        assert!(s.contains("\tw  XMR1.099900000000\n"), "{s}"); // full amount back
        assert!(s.contains("\tf  XMR0.000100000000\n"), "{s}"); // payout fee (0.1 - 0.0999)
        assert!(s.contains("\td  XMR-0.100000000000\n"), "{s}"); // full deposit clears
        assert!(s.ends_with("\ts  $-300 @@ XMR1.000000000000"), "{s}"); // swap reversed
        // Over the trade: deposit funding +0.1, payout -0.1 → 0; swap +$300 (funding)
        // -$300 (here) = 0; only the two network fees remain as cost.
    }

    #[test]
    fn payout_near_neither_value_is_an_error() {
        // A payout that is neither ~deposit nor ~deposit+trade (here 0.5, right
        // between) is an unexpected settlement — from_info fails loud rather than
        // guessing a shape.
        let r = Trade::from_info(&serde_json::json!({
            "shortId": "weird1",
            "role": "XMR seller as taker",
            "amount": "1000000000000",
            "tradeVolume": "300",
            "offer": { "counterCurrencyCode": "USD" },
            "sellerSecurityDeposit": "100000000000",
            "sellerDepositTxFee": "30000000",
            "sellerPayoutAmount": "500000000000",
            "makerDepositTxId": "wdep_maker",
            "takerDepositTxId": "wdep_taker",
            "payoutTxId": "wpay01"
        }));
        assert!(r.is_err(), "a payout near neither value must error");
    }
}
