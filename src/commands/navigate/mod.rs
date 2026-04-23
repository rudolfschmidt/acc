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
use crate::commands::util::format_amount;
use crate::decimal::Decimal;
use crate::filter::PatternMatcher;
use crate::loader::Journal;

const SEL_BG: Color = Color::Rgb(40, 40, 55);

pub fn run(journal: &Journal, show_empty: bool) -> Result<(), String> {
    let root = Account::from_transactions(&journal.transactions);
    let precisions = &journal.precisions;

    enable_raw_mode().map_err(|e| e.to_string())?;
    stdout()
        .execute(EnterAlternateScreen)
        .map_err(|e| e.to_string())?;

    let mut terminal =
        Terminal::new(CrosstermBackend::new(stdout())).map_err(|e| e.to_string())?;
    let mut app = App::new(&root, precisions, show_empty);
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
                KeyCode::Right => {
                    if let Some(row) = app.visible.get(app.cursor) {
                        if row.has_children && !row.expanded {
                            app.toggle();
                        }
                    }
                }
                KeyCode::Left => {
                    if let Some(row) = app.visible.get(app.cursor) {
                        if row.expanded {
                            app.toggle();
                        }
                    }
                }
                KeyCode::Backspace => {
                    if !app.search.is_empty() {
                        app.search.pop();
                        app.update_search();
                    } else if let Some(row) = app.visible.get(app.cursor) {
                        if row.expanded {
                            app.toggle();
                        }
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
    fn new(
        root: &'a Account,
        precisions: &'a HashMap<String, usize>,
        show_empty: bool,
    ) -> Self {
        let mut app = App {
            root,
            precisions,
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
        if let Some(row) = self.visible.get(self.cursor) {
            if row.has_children {
                let name = row.account.fullname.clone();
                if self.expanded.contains(&name) {
                    self.expanded.remove(&name);
                } else {
                    self.expanded.insert(name);
                }
                self.rebuild_visible();
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
        .filter(|(c, v)| {
            let prec = precisions.get(*c).copied().unwrap_or(2);
            !v.is_display_zero(prec)
        })
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
        .filter(|(c, v)| {
            let prec = precisions.get(*c).copied().unwrap_or(2);
            !v.is_display_zero(prec)
        })
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

fn draw(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(frame.area());

    let width = chunks[0].width as usize;
    let amount_w = max_amount_width(app.root, app.precisions).min(width / 2);
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

        let commodities: Vec<_> = row
            .account
            .total()
            .into_iter()
            .filter(|(c, v)| {
                let prec = app.precisions.get(c).copied().unwrap_or(2);
                !v.is_display_zero(prec)
            })
            .map(|(c, v)| (format_amount(&c, &v, app.precisions), v.is_negative()))
            .collect();

        if commodities.is_empty() {
            items.push(make_row(
                &format!(" {}{}{}", indent, icon, row.account.name),
                "0",
                Color::DarkGray,
                selected,
                width,
            ));
        } else {
            for (j, (amount, neg)) in commodities.iter().enumerate() {
                let name = if j == 0 {
                    format!(" {}{}{}", indent, icon, row.account.name)
                } else {
                    String::new()
                };
                let color = if *neg { Color::Red } else { Color::Green };
                items.push(make_row(&name, amount, color, selected, width));
            }
        }
    }

    frame.render_widget(List::new(items), chunks[0]);

    let footer_text = if app.search.is_empty() {
        let total = format_balance(&app.root.total(), app.precisions, amount_w);
        format!(
            " Total: {}  |  Esc:quit  ↑↓:navigate  Enter:toggle  type to search",
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
    amount: &str,
    amount_color: Color,
    selected: bool,
    width: usize,
) -> ListItem<'a> {
    let bg = if selected { SEL_BG } else { Color::Reset };
    let name_style = if selected {
        Style::default()
            .fg(Color::White)
            .bg(bg)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().bg(bg)
    };
    let pad = width.saturating_sub(name.chars().count() + amount.chars().count());
    ListItem::new(Line::from(vec![
        Span::styled(name.to_string(), name_style),
        Span::styled(" ".repeat(pad), Style::default().bg(bg)),
        Span::styled(
            amount.to_string(),
            Style::default().fg(amount_color).bg(bg),
        ),
    ]))
}
