# 07 — Balance assertions and assignments

Two uses of the `=` syntax on a posting, both modelled on
ledger-cli:

1. **Assertion** — `AMOUNT = EXPECTED`. Posting has its own
   amount; the `=` says *and after this posting, the account's
   running balance must equal `EXPECTED`*. If it doesn't, acc
   errors out at load time.
2. **Assignment** — `= EXPECTED` (no amount on the posting). Same
   syntax without the posting amount; acc fills in whatever amount
   brings the account to `EXPECTED`.

## Journal using both

```
2024-01-01 opening
    assets:checking  $1000.00
    equity           $-1000.00

2024-01-15 grocery
    expenses:food     $50.00
    assets:checking  $-50.00 = $950.00

2024-02-01 salary
    assets:checking  $2000.00
    income:salary   $-2000.00

2024-02-15 reconcile
    assets:checking  = $2950.00
    equity:adjust
```

- Line 5: posting has amount `$-50.00` AND an assertion
  `= $950.00`. After this posting runs, the running balance on
  `assets:checking` must be `$950.00` (it was `$1000.00`, minus
  `$50.00`, so yes).
- Line 13: no amount on `assets:checking`. The `= $2950.00` is
  an **assignment** — the booker computes the delta needed to go
  from `$2950.00` (current running `$950 + $2000`) to `$2950.00`,
  and fills in `$0.00`. `equity:adjust` gets the corresponding
  counter amount (also zero, here — the account already equals
  the target). Useful for reconciliation against a bank
  statement.

## It loaded, so the assertions passed

```
$ acc -f journal.ledger bal
 $2950.00 assets
 $2950.00   checking
$-1000.00 equity
   $50.00 expenses
   $50.00   food
$-2000.00 income
$-2000.00   salary
---------
        0
```

`print --raw` shows the source untouched. The assertion and
assignment syntax survives into the raw output:

```
$ acc -f journal.ledger print --raw
2024-01-01 opening
    assets:checking  $1000.00
    equity           $-1000.00

2024-01-15 grocery
    expenses:food     $50.00
    assets:checking  $-50.00 = $950.00

2024-02-01 salary
    assets:checking  $2000.00
    income:salary   $-2000.00

2024-02-15 reconcile
    assets:checking  = $2950.00
    equity:adjust
```

## Failure case

Change one assertion to a wrong number:

```
2024-01-15 grocery
    expenses:food     $50.00
    assets:checking  $-50.00 = $999.00
```

Load error with source excerpt:

```
$ acc -f journal.ledger bal
While parsing file "journal.ledger" at line 5:
>> balance assertion on `assets:checking` failed (expected $999.00, got $950.00)

5 | 2024-01-15 grocery
6 |     expenses:food     $50.00
7 |     assets:checking  $-50.00 = $999.00
```

Exit code 1. No partial output — the whole journal is rejected
until the assertion is fixed or removed.

## When to use which

- **Assertions**: running periodic checkpoints. At the end of
  every month, after reconciling against a bank statement, plant
  an assertion. If anything later goes wrong in your journal
  (typo, split transaction mis-entered), the assertion will catch
  it immediately instead of letting drift accumulate silently.
- **Assignments**: reconciliation itself. Write what your bank
  says the balance is, let acc figure out how much is missing or
  extra. The counter-posting captures the delta on whichever
  account you're using as an adjustment account.

Both features are strict: no fuzzy-matching, no tolerance window.
If the numbers don't line up exactly, it's an error.
