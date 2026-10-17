//! Everything that draws to the screen.
//!
//! Layout:
//!
//! ```text
//!  nudu  ~ › .local › share                     btrfs · 410 GiB / 476 GiB ▕████▊ ▏
//! ╭─ 91.4 GiB · 412 204 items ─────────────╮╭─ Steam ───────────────╮
//! │ ▸ 󰉋 Steam        62.1 GiB ━━━━━━━━  68% ││ Size       62.1 GiB   │
//! │   󰉋 containers   14.0 GiB ━━╸       15% ││ ...                   │
//! ╰─────────────────────────────────────────╯╰───────────────────────╯
//!  ↑↓ move  ⏎ open  ← back  s n c m sort  ? help  q quit
//! ```
//!
//! Colors are the terminal's own ANSI palette, so nudu follows your theme.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, HighlightSpacing, List, ListItem, Paragraph, Wrap};

use crate::app::{App, Row};
use crate::scanning::Snapshot;
use crate::scan::{Entry, Kind, Skipped};
use crate::units::{group_digits, human_size, relative_time};

/// Main accent color (bars, badge, key hints).
const ACCENT: Color = Color::Blue;
/// Secondary text, borders and empty bar tracks.
const MUTED: Color = Color::DarkGray;
/// Below this width the details pane is hidden.
const DETAILS_MIN_WIDTH: u16 = 90;
const DETAILS_WIDTH: u16 = 34;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let [header, main, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    draw_header(frame, header, app);

    if main.width >= DETAILS_MIN_WIDTH {
        let [list, details] =
            Layout::horizontal([Constraint::Fill(1), Constraint::Length(DETAILS_WIDTH)])
                .areas(main);
        draw_list(frame, list, app);
        draw_details(frame, details, app);
    } else {
        draw_list(frame, main, app);
    }

    draw_footer(frame, footer, app);

    if app.show_help {
        draw_help(frame);
    }
}

// ── Header ──────────────────────────────────────────────────────────────────

fn draw_header(frame: &mut Frame, area: Rect, app: &App) {
    let fs_line = fs_summary(app);
    let fs_width = fs_line.width() as u16;
    let [left, right] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(fs_width)]).areas(area);

    let mut spans = vec![Span::styled(" nudu ", Style::new().fg(Color::Black).bg(ACCENT).bold())];
    if app.is_root {
        // A clear reminder that deletes (and everything else) run as root.
        spans.push(Span::styled(" root ", Style::new().fg(Color::Black).bg(Color::Red).bold()));
    }
    spans.push(Span::raw("  "));
    spans.extend(breadcrumb(&app.display_path()));
    frame.render_widget(Line::from(spans), left);
    frame.render_widget(fs_line, right);
}

/// `~/.local/share` -> `~ › .local › share`, last part in bold.
fn breadcrumb(path: &str) -> Vec<Span<'static>> {
    let mut parts: Vec<String> = path
        .split('/')
        .filter(|p| !p.is_empty())
        .map(String::from)
        .collect();
    if path.starts_with('/') {
        parts.insert(0, "/".into());
    }
    let last = parts.len().saturating_sub(1);
    let mut spans = Vec::new();
    for (i, part) in parts.into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" › ", Style::new().fg(MUTED)));
        }
        spans.push(if i == last {
            Span::styled(part, Style::new().bold())
        } else {
            Span::raw(part)
        });
    }
    spans
}

/// `btrfs · 410.2 GiB / 476.0 GiB ▕████▊   ▏`
fn fs_summary(app: &App) -> Line<'static> {
    let Some(fs) = &app.fs else {
        return Line::default();
    };
    let ratio = fs.used_ratio();
    let color = if ratio >= 0.9 {
        Color::Red
    } else if ratio >= 0.75 {
        Color::Yellow
    } else {
        Color::Green
    };
    Line::from(vec![
        Span::styled(fs.fs_type.clone(), Style::new().bold()),
        Span::styled(" · ", Style::new().fg(MUTED)),
        Span::raw(format!(
            "{} / {} ",
            human_size(fs.used),
            human_size(fs.total)
        )),
        Span::styled("▕", Style::new().fg(MUTED)),
        Span::styled(block_gauge(ratio, 10), Style::new().fg(color)),
        Span::styled("▏ ", Style::new().fg(MUTED)),
    ])
}

