//! `navigate` command — interactive TUI for browsing the account tree.
//!
//! ratatui-based: alternate screen, raw-mode key input, live search,
//! expandable/collapsible branches, cursor + scroll management.
//! Key bindings:
//!
//! - `↑` / `↓`: move cursor
//! - `Enter` / `Space`: toggle expand
//! - `→`: expand only, `←`: collapse only
//! - `PgUp` / `PgDn`: whole-page jump; `Ctrl+U` / `Ctrl+D`: half-page
//! - `Home` / `End`: first / last row
//! - Typing letters: live-filter by pattern (same DSL as `filter::`)
//! - `Backspace`: drop last search char, else collapse
//! - `Esc` / `Ctrl+C`: quit

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::stdout;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::commands::account::Account;
use crate::commands::util::{format_amount, shows_nonzero};
use crate::decimal::Decimal;
use crate::filter::PatternMatcher;
use crate::loader::{Journal, LabelView};

const SEL_BG: Color = Color::Rgb(40, 40, 55);

/// Minimum gap between the account column and the amount, matching `reg`.
const GAP: usize = 2;

pub fn run(journal: &Journal, show_empty: bool) -> Result<(), String> {
    let root = Account::from_transactions(&journal.transactions);

    enable_raw_mode().map_err(|e| e.to_string())?;
    stdout()
        .execute(EnterAlternateScreen)
        .map_err(|e| e.to_string())?;

    let mut terminal =
        Terminal::new(CrosstermBackend::new(stdout())).map_err(|e| e.to_string())?;
    let mut app = App::new(&root, journal, show_empty);
    let mut frame_height: u16 = 20;

    loop {
        terminal
            .draw(|frame| {
                frame_height = frame.area().height;
                app.adjust_scroll(frame_height.saturating_sub(1) as usize);
                draw(frame, &app);
            })
            .map_err(|e| e.to_string())?;

        if let Ok(Event::Key(key)) = event::read() {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            if key.code == KeyCode::Esc
                || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
            {
                break;
            }
            let half_page = (frame_height / 2).max(1) as usize;
            match key.code {
                KeyCode::Up => app.up(),
                KeyCode::Down => app.down(),
                KeyCode::Home => app.go_top(),
                KeyCode::End => app.go_bottom(),
                KeyCode::PageUp => app.page_up(half_page * 2),
                KeyCode::PageDown => app.page_down(half_page * 2),
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.page_up(half_page);
                }
                KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.page_down(half_page);
                }
                KeyCode::Char(' ') | KeyCode::Enter => app.toggle(),
                KeyCode::Tab => app.toggle_all(),
                KeyCode::Right => {
                    if let Some(row) = app.visible.get(app.cursor)
                        && row.has_children && !row.expanded {
                            app.toggle();
                        }
                }
                KeyCode::Left => {
                    if let Some(row) = app.visible.get(app.cursor)
                        && row.expanded {
                            app.toggle();
                        }
                }
                KeyCode::Backspace => {
                    if !app.search.is_empty() {
                        app.search.pop();
                        app.update_search();
                    } else if let Some(row) = app.visible.get(app.cursor)
                        && row.expanded {
                            app.toggle();
                        }
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.search.push(c);
                    app.update_search();
                }
                _ => {}
            }
        }
    }

    disable_raw_mode().map_err(|e| e.to_string())?;
    stdout()
        .execute(LeaveAlternateScreen)
        .map_err(|e| e.to_string())?;
    Ok(())
}

struct App<'a> {
    root: &'a Account,
    journal: &'a Journal,
    precisions: &'a HashMap<String, usize>,
    expanded: BTreeSet<String>,
    cursor: usize,
    scroll_offset: usize,
    visible: Vec<Row<'a>>,
    show_empty: bool,
    search: String,
    search_matcher: Option<PatternMatcher>,
}

struct Row<'a> {
    account: &'a Account,
    depth: usize,
    has_children: bool,
    expanded: bool,
}

impl<'a> App<'a> {
    fn new(root: &'a Account, journal: &'a Journal, show_empty: bool) -> Self {
        let mut app = App {
            root,
            journal,
            precisions: &journal.precisions,
            expanded: BTreeSet::new(),
            cursor: 0,
            scroll_offset: 0,
            visible: Vec::new(),
            show_empty,
            search: String::new(),
            search_matcher: None,
        };
        app.rebuild_visible();
        app
    }

    fn rebuild_visible(&mut self) {
        self.visible.clear();
        self.collect_visible(self.root, 0);
        if self.cursor >= self.visible.len() && !self.visible.is_empty() {
            self.cursor = self.visible.len() - 1;
        }
    }

