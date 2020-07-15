# acc: double-entry accounting for the command line
acc is a plaintext double-entry accounting command line tool. It tracks commodities like fiat money, crypto currencies or time, using a strict following of the double-entry accounting principle. It is inspired by ledger(1) and hledger(2) and uses the ledger file format. 

```
acc [-f FILE] [balance|register|print]
```

acc read data from one or more files in the ledger file format and generate reports based on the provided data and selected options. It only reads data and never write any data to your ledger files. 

## FAQ
### Why another ledger implementation?
...
### Meaning of the name acc
acc is an abbreviation for accounting.

## ToDo
* Implementing a yaml file format

## References
1: https://github.com/ledger/ledger

2: https://github.com/simonmichael/hledger
