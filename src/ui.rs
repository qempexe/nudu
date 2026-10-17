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

use std::cmp::Reverse;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Clear, HighlightSpacing, List, ListItem, Paragraph, Wrap,
};

use crate::age::{self, Bucket};
use crate::app::{App, Row};
use crate::filetype::Category;
use crate::owners::Owner;
use crate::scan::{Entry, Kind, Skipped};
use crate::scanning::Snapshot;
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

    if let Some(index) = app.confirm {
        draw_confirm(frame, app, index);
    }
    if let Some(index) = app.deleting {
        draw_deleting(frame, app, index);
    }
    if app.drives.is_some() {
        draw_drives(frame, app);
    }
    if app.owners_view.is_some() {
        draw_owners(frame, app);
    }
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

    let mut spans = vec![Span::styled(
        " nudu ",
        Style::new().fg(Color::Black).bg(ACCENT).bold(),
    )];
    if app.is_root {
        // A clear reminder that deletes (and everything else) run as root.
        spans.push(Span::styled(
            " root ",
            Style::new().fg(Color::Black).bg(Color::Red).bold(),
        ));
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
        let used = full + usize::from(!eighths.is_multiple_of(8));
        out.push_str(&" ".repeat(width - used));
    }
    out
}

// ── File list ───────────────────────────────────────────────────────────────

fn draw_list(frame: &mut Frame, area: Rect, app: &mut App) {
    let dir = app.current_dir();
    let title = match app.cold {
        // Cold view: how much old data is in here.
        Some(cold) => Line::from(vec![
            Span::raw(" "),
            Span::styled(
                human_size(app.size_of(dir)),
                Style::new().fg(Bucket::Older.color()).bold(),
            ),
            Span::styled(
                format!(" older than {} ", cold.label()),
                Style::new().fg(MUTED),
            ),
        ]),
        None => Line::from(vec![
            Span::raw(" "),
            Span::styled(human_size(dir.disk), Style::new().bold()),
            Span::styled(" · ", Style::new().fg(MUTED)),
            Span::raw(format!("{} items ", group_digits(dir.items))),
        ]),
    };
    let arrow = if app.sort.descending() { "↓" } else { "↑" };
    let sort_label = Line::from(vec![
        Span::styled(" sort ", Style::new().fg(MUTED)),
        Span::styled(
            format!("{} {arrow} ", app.sort.key.label()),
            Style::new().fg(ACCENT),
        ),
    ])
    .right_aligned();
    let mut block = pane(title).title(sort_label);
    if app.heat {
        block = block.title_bottom(heat_legend());
    }

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

    let largest = dir
        .children
        .iter()
        .map(|e| app.size_of(e))
        .max()
        .unwrap_or(0);
    let style = RowStyle {
        icons: app.settings.icons,
        heat: app.heat,
        owners: app.show_owners,
        name_width,
        bar_width,
        largest,
        total: app.size_of(dir),
    };
    let mut items: Vec<ListItem> = app
        .rows()
        .into_iter()
        .map(|row| match row {
            Row::Parent => parent_item(style.icons),
            Row::Entry(e) => entry_item(e, app.size_of(e), &style),
        })
        .collect();

    app.page_size = block.inner(area).height.max(1) as usize;

    // Nothing but `..` to show: say why, as an extra (unselectable) row.
    let parent_rows = usize::from(app.rows().iter().any(|r| matches!(r, Row::Parent)));
    if items.len() == parent_rows {
        let text = match app.cold {
            Some(cold) => format!("  nothing older than {} in here", cold.label()),
            None => "  (empty directory)".to_string(),
        };
        items.push(ListItem::new(Line::styled(text, Style::new().fg(MUTED))));
    }

    let list = List::new(items)
        .block(block)
        .highlight_symbol(Span::styled("▸ ", Style::new().fg(ACCENT).bold()))
        .highlight_spacing(HighlightSpacing::Always)
        .highlight_style(Style::new().bg(MUTED).add_modifier(Modifier::BOLD));
    frame.render_stateful_widget(list, area, &mut app.list);
}

/// `● today ● week ● month ● 6 months ● year ● older` on the list border.
fn heat_legend() -> Line<'static> {
    let short = ["today", "week", "month", "6mo", "year", "older"];
    let mut spans = vec![Span::raw(" ")];
    for (bucket, label) in Bucket::ALL.into_iter().zip(short) {
        spans.push(Span::styled("● ", Style::new().fg(bucket.color())));
        spans.push(Span::styled(format!("{label} "), Style::new().fg(MUTED)));
    }
    Line::from(spans).right_aligned()
}

