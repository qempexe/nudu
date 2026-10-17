//! Application state and the main event loop.

use std::io;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::widgets::ListState;

use crate::fsinfo::FsInfo;
use crate::scan::{Entry, Kind};
use crate::ui;

/// One row of the browser: either the `..` link or a real entry.
pub enum Row<'a> {
    Parent,
    Entry(&'a Entry),
}

pub struct App {
    /// The root of the scanned tree.
    pub root: Entry,
    /// The path the user asked us to scan, as typed.
    pub root_path: PathBuf,
    /// Where we are: child indexes from the root down to the open directory.
    /// Empty means we are looking at the root itself.
    trail: Vec<usize>,
    /// The cursor row we left behind in each parent, restored on the way back.
    saved_rows: Vec<usize>,
    /// Which row is highlighted, and how far the list is scrolled.
    pub list: ListState,
    /// Rows visible in the list, updated on every draw (used by PgUp/PgDn).
    pub page_size: usize,
    /// Set to true to leave the event loop.
    pub should_quit: bool,
    /// Type and fullness of the scanned filesystem, for the header.
    pub fs: Option<FsInfo>,
    /// Draw Nerd Font icons (false = plain characters).
    pub icons: bool,
    /// The user's home directory, shown as `~` in the breadcrumb.
    pub home: Option<PathBuf>,
    /// "Now", fixed at startup, for "3 days ago" style times.
    pub now: i64,
}

impl App {
    pub fn new(root: Entry, root_path: PathBuf, fs: Option<FsInfo>, icons: bool) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let mut app = Self {
            fs,
            icons,
            home: std::env::var_os("HOME").map(PathBuf::from),
            now,
            root,
            root_path,
            trail: Vec::new(),
            saved_rows: Vec::new(),
            list: ListState::default(),
            page_size: 1,
            should_quit: false,
        };
        app.select_row(0);
        app
    }

    /// The directory currently open.
    pub fn current_dir(&self) -> &Entry {
        self.trail
            .iter()
            .fold(&self.root, |dir, &i| &dir.children[i])
    }

    /// Full path of the directory currently open.
    pub fn current_path(&self) -> PathBuf {
        let mut path = self.root_path.clone();
        let mut dir = &self.root;
        for &i in &self.trail {
            dir = &dir.children[i];
            path.push(&dir.name);
        }
        path
    }

    /// True when a `..` row sits on top of the list.
    fn has_parent_row(&self) -> bool {
        !self.trail.is_empty()
    }

    /// The rows shown in the list right now, `..` first when not at the root.
    pub fn rows(&self) -> Vec<Row<'_>> {
        let mut rows = Vec::new();
        if self.has_parent_row() {
            rows.push(Row::Parent);
        }
        rows.extend(self.current_dir().children.iter().map(Row::Entry));
        rows
    }

    /// The row under the cursor, if any.
    pub fn selected_row(&self) -> Option<Row<'_>> {
        let row = self.list.selected()?;
        let offset = usize::from(self.has_parent_row());
        if row < offset {
            return Some(Row::Parent);
        }
        self.current_dir().children.get(row - offset).map(Row::Entry)
    }

    /// Path of the open directory with the home folder shortened to `~`.
    pub fn display_path(&self) -> String {
        let path = self.current_path();
        if let Some(home) = &self.home
            && let Ok(rest) = path.strip_prefix(home)
        {
            return if rest.as_os_str().is_empty() {
                "~".into()
            } else {
                format!("~/{}", rest.display())
            };
        }
        path.display().to_string()
    }

    fn row_count(&self) -> usize {
        self.current_dir().children.len() + usize::from(self.has_parent_row())
    }

    /// Draw, wait for input, react, repeat - until the user quits.
    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        while !self.should_quit {
            terminal.draw(|frame| ui::draw(frame, self))?;
            self.handle_event(event::read()?);
        }
        Ok(())
    }

    fn handle_event(&mut self, event: Event) {
        // Only react to key presses (not releases or repeats on some terminals).
        let Event::Key(key) = event else { return };
        if key.kind != KeyEventKind::Press {
            return;
        }
        let page = self.page_size as isize;
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Down | KeyCode::Char('j') => self.move_by(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_by(-1),
            KeyCode::PageDown => self.move_by(page),
            KeyCode::PageUp => self.move_by(-page),
            KeyCode::Home => self.move_by(isize::MIN / 2),
            KeyCode::End => self.move_by(isize::MAX / 2),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => self.open_selected(),
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Char('<') => self.go_up(),
            _ => {}
        }
    }

    /// Moves the selection by `delta` rows, clamped to the list bounds.
    fn move_by(&mut self, delta: isize) {
        let len = self.row_count();
        if len == 0 {
            return;
        }
        let at = self.list.selected().unwrap_or(0) as isize;
        let next = at.saturating_add(delta).clamp(0, len as isize - 1);
        self.list.select(Some(next as usize));
    }

    /// Selects `row`, or nothing if the list is empty.
    fn select_row(&mut self, row: usize) {
        let len = self.row_count();
        self.list
            .select(if len == 0 { None } else { Some(row.min(len - 1)) });
    }

    /// Enter: descend into the selected directory, or go up on `..`.
    fn open_selected(&mut self) {
        let Some(row) = self.list.selected() else { return };
        let offset = usize::from(self.has_parent_row());
        if row < offset {
            self.go_up();
            return;
        }
        let index = row - offset;
        if self.current_dir().children[index].kind != Kind::Dir {
            return;
        }
        self.saved_rows.push(row);
        self.trail.push(index);
        // Start on the first real entry, not on `..`.
        *self.list.offset_mut() = 0;
        self.select_row(1);
    }

    /// Back to the parent directory, with the cursor where we left it.
    fn go_up(&mut self) {
        if self.trail.pop().is_none() {
            return;
        }
        let row = self.saved_rows.pop().unwrap_or(0);
        *self.list.offset_mut() = 0;
        self.select_row(row);
    }
}