/// A gauge in eighths of a cell: `████▊     `.
fn block_gauge(ratio: f64, width: usize) -> String {
    const PARTS: [&str; 8] = ["", "▏", "▎", "▍", "▌", "▋", "▊", "▉"];
    let eighths = (ratio.clamp(0.0, 1.0) * (width * 8) as f64).round() as usize;
    let full = eighths / 8;
    let mut out = "█".repeat(full);
    if full < width {
        out.push_str(PARTS[eighths % 8]);
        let used = full + usize::from(eighths % 8 > 0);
        out.push_str(&" ".repeat(width - used));
    }
    out
}

// ── File list ───────────────────────────────────────────────────────────────

fn draw_list(frame: &mut Frame, area: Rect, app: &mut App) {
    let dir = app.current_dir();
    let title = Line::from(vec![
        Span::raw(" "),
        Span::styled(human_size(dir.disk), Style::new().bold()),
        Span::styled(" · ", Style::new().fg(MUTED)),
        Span::raw(format!("{} items ", group_digits(dir.items))),
    ]);
    let arrow = if app.sort.descending() { "↓" } else { "↑" };
    let sort_label = Line::from(vec![
        Span::styled(" sort ", Style::new().fg(MUTED)),
        Span::styled(format!("{} {arrow} ", app.sort.key.label()), Style::new().fg(ACCENT)),
    ])
    .right_aligned();
    let block = pane(title).title(sort_label);

    // Columns: [▸ ][icon ][name…][ size][ bar][ pct]
    let inner = block.inner(area).width.saturating_sub(2) as usize;
    let bar_width = (inner / 4).clamp(8, 24);
    let fixed = 2 + 1 + 10 + 1 + bar_width + 1 + 4;
    let (bar_width, name_width) = if inner >= fixed + 8 {
        (bar_width, inner - fixed)
    } else {
        // Too narrow for bars: give everything to the name.
        (0, inner.saturating_sub(2 + 1 + 10 + 1 + 4))
    };

    let largest = dir.children.iter().map(|e| e.disk).max().unwrap_or(0);
    let total = dir.disk;
    let items: Vec<ListItem> = app
        .rows()
        .into_iter()
        .map(|row| match row {
            Row::Parent => parent_item(app.icons),
            Row::Entry(e) => entry_item(e, app.icons, name_width, bar_width, largest, total),
        })
        .collect();

    app.page_size = block.inner(area).height.max(1) as usize;

    if items.is_empty() {
        let empty = Paragraph::new(Line::styled("  (empty directory)", Style::new().fg(MUTED)))
            .block(block);
        frame.render_widget(empty, area);
        return;
    }

    let list = List::new(items)
        .block(block)
        .highlight_symbol(Span::styled("▸ ", Style::new().fg(ACCENT).bold()))
        .highlight_spacing(HighlightSpacing::Always)
        .highlight_style(Style::new().bg(MUTED).add_modifier(Modifier::BOLD));
    frame.render_stateful_widget(list, area, &mut app.list);
}

fn parent_item(icons: bool) -> ListItem<'static> {
    let icon = if icons { "󰁝" } else { "^" };
    ListItem::new(Line::from(vec![
        Span::styled(format!("{icon} "), Style::new().fg(MUTED)),
        Span::styled("..", Style::new().fg(MUTED)),
    ]))
}

fn entry_item(
    entry: &Entry,
    icons: bool,
    name_width: usize,
    bar_width: usize,
    largest: u64,
    total: u64,
) -> ListItem<'static> {
    let (icon, icon_color) = icon_for(entry, icons);
    // Skipped folders carry a short tag like `vfat` or `snapshots`.
    let tag = entry.skipped.as_ref().map(skipped_tag).unwrap_or_default();
    let tag_width = if tag.is_empty() { 0 } else { tag.chars().count() + 1 };
    let mut name = fit(
        &entry.name.to_string_lossy(),
        name_width.saturating_sub(tag_width),
    );
    if entry.error {
        // Swap the last padding space for a warning mark.
        name.pop();
        name.push('!');
    }
    let name_style = match (&entry.skipped, entry.kind) {
        (Some(_), _) => Style::new().fg(MUTED),
        (None, Kind::Dir) => Style::new().bold(),
        _ => Style::new(),
    };

    let mut spans = vec![
        Span::styled(format!("{icon} "), Style::new().fg(icon_color)),
        Span::styled(name, name_style),
    ];
    if !tag.is_empty() {
        spans.push(Span::styled(format!(" {tag}"), Style::new().fg(Color::Yellow)));
    }
    spans.push(Span::raw(format!(" {:>10}", human_size(entry.disk))));
    if bar_width > 0 {
        let (fill, track) = line_bar(ratio(entry.disk, largest), bar_width);
        spans.push(Span::raw(" "));
        spans.push(Span::styled(fill, Style::new().fg(ACCENT)));
        spans.push(Span::styled(track, Style::new().fg(MUTED)));
    }
    spans.push(Span::styled(
        format!(" {:>4}", percent(entry.disk, total)),
        Style::new().fg(MUTED),
    ));
    ListItem::new(Line::from(spans))
}