fn parent_item(icons: bool) -> ListItem<'static> {
    let icon = if icons { "󰁝" } else { "^" };
    ListItem::new(Line::from(vec![
        Span::styled(format!("{icon} "), Style::new().fg(MUTED)),
        Span::styled("..", Style::new().fg(MUTED)),
    ]))
}

/// What every row of the list shares.
struct RowStyle {
    icons: bool,
    heat: bool,
    /// Show the owner tag after each name (`o`).
    owners: bool,
    name_width: usize,
    bar_width: usize,
    /// The biggest `size` in the folder: its bar is full.
    largest: u64,
    /// The folder's own `size`, for percentages.
    total: u64,
}

/// `size` is what the size column shows: space on disk, or the cold part.
fn entry_item(entry: &Entry, size: u64, style: &RowStyle) -> ListItem<'static> {
    let RowStyle {
        icons,
        heat,
        owners,
        name_width,
        bar_width,
        largest,
        total,
    } = *style;
    let (icon, icon_color) = icon_for(entry, icons);
    // Skipped folders carry a short tag like `vfat` or `snapshots`.
    let tag = match (&entry.skipped, &entry.other_fs) {
        (Some(skipped), _) => skipped_tag(skipped),
        (None, Some(fs)) => fs.clone(),
        (None, None) if owners && entry.owner != Owner::None => {
            fit(&entry.owner.label(), (name_width / 2).max(8))
                .trim_end()
                .to_string()
        }
        (None, None) => String::new(),
    };
    let tag_color = if entry.skipped.is_none() && entry.other_fs.is_none() {
        owner_color(entry.owner)
    } else {
        Color::Yellow
    };
    let tag_width = if tag.is_empty() {
        0
    } else {
        tag.chars().count() + 1
    };
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
        spans.push(Span::styled(format!(" {tag}"), Style::new().fg(tag_color)));
    }
    // `≈` marks sizes guessed on filesystems without block counts.
    let approx = if entry.estimated { "≈" } else { " " };
    spans.push(Span::raw(format!("{approx}{:>10}", human_size(size))));
    // In heat mode, the bar is colored by how long ago this last changed.
    let bucket = Bucket::of(entry.mtime);
    let bar_color = if heat { bucket.color() } else { ACCENT };
    if bar_width > 0 {
        let (fill, track) = line_bar(ratio(size, largest), bar_width);
        spans.push(Span::raw(" "));
        spans.push(Span::styled(fill, Style::new().fg(bar_color)));
        spans.push(Span::styled(track, Style::new().fg(MUTED)));
    }
    let last = if heat {
        Span::styled(
            format!(" {:>4}", age::short(entry.mtime)),
            Style::new().fg(bucket.color()),
        )
    } else {
        Span::styled(
            format!(" {:>4}", percent(size, total)),
            Style::new().fg(MUTED),
        )
    };
    spans.push(last);
    ListItem::new(Line::from(spans))
}

/// Icon and its color. Nerd Font glyphs, or plain characters with --no-icons.
fn icon_for(entry: &Entry, icons: bool) -> (&'static str, Color) {
    if entry.other_fs.is_some() && entry.skipped.is_none() {
        // A drive that has been scanned: same drive icon, normal color.
        return (if icons { "󰋊" } else { "#" }, ACCENT);
    }
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
    if whole == 0 {
        0.0
    } else {
        part as f64 / whole as f64
    }
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
        Some(Row::Parent) => (
            "..".to_string(),
            vec![field("Action", "go up one level".into())],
        ),
        Some(Row::Entry(e)) => (
            e.name.to_string_lossy().into_owned(),
            entry_details(e, app, width + 2),
        ),
    };
    let title = Line::from(vec![
        Span::raw(" "),
        Span::styled(
            fit(&title, width).trim_end().to_string(),
            Style::new().bold(),
        ),
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
        Skipped::OtherFs(usage) => usage.fs_type.clone(),
        Skipped::Virtual(fs) => fs.clone(),
        Skipped::Snapshots => "snapshots".into(),
    }
}

