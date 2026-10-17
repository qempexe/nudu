//! Application state and the main event loop.

use std::io;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::widgets::ListState;

use crate::fsinfo::FsInfo;
use crate::scan::{Entry, Kind};
use crate::sort::{Sort, SortKey};
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
    /// How entries are sorted on screen.
    pub sort: Sort,
    /// Child indexes of the open directory in display order (see sort.rs).
    order: Vec<usize>,
    /// Which row is highlighted, and how far the list is scrolled.
    pub list: ListState,
    /// Rows visible in the list, updated on every draw (used by PgUp/PgDn).
    pub page_size: usize,
    /// Whether the help popup is open.
    pub show_help: bool,
    /// Set to true to leave the event loop.
    pub should_quit: bool,
    /// Running with root rights.
    pub is_root: bool,
    /// The user asked to rescan with sudo; main() restarts nudu after quitting.
    pub relaunch_sudo: bool,
    /// Type and fullness of the scanned filesystem, for the header.
    pub fs: Option<FsInfo>,
    /// Draw Nerd Font icons (false = plain characters).
    pub icons: bool,
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
            now,
            root,
            root_path,
            trail: Vec::new(),
            sort: Sort::default(),
            order: Vec::new(),
            list: ListState::default(),
            page_size: 1,
            show_help: false,
            should_quit: false,
            is_root: crate::privilege::is_root(),
            relaunch_sudo: false,
        };
        app.refresh_order();
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
        let children = &self.current_dir().children;
        let mut rows = Vec::with_capacity(self.order.len() + 1);
        if self.has_parent_row() {
            rows.push(Row::Parent);
        }
        rows.extend(self.order.iter().map(|&i| Row::Entry(&children[i])));
        rows
    }

    /// The row under the cursor, if any.
    pub fn selected_row(&self) -> Option<Row<'_>> {
        let row = self.list.selected()?;
        if row == 0 && self.has_parent_row() {
            return Some(Row::Parent);
        }
        let index = self.child_index(row)?;
        Some(Row::Entry(&self.current_dir().children[index]))
    }

    /// Maps a list row to the index of that child in the scanned tree.
    fn child_index(&self, row: usize) -> Option<usize> {
        let offset = usize::from(self.has_parent_row());
        self.order.get(row.checked_sub(offset)?).copied()
    }

    /// Maps a child index back to the list row showing it.
    fn row_of(&self, index: usize) -> Option<usize> {
        let offset = usize::from(self.has_parent_row());
        self.order.iter().position(|&i| i == index).map(|p| p + offset)
    }

    /// Path of the open directory with the home folder shortened to `~`.
    pub fn display_path(&self) -> String {
        crate::units::shorten_home(&self.current_path())
    }

    fn row_count(&self) -> usize {
        self.order.len() + usize::from(self.has_parent_row())
    }

    /// True when some folders were unreadable and sudo could help.
    pub fn can_sudo(&self) -> bool {
        self.root.error && !self.is_root
    }

    /// Recomputes the display order of the open directory.
    fn refresh_order(&mut self) {
        self.order = self.sort.order(&self.current_dir().children);
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
        // While help is open, any key just closes it.
        if self.show_help {
            self.show_help = false;
            return;
        }
        let page = self.page_size as isize;
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Char('?') => self.show_help = true,
            KeyCode::Char('S') if self.can_sudo() => {
                self.relaunch_sudo = true;
                self.should_quit = true;
            }
            KeyCode::Down | KeyCode::Char('j') => self.move_by(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_by(-1),
            KeyCode::PageDown => self.move_by(page),
            KeyCode::PageUp => self.move_by(-page),
            KeyCode::Home => self.move_by(isize::MIN / 2),
            KeyCode::End => self.move_by(isize::MAX / 2),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => self.open_selected(),
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Char('<') => self.go_up(),
            KeyCode::Char('s') => self.sort_by(SortKey::Size),
            KeyCode::Char('n') => self.sort_by(SortKey::Name),
            KeyCode::Char('c') => self.sort_by(SortKey::Items),
            KeyCode::Char('m') => self.sort_by(SortKey::Modified),
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

    /// Changes the sort and keeps the cursor on the same entry.
    fn sort_by(&mut self, key: SortKey) {
        let selected = self.list.selected().and_then(|r| self.child_index(r));
        self.sort = self.sort.toggle(key);
        self.refresh_order();
        if let Some(index) = selected
            && let Some(row) = self.row_of(index)
        {
            self.select_row(row);
        }
    }

    /// Enter: descend into the selected directory, or go up on `..`.
    fn open_selected(&mut self) {
        let Some(row) = self.list.selected() else { return };
        if row == 0 && self.has_parent_row() {
            self.go_up();
            return;
        }
        let Some(index) = self.child_index(row) else { return };
        let entry = &self.current_dir().children[index];
        // Files can't be opened, and skipped folders were never read.
        if entry.kind != Kind::Dir || entry.skipped.is_some() {
            return;
        }
        self.trail.push(index);
        self.refresh_order();
        // Start on the first real entry, not on `..`.
        *self.list.offset_mut() = 0;
        self.select_row(1);
    }

    /// Back to the parent directory, with the cursor on the folder we left.
    fn go_up(&mut self) {
        let Some(left) = self.trail.pop() else { return };
        self.refresh_order();
        *self.list.offset_mut() = 0;
        let row = self.row_of(left).unwrap_or(0);
        self.select_row(row);
    }
}
