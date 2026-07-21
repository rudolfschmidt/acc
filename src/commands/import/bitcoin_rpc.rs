//! Bitcoin import entry (`wallet.coin bitcoin`).
//!
//! bitcoind and litecoind speak the identical Bitcoin Core RPC, so the whole
//! implementation lives once in [`super::bitcoin_lib`]; this file is the thin
//! per-coin entry point that forwards to it, kept separate so every importable
//! coin has its own discoverable module.

use crate::error::Error;

pub(super) fn run(conf_path: &str, write: bool) -> Result<(), Error> {
    super::bitcoin_lib::run(conf_path, write)
}
