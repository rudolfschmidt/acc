//! Claude-Code-style diff preview for the import dry-run: a few existing
//! lines as context, then the new lines on a green band with `+`.
//!
//! Colour is emitted only when stdout is a terminal (the `colored` crate
//! auto-detects), so piping the dry-run gives clean plain text.

use std::path::Path;

use colored::Colorize;

/// Existing lines of context shown above the additions.
const CONTEXT: usize = 12;

/// Spaces a tab is shown as in the diff (tabs can't carry the band).
const TAB_WIDTH: usize = 4;

/// Width of the line-number gutter.
const GUTTER: usize = 9;

/// The dark-green band behind every addition (#022800).
const BAND: (u8, u8, u8) = (2, 40, 0);

// A cohesive pastel palette for the additions, on the #022800 band —
// anchored on the lavender-blue accounts and sage comment: warm peach
// date, soft-light title, and clear green / red amounts.
const C_DATE: (u8, u8, u8) = (250, 179, 135); // #fab387 peach
const C_TITLE: (u8, u8, u8) = (205, 214, 244); // #cdd6f4 soft text
const C_COMMENT: (u8, u8, u8) = (110, 135, 110); // #6e876e sage
const C_ACCOUNT: (u8, u8, u8) = (177, 185, 249); // #b1b9f9 lavender-blue
const C_POS: (u8, u8, u8) = (166, 227, 161); // #a6e3a1 green (positive)
const C_NEG: (u8, u8, u8) = (255, 107, 107); // #ff6b6b red (negative)

pub fn diff_preview(existing: &str, added: &str, count: usize, file: &Path, skipped: usize, written: bool) {
    let existing_lines: Vec<&str> = existing.lines().collect();
    let total = existing_lines.len();

    // The additions mirror exactly what `--execute` appended: a leading
    // blank separator line, then the already-aligned transaction blocks.
    let new_text = format!("\n{}", added.trim_end());
    let new_lines: Vec<&str> = new_text.lines().collect();

    // Header tells the mode apart: a green ✓ "Update" when it was written,
    // a yellow ! "Preview … dry-run" when nothing has been touched.
    let n = count.to_string().bold();
    if written {
        println!("{} {}", "✓".green(), format!("Update({})", display_path(file)).bold());
        println!("  {} added {} transactions · {} already present", "⎿".dimmed(), n, skipped);
    } else {
        println!("{} {}", "!".yellow(), format!("Preview({})", display_path(file)).bold());
        println!(
            "  {} would add {} transactions · {} already present · add {} to append",
            "⎿".dimmed(),
            n,
            skipped,
            "--execute".bold(),
        );
    }

    // Full-width green band: pad every addition out to the terminal width
    // so the highlight spans the whole line like the editor diff. A long
    // `; csv:` comment keeps its full (green) content and simply wraps.
    // Body starts after the gutter + 1 space.
    let cols = crossterm::terminal::size().map(|(c, _)| c as usize).unwrap_or(100);
    let band = cols.saturating_sub(GUTTER + 1).max(1);

    // Context: trailing existing lines, dim line numbers, grey content
    // (#a3a39f) — the unchanged surroundings. Tabs are expanded so the
    // indentation matches the additions below (which can't keep tabs — a
    // terminal won't paint the band across a tab cell).
    let ctx_start = total.saturating_sub(CONTEXT);
    for (i, line) in existing_lines[ctx_start..].iter().enumerate() {
        let n = ctx_start + i + 1;
        let line = untab(line);
        println!("{}  {}", gutter(n).dimmed(), line.dimmed());
    }

    // Additions: the line number and the `+` share the accent green
    // (#50c850); the content is off-white (#f8f8f2); everything sits on a
    // dark-green band (#022800) padded out to the full width.
    for (i, line) in new_lines.iter().enumerate() {
        let line = untab(line);
        let n = total + i + 1;
        let pad = band.saturating_sub(1 + line.chars().count());
        println!(
            "{}{}{}{}{}",
            gutter(n).truecolor(80, 200, 80).on_truecolor(BAND.0, BAND.1, BAND.2),
            " ".on_truecolor(BAND.0, BAND.1, BAND.2),
            "+".truecolor(80, 200, 80).on_truecolor(BAND.0, BAND.1, BAND.2),
            colorize_added(&line),
            " ".repeat(pad).on_truecolor(BAND.0, BAND.1, BAND.2),
        );
    }
}

fn gutter(n: usize) -> String {
    format!("{:>width$}", n, width = GUTTER)
}

/// The target path for the header, with the home directory collapsed back
/// to `~` (the form the profile is written in).
fn display_path(file: &Path) -> String {
    let s = file.to_string_lossy();
    if let Ok(home) = std::env::var("HOME")
        && let Some(rest) = s.strip_prefix(&home)
    {
        return format!("~{}", rest);
    }
    s.into_owned()
}

/// Expand tabs to spaces. A terminal won't paint the active background
/// colour across a tab cell, so the indenting tab in a posting line would
/// leave an un-highlighted gap in the band; spaces fill it.
fn untab(line: &str) -> String {
    line.replace('\t', &" ".repeat(TAB_WIDTH))
}

/// Colour one addition line by its ledger role, using `colored`'s native
/// styles: `<date> * <title>` header, `; csv: …` comment, or a
/// `<account>  <amount>` posting. Amounts are green when positive, red
/// when negative.
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

    // Posting: indent, account, then (optionally) the gap + amount.
    let indent = &line[..line.len() - trimmed.len()];
    match trimmed.split_once(' ') {
        Some((account, rest)) => {
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
