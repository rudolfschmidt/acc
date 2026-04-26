# 08 — `acc diff`

Source-level comparison of two ledger files or directory trees,
ignoring whitespace (`diff -w` semantics) and rendering the
result in `git diff` style. Two invocation modes:

- **Explicit:** `acc diff OLD NEW` — both sides given directly.
- **Snapshot:** `acc diff --snapshot DIR [PATH...]` — acc finds
  the matching path inside `DIR` via longest-suffix match.

Both modes accept files or directories on each side, but the
**types must match**: file vs. file or directory vs. directory.
Mixed types are an error.

All examples below use this directory layout:

```
/tmp/diff-ex
├── old
│   ├── cash
│   │   ├── feb.ledger
│   │   └── jan.ledger
│   └── income
│       └── salary.ledger
├── new
│   ├── cash
│   │   ├── feb.ledger
│   │   └── jan.ledger
│   └── income
│       └── bonus.ledger
└── snap                  (mirror of `old/`, used as snapshot root)
    ├── cash
    │   ├── feb.ledger
    │   └── jan.ledger
    └── income
        └── salary.ledger
```

`old/cash/jan.ledger` and `new/cash/jan.ledger` are byte-identical.
`feb.ledger` differs between sides ($12 → $14). `salary.ledger`
exists only in `old/`, `bonus.ledger` only in `new/`.

---

## Mode 1: explicit `OLD NEW`

### File vs. file, identical content

```
$ acc diff old/cash/jan.ledger new/cash/jan.ledger
1 files compared, 0 with differences
```

Exit code `0`. No hunks, no `--- / +++` headers — when the
content is identical, only the summary line is printed.

### File vs. file, content differs

```
$ acc diff old/cash/feb.ledger new/cash/feb.ledger
--- old/cash/feb.ledger
+++ new/cash/feb.ledger
@@ -1,3 +1,3 @@
 2024-02-10 * Lunch
-	expenses:food  $12.00
-	assets:cash   $-12.00
+	expenses:food  $14.00
+	assets:cash   $-14.00

1 files compared, 1 with differences
```

Exit code `1`. Hunk header `@@ -1,3 +1,3 @@` says: in OLD lines
1–3, in NEW lines 1–3. Two postings are removed (old amounts) and
two added (new amounts); the transaction header line is context.

### Directory vs. directory (whole tree)

```
$ acc diff old new
--- old/cash/feb.ledger
+++ new/cash/feb.ledger
@@ -1,3 +1,3 @@
 2024-02-10 * Lunch
-	expenses:food  $12.00
-	assets:cash   $-12.00
+	expenses:food  $14.00
+	assets:cash   $-14.00

+ only in NEW: new/income/bonus.ledger
- only in OLD: old/income/salary.ledger
2 files compared, 1 with differences
```

Both directories are walked recursively for `.ledger` files;
files are paired by **relative path** (`cash/feb.ledger`,
`income/salary.ledger`, etc.). Files present on only one side
are reported as `+ only in NEW` or `- only in OLD` without a
content diff.

`jan.ledger` is identical on both sides, so it's silently
counted in `2 files compared` but produces no hunk.

### File vs. directory (mixed types — error)

```
$ acc diff old/cash/jan.ledger new
mixed types: old/cash/jan.ledger is a file, new is a directory
```

Exit code `1`. `acc diff` requires both sides to be the same
kind. To compare a single file inside a tree, use `--snapshot`
or give the explicit nested path.

### Non-existent path (error)

```
$ acc diff old/cash/jan.ledger /tmp/diff-ex/nope.ledger
mixed types: old/cash/jan.ledger is a file, /tmp/diff-ex/nope.ledger is missing
```

Same error path as mixed types: a missing file isn't a file or a
directory, so the type-match check fails.

---

## Mode 2: `--snapshot`

### Single file, snapshot finds the match

From `/tmp/diff-ex/new/cash`:

```
$ acc diff --snapshot /tmp/diff-ex/snap feb.ledger
--- /tmp/diff-ex/snap/cash/feb.ledger
+++ /tmp/diff-ex/new/cash/feb.ledger
@@ -1,3 +1,3 @@
 2024-02-10 * Lunch
-	expenses:food  $12.00
-	assets:cash   $-12.00
+	expenses:food  $14.00
+	assets:cash   $-14.00

1 files compared, 1 with differences
```