/// Icon and its color. Nerd Font glyphs, or plain characters with --no-icons.
fn icon_for(entry: &Entry, icons: bool) -> (&'static str, Color) {
    if let Some(skipped) = &entry.skipped {
        return match (skipped, icons) {
            (Skipped::Snapshots, true) => ("󰁯", Color::Yellow),
            (_, true) => ("󰋊", Color::Yellow),
            (_, false) => ("#", Color::Yellow),
        };
    }
    match (entry.kind, icons) {
        (Kind::Dir, true) => ("󰉋", ACCENT),
        (Kind::File, true) => ("󰈔", MUTED),
        (Kind::Symlink, true) => ("󰌷", Color::Cyan),
        (Kind::Other, true) => ("󰠱", Color::Magenta),
        (Kind::Dir, false) => ("/", ACCENT),
        (Kind::File, false) => ("·", MUTED),
        (Kind::Symlink, false) => ("@", Color::Cyan),
        (Kind::Other, false) => ("*", Color::Magenta),
    }
}

/// A thin bar in half cells: `━━━━╸` plus a muted `━━━` track.
fn line_bar(ratio: f64, width: usize) -> (String, String) {
    let halves = (ratio.clamp(0.0, 1.0) * (width * 2) as f64).round() as usize;
    let full = halves / 2;
    let mut fill = "━".repeat(full);
    let mut used = full;
    if halves % 2 == 1 {
        fill.push('╸');
        used += 1;
    }
    (fill, "━".repeat(width - used))
}

fn ratio(part: u64, whole: u64) -> f64 {
    if whole == 0 { 0.0 } else { part as f64 / whole as f64 }
}

/// `68%`, `<1%`, or blank for an empty folder.
fn percent(part: u64, whole: u64) -> String {
    if whole == 0 {
        return String::new();
    }
    let pct = ratio(part, whole) * 100.0;
    if part > 0 && pct < 1.0 {
        "<1%".into()
    } else {
        format!("{:.0}%", pct)
    }
}

/// Pads or truncates (with `…`) to exactly `width` characters.
fn fit(text: &str, width: usize) -> String {
    let len = text.chars().count();
    if len <= width {
        format!("{text}{}", " ".repeat(width - len))
    } else if width == 0 {
        String::new()
    } else {
        let mut cut: String = text.chars().take(width - 1).collect();
        cut.push('…');
        cut
    }
}

// ── Details pane ────────────────────────────────────────────────────────────

fn draw_details(frame: &mut Frame, area: Rect, app: &App) {
    let width = area.width.saturating_sub(4) as usize;
    let (title, lines) = match app.selected_row() {
        None => ("empty".to_string(), vec![]),
        Some(Row::Parent) => ("..".to_string(), vec![field("Action", "go up one level".into())]),
        Some(Row::Entry(e)) => (
            e.name.to_string_lossy().into_owned(),
            entry_details(e, app),
        ),
    };
    let title = Line::from(vec![
        Span::raw(" "),
        Span::styled(fit(&title, width).trim_end().to_string(), Style::new().bold()),
        Span::raw(" "),
    ]);
    // Wrap long values instead of cutting them off at the border.
    let details = Paragraph::new(lines)
        .block(pane(title))
        .wrap(Wrap { trim: false });
    frame.render_widget(details, area);
}

/// The short label shown after a skipped folder's name.
fn skipped_tag(skipped: &Skipped) -> String {
    match skipped {
        Skipped::OtherFs(fs) | Skipped::Virtual(fs) => fs.clone(),
        Skipped::Snapshots => "snapshots".into(),
    }
}

