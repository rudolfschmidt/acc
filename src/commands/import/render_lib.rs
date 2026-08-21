//! Claude-Code-style diff preview for the import dry-run: a few existing lines
//! as context, then the new lines on a green band with `+`, role-coloured by
//! ledger element (date, title, comment, account, amount). The band geometry,
//! gutter, context row, and path helper all come from [`crate::diff`]; this
//! module owns only import's addition palette and the preview's overall shape.
//!
//! Colour is emitted only when stdout is a terminal (the `colored` crate
//! auto-detects), so piping the dry-run gives clean plain text.

use std::path::Path;

use colored::Colorize;

use crate::diff::{self, BAND, GUTTER};

// Import's addition palette — role colours on the green band. Import-only:
// reconcile paints its diff plain white, so these never touch it.
const C_DATE: (u8, u8, u8) = (250, 179, 135); // #fab387 peach
const C_TITLE: (u8, u8, u8) = (205, 214, 244); // #cdd6f4 soft text
const C_COMMENT: (u8, u8, u8) = (110, 135, 110); // #6e876e sage
const C_ACCOUNT: (u8, u8, u8) = (177, 185, 249); // #b1b9f9 lavender-blue
const C_POS: (u8, u8, u8) = (166, 227, 161); // #a6e3a1 green (positive)
const C_NEG: (u8, u8, u8) = (255, 107, 107); // #ff6b6b red (negative)

pub fn diff_preview(existing: &str, added: &str, count: usize, file: &Path, skipped: usize, written: bool) {
    let existing_lines: Vec<&str> = existing.lines().collect();
    let total = existing_lines.len();

    // The additions mirror exactly what `--execute` appended: a blank
    // separator line before the already-aligned blocks — but only when there
    // is existing content to separate from. A fresh file starts straight at
    // the first transaction, no leading blank.
    let new_text = if existing.trim().is_empty() {
        added.trim_end().to_string()
    } else {
        format!("\n{}", added.trim_end())
    };
    let new_lines: Vec<&str> = new_text.lines().collect();

    // Context: trailing existing lines, dimmed. `diff::context_line` is the
    // shared renderer, so reconcile shows identical context above its removals.
    let ctx_start = total.saturating_sub(diff::CONTEXT);
    for (i, line) in existing_lines[ctx_start..].iter().enumerate() {
        println!("{}", diff::context_line(ctx_start + i + 1, line));
    }

    // Additions: the gutter and `+` are accent green (#50c850); the content is
    // role-coloured by `colorize_added`. The band geometry (padding to the row
    // boundary) is `diff::band_line`, shared with reconcile — only the palette
    // differs, so a colour change here can never move reconcile's columns.
    let cols = diff::term_cols();
    for (i, line) in new_lines.iter().enumerate() {
        let n = total + i + 1;
        let line = diff::untab(line);
        let mark = format!(
            "{}{}{}",
            diff::gutter(n).truecolor(80, 200, 80).on_truecolor(BAND.0, BAND.1, BAND.2),
            " ".on_truecolor(BAND.0, BAND.1, BAND.2),
            "+".truecolor(80, 200, 80).on_truecolor(BAND.0, BAND.1, BAND.2),
        );
        println!(
            "{}",
            diff::band_line(&mark, GUTTER + 2, &colorize_added(&line), line.chars().count(), cols, BAND),
        );
    }

    // Standardised result line at the end, matching the other commands'
    // ✓ / ! summaries.
    println!();
    println!("{}", summary(count, file, skipped, written));
}

/// The standardised ✓/! result line the diff preview ends on. Returned as a
/// string so the piped path can send it to stderr (keeping stdout pure ledger)
/// while the terminal path prints it to stdout.
pub fn summary(count: usize, file: &Path, skipped: usize, written: bool) -> String {
    let path = diff::display_path(file);
    let t = if count == 1 { "transaction" } else { "transactions" };
    if written {
        format!(
            "{} Added {} {} to {} ({} already present).",
            "✓".green(),
            count,
            t,
            path,
            skipped,
        )
    } else {
        format!(
            "{} {} {} would be added to {} ({} already present). Re-run with {} to apply.",
            "!".yellow(),
            count,
            t,
            path,
            skipped,
            "-e".bold(),
        )
    }
}

/// Colour one import addition line by its ledger role, on the green band: a
/// `<date> * <title>` header (peach date + soft bold title), a `; …` comment
/// (sage), or a `<account>  <amount>` posting (lavender account, green/red
/// amount). Import-only — reconcile renders its lines plain white.
fn colorize_added(line: &str) -> String {
    if line.is_empty() {
        return String::new(); // pad fills the band
    }
    let trimmed = line.trim_start();
    // Paint a token in `fg` on the band.
    let tc = |s: &str, fg: (u8, u8, u8)| {
        s.truecolor(fg.0, fg.1, fg.2)
            .on_truecolor(BAND.0, BAND.1, BAND.2)
            .to_string()
    };

    // Transaction header — the only un-indented line: date + bold title.
    if !line.starts_with(' ') {
        let (date, rest) = line.split_once(' ').unwrap_or((line, ""));
        let title = format!(" {}", rest)
            .truecolor(C_TITLE.0, C_TITLE.1, C_TITLE.2)
            .bold()
            .on_truecolor(BAND.0, BAND.1, BAND.2)
            .to_string();
        return format!("{}{}", tc(date, C_DATE), title);
    }

    // Embedded source comment.
    if trimmed.starts_with(';') {
        return tc(line, C_COMMENT);
    }

    // Posting: indent, account, then (optionally) the gap + amount. Split at the
    // first space with `find`, NOT `split_once` — `split_once` consumes that
    // space, dropping one column of the alignment gap and shifting the amount a
    // column left of where `format` put it. `find` keeps the space in `rest`.
    let indent = &line[..line.len() - trimmed.len()];
    match trimmed.find(' ') {
        Some(idx) => {
            let account = &trimmed[..idx];
            let rest = &trimmed[idx..];
            let amount = rest.trim_start();
            let gap = &rest[..rest.len() - amount.len()];
            let amount_c = if amount.contains('-') { C_NEG } else { C_POS };
            format!(
                "{}{}{}{}",
                tc(indent, C_ACCOUNT),
                tc(account, C_ACCOUNT),
                tc(gap, C_ACCOUNT),
                tc(amount, amount_c),
            )
        }
        None => format!("{}{}", tc(indent, C_ACCOUNT), tc(trimmed, C_ACCOUNT)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ANSI-strip helper for the layout test below.
    fn strip_ansi(s: &str) -> String {
        let mut out = String::new();
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                for c2 in chars.by_ref() {
                    if c2 == 'm' {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    #[test]
    fn colorize_preserves_every_column() {
        colored::control::set_override(true);
        // A posting exactly as `format_source` emits it (untabbed): account,
        // an aligned run of spaces, then the amount. Re-colouring must not move
        // the amount by a single column.
        let line = "    assets:12:bitcoin-uncategorized        BTC0.00018958";
        assert_eq!(strip_ansi(&colorize_added(line)), line);
    }
}
