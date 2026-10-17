//! Everything that draws to the screen.
//!
//! The layout mirrors ncdu: a title bar at the top, the current path,
//! the file list in the middle and a status bar at the bottom.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{List, ListItem, Paragraph};

use crate::app::App;
use crate::scan::{Entry, Kind};
use crate::units::human_size;

/// Width of the usage bar between the brackets.
const BAR_WIDTH: usize = 10;

/// Reverse video: the classic ncdu bar look, follows the terminal's colors.
fn bar_style() -> Style {
    Style::new().add_modifier(Modifier::REVERSED)
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let [title, path, body, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    draw_title(frame, title);
    draw_path(frame, path, app);
    draw_body(frame, body, app);
    draw_status(frame, status, app);
}

fn draw_title(frame: &mut Frame, area: Rect) {
    let text = format!(
        "nudu {} ~ Use the arrow keys to navigate, press ? for help",
        env!("CARGO_PKG_VERSION")
    );
    frame.render_widget(Paragraph::new(text).style(bar_style()), area);
}

fn draw_path(frame: &mut Frame, area: Rect, app: &App) {
    let label = format!("--- {} ", app.root_path.display());
    let fill = "-".repeat((area.width as usize).saturating_sub(label.chars().count()));
    frame.render_widget(Paragraph::new(format!("{label}{fill}")), area);
}

fn draw_body(frame: &mut Frame, area: Rect, app: &mut App) {
    app.page_size = area.height.max(1) as usize;

    let entries = app.current();
    // Bars are relative to the biggest entry in this directory, like ncdu.
    let largest = entries.iter().map(|e| e.disk).max().unwrap_or(0);
    let items: Vec<ListItem> = entries.iter().map(|e| row(e, largest)).collect();

    let list = List::new(items).highlight_style(bar_style());
    frame.render_stateful_widget(list, area, &mut app.list);
}

/// One line of the browser, e.g. ` 91.4 GiB [##########] /.local`
fn row(entry: &Entry, largest: u64) -> ListItem<'static> {
    let flag = flag(entry);
    let size = human_size(entry.disk);
    let bar = usage_bar(entry.disk, largest);
    let prefix = if entry.kind == Kind::Dir { "/" } else { " " };
    let name = entry.name.to_string_lossy();
    ListItem::new(Line::from(format!("{flag}{size:>10} [{bar}] {prefix}{name}")))
}

/// The single-character column ncdu shows before the size.
fn flag(entry: &Entry) -> char {
    match entry.kind {
        _ if entry.error => '!',
        Kind::Symlink => '@',
        Kind::Other => '>',
        Kind::Dir if entry.children.is_empty() => 'e',
        _ => ' ',
    }
}

/// `##########`, `####      `, ... scaled to the largest sibling.
fn usage_bar(size: u64, largest: u64) -> String {
    let filled = if largest == 0 {
        0
    } else {
        ((size as f64 / largest as f64) * BAR_WIDTH as f64).round() as usize
    };
    format!("{}{}", "#".repeat(filled), " ".repeat(BAR_WIDTH - filled))
}

fn draw_status(frame: &mut Frame, area: Rect, app: &App) {
    let text = format!(
        " Total disk usage: {}  Apparent size: {}  Items: {}",
        human_size(app.root.disk),
        human_size(app.root.apparent),
        app.root.items
    );
    frame.render_widget(Paragraph::new(text).style(bar_style()), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bar_scales_to_largest() {
        assert_eq!(usage_bar(100, 100), "##########");
        assert_eq!(usage_bar(50, 100), "#####     ");
        assert_eq!(usage_bar(0, 100), "          ");
        assert_eq!(usage_bar(0, 0), "          ");
    }
}