fn entry_details(e: &Entry, app: &App, width: usize) -> Vec<Line<'static>> {
    if let Some(skipped) = &e.skipped {
        return skipped_details(skipped);
    }
    let kind = match e.kind {
        Kind::Dir => "Directory".to_string(),
        Kind::File => format!("File · {}", Category::of(&e.name).label()),
        Kind::Symlink => "Symlink".into(),
        Kind::Other => "Special file".into(),
    };
    let mut lines = vec![
        field("Type", kind),
        field("Size", human_size(e.apparent)),
        field(
            "On disk",
            if e.estimated {
                format!("≈ {}", human_size(e.disk))
            } else {
                human_size(e.disk)
            },
        ),
    ];
    if e.estimated {
        // Under the value column, wrapped to fit the pane.
        lines.extend(field_wrapped(
            "",
            "estimated: this drive doesn't report disk use",
            width,
            Style::new().fg(MUTED),
        ));
    }
    if let Some(line) = savings_line(e) {
        lines.push(line);
    }
    if e.kind == Kind::Dir {
        lines.push(field("Items", group_digits(e.items)));
    }
    if let Some(fs) = &e.other_fs {
        lines.push(field("Filesystem", format!("{fs} (drive)")));
    }
    if e.owner != Owner::None {
        let style = Style::new().fg(owner_color(e.owner));
        lines.extend(field_wrapped("Owner", &e.owner.label(), width, style));
    }
    lines.push(field("Modified", relative_time(app.now, e.mtime)));
    let bucket = Bucket::of(e.mtime);
    lines.push(Line::from(vec![
        Span::styled(format!(" {:<11}", "Heat"), Style::new().fg(MUTED)),
        Span::styled("● ", Style::new().fg(bucket.color())),
        Span::raw(bucket.label()),
    ]));
    if e.error {
        lines.extend(error_lines(e, app));
    }

    // Share of the open folder, as a bar.
    let share = ratio(e.disk, app.current_dir().disk);
    lines.push(Line::default());
    lines.push(heading(
        "Share of folder",
        Some(percent(e.disk, app.current_dir().disk)),
        width,
    ));
    let (fill, track) = line_bar(share, width.saturating_sub(1));
    lines.push(Line::from(vec![
        Span::raw(" "),
        Span::styled(fill, Style::new().fg(ACCENT)),
        Span::styled(track, Style::new().fg(MUTED)),
    ]));

    if e.kind == Kind::Dir && !e.children.is_empty() {
        lines.push(Line::default());
        lines.push(heading("Biggest inside", None, width));
        lines.extend(biggest_inside(e, app.settings.icons, width));

        lines.push(Line::default());
        lines.push(heading("By type", None, width));
        lines.extend(by_type(e, app.settings.icons, width));

        lines.push(Line::default());
        lines.push(heading("By age", None, width));
        lines.extend(by_age(e, width));
    }
    lines
}

/// How much of a folder changed recently, and how much has been sitting.
fn by_age(e: &Entry, width: usize) -> Vec<Line<'static>> {
    let totals = e.age_totals();
    let all: u64 = totals.iter().sum();
    // [ ●][label     ][ size][ bar][ pct]
    let bar_width = width.saturating_sub(1 + 2 + 11 + 10 + 1 + 5).max(1);
    Bucket::ALL
        .into_iter()
        .filter(|&b| totals[b as usize] > 0)
        .map(|bucket| {
            let bytes = totals[bucket as usize];
            let (fill, track) = line_bar(ratio(bytes, all), bar_width);
            Line::from(vec![
                Span::styled(" ● ", Style::new().fg(bucket.color())),
                Span::raw(format!("{:<11}", bucket.label())),
                Span::raw(format!("{:>10} ", human_size(bytes))),
                Span::styled(fill, Style::new().fg(bucket.color())),
                Span::styled(track, Style::new().fg(MUTED)),
                Span::styled(
                    format!(" {:>4}", percent(bytes, all)),
                    Style::new().fg(MUTED),
                ),
            ])
        })
        .collect()
}

/// `Saved  9.3 GiB · 26%` when the data takes less room than its size
/// (compression, sparse files), or the opposite for lots of tiny files.
fn savings_line(e: &Entry) -> Option<Line<'static>> {
    const MIB: u64 = 1024 * 1024;
    if e.apparent > e.disk && e.apparent - e.disk >= MIB.min(e.apparent / 100).max(1) {
        let saved = e.apparent - e.disk;
        return Some(Line::from(vec![
            Span::styled(format!(" {:<11}", "Saved"), Style::new().fg(MUTED)),
            Span::styled(
                format!("{} · {}", human_size(saved), percent(saved, e.apparent)),
                Style::new().fg(Color::Green),
            ),
        ]));
    }
    // Blocks are allocated whole, so many small files take more room
    // than their sizes add up to. Only worth a mention when it's big.
    if e.disk > e.apparent && e.disk - e.apparent >= MIB && e.disk - e.apparent > e.apparent / 10 {
        return Some(Line::from(vec![
            Span::styled(format!(" {:<11}", "Overhead"), Style::new().fg(MUTED)),
            Span::styled(
                human_size(e.disk - e.apparent),
                Style::new().fg(Color::Yellow),
            ),
            Span::styled(" small files", Style::new().fg(MUTED)),
        ]));
    }
    None
}

