//! Configuration from TOML. acc reads every `.toml` file in the `CONFIG_DIR`
//! directory and merges them into commodity, label, role and exchange settings.
//! What a table is comes from its shape: `[[commodity]]`, `[[label]]`, `[cta]` /
//! `[slippage]` / `[holding]` / `[capital]`, `[exchange]`. A caller may split
//! them across files or keep everything in one; all `.toml` files merge.
//!
//! The config is turned into synthetic parser [`Entry`]s ([`Config::entries`])
//! and prepended to the parse stream, so the resolver builds commodities /
//! roles / labels — and resolves `$role:slot` account references — the exact
//! same way it did from `commodity` / `account` directives. One code path, fed
//! by TOML instead of a bespoke DSL.

use std::sync::Arc;

use serde::Deserialize;

use crate::parser::entry::Entry;
use crate::parser::located::Located;

#[derive(Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    commodity: Vec<Commodity>,
    #[serde(default)]
    label: Vec<Label>,
    #[serde(default)]
    exchange: Exchange,
    #[serde(default)]
    cta: GainLoss,
    #[serde(default)]
    slippage: GainLoss,
    #[serde(default)]
    holding: GainLoss,
    #[serde(default)]
    capital: GainLoss,
}

#[derive(Deserialize)]
struct Commodity {
    symbol: String,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    parities: Vec<String>,
    precision: usize,
}

#[derive(Deserialize)]
struct Label {
    account: String,
    skip: usize,
    balance: String,
    register: String,
}

/// The gain / loss account pair for a valuation role (`[cta]`, `[holding]`, …).
#[derive(Deserialize, Default)]
struct GainLoss {
    gain: Option<String>,
    loss: Option<String>,
}

#[derive(Deserialize, Default)]
struct Exchange {
    /// Default `-X` valuation target when a report sets none.
    default: Option<String>,
    /// openexchangerates.org app id for `acc update` (name the provider so
    /// other rate APIs can be added as their own keys).
    openexchangerates: Option<String>,
}

impl Config {
    /// Read + merge every `.toml` file in the `CONFIG_DIR` directory. Empty when
    /// the variable is unset or the directory is absent, so a standalone acc
    /// without config still runs. Used only for the `[exchange]` settings needed
    /// before a load (the `-X` default, the fiat API key); commodities / roles /
    /// labels reach the resolver as parsed entries from the file stream instead
    /// (see [`entries_from_str`] and the loader's extension dispatch).
    pub fn from_env() -> Config {
        match std::env::var("CONFIG_DIR") {
            Ok(dir) => Config::from_dir(std::path::Path::new(&dir)),
            Err(_) => Config::default(),
        }
    }

    fn from_dir(dir: &std::path::Path) -> Config {
        let mut merged = Config::default();
        let Ok(entries) = std::fs::read_dir(dir) else {
            return merged;
        };
        let mut paths: Vec<_> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
        paths.sort();
        for path in paths {
            if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }
            if let Ok(text) = std::fs::read_to_string(&path)
                && let Ok(cfg) = toml::from_str::<Config>(&text)
            {
                merged.commodity.extend(cfg.commodity);
                merged.label.extend(cfg.label);
                merged.cta.merge(cfg.cta);
                merged.slippage.merge(cfg.slippage);
                merged.holding.merge(cfg.holding);
                merged.capital.merge(cfg.capital);
                merged.exchange.merge(cfg.exchange);
            }
        }
        merged
    }

    /// The config as synthetic parser entries, so the resolver handles them like
    /// the old `commodity` / `account` directives: `Entry::Commodity` for
    /// commodities, `Entry::RoleAccount` for roles and display labels. `file` is
    /// the source `.toml` path, carried for resolver error messages.
    fn entries(&self, file: &Arc<str>) -> Vec<Located<Entry>> {
        let mut v = Vec::new();
        for c in &self.commodity {
            v.push(entry(file, Entry::Commodity {
                symbol: c.symbol.clone(),
                aliases: c.aliases.clone(),
                parities: c.parities.clone(),
                precision: Some(c.precision),
            }));
        }
        push_role(&mut v, file, "cta gain", &self.cta.gain);
        push_role(&mut v, file, "cta loss", &self.cta.loss);
        push_role(&mut v, file, "slippage gain", &self.slippage.gain);
        push_role(&mut v, file, "slippage loss", &self.slippage.loss);
        push_role(&mut v, file, "holding gain", &self.holding.gain);
        push_role(&mut v, file, "holding loss", &self.holding.loss);
        push_role(&mut v, file, "capital gain", &self.capital.gain);
        push_role(&mut v, file, "capital loss", &self.capital.loss);
        for l in &self.label {
            // `skip` leading wildcard segments then the account — the same
            // account string an old `account $segment:…` label directive used.
            let acct = format!("{}{}", "$segment:".repeat(l.skip), l.account);
            v.push(entry(file, Entry::RoleAccount {
                role: format!("label-balance {}", l.balance),
                account: acct.clone(),
            }));
            v.push(entry(file, Entry::RoleAccount {
                role: format!("label-register {}", l.register),
                account: acct,
            }));
        }
        v
    }
}