    fn collect_visible(&mut self, account: &'a Account, depth: usize) {
        for child in account.children.values() {
            if !self.show_empty && !child.has_balance(self.precisions) {
                continue;
            }
            if let Some(ref matcher) = self.search_matcher {
                if !child.matches(&Some(matcher.clone())) {
                    continue;
                }
                if child.children.is_empty() && !matcher.matches(&child.fullname) {
                    continue;
                }
            }
            let has_visible_children = self.has_visible_children(child);
            let is_expanded = self.expanded.contains(&child.fullname);
            let auto_expand = self.search_matcher.is_some() && has_visible_children;

            self.visible.push(Row {
                account: child,
                depth,
                has_children: has_visible_children,
                expanded: is_expanded || auto_expand,
            });
            if is_expanded || auto_expand {
                self.collect_visible(child, depth + 1);
            }
        }
    }

    fn has_visible_children(&self, account: &Account) -> bool {
        if let Some(ref matcher) = self.search_matcher {
            account
                .children
                .values()
                .any(|c| c.matches(&Some(matcher.clone())))
        } else {
            !account.children.is_empty()
        }
    }

    fn toggle(&mut self) {
        if let Some(row) = self.visible.get(self.cursor)
            && row.has_children {
                let name = row.account.fullname.clone();
                if self.expanded.contains(&name) {
                    self.expanded.remove(&name);
                } else {
                    self.expanded.insert(name);
                }
                self.rebuild_visible();
            }
    }

    /// Fold/unfold the whole tree: collapse everything when anything is
    /// expanded, otherwise expand every node that has children.
    fn toggle_all(&mut self) {
        if self.expanded.is_empty() {
            self.expand_all(self.root);
        } else {
            self.expanded.clear();
        }
        self.rebuild_visible();
    }

    fn expand_all(&mut self, account: &'a Account) {
        for child in account.children.values() {
            if !child.children.is_empty() {
                self.expanded.insert(child.fullname.clone());
                self.expand_all(child);
            }
        }
    }