fn error_lines(e: &Entry, app: &App) -> Vec<Line<'static>> {
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
    let mut lines = vec![
        Line::default(),
        Line::from(vec![
            Span::styled(" ! ", Style::new().fg(Color::Red).bold()),
            Span::styled(what, Style::new().fg(Color::Red)),
        ]),
    ];
    for hint in hints {
        lines.push(Line::styled(format!("   {hint}"), Style::new().fg(MUTED)));
    }
    lines
}

/// A section title, with an optional value pushed to the right edge.
fn heading(title: &str, right: Option<String>, width: usize) -> Line<'static> {
    let right = right.unwrap_or_default();
    let gap = width.saturating_sub(title.chars().count() + right.chars().count() + 1);
    Line::from(vec![
        Span::styled(format!(" {title}"), Style::new().bold()),
        Span::raw(" ".repeat(gap)),
        Span::styled(right, Style::new().fg(MUTED)),
    ])
}

/// The five largest entries inside a folder, so you can peek without opening it.
fn biggest_inside(e: &Entry, icons: bool, width: usize) -> Vec<Line<'static>> {
    let mut top: Vec<&Entry> = e.children.iter().collect();
    top.sort_by_key(|c| Reverse(c.disk));
    top.truncate(5);
    let largest = top.first().map(|c| c.disk).unwrap_or(0);
    // [ icon ][name…][ size][ bar]
    let bar_width = 6;
    let name_width = width.saturating_sub(1 + 2 + 10 + 1 + bar_width);
    top.into_iter()
        .map(|child| {
            let (icon, color) = icon_for(child, icons);
            let (fill, track) = line_bar(ratio(child.disk, largest), bar_width);
            Line::from(vec![
                Span::styled(format!(" {icon} "), Style::new().fg(color)),
                Span::raw(fit(&child.name.to_string_lossy(), name_width)),
                Span::raw(format!("{:>10} ", human_size(child.disk))),
                Span::styled(fill, Style::new().fg(ACCENT)),
                Span::styled(track, Style::new().fg(MUTED)),
            ])
        })
        .collect()
}

/// What kind of data a folder holds: video, images, code...
fn by_type(e: &Entry, icons: bool, width: usize) -> Vec<Line<'static>> {
    let totals = e.type_totals();
    let all: u64 = totals.iter().sum();
    let mut rows: Vec<(Category, u64)> = Category::ALL
        .iter()
        .map(|&c| (c, totals[c as usize]))
        .filter(|&(_, bytes)| bytes > 0)
        .collect();
    rows.sort_by_key(|&(_, bytes)| Reverse(bytes));
    // [ icon ][label    ][ size][ bar][ pct]
    let bar_width = width.saturating_sub(1 + 2 + 10 + 10 + 1 + 5).max(1);
    rows.into_iter()
        .map(|(category, bytes)| {
            let (fill, track) = line_bar(ratio(bytes, all), bar_width);
            Line::from(vec![
                Span::styled(
                    format!(" {} ", category.icon(icons)),
                    Style::new().fg(MUTED),
                ),
                Span::raw(format!("{:<10}", category.label())),
                Span::raw(format!("{:>10} ", human_size(bytes))),
                Span::styled(fill, Style::new().fg(ACCENT)),
                Span::styled(track, Style::new().fg(MUTED)),
                Span::styled(
                    format!(" {:>4}", percent(bytes, all)),
                    Style::new().fg(MUTED),
                ),
            ])
        })
        .collect()
}

