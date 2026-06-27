use super::*;

#[test]
fn fmt_amount_pads_to_precision() {
    assert_eq!(fmt_amount("-3", 2), "-3.00");
    assert_eq!(fmt_amount("-64.60", 2), "-64.60");
    assert_eq!(fmt_amount("0.25", 2), "0.25");
    assert_eq!(fmt_amount("2407.5", 2), "2407.50");
    assert_eq!(fmt_amount("100", 0), "100");
}

#[test]
fn slug_lowercases_and_dashes() {
    assert_eq!(slug("Foo Bar & Baz"), "foo-bar-&-baz");
}

#[test]
fn parse_csv_handles_quoted_commas() {
    let rows = parse_csv("a,\"x, y\",c\n1,2,3\n");
    assert_eq!(rows[0], vec!["a", "x, y", "c"]);
    assert_eq!(rows[1], vec!["1", "2", "3"]);
}

#[test]
fn negate_flips_sign() {
    assert_eq!(negate("-101599.0"), "101599.0");
    assert_eq!(negate("126.28"), "-126.28");
}

#[test]
fn pad_amount_right_aligns_at_align() {
    let line = pad_amount("assets:bank", "€-47.00");
    assert_eq!(line.chars().count(), ALIGN);
    assert!(line.starts_with("assets:bank"));
    assert!(line.ends_with("€-47.00"));
}

fn write(dir: &std::path::Path, name: &str, content: &str) -> std::path::PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, content).unwrap();
    p
}

/// Column order mirrors a typical bank layout (date,_,payee,_,_,ref,_,amount,_,fxcur,_).
fn row(date: &str, payee: &str, amount: &str, fxcur: &str) -> Vec<String> {
    vec![date, "", payee, "", "", "ref", "", amount, "", fxcur, ""]
        .into_iter()
        .map(String::from)
        .collect()
}

fn test_profile(dir: &std::path::Path) -> Profile {
    let com = write(dir, "com.ledger", "commodity €\n    alias EUR\n    precision 2\n");
    let conf = write(
        dir,
        "bank.conf",
        &format!(
            "field.date 0\nfield.payee 2\nfield.reference 5\nfield.amount 7\nfield.fx-currency 9\n\
             commodities {}\noutput.file /tmp/x.ledger\noutput.title bank | me\n\
             output.account a:bank\noutput.commodity €\n\
             identity date amount payee\n\
             default => exp:{{payee}}\n\
             payee foo => exp:foo\n",
            com.display()
        ),
    );
    Profile::load(conf.to_str().unwrap()).unwrap()
}

#[test]
fn rule_then_slug_default() {
    let dir = std::env::temp_dir().join(format!("acc-import-cat-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = test_profile(&dir);
    assert_eq!(p.precision, 2);
    assert_eq!(p.categorize(&row("2025-11-01", "Foo Shop", "-12.5", "EUR")), "exp:foo");
    assert_eq!(p.categorize(&row("2025-11-01", "Bar Baz", "-1", "")), "exp:bar-baz");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn semicolon_conditions_are_anded() {
    let dir = std::env::temp_dir().join(format!("acc-import-and-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let com = write(&dir, "com.ledger", "commodity €\n    precision 2\n");
    let conf = write(
        &dir,
        "bank.conf",
        &format!(
            "field.date 0\nfield.payee 2\nfield.type 4\nfield.reference 5\nfield.amount 7\nfield.fx-currency 9\n\
             commodities {}\noutput.file /tmp/x.ledger\noutput.title t | t\n\
             output.account a:bank\noutput.commodity €\n\
             identity date amount payee\n\
             default => exp:{{payee}}\n\
             payee foo; type bar => special:foobar\n",
            com.display()
        ),
    );
    let p = Profile::load(conf.to_str().unwrap()).unwrap();
    // columns: 0 date, 2 payee, 4 type, 5 ref, 7 amount, 9 fxcur
    let mk = |payee: &str, ty: &str| -> Vec<String> {
        vec!["2025-01-01", "", payee, "", ty, "ref", "", "-1", "", "", ""]
            .into_iter()
            .map(String::from)
            .collect()
    };
    // both conditions hold (AND) → the special account
    assert_eq!(p.categorize(&mk("Foo Inc", "bar type")), "special:foobar");
    // only one holds → falls through to the slug default
    assert_eq!(p.categorize(&mk("Foo Inc", "other")), "exp:foo-inc");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn render_uses_symbol_precision_and_bare_counter() {
    let dir = std::env::temp_dir().join(format!("acc-import-render-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = test_profile(&dir);
    let block = p.render_transaction(&row("2025-11-01", "Foo Shop", "-12.5", "EUR"));
    assert!(block.contains("2025-11-01 * bank | me"));
    assert!(block.contains("; csv:"));
    assert!(block.contains("€-12.50")); // padded to precision
    assert!(block.contains("a:bank"));
    assert!(block.contains("exp:foo"));
    // domestic row → counter posting is bare (no amount)
    assert!(block.trim_end().ends_with("exp:foo"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn dedup_skips_rows_already_in_ledger() {
    let dir = std::env::temp_dir().join(format!("acc-import-dedup-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = test_profile(&dir);
    let dup = row("2025-10-01", "Foo Shop", "-12.5", "EUR");
    // An existing entry carrying that exact row in its ; csv: comment.
    let existing = format!("2025-10-01 * bank | me\n\t; csv: {}\n\ta:bank\t€-12.50\n\texp:foo\n",
        dup.iter().map(|f| format!("\"{}\"", f)).collect::<Vec<_>>().join(","));
    let seen = existing_identities(&existing, &p, 11);
    assert!(seen.contains_key(&p.identity_key(&dup)));
    // A different row is not present.
    let fresh = row("2025-11-05", "Bar Baz", "-9.99", "");
    assert!(!seen.contains_key(&p.identity_key(&fresh)));
    std::fs::remove_dir_all(&dir).ok();
}