/// Parse one config `.toml` source into its synthetic parser entries. The
/// loader calls this for every `.toml` file in the stream (dispatched by
/// extension), so commodities / roles / labels flow through the same parallel
/// read as the ledgers and the resolver evaluates the whole stream together.
pub fn entries_from_str(
    source: &str,
    file: &Arc<str>,
) -> Result<Vec<Located<Entry>>, toml::de::Error> {
    Ok(toml::from_str::<Config>(source)?.entries(file))
}

/// The default `-X` valuation target from `[exchange] default`, or `None`.
pub fn default_exchange() -> Option<String> {
    Config::from_env().exchange.default
}

/// The openexchangerates.org API key from `[exchange] openexchangerates`.
pub fn openexchangerates() -> Option<String> {
    Config::from_env().exchange.openexchangerates
}

fn entry(file: &Arc<str>, value: Entry) -> Located<Entry> {
    Located { file: file.clone(), line: 0, value }
}

fn push_role(v: &mut Vec<Located<Entry>>, file: &Arc<str>, role: &str, account: &Option<String>) {
    if let Some(a) = account {
        v.push(entry(file, Entry::RoleAccount { role: role.to_string(), account: a.clone() }));
    }
}

impl GainLoss {
    /// Merge another gain/loss pair in; a later file's set field wins.
    fn merge(&mut self, other: GainLoss) {
        set(&mut self.gain, other.gain);
        set(&mut self.loss, other.loss);
    }
}

impl Exchange {
    fn merge(&mut self, other: Exchange) {
        set(&mut self.default, other.default);
        set(&mut self.openexchangerates, other.openexchangerates);
    }
}

fn set(slot: &mut Option<String>, value: Option<String>) {
    if value.is_some() {
        *slot = value;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_becomes_synthetic_entries() {
        let entries = entries_from_str(
            "[[commodity]]\nsymbol = \"$\"\naliases = [\"USD\"]\nprecision = 2\n\
             [holding]\ngain = \"in:hg\"\nloss = \"ex:hl\"\n\
             [[label]]\naccount = \"bb\"\nskip = 1\nbalance = \"bal\"\nregister = \"reg\"\n",
            &Arc::from("<test>"),
        )
        .unwrap();
        // 1 commodity + 2 roles (holding gain/loss) + 2 labels (balance/register).
        assert_eq!(entries.len(), 5);
        assert!(matches!(&entries[0].value, Entry::Commodity { symbol, .. } if symbol == "$"));
        assert!(entries.iter().any(|e| matches!(&e.value,
            Entry::RoleAccount { role, account } if role == "holding gain" && account == "in:hg")));
        // `skip = 1` becomes the `$segment:bb` pattern the resolver anchors.
        assert!(entries.iter().any(|e| matches!(&e.value,
            Entry::RoleAccount { role, account }
                if role == "label-balance bal" && account == "$segment:bb")));
    }
}