/// What the details pane says about a folder that was not scanned.
fn skipped_details(skipped: &Skipped) -> Vec<Line<'static>> {
    let note = |text: &str| Line::styled(format!(" {text}"), Style::new().fg(MUTED));
    match skipped {
        Skipped::OtherFs(usage) => {
            let ratio = if usage.total == 0 {
                0.0
            } else {
                usage.used as f64 / usage.total as f64
            };
            vec![
                field("Type", "Mounted drive".into()),
                field("Filesystem", usage.fs_type.clone()),
                field(
                    "Used",
                    format!("{} of {}", human_size(usage.used), human_size(usage.total)),
                ),
                Line::from(vec![
                    Span::raw(" "),
                    Span::styled(block_gauge(ratio, 20), Style::new().fg(ACCENT)),
                    Span::styled(format!(" {:.0}%", ratio * 100.0), Style::new().fg(MUTED)),
                ]),
                Line::default(),
                note("Not scanned yet."),
                Line::from(vec![
                    Span::raw(" Press "),
                    Span::styled("⏎", Style::new().fg(ACCENT).bold()),
                    Span::raw(" to scan it."),
                ]),
            ]
        }
        Skipped::Virtual(fs) => vec![
            field("Type", "Mount point".into()),
            field("Filesystem", fs.clone()),
            Line::default(),
            note("Kernel filesystem with no"),
            note("real files. Never scanned."),
        ],
        Skipped::Snapshots => vec![
            field("Type", "Snapshots".into()),
            field("Filesystem", "btrfs".into()),
            Line::default(),
            note("Snapshots share data with"),
            note("your files, so counting them"),
            note("would double your totals."),
        ],
    }
}

/// Like `field`, but a value too long for the pane continues on the next
/// lines, lined up under the value column, instead of being cut off.
fn field_wrapped(label: &str, value: &str, width: usize, style: Style) -> Vec<Line<'static>> {
    const LABEL: usize = 12; // " " + 11-wide label
    let room = width.saturating_sub(LABEL).max(8);
    wrap_value(value, room)
        .into_iter()
        .enumerate()
        .map(|(i, chunk)| {
            let lead = if i == 0 {
                format!(" {label:<11}")
            } else {
                " ".repeat(LABEL)
            };
            Line::from(vec![
                Span::styled(lead, Style::new().fg(MUTED)),
                Span::styled(chunk, style),
            ])
        })
        .collect()
}

/// Splits `text` into pieces of at most `room` characters, preferring to
/// break right after `:`, `-`, `.`, `/` or a space, so names stay readable:
/// `pacman:docker-desktop` → `pacman:docker-` / `desktop`.
fn wrap_value(text: &str, room: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut rest: Vec<char> = text.chars().collect();
    while rest.len() > room {
        let window = &rest[..room];
        let cut = window
            .iter()
            .rposition(|c| matches!(c, ':' | '-' | '.' | '/' | ' ' | '_'))
            .map(|i| i + 1)
            .filter(|&i| i > room / 3) // don't leave a tiny first piece
            .unwrap_or(room);
        chunks.push(
            rest[..cut]
                .iter()
                .collect::<String>()
                .trim_end()
                .to_string(),
        );
        rest = rest[cut..].to_vec();
    }
    chunks.push(rest.into_iter().collect());
    chunks
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
    // The result of the last action replaces the hints until the next key.
    if let Some(msg) = &app.message {
        let (mark, color) = if msg.error {
            ("!", Color::Red)
        } else {
            ("✓", Color::Green)
        };
        let line = Line::from(vec![
            Span::styled(format!(" {mark} "), Style::new().fg(color).bold()),
            Span::styled(msg.text.clone(), Style::new().fg(color)),
        ]);
        frame.render_widget(line, area);
        return;
    }
    let mut hints = vec![
        ("↑↓", "move"),
        ("⏎", "open"),
        ("←", "back"),
        ("s n c m", "sort"),
    ];
    if !app.settings.read_only {
        hints.push(("d", "delete"));
    }
    hints.push(("a", if app.heat { "heat off" } else { "heat" }));
    hints.push(("A", "cold"));
    hints.push(("O", "owners"));
    hints.push(("r", "rescan"));
    hints.push(("D", "drives"));
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

/// Help, in two columns so it fits small terminals: moving | doing.
const HELP_LEFT: &[(&str, &str)] = &[
    ("↑ ↓  k j", "move"),
    ("PgUp PgDn", "jump a page"),
    ("Home End", "first / last"),
    ("⏎ → l", "open folder"),
    ("← h <", "back"),
    ("⏎ on drive", "scan that drive"),
    ("D", "all drives"),
    ("g", "new drive"),
];

const HELP_RIGHT: &[(&str, &str)] = &[
    ("s n", "sort size / name"),
    ("c m", "sort items / date"),
    ("", "again to reverse"),
    ("a", "heat colors (age)"),
    ("A", "cold data view"),
    ("o", "owner tags"),
    ("O", "owners ranking"),
    ("d Del", "delete (asks first)"),
    ("r", "rescan folder"),
    ("S", "rescan with sudo"),
    ("?", "this help"),
    ("q Esc", "quit"),
];

fn draw_help(frame: &mut Frame) {
    let rows = HELP_LEFT.len().max(HELP_RIGHT.len());
    let width = 70;
    let height = rows as u16 + 4;
    let area = centered(frame.area(), width, height);

    let column = |entry: Option<&(&str, &str)>| -> Vec<Span<'static>> {
        let (keys, action) = entry.copied().unwrap_or(("", ""));
        vec![
            Span::styled(format!("  {keys:<11}"), Style::new().fg(ACCENT).bold()),
            Span::styled(format!("{action:<20}"), Style::new().fg(MUTED)),
        ]
    };
    let mut lines = vec![Line::default()];
    for i in 0..rows {
        let mut spans = column(HELP_LEFT.get(i));
        spans.extend(column(HELP_RIGHT.get(i)));
        lines.push(Line::from(spans));
    }
    let title = Line::from(Span::styled(" keys ", Style::new().bold()));
    let hint =
        Line::from(Span::styled(" any key to close ", Style::new().fg(MUTED))).right_aligned();

    // Clear wipes what is underneath so the popup doesn't show through.
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines).block(pane(title).title_bottom(hint)),
        area,
    );
}