fn entry_details(e: &Entry, app: &App) -> Vec<Line<'static>> {
    if let Some(skipped) = &e.skipped {
        return skipped_details(skipped);
    }
    let kind = match e.kind {
        Kind::Dir => "Directory",
        Kind::File => "File",
        Kind::Symlink => "Symlink",
        Kind::Other => "Special file",
    };
    let mut lines = vec![
        field("Type", kind.into()),
        field("Size", human_size(e.apparent)),
        field("On disk", human_size(e.disk)),
        field("Share", format!("{} of folder", percent(e.disk, app.current_dir().disk))),
    ];
    if e.kind == Kind::Dir {
        lines.push(field("Items", group_digits(e.items)));
    }
    lines.push(field("Modified", relative_time(app.now, e.mtime)));
    if e.error {
        // Short lines that fit the pane; each one stands on its own.
        let what = if e.kind == Kind::Dir {
            "Some items are unreadable"
        } else {
            "Could not be read"
        };
        let hints: &[&str] = if app.is_root {
            &["Not readable even as root."]
        } else if e.kind == Kind::Dir {
            &["Sizes may be too low.", "Press S to rescan with sudo."]
        } else {
            &["Press S to rescan with sudo."]
        };
        lines.push(Line::default());
        lines.push(Line::from(vec![
            Span::styled(" ! ", Style::new().fg(Color::Red).bold()),
            Span::styled(what, Style::new().fg(Color::Red)),
        ]));
        for hint in hints {
            lines.push(Line::styled(format!("   {hint}"), Style::new().fg(MUTED)));
        }
    }
    lines
}

/// What the details pane says about a folder that was not scanned.
fn skipped_details(skipped: &Skipped) -> Vec<Line<'static>> {
    let (kind, fs, why): (&str, String, &[&str]) = match skipped {
        Skipped::OtherFs(fs) => (
            "Mount point",
            fs.clone(),
            &["Another filesystem.", "Not scanned: use --cross-fs", "to include it."],
        ),
        Skipped::Virtual(fs) => (
            "Mount point",
            fs.clone(),
            &["Kernel filesystem with no", "real files. Never scanned."],
        ),
        Skipped::Snapshots => (
            "Snapshots",
            "btrfs".into(),
            &["Snapshots share data with", "your files, so counting them", "would double your totals."],
        ),
    };
    let mut lines = vec![field("Type", kind.into()), field("Filesystem", fs), Line::default()];
    lines.extend(why.iter().map(|w| Line::styled(format!(" {w}"), Style::new().fg(MUTED))));
    lines
}

/// ` Label     value`
fn field(label: &str, value: String) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!(" {label:<11}"), Style::new().fg(MUTED)),
        Span::raw(value),
    ])
}

// ── Footer ──────────────────────────────────────────────────────────────────

fn draw_footer(frame: &mut Frame, area: Rect, app: &App) {
    let mut hints = vec![
        ("↑↓", "move"),
        ("⏎", "open"),
        ("←", "back"),
        ("s n c m", "sort"),
    ];
    if app.can_sudo() {
        hints.push(("S", "sudo"));
    }
    hints.extend([("?", "help"), ("q", "quit")]);
    let mut spans = vec![Span::raw(" ")];
    for (key, label) in hints {
        spans.push(Span::styled(key, Style::new().fg(ACCENT).bold()));
        spans.push(Span::styled(format!(" {label}   "), Style::new().fg(MUTED)));
    }
    frame.render_widget(Line::from(spans), area);
}

// ── Help popup ──────────────────────────────────────────────────────────────

const HELP: &[(&str, &str)] = &[
    ("↑ ↓  k j", "move"),
    ("PgUp PgDn", "jump a page"),
    ("Home End", "first / last"),
    ("⏎ → l", "open folder"),
    ("← h <", "back"),
    ("", ""),
    ("s", "sort by size"),
    ("n", "sort by name"),
    ("c", "sort by item count"),
    ("m", "sort by last modified"),
    ("", "press again to reverse"),
    ("", ""),
    ("S", "rescan with sudo"),
    ("?", "this help"),
    ("q Esc", "quit"),
];

