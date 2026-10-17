//! Everything that draws to the screen.
//!
//! The layout mirrors ncdu: a title bar at the top, the current path,
//! the file list in the middle and a status bar at the bottom.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::scan::Kind;
use crate::units::human_size;

/// Reverse video: the classic ncdu bar look, follows the terminal's colors.
fn bar_style() -> Style {
    Style::new().add_modifier(Modifier::REVERSED)
}

pub fn draw(frame: &mut Frame, app: &App) {
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

fn draw_body(frame: &mut Frame, area: Rect, app: &App) {
    // Plain list for now; bars, selection and scrolling come next.
    let lines: Vec<Line> = app
        .root
        .children
        .iter()
        .take(area.height as usize)
        .map(|child| {
            let slash = if child.kind == Kind::Dir { "/" } else { "" };
            Line::from(format!(
                "{:>10}  {}{slash}",
                human_size(child.disk),
                child.name.to_string_lossy()
            ))
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
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