/// One color per kind of owner, so tags are easy to tell apart.
fn owner_color(owner: Owner) -> Color {
    match owner {
        Owner::Package(_) | Owner::Packages(_) => Color::Cyan,
        Owner::Unowned => Color::Red,
        Owner::Git(_) => Color::Green,
        Owner::Flatpak(_) | Owner::Steam(_) => Color::Magenta,
        Owner::Docker | Owner::Containers => Color::Blue,
        Owner::Cache(_) | Owner::Trash => Color::Yellow,
        Owner::Config(_) | Owner::Data(_) => Color::Gray,
        Owner::None => MUTED,
    }
}

// ── Owners popup ────────────────────────────────────────────────────────────

/// Who uses the most space in the open folder.
fn draw_owners(frame: &mut Frame, app: &mut App) {
    let Some(view) = &mut app.owners_view else {
        return;
    };
    let width = 72u16.min(frame.area().width);
    let height = (view.rows.len() as u16 + 4).clamp(6, frame.area().height.saturating_sub(2));
    let area = centered(frame.area(), width, height);

    // [▸ ][owner…][ size][ bar][ pct]
    let inner = width.saturating_sub(4) as usize;
    let bar_width = 14;
    let name_width = inner.saturating_sub(2 + 11 + 1 + bar_width + 5);
    let largest = view.rows.first().map(|r| r.1).unwrap_or(0);
    let items: Vec<ListItem> = view
        .rows
        .iter()
        .map(|&(owner, bytes)| {
            let (fill, track) = line_bar(ratio(bytes, largest), bar_width);
            ListItem::new(Line::from(vec![
                Span::styled(
                    fit(&owner.label(), name_width),
                    Style::new().fg(owner_color(owner)),
                ),
                Span::raw(format!(" {:>10} ", human_size(bytes))),
                Span::styled(fill, Style::new().fg(owner_color(owner))),
                Span::styled(track, Style::new().fg(MUTED)),
                Span::styled(
                    format!(" {:>4}", percent(bytes, view.total)),
                    Style::new().fg(MUTED),
                ),
            ]))
        })
        .collect();

    let title = Line::from(vec![
        Span::styled(" owners ", Style::new().bold()),
        Span::styled(
            format!("in {} ", fit_left(&view.folder, 30)),
            Style::new().fg(MUTED),
        ),
    ]);
    let hint = Line::from(vec![
        Span::styled(
            format!(" {} owners · ", view.rows.len()),
            Style::new().fg(MUTED),
        ),
        Span::styled("↑↓ ", Style::new().fg(ACCENT).bold()),
        Span::styled("scroll · any other key closes ", Style::new().fg(MUTED)),
    ])
    .right_aligned();
    let list = List::new(items)
        .block(pane(title).title_bottom(hint))
        .highlight_symbol(Span::styled("▸ ", Style::new().fg(ACCENT).bold()))
        .highlight_spacing(HighlightSpacing::Always)
        .highlight_style(Style::new().bg(MUTED).add_modifier(Modifier::BOLD));
    frame.render_widget(Clear, area);
    frame.render_stateful_widget(list, area, &mut view.list);
}