fn draw_help(frame: &mut Frame) {
    let width = 40;
    let height = HELP.len() as u16 + 4;
    let area = centered(frame.area(), width, height);

    let mut lines = vec![Line::default()];
    lines.extend(HELP.iter().map(|&(keys, action)| {
        Line::from(vec![
            Span::styled(format!("  {keys:<12}"), Style::new().fg(ACCENT).bold()),
            Span::styled(action, Style::new().fg(MUTED)),
        ])
    }));
    let title = Line::from(Span::styled(" keys ", Style::new().bold()));
    let hint = Line::from(Span::styled(" any key to close ", Style::new().fg(MUTED)))
        .right_aligned();

    // Clear wipes what is underneath so the popup doesn't show through.
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines).block(pane(title).title_bottom(hint)),
        area,
    );
}

// ── Scanning screen ─────────────────────────────────────────────────────────

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// The screen shown while a scan runs.
pub fn draw_scanning(frame: &mut Frame, s: &Snapshot) {
    let [header, main, _] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    let mut spans = vec![
        Span::styled(" nudu ", Style::new().fg(Color::Black).bg(ACCENT).bold()),
        Span::styled("  scanning ", Style::new().fg(MUTED)),
    ];
    spans.extend(breadcrumb(&s.path));
    frame.render_widget(Line::from(spans), header);

    let width = 60.min(main.width);
    let inner = width.saturating_sub(8) as usize;
    let area = centered(main, width, 11);

    let secs = s.elapsed.as_secs_f64();
    let rate = if secs > 0.2 { (s.items as f64 / secs) as u64 } else { 0 };
    let frame_no = (s.elapsed.as_millis() / 100) as usize % SPINNER.len();

    let mut lines = vec![
        Line::default(),
        Line::from(vec![
            Span::styled(format!("  {}  ", SPINNER[frame_no]), Style::new().fg(ACCENT).bold()),
            Span::raw(fit_left(&s.current, inner)),
        ]),
        Line::default(),
        stat("Items", group_digits(s.items), Style::new()),
        stat("Size", human_size(s.bytes), Style::new()),
        stat("Elapsed", format!("{secs:.1} s"), Style::new()),
        stat("Speed", format!("{} items/s", group_digits(rate)), Style::new()),
    ];
    if s.errors > 0 {
        let red = Style::new().fg(Color::Red);
        lines.push(stat("Unreadable", group_digits(s.errors), red));
    }

    let title = Line::from(Span::styled(" scanning ", Style::new().bold()));
    let hint = Line::from(Span::styled(" esc to cancel ", Style::new().fg(MUTED))).right_aligned();
    frame.render_widget(Paragraph::new(lines).block(pane(title).title_bottom(hint)), area);
}

/// `   Label       value`, indented to line up under the spinner.
fn stat(label: &str, value: String, style: Style) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("   {label:<12}"), Style::new().fg(MUTED)),
        Span::styled(value, style),
    ])
}

/// Keeps the end of a long path: `…/steamapps/common/Game`.
fn fit_left(text: &str, width: usize) -> String {
    let len = text.chars().count();
    if len <= width || width == 0 {
        return text.to_string();
    }
    let tail: String = text.chars().skip(len - (width - 1)).collect();
    format!("…{tail}")
}

/// A `width` x `height` rectangle in the middle of `area`.
fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let [_, middle, _] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(height.min(area.height)),
        Constraint::Fill(1),
    ])
    .areas(area);
    let [_, center, _] = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(width.min(area.width)),
        Constraint::Fill(1),
    ])
    .areas(middle);
    center
}

/// A rounded, muted panel with a title on the top border.
fn pane(title: Line<'static>) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(MUTED))
        .title(title)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_bar_halves() {
        assert_eq!(line_bar(1.0, 4), ("━━━━".into(), "".into()));
        assert_eq!(line_bar(0.5, 4), ("━━".into(), "━━".into()));
        assert_eq!(line_bar(0.375, 4), ("━╸".into(), "━━".into()));
        assert_eq!(line_bar(0.0, 4), ("".into(), "━━━━".into()));
    }

    #[test]
    fn gauge_has_fixed_width() {
        for r in [0.0, 0.13, 0.5, 0.99, 1.0] {
            assert_eq!(block_gauge(r, 10).chars().count(), 10);
        }
    }

    #[test]
    fn percent_formats() {
        assert_eq!(percent(68, 100), "68%");
        assert_eq!(percent(1, 1000), "<1%");
        assert_eq!(percent(0, 0), "");
    }

    #[test]
    fn fit_pads_and_truncates() {
        assert_eq!(fit("abc", 5), "abc  ");
        assert_eq!(fit("abcdef", 4), "abc…");
    }
}