The working file `feb.ledger` resolves to
`/tmp/diff-ex/new/cash/feb.ledger`. acc walks the components from
the right, looking for the longest suffix that exists under
`/tmp/diff-ex/snap` — finds `cash/feb.ledger` and pairs them.
You don't have to type the nested snapshot path.

### Whole working tree against the snapshot

From `/tmp/diff-ex/new`:

```
$ acc diff --snapshot /tmp/diff-ex/snap .
--- /tmp/diff-ex/snap/cash/feb.ledger
+++ /tmp/diff-ex/new/cash/feb.ledger
@@ -1,3 +1,3 @@
 2024-02-10 * Lunch
-	expenses:food  $12.00
-	assets:cash   $-12.00
+	expenses:food  $14.00
+	assets:cash   $-14.00

+ only in NEW: /tmp/diff-ex/new/income/bonus.ledger
- only in OLD: /tmp/diff-ex/snap/income/salary.ledger
2 files compared, 1 with differences
```

`.` resolves to the cwd. Suffix-match falls all the way through to
the empty suffix — `snap/` itself becomes the paired root — and
both directories are walked recursively, same logic as the
explicit dir-vs-dir mode.

### Multiple paths

```
$ acc diff --snapshot /tmp/diff-ex/snap cash income
--- /tmp/diff-ex/snap/cash/feb.ledger
+++ /tmp/diff-ex/new/cash/feb.ledger
@@ -1,3 +1,3 @@
 2024-02-10 * Lunch
-	expenses:food  $12.00
-	assets:cash   $-12.00
+	expenses:food  $14.00
+	assets:cash   $-14.00

+ only in NEW: /tmp/diff-ex/new/income/bonus.ledger
- only in OLD: /tmp/diff-ex/snap/income/salary.ledger
2 files compared, 1 with differences
```

Each positional path is resolved and matched against the snapshot
independently; the resulting file pairs are concatenated. Order
of the paths affects only which pair gets emitted first.

### Snapshot is a file (error)

```
$ acc diff --snapshot old/cash/jan.ledger new/cash/jan.ledger
snapshot root old/cash/jan.ledger is not a directory
```

`--snapshot` always expects a **directory**. The flag is
specifically for backup-tree layouts where the same nested
structure exists under a different root.

### Snapshot path doesn't exist on the working side

```
$ acc diff --snapshot /tmp/diff-ex/snap /tmp/elsewhere/foo.ledger
resolve /tmp/elsewhere/foo.ledger: No such file or directory (os error 2)
```

acc resolves every working-side path to an absolute filesystem
path before suffix-matching. Missing paths fail at canonicalisation.

---

## Argument-count errors (clap-style)

### No arguments — short help

```
$ acc diff
Compare two ledger files or directory trees. Whitespace differences (indent, column alignment) are ignored — only actual character differences are shown, like `diff -w`. Output follows `git diff` conventions (`--- / +++ / @@`)

Usage: acc diff [OPTIONS] [PATHS]...

Arguments:
  [PATHS]...  Paths. Without `--snapshot`: exactly two paths (OLD NEW). With `--snapshot`: one or more working paths; defaults to the current directory if none given

Options:
      --snapshot <DIR>  Snapshot root directory. When set, acc locates the matching path inside this tree by longest-suffix match against each positional PATH. The positional args become working-side paths only; the snapshot-side paths are derived
  -h, --help            Print help (see more with '--help')
```

clap's auto-generated short help (configured via
`#[command(arg_required_else_help = true)]`).

### Wrong number of paths without `--snapshot`

```
$ acc diff foo.ledger
error: expected 2 paths (OLD NEW) without --snapshot, got 1

Usage: diff [OPTIONS] [PATHS]...

For more information, try '--help'.
```

```
$ acc diff a.ledger b.ledger c.ledger
error: expected 2 paths (OLD NEW) without --snapshot, got 3

Usage: diff [OPTIONS] [PATHS]...

For more information, try '--help'.
```

Exit code `2`. The path-count rule is conditional on whether
`--snapshot` is set, which clap-derive cannot express directly —
the check runs post-parse but goes through clap's
`Command::error()` machinery so the formatting matches every
other invalid-invocation error.