// ── Drives popup ────────────────────────────────────────────────────────────

/// Every mounted drive, with its type and how full it is.
fn draw_drives(frame: &mut Frame, app: &mut App) {
    let icons = app.settings.icons;
    let Some(view) = &mut app.drives else { return };
    let width = 76u16.min(frame.area().width);
    let height = (view.drives.len() as u16 + 4).clamp(6, frame.area().height);
    let area = centered(frame.area(), width, height);

    // Columns: [▸ ][icon ][mount point…][ type][ used / total][ gauge]
    let inner = width.saturating_sub(4) as usize;
    let point_width = inner.saturating_sub(2 + 2 + 12 + 24 + 11).max(10);

    let items: Vec<ListItem> = view
        .drives
        .iter()
        .map(|(drive, usage)| {
            let ratio = if usage.total == 0 {
                0.0
            } else {
                usage.used as f64 / usage.total as f64
            };
            let mut point = crate::units::shorten_home(&drive.mount.point);
            if drive.extra_mounts > 0 {
                point.push_str(&format!(" +{}", drive.extra_mounts));
            }
            let color = if ratio >= 0.9 {
                Color::Red
            } else if ratio >= 0.75 {
                Color::Yellow
            } else {
                ACCENT
            };
            ListItem::new(Line::from(vec![
                Span::styled(if icons { "󰋊 " } else { "# " }, Style::new().fg(ACCENT)),
                Span::styled(fit(&point, point_width), Style::new().bold()),
                Span::styled(
                    format!(" {:<11}", fit(&drive.mount.fs_type, 11)),
                    Style::new().fg(Color::Yellow),
                ),
                Span::raw(format!(
                    " {:>10} / {:<10}",
                    human_size(usage.used),
                    human_size(usage.total)
                )),
                Span::styled(
                    format!(" {}", block_gauge(ratio, 10)),
                    Style::new().fg(color),
                ),
            ]))
        })
        .collect();

    let title = Line::from(Span::styled(" drives ", Style::new().bold()));
    let hint = Line::from(vec![
        Span::styled(" ⏎ ", Style::new().fg(ACCENT).bold()),
        Span::styled("open · ", Style::new().fg(MUTED)),
        Span::styled("any other key ", Style::new().fg(ACCENT).bold()),
        Span::styled("close ", Style::new().fg(MUTED)),
    ])
    .right_aligned();
    let list = List::new(items)
        .block(pane(title).title_bottom(hint))
        .highlight_symbol(Span::styled("▸ ", Style::new().fg(ACCENT).bold()))
        .highlight_spacing(HighlightSpacing::Always)
        .highlight_style(Style::new().bg(MUTED).add_modifier(Modifier::BOLD));
    frame.render_widget(Clear, area);
    frame.render_stateful_widget(list, area, &mut view.list);
}

// ── Delete popups ───────────────────────────────────────────────────────────

