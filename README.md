# acc
acc(ounting) is a plaintext double-entry accounting command line tool. 

## Warning
acc is currently in beta and not for production ready

## About
acc tracks commodities like fiat money or crypto currencies using a strict following of the double-entry accounting principles. It is inspired by ledger(1) and hledger(2) and uses the ledger file format.

```
acc [-f FILE] [command] [arguments]
```

The order of the command line arguments does not matter. They are parsed first and handled after.

### Quick Start

#### Installation

With [cargo](https://github.com/rust-lang/cargo):

```
cargo install acc
```

#### Create Ledger File
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

Use one of the listed commands below.

### Commands

#### Balance Report

```
$ acc -f [file] [bal|balance] [--flat|--tree]
```

##### Tree Balance Report

```
$ acc -f demo.ledger bal --tree
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

##### Flat Balance Report

```
$ acc -f demo.ledger bal --flat
   $40.00 assets:cash
 $3094.00 assets:checking
$-1234.00 equity
  $100.00 expenses:food:groceries
$-2000.00 income:consulting
```

#### Register Report

##### Syntax

```
$ acc -f [file] [reg|register]
```

##### Example

```
$ acc -f demo.ledger reg 
```

```
2020-01-01 opening balances    assets:checking            $ 1234.001       $1234.00
                               equity                     $-1234.001       $   0.00
2020-03-15 client payment      assets:checking            $ 2000.001       $2000.00
                               income:consulting          $-2000.001       $   0.00
2020-03-20 Sprouts             expenses:food:groceries    $  100.001       $ 100.00
                               assets:cash                $   40.001       $ 140.00
                               assets:checking            $ -140.001       $   0.00
```

#### Print Report

##### Syntax

```
$ acc -f [file] [print] [--eval|--raw]
```

##### Evaluated Print Report

It interprets the posting amounts and fill them with useful numbers

```
$ acc -f demo.ledger print --eval
```

```
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

##### Raw Print Report

It prints the data how it is but just formated. Useful when you want to format your ledger files.

```
$ acc -f demo.ledger print --raw
```

```
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

#### Accounts Report

Print the accounts in alphabetical order.

##### Syntax

```
$ acc -f demo.ledger accounts [--tree|--flat]
```

##### Tree Output

```
$ acc -f demo.ledger accounts --tree
```

```
assets
  checking
expenses
  food
    groceries
```

Future Planed...

##### Flat Output

```
$ acc -f demo.ledger accounts --flat
```

```
assets:cash
assets:checking
equity
expenses:food:groceries
income:consulting
```

#### Codes Report

Print the codes in natural order.

```
acc -f demo.ledger codes
```

```
123
456
789
```

## ToDo
* Add transaction balance check
* Add support for expressions
* Add support for periodic transactions
* Add support for the yaml file format

## References
1: https://github.com/ledger/ledger

2: https://github.com/simonmichael/hledger
