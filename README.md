# acc

[![crates.io](https://img.shields.io/crates/v/acc.svg)](https://crates.io/crates/acc)
[![license](https://img.shields.io/crates/l/acc.svg)](LICENSE)

> **acc(ounting)** — a plaintext double-entry accounting CLI in the
> ledger tradition, written in Rust.

acc reads the [ledger](https://www.ledger-cli.org/) journal format and
continues John Wiegley's CLI-first lineage: reports, filters, currency
conversion, an interactive navigator — all driven from plaintext files
you own and edit with whatever tools you already use. Independent
codebase, its own semantic choices, no database, no cloud, no account.

---

## Quick start

```sh
cargo install acc
```

Put this in `journal.ledger`:

```
2024-01-01 (opening) initial balances
    assets:checking           $5000.00
    equity:opening           $-5000.00

2024-01-05 (42) Groceries
    expenses:food              $58.20
    assets:checking

2024-01-10 * paycheck
    assets:checking           $2500.00
    income:salary            $-2500.00
```

Run:

```
$ acc -f journal.ledger balance
 $7441.80 assets
 $7441.80   checking
$-5000.00 equity
$-5000.00   opening
   $58.20 expenses
   $58.20   food
$-2500.00 income
$-2500.00   salary
---------
        0
```

```
$ acc -f journal.ledger register
2024-01-01 initial balances  assets:checking   $5000.00  $5000.00
                             equity:opening   $-5000.00         0
2024-01-05 Groceries         expenses:food       $58.20    $58.20
                             assets:checking    $-58.20         0
2024-01-10 * paycheck        assets:checking   $2500.00  $2500.00
                             income:salary    $-2500.00         0
```

Commodities, valuation roles and display labels are configured as TOML
files in a directory named by the `CONFIG_DIR` environment variable;
every `.toml` there is read alongside your ledgers (dispatched by
extension). Transactions, `P` prices and `~ ` periodics live in the
`.ledger` files themselves.

---

## Installation

From crates.io:

```
cargo install acc
```

From source:

```
git clone https://github.com/rudolfschmidt/acc
cd acc
cargo build --release
./target/release/acc --help
```

Minimum Rust: **1.85** (edition 2024). Runs anywhere Rust builds
(Linux, macOS, Windows, BSDs). On Linux/BSD the build needs a system
OpenSSL (HTTPS for `acc update` goes through native-tls).

---

## Documentation

The full reference — journal format, filtering, currency conversion,
rate updates, imports, directives, worked examples — is moving to a
dedicated site, **under construction**:

<https://github.com/rudolfschmidt/acc-website>

Until it lands, `acc --help` (and `acc <command> --help`) documents
every command and flag.

---

## Changelog

See [CHANGELOG.md](CHANGELOG.md).

## Contributing

Bug reports, patches, and feature discussion are welcome at
<https://github.com/rudolfschmidt/acc>. Before sending a patch, please
run `cargo test` and `cargo clippy` locally.

## License

GPL-3.0. See [LICENSE](LICENSE).