/// "Are you sure?" before anything is removed.
fn draw_confirm(frame: &mut Frame, app: &App, index: usize) {
    let entry = app.child(index);
    let width = 52u16.min(frame.area().width);
    let inner = width.saturating_sub(6) as usize;
    let (icon, icon_color) = icon_for(entry, app.settings.icons);
    let path = format!("{}/{}", app.display_path(), entry.name.to_string_lossy());
    let path = path.replacen("//", "/", 1);

    let mut lines = vec![
        Line::default(),
        Line::from(vec![
            Span::styled(format!("  {icon} "), Style::new().fg(icon_color)),
            Span::styled(
                fit(&entry.name.to_string_lossy(), inner)
                    .trim_end()
                    .to_string(),
                Style::new().bold(),
            ),
        ]),
        Line::styled(
            format!("  {}", fit_left(&path, inner)),
            Style::new().fg(MUTED),
        ),
        Line::default(),
        field("  Size", format!("{} on disk", human_size(entry.disk))),
    ];
    if entry.kind == Kind::Dir {
        lines.push(field("  Items", group_digits(entry.items)));
    }
    // Package files should be removed with the package manager, so the
    // system's package database stays correct.
    if let Some(package) = entry.owner.package() {
        let command = match entry.owner.label().split(':').next() {
            Some("pacman") => format!("sudo pacman -R {package}"),
            Some("dpkg") => format!("sudo apt remove {package}"),
            _ => format!("your package manager ({package})"),
        };
        lines.push(Line::default());
        lines.push(Line::styled(
            format!("  Installed by package {package}."),
            Style::new().fg(Color::Yellow),
        ));
        lines.push(Line::styled(
            "  Better to remove it with:",
            Style::new().fg(Color::Yellow),
        ));
        lines.push(Line::styled(
            format!("  {command}"),
            Style::new().fg(Color::Yellow).bold(),
        ));
    } else if let Owner::Packages(n) = entry.owner {
        lines.push(Line::default());
        lines.push(Line::styled(
            format!("  Holds files of {n} packages."),
            Style::new().fg(Color::Yellow),
        ));
    }
    lines.push(Line::default());
    lines.push(Line::styled(
        "  This cannot be undone.",
        Style::new().fg(Color::Red).bold(),
    ));
    if app.is_root {
        lines.push(Line::styled(
            "  Running as root: nothing is protected.",
            Style::new().fg(Color::Red),
        ));
    }
    lines.push(Line::default());

    let height = lines.len() as u16 + 2;
    let area = centered(frame.area(), width, height);
    let title = Line::from(Span::styled(" delete ", Style::new().fg(Color::Red).bold()));
    let hint = Line::from(vec![
        Span::styled(" y ", Style::new().fg(Color::Red).bold()),
        Span::styled("delete · ", Style::new().fg(MUTED)),
        Span::styled("any other key ", Style::new().fg(ACCENT).bold()),
        Span::styled("cancel ", Style::new().fg(MUTED)),
    ])
    .right_aligned();
    let block = pane(title)
        .title_bottom(hint)
        .border_style(Style::new().fg(Color::Red));
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// Shown for the moment a delete takes.
fn draw_deleting(frame: &mut Frame, app: &App, index: usize) {
    let entry = app.child(index);
    let width = 44u16.min(frame.area().width);
    let area = centered(frame.area(), width, 5);
    let name = fit(
        &entry.name.to_string_lossy(),
        width.saturating_sub(8) as usize,
    );
    let lines = vec![
        Line::default(),
        Line::from(vec![
            Span::styled("  ⠿ ", Style::new().fg(Color::Red).bold()),
            Span::raw(name.trim_end().to_string()),
        ]),
    ];
    let title = Line::from(Span::styled(
        " deleting ",
        Style::new().fg(Color::Red).bold(),
    ));
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines).block(pane(title)), area);
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
    let rate = if secs > 0.2 {
        (s.items as f64 / secs) as u64
    } else {
        0
    };
    let frame_no = (s.elapsed.as_millis() / 100) as usize % SPINNER.len();

    let mut lines = vec![
        Line::default(),
        Line::from(vec![
            Span::styled(
                format!("  {}  ", SPINNER[frame_no]),
                Style::new().fg(ACCENT).bold(),
            ),
            Span::raw(fit_left(&s.current, inner)),
        ]),
        Line::default(),
        stat("Items", group_digits(s.items), Style::new()),
        stat("Size", human_size(s.bytes), Style::new()),
        stat("Elapsed", format!("{secs:.1} s"), Style::new()),
        stat(
            "Speed",
            format!("{} items/s", group_digits(rate)),
            Style::new(),
        ),
    ];
    if s.errors > 0 {
        let red = Style::new().fg(Color::Red);
        lines.push(stat("Unreadable", group_digits(s.errors), red));
    }

    let title = Line::from(Span::styled(" scanning ", Style::new().bold()));
    let hint = Line::from(Span::styled(" esc to cancel ", Style::new().fg(MUTED))).right_aligned();
    frame.render_widget(
        Paragraph::new(lines).block(pane(title).title_bottom(hint)),
        area,
    );
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
    fn wraps_long_values_at_natural_breaks() {
        assert_eq!(wrap_value("pacman:firefox", 20), ["pacman:firefox"]);
        assert_eq!(
            wrap_value("pacman:docker-desktop", 19),
            ["pacman:docker-", "desktop"]
        );
        assert_eq!(
            wrap_value("flatpak:org.gimp.GIMP", 12),
            ["flatpak:org.", "gimp.GIMP"]
        );
        // No good break point: cut at the edge.
        assert_eq!(wrap_value("abcdefghij", 4), ["abcd", "efgh", "ij"]);
    }

    #[test]
    fn fit_pads_and_truncates() {
        assert_eq!(fit("abc", 5), "abc  ");
        assert_eq!(fit("abcdef", 4), "abc…");
    }
}