    fn up(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    fn down(&mut self) {
        if !self.visible.is_empty() {
            self.cursor = (self.cursor + 1).min(self.visible.len() - 1);
        }
    }

    fn page_up(&mut self, n: usize) {
        self.cursor = self.cursor.saturating_sub(n);
    }

    fn page_down(&mut self, n: usize) {
        if !self.visible.is_empty() {
            self.cursor = (self.cursor + n).min(self.visible.len() - 1);
        }
    }

    fn go_top(&mut self) {
        self.cursor = 0;
    }

    fn go_bottom(&mut self) {
        if !self.visible.is_empty() {
            self.cursor = self.visible.len() - 1;
        }
    }

    fn update_search(&mut self) {
        self.search_matcher = if self.search.is_empty() {
            None
        } else {
            Some(PatternMatcher::new(&self.search))
        };
        self.cursor = 0;
        self.scroll_offset = 0;
        self.rebuild_visible();
    }

    fn adjust_scroll(&mut self, visible_height: usize) {
        if visible_height == 0 || self.visible.is_empty() {
            return;
        }
        self.cursor = self.cursor.min(self.visible.len() - 1);

        if self.cursor < self.scroll_offset {
            self.scroll_offset = self.cursor;
        }

        loop {
            let lines: usize = self.visible[self.scroll_offset..=self.cursor]
                .iter()
                .map(|r| row_display_lines(r, self.precisions))
                .sum();
            if lines <= visible_height || self.scroll_offset >= self.cursor {
                break;
            }
            self.scroll_offset += 1;
        }
    }
}

fn row_display_lines(row: &Row, precisions: &HashMap<String, usize>) -> usize {
    row.account
        .total()
        .iter()
        .filter(|(c, v)| shows_nonzero(c, v, precisions))
        .count()
        .max(1)
}

fn format_balance(
    balance: &BTreeMap<String, Decimal>,
    precisions: &HashMap<String, usize>,
    max_width: usize,
) -> String {
    let parts: Vec<_> = balance
        .iter()
        .filter(|(c, v)| shows_nonzero(c, v, precisions))
        .map(|(c, v)| format_amount(c, v, precisions))
        .collect();
    if parts.is_empty() {
        return "0".to_string();
    }
    let mut result = String::new();
    for (i, part) in parts.iter().enumerate() {
        let sep = if i > 0 { ", " } else { "" };
        let next = format!("{}{}", sep, part);
        if result.len() + next.len() > max_width && !result.is_empty() {
            result.push_str(", ...");
            break;
        }
        result.push_str(&next);
    }
    result
}

fn max_amount_width(account: &Account, precisions: &HashMap<String, usize>) -> usize {
    let mut max = 0;
    for child in account.children.values() {
        let parts: Vec<_> = child
            .total()
            .iter()
            .filter(|(_, v)| !v.is_zero())
            .map(|(c, v)| format_amount(c, v, precisions))
            .collect();
        let w = if parts.is_empty() {
            1
        } else {
            parts.join(", ").chars().count()
        };
        max = max.max(w).max(max_amount_width(child, precisions));
    }
    max
}

/// The register label for `account`, formatted the way `reg` shows it —
/// ` (label)` — or `None` when the account carries no label.
fn label_text(journal: &Journal, account: &str) -> Option<String> {
    journal
        .label_for(account, LabelView::Register)
        .map(|label| format!(" ({})", label))
}

fn draw(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(frame.area());

    let width = chunks[0].width as usize;
    let amount_w = max_amount_width(app.root, app.precisions).min(width / 2);
    // Longest account name across the visible rows (" " + indent + icon +
    // leaf + label); every amount starts GAP spaces after this column.
    let name_col = app
        .visible
        .iter()
        .map(|row| {
            let label_w = label_text(app.journal, &row.account.fullname)
                .map(|s| s.chars().count())
                .unwrap_or(0);
            1 + row.depth * 2 + 2 + row.account.name.chars().count() + label_w
        })
        .max()
        .unwrap_or(0);
    // Widest displayed amount across the visible rows; amounts are right-
    // aligned within a field this wide, so the digits line up on the right.
    let amount_col_w = app
        .visible
        .iter()
        .map(|row| {
            row.account
                .total()
                .into_iter()
                .filter(|(c, v)| shows_nonzero(c, v, app.precisions))
                .map(|(c, v)| format_amount(&c, &v, app.precisions).chars().count())
                .max()
                .unwrap_or(1)
        })
        .max()
        .unwrap_or(0);
    let list_height = chunks[0].height as usize;
    let end = (app.scroll_offset + list_height).min(app.visible.len());

    let mut items: Vec<ListItem> = Vec::new();
    for (i, row) in app.visible[app.scroll_offset..end].iter().enumerate() {
        let selected = (i + app.scroll_offset) == app.cursor;
        let indent = "  ".repeat(row.depth);
        let icon = match (row.has_children, row.expanded) {
            (true, true) => "▼ ",
            (true, false) => "▶ ",
            _ => "  ",
        };

        let label = label_text(app.journal, &row.account.fullname);

        let commodities: Vec<_> = row
            .account
            .total()
            .into_iter()
            .filter(|(c, v)| shows_nonzero(c, v, app.precisions))
            .map(|(c, v)| (format_amount(&c, &v, app.precisions), v.is_negative()))
            .collect();

        if commodities.is_empty() {
            items.push(make_row(
                &format!(" {}{}{}", indent, icon, row.account.name),
                label.as_deref(),
                "0",
                Color::DarkGray,
                selected,
                name_col,
                amount_col_w,
                width,
            ));
        } else {
            for (j, (amount, neg)) in commodities.iter().enumerate() {
                // The label sits on the first commodity line only; wrapped
                // amount lines carry no name and no label.
                let (name, label) = if j == 0 {
                    (
                        format!(" {}{}{}", indent, icon, row.account.name),
                        label.as_deref(),
                    )
                } else {
                    (String::new(), None)
                };
                let color = if *neg { Color::Red } else { Color::Green };
                items.push(make_row(
                    &name,
                    label,
                    amount,
                    color,
                    selected,
                    name_col,
                    amount_col_w,
                    width,
                ));
            }
        }
    }

    frame.render_widget(List::new(items), chunks[0]);

    let footer_text = if app.search.is_empty() {
        let total = format_balance(&app.root.total(), app.precisions, amount_w);
        format!(
            " Total: {}  |  Esc:quit  ↑↓:nav  Enter:toggle  Tab:fold/unfold all  type to search",
            total.trim()
        )
    } else {
        format!(" /{}", app.search)
    };
    frame.render_widget(
        Paragraph::new(footer_text).style(Style::default().fg(Color::DarkGray)),
        chunks[1],
    );
}

fn make_row<'a>(
    name: &str,
    label: Option<&str>,
    amount: &str,
    amount_color: Color,
    selected: bool,
    name_col: usize,
    amount_w: usize,
    width: usize,
) -> ListItem<'a> {
    ListItem::new(row_line(
        name,
        label,
        amount,
        amount_color,
        selected,
        name_col,
        amount_w,
        width,
    ))
}

