//! Neutral diff-rendering primitives shared by more than one command: the
//! green/red banded add/remove rows, the dimmed context row, the line-number
//! gutter, tab expansion, and the `~`-collapsed path. `import`'s dry-run preview
//! (`commands::import::render_lib`) layers its role-colour palette on top of
//! these; the wrapper's `reconcile` diff consumes them directly. Owning the band
//! GEOMETRY here — and only here — is what keeps the two commands' diffs column-
//! aligned while their palettes stay independent: a colour change to one can no
//! longer move the other's columns.
//!
//! Colour is emitted only when stdout is a terminal (the `colored` crate
//! auto-detects), so piping a diff gives clean plain text.

use std::path::Path;

use colored::Colorize;

/// Existing lines of context shown above the additions. Public so a command
/// (import, reconcile) can cap its own context window to the same size.
pub const CONTEXT: usize = 12;

/// Spaces a tab is shown as in a diff (tabs can't carry the band).
const TAB_WIDTH: usize = 4;

/// Width of the line-number gutter.
pub(crate) const GUTTER: usize = 9;

/// The dark-green band behind every addition (#022800).
pub(crate) const BAND: (u8, u8, u8) = (2, 40, 0);

/// The dark-red band behind every removal (#3d0100), mirroring [`BAND`].
const REMOVE_BAND: (u8, u8, u8) = (61, 1, 0);

/// One context line: the dimmed line-number gutter, two spaces, then the
/// (untabbed) source line dimmed. Content starts at column `GUTTER + 2`, aligned
/// with the `+`/`-` of the addition/removal rows.
pub fn context_line(n: usize, line: &str) -> String {
    format!("{}  {}", gutter(n).dimmed(), untab(line).dimmed())
}

/// One addition on the green band, with the line-number gutter: gutter, `+` and
/// content all white (`colored`'s SGR 37) on the dark-green band, padded to the
/// terminal width. The neutral (white) renderer; `import` instead paints its own
/// role-coloured additions via [`band_line`].
pub fn addition_line_n(n: usize, line: &str) -> String {
    banded(n, "+", line, BAND)
}

/// One removal on the red band — the mirror of [`addition_line_n`], `-` in the
/// same column as the `+`, only the band is red.
pub fn removed_line_n(n: usize, line: &str) -> String {
    banded(n, "-", line, REMOVE_BAND)
}

/// The shared body of the white add/remove rows: a `glyph` mark then white
/// content on `band`, all through the one [`band_line`] geometry.
fn banded(n: usize, glyph: &str, line: &str, band: (u8, u8, u8)) -> String {
    let mark = format!(
        "{}{}{}",
        gutter(n).white().on_truecolor(band.0, band.1, band.2),
        " ".on_truecolor(band.0, band.1, band.2),
        glyph.white().on_truecolor(band.0, band.1, band.2),
    );
    let content = untab(line);
    let visible = content.chars().count();
    let painted = content.white().on_truecolor(band.0, band.1, band.2).to_string();
    band_line(&mark, GUTTER + 2, &painted, visible, term_cols(), band)
}

/// Paint one banded row: `mark` (already coloured, `mark_visible` cells wide),
/// then already-coloured `content` (`content_visible` cells wide), padded with
/// band spaces to the next full terminal row so the band never breaks on wrap —
/// a single-row pad would leave a wrapped remainder un-highlighted. The caller
/// owns the palette (it colours `mark` and `content`); this owns only the
/// geometry, so import (role colours) and reconcile (white) share ONE band
/// renderer and cannot drift in column layout.
pub(crate) fn band_line(
    mark: &str,
    mark_visible: usize,
    content: &str,
    content_visible: usize,
    cols: usize,
    band: (u8, u8, u8),
) -> String {
    let visible = mark_visible + content_visible;
    let pad = (cols - visible % cols) % cols;
    format!(
        "{}{}{}",
        mark,
        content,
        " ".repeat(pad).on_truecolor(band.0, band.1, band.2),
    )
}

/// The terminal width in columns, falling back to 100 when it can't be queried
/// (e.g. piped output). Never zero, so the `% cols` padding math stays safe.
pub(crate) fn term_cols() -> usize {
    crossterm::terminal::size().map(|(c, _)| c as usize).unwrap_or(100).max(1)
}

pub(crate) fn gutter(n: usize) -> String {
    format!("{:>width$}", n, width = GUTTER)
}

/// The target path for a header, with the home directory collapsed back to `~`
/// (the form the profile is written in).
pub fn display_path(file: &Path) -> String {
    let s = file.to_string_lossy();
    if let Ok(home) = std::env::var("HOME")
        && let Some(rest) = s.strip_prefix(&home)
    {
        return format!("~{}", rest);
    }
    s.into_owned()
}

/// Expand tabs to spaces. A terminal won't paint the active background colour
/// across a tab cell, so an indenting tab would leave an un-highlighted gap in
/// the band; spaces fill it.
pub(crate) fn untab(line: &str) -> String {
    line.replace('\t', &" ".repeat(TAB_WIDTH))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Banded diff rows paint their content in white (`colored`'s `white`, SGR
    /// 37) on the configured band — additions on the green band, removals on the
    /// red band.
    #[test]
    fn banded_lines_are_white_on_their_band() {
        colored::control::set_override(true);
        let add = addition_line_n(1, "\tfoo:33:eon  $60.00");
        let rem = removed_line_n(1, "\tfoo:12:eon");
        colored::control::unset_override();
        // The band background then white foreground (SGR 37) — `colored` emits
        // background before foreground. Addition on the green band …
        assert!(add.contains(&format!("48;2;{};{};{};37", BAND.0, BAND.1, BAND.2)), "{add:?}");
        // … removal on the red band.
        assert!(
            rem.contains(&format!("48;2;{};{};{};37", REMOVE_BAND.0, REMOVE_BAND.1, REMOVE_BAND.2)),
            "{rem:?}",
        );
    }
}
