![alt text](https://www.diamondsite.de/img/projects/acc.jpg "acc accounting plaintext double-entry accounting command line tool")
# acc

acc(ounting) is a plaintext double-entry accounting command line tool. It is open source and a free alternative to properary accounting software.

acc tracks commodities like fiat money or crypto currencies using a strict following of the double-entry accounting principles. It is inspired by [ledger](https://github.com/ledger/ledger) and [hledger](https://github.com/simonmichael/hledger) and uses the ledger file format.

## Installation

With [cargo](https://github.com/rust-lang/cargo):

```
cargo install acc
```

From source:

```
git clone https://github.com/rudolfschmidt/acc
cd acc
cargo build --release
./target/release/acc -f demo.ledger print
```

## Quick Start

Record transactions in a plain text file using your favorite texteditor

```
2020-01-01 (123) opening balances
    assets:checking           $1234.00
    equity

2020-03-15 (456) client payment
    assets:checking           $2000.00
    income:consulting

2020-03-20 (789) Sprouts
    expenses:food:groceries   $100.00
    assets:cash                $40.00
    assets:checking
```

## Global Options

```
acc [-f FILE] [-d DIR] [--begin DATE] [--end DATE] [--sort FIELD] [--future] <COMMAND>
```

| Option | Short | Description |
|--------|-------|-------------|
| `--file FILE` | `-f` | Ledger file to process (can be specified multiple times) |
| `--dir DIR` | `-d` | Load all files from directory recursively |
| `--begin DATE` | `-b` | Include only transactions on or after this date (YYYY-MM-DD) |
| `--end DATE` | `-e` | Include only transactions before this date (YYYY-MM-DD) |
| `--sort FIELD` | `-S` | Sort by: date, amount, account, description. `rev:` prefix for reverse |
| `--future` | | Include future transactions (default: only up to today) |

## Commands

### bal(ance)

```
$ acc -f demo.ledger bal
 $3134.00 assets
   $40.00   cash
 $3094.00   checking
$-1234.00 equity
  $100.00 expenses
  $100.00   food
  $100.00     groceries
$-2000.00 income
$-2000.00   consulting
---------
        0
```

```
$ acc -f demo.ledger bal --flat
   $40.00 assets:cash
 $3094.00 assets:checking
$-1234.00 equity
  $100.00 expenses:food:groceries
$-2000.00 income:consulting
---------
        0
```

### reg(ister)

```
$ acc -f demo.ledger reg
2020-01-01 opening balances    assets:checking             $1234.00    $1234.00
                               equity                     $-1234.00           0
2020-03-15 client payment      assets:checking             $2000.00    $2000.00
                               income:consulting          $-2000.00           0
2020-03-20 Sprouts             expenses:food:groceries      $100.00     $100.00
                               assets:cash                   $40.00     $140.00
                               assets:checking             $-140.00           0
```

### print

Default shows balanced/explicit amounts:

```
$ acc -f demo.ledger print
2020-01-01 opening balances
	assets:checking            $ 1234.00
	equity                     $-1234.00

2020-03-15 client payment
	assets:checking            $ 2000.00
	income:consulting          $-2000.00

2020-03-20 Sprouts
	expenses:food:groceries    $ 100.00
	assets:cash                $ 40.00
	assets:checking            $-140.00
```

`--raw` shows original data without computed amounts:

```
$ acc -f demo.ledger print --raw
2020-01-01 opening balances
	assets:checking            $ 1234.00
	equity

2020-03-15 client payment
	assets:checking            $ 2000.00
	income:consulting

2020-03-20 Sprouts
	expenses:food:groceries    $ 100.00
	assets:cash                $ 40.00
	assets:checking
```

### accounts

```
$ acc -f demo.ledger accounts
assets:cash
assets:checking
equity
expenses:food:groceries
income:consulting
```

```
$ acc -f demo.ledger accounts --tree
assets
  cash
  checking
equity
expenses
  food
    groceries
income
  consulting
```

### codes

```
$ acc -f demo.ledger codes
123
456
789
```

### nav(igate)

Interactive account tree browser. Navigate with vim keybindings.

```
$ acc -f demo.ledger nav
```

| Key | Action |
|-----|--------|
| `j` / `↓` | Move down |
| `k` / `↑` | Move up |
| `Enter` / `Space` | Expand/collapse |
| `l` / `→` | Expand |
| `h` / `←` | Collapse |
| `G` | Jump to bottom |
| `gg` | Jump to top |
| `Ctrl-d` | Half page down |
| `Ctrl-u` | Half page up |
| `q` / `Esc` | Quit |

### val(idate)

Check journal for inconsistencies. Currently checks:
* Commodity symbols must be uppercase (single-char symbols like `$`, `€`, `£` are excluded)

```
$ acc -f demo.ledger val
No issues found.
```

## Directives

### Include

Includes another ledger file within a ledger file

```
include file.ledger
include files/file.ledger
```

Includes all files with extension `ledger`

```
include *.ledger
include files/*.ledger
```

Includes any file

```
include *.*
include files/*.*
```

Include files from any directory inside directory `files` (first level)

```
include files/*/*.*
include files/*/*.ledger
```

Includes all files from all directories under directory `files` (recursive)

```
include files/**/*.*
include files/**/*.ledger
```

## FAQ

### Why should you not use properary software, specially for accounting?

It is ok to make money with software that costs time and effort to create it.

We ignore the fact that using proprietary software is a bad idea most of the time, there are situations in life that make it even much worse to use proprietary software and accounting and money are one of them.

The nature of accounting is that you organize the most sensitive data about yourself, your financial data.

Most proprietary accounting software will ask you to go online to connect to their server. At the moment you do so, they will store your data on their servers to "maintain" it.

Your data is on a machine of a profit-oriented company that is interested to make money out of you.

If they decide to sell your data, how can you know about it or stop them?

You cannot, because everything is closed, their software, their server.

You traded comfort against freedom and in most cases paid even for it.

Have you ever tried to buy something per invoice?

Most of the time you will be forwarded to another company that checks your credit rating. Have you ever made the experience to get rejected? If so, did they tell you the reason for it? Most of the time they do not, and there is a reason for it. They do not want you to know what they know about you and what the sources of the information are.

There is probably nothing more sensitive and private than financial data.

If freedom and privacy matter anything to you, care about your finances or others will do!

### Why should you track your money?
1. Whatever you respect in your life, stays; whatever you do not respect, goes. If you do not respect your money, you will have a hard time to keep it. It will run away in direction you do not know. Money needs care and attention like everything valuable else. The best way to care is by knowing your finances.

2. Another reason is to have control over your life. Which cost exist, why they exist, and the most important question, is the amount correct. You cannot believe how many times I got invoices with different numbers that I had to complain for. If the time difference are some days, you can do it manually, but try to remember which number you got if the difference are months or even one year. Some institutions hope that you forget them and they can trick you by sending invoices with different amounts to look like that they just sent you a copy.

3. Another important reasons, maybe the most important, is to be able to take decisions based on financial data rather than on temporary feelings. A simple real-life scenario could be the question if you spent your money to go out and eat food at a restaurant or save some bucks by eating at home. Track your grocery costs and calculate the daily average and you get suprised how much you "eat at home". Eating outside will not look expensive anymore I bet.

## Changelog

See [CHANGELOG.md](CHANGELOG.md)