/// Lay out one row: the account name (plus its optional `reg`-style label),
/// then padding to the amount column (`name_col` + `GAP`), then the amount
/// right-aligned within a field `amount_w` wide, then trailing fill. The
/// longest visible name keeps exactly `GAP` before the amount field; all
/// amounts end in a single right-aligned column so their digits line up.
fn row_line<'a>(
    name: &str,
    label: Option<&str>,
    amount: &str,
    amount_color: Color,
    selected: bool,
    name_col: usize,
    amount_w: usize,
    width: usize,
) -> Line<'a> {
    let bg = if selected { SEL_BG } else { Color::Reset };
    let name_style = if selected {
        Style::default()
            .fg(Color::White)
            .bg(bg)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().bg(bg)
    };
    let label_len = label.map(|l| l.chars().count()).unwrap_or(0);
    let name_len = name.chars().count() + label_len;
    let amount_len = amount.chars().count();
    let lead =
        name_col.saturating_sub(name_len) + GAP + amount_w.saturating_sub(amount_len);
    let trail = width.saturating_sub(name_len + lead + amount_len);
    let mut spans = vec![Span::styled(name.to_string(), name_style)];
    if let Some(label) = label {
        spans.push(Span::styled(
            label.to_string(),
            Style::default().fg(Color::LightBlue).bg(bg),
        ));
    }
    spans.push(Span::styled(" ".repeat(lead), Style::default().bg(bg)));
    spans.push(Span::styled(
        amount.to_string(),
        Style::default().fg(amount_color).bg(bg),
    ));
    spans.push(Span::styled(" ".repeat(trail), Style::default().bg(bg)));
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    /// Column just past the amount in a laid-out row: sum of the name, lead
    /// and amount span widths (spans are `[name, lead, amount, trail]`).
    fn amount_end_col(line: &Line) -> usize {
        line.spans[0].content.chars().count()
            + line.spans[1].content.chars().count()
            + line.spans[2].content.chars().count()
    }

    #[test]
    fn amounts_share_one_right_aligned_column() {
        let name_col = 20;
        let amount_w = 10;
        let width = 60;
        // Different name lengths AND amount widths; all must end in one column.
        let cases = [("a", "€1.00"), ("equity", "€-45988.48"), ("ex", "€21.21")];
        for (name, amount) in cases {
            let line = row_line(name, None, amount, Color::Green, false, name_col, amount_w, width);
            assert_eq!(
                amount_end_col(&line),
                name_col + GAP + amount_w,
                "name={name} amount={amount}"
            );
        }
        // The longest name (length == name_col) with the widest amount keeps
        // exactly GAP of lead; a narrower amount gets more (right-aligned).
        let longest = "x".repeat(name_col);
        let widest = row_line(&longest, None, "€-45988.48", Color::Green, false, name_col, amount_w, width);
        assert_eq!(widest.spans[1].content.chars().count(), GAP);
        let narrow = row_line(&longest, None, "€1.00", Color::Green, false, name_col, amount_w, width);
        assert!(narrow.spans[1].content.chars().count() > GAP);
    }

    #[test]
    fn draw_renders_amounts_right_aligned() {
        // Tree with different-length names and amount widths.
        let mut root = Account::root();
        root.find_or_create("equity").add_amount("€", Decimal::new(-4598848, 100));
        root.find_or_create("ex").add_amount("€", Decimal::new(2121, 100));
        root.find_or_create("aim").add_amount("€", Decimal::new(-47408, 100));
        let journal = Journal::default();

        let app = App::new(&root, &journal, false);
        let mut terminal = Terminal::new(TestBackend::new(60, 10)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let buf = terminal.backend().buffer();

        // The list occupies all but the last (footer) row. Right-aligned amounts
        // all END at the same column: the rightmost non-space cell per row.
        let mut ends = Vec::new();
        for y in 0..buf.area().height - 1 {
            let mut last = None;
            for x in 0..buf.area().width {
                if buf[(x, y)].symbol() != " " {
                    last = Some(x);
                }
            }
            if let Some(x) = last {
                ends.push(x);
            }
        }
        assert_eq!(ends.len(), 3, "expected 3 amount rows, got {ends:?}");
        assert!(
            ends.iter().all(|c| *c == ends[0]),
            "amounts not right-aligned in one column: {ends:?}"
        );
    }

    #[test]
    fn draw_shows_register_label_inline() {
        let mut root = Account::root();
        root.find_or_create("cc:12").add_amount("€", Decimal::new(4242, 100));
        let mut journal = Journal::default();
        journal
            .labels_register
            .exact
            .insert("cc:12".to_string(), "brokerage".to_string());

        // Expand `cc` so `cc:12` (which carries the label) is visible.
        let mut app = App::new(&root, &journal, false);
        app.expanded.insert("cc".to_string());
        app.rebuild_visible();

        let mut terminal = Terminal::new(TestBackend::new(60, 10)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let buf = terminal.backend().buffer();

        // The label appears inline as `(brokerage)` somewhere in the list.
        let mut screen = String::new();
        for y in 0..buf.area().height {
            for x in 0..buf.area().width {
                screen.push_str(buf[(x, y)].symbol());
            }
            screen.push('\n');
        }
        assert!(
            screen.contains("(brokerage)"),
            "register label not rendered inline:\n{screen}"
        );
    }
}
