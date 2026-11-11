//! Application state and the main event loop.

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::widgets::ListState;

use crate::age::{self, Cold};
use crate::delete;
use crate::filter::Filter;
use crate::fsinfo::FsInfo;
use crate::mounts::{Drive, MountTable};
use crate::owners::{self, Owner};
use crate::scan::{self, Entry, Kind, MountUsage, ScanOptions, Skipped};
use crate::scanning;
use crate::sort::{Sort, SortKey};
use crate::tree;
use crate::ui;
use crate::units::human_size;

/// One row of the browser: either the `..` link or a real entry.
pub enum Row<'a> {
    Parent,
    Entry(&'a Entry),
}

/// Choices from the command line that the browser needs.
pub struct Settings {
    /// Draw Nerd Font icons (false = plain characters).
    pub icons: bool,
    /// Turn deleting off entirely.
    pub read_only: bool,
    /// How the original scan was made, reused when rescanning after a delete.
    pub scan: ScanOptions,
}

/// Work that needs the terminal (it shows the progress screen),
/// so the event loop runs it between frames.
pub enum Job {
    /// Scan the mounted drive at this child index, then open it.
    ScanMount(usize),
    /// Scan the open folder again. `reason` explains why, when nudu does
    /// it by itself (a drive was unmounted).
    Rescan { reason: Option<String> },
    /// Start over in another place, like a drive picked from the list.
    OpenRoot {
        path: PathBuf,
        reason: Option<String>,
    },
}

/// The drives popup: every mounted disk, with how full it is.
pub struct DrivesView {
    pub drives: Vec<(Drive, MountUsage)>,
    pub list: ListState,
}

/// How often to look for newly mounted or unmounted drives while idle.
const MOUNT_CHECK: Duration = Duration::from_secs(1);

/// A one-line note in the footer, cleared by the next key press.
pub struct Message {
    pub text: String,
    pub error: bool,
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
    pub settings: Settings,
    /// "Now", fixed at startup, for "3 days ago" style times.
    pub now: i64,
    /// `a`: color bars by age instead of the accent color.
    pub heat: bool,
    /// `A`: only show data older than this, sized by how much of it there is.
    pub cold: Option<Cold>,
    /// `x`: sizes only count what deleting would free (data that is not
    /// shared with snapshots or copies).
    pub exclusive: bool,
    /// A delete removed shared data, so `shared` on what is left may be
    /// too high until the next rescan.
    pub shared_stale: bool,
    /// Child index waiting for the user to confirm its deletion.
    pub confirm: Option<usize>,
    /// Child index being deleted right now (drawn as "deleting...").
    pub deleting: Option<usize>,
    /// `space`: entries marked to be deleted together, by full path, so
    /// marks survive sorting, moving between folders and rescans.
    pub marks: Vec<PathBuf>,
    /// The marked entries wait for the user to confirm their deletion.
    pub confirm_marks: bool,
    /// The marked entries are being deleted right now.
    pub deleting_marks: bool,
    /// Footer note from the last action.
    pub message: Option<Message>,
    /// A scan waiting to run (see `Job`).
    job: Option<Job>,
    /// The mount table when we last looked, to notice drives coming and going.
    mounts_seen: String,
    /// The drive mounted most recently while nudu was open (`g` opens it).
    pub new_drive: Option<PathBuf>,
    /// The drives popup, when open.
    pub drives: Option<DrivesView>,
    /// `o`: show owner tags in the list.
    pub show_owners: bool,
    /// `O`: who uses the most space in the open folder.
    pub owners_view: Option<OwnersView>,
    /// `/`: only show names containing some text. Belongs to the open
    /// folder, so moving to another folder drops it.
    pub filter: Option<Filter>,
}

/// The owners popup: owners of everything in the open folder, biggest first.
pub struct OwnersView {
    pub folder: String,
    pub total: u64,
    pub rows: Vec<(Owner, u64)>,
    pub list: ListState,
}

impl App {
    pub fn new(root: Entry, root_path: PathBuf, fs: Option<FsInfo>, settings: Settings) -> Self {
        let now = age::now();
        let mut app = Self {
            fs,
            settings,
            now,
            heat: false,
            cold: None,
            exclusive: false,
            shared_stale: false,
            confirm: None,
            deleting: None,
            marks: Vec::new(),
            confirm_marks: false,
            deleting_marks: false,
            message: None,
            job: None,
            mounts_seen: read_mounts(),
            new_drive: None,
            drives: None,
            show_owners: false,
            owners_view: None,
            filter: None,
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
        self.order
            .iter()
            .position(|&i| i == index)
            .map(|p| p + offset)
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

    /// What the size column shows for an entry: its space on disk, in the
    /// cold view only the part of it that is old, or in the exclusive view
    /// only the part nothing else shares.
    pub fn size_of(&self, entry: &Entry) -> u64 {
        match self.cold {
            Some(cold) => cold.bytes(&entry.age_totals()),
            None if self.exclusive => entry.disk.saturating_sub(entry.shared),
            None => entry.disk,
        }
    }

    /// True when the scan found data shared with snapshots or copies.
    pub fn has_shared(&self) -> bool {
        self.root.shared > 0
    }

    /// `x`: switch between space on disk and exclusive sizes.
    fn toggle_exclusive(&mut self) {
        if !self.exclusive && !self.has_shared() {
            self.note("Nothing here is shared with snapshots or copies.", false);
            return;
        }
        let selected = self.list.selected().and_then(|r| self.child_index(r));
        self.exclusive = !self.exclusive;
        // One special view at a time: cold sizes can't also be exclusive.
        self.cold = None;
        self.refresh_order();
        let row = selected.and_then(|i| self.row_of(i)).unwrap_or(0);
        *self.list.offset_mut() = 0;
        self.select_row(row);
        if self.exclusive {
            self.note("Exclusive view: what deleting would free", false);
        } else {
            self.note("Exclusive view off", false);
        }
    }

    /// Recomputes the display order of the open directory.
    fn refresh_order(&mut self) {
        let children = &self.current_dir().children;
        let mut order = self.sort.order(children, |e| self.size_of(e));
        if self.cold.is_some() {
            // The cold view hides everything without old data in it.
            order.retain(|&i| self.size_of(&children[i]) > 0);
        }
        if let Some(filter) = &self.filter {
            order.retain(|&i| filter.matches(&children[i].name.to_string_lossy()));
        }
        self.order = order;
    }

    /// `A`: one year, six months, off. Keeps the cursor on the same entry
    /// when it is still visible.
    fn cycle_cold(&mut self) {
        let selected = self.list.selected().and_then(|r| self.child_index(r));
        self.cold = Cold::next(self.cold);
        self.exclusive = false;
        self.refresh_order();
        let row = selected.and_then(|i| self.row_of(i)).unwrap_or(0);
        *self.list.offset_mut() = 0;
        self.select_row(row);
        match self.cold {
            Some(cold) => self.note(
                &format!("Cold view: data older than {}", cold.label()),
                false,
            ),
            None => self.note("Cold view off", false),
        }
    }

    /// A child of the open directory, by its index in the tree.
    pub fn child(&self, index: usize) -> &Entry {
        &self.current_dir().children[index]
    }

    /// Draw, wait for input, react, repeat - until the user quits.
    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        while !self.should_quit {
            terminal.draw(|frame| ui::draw(frame, self))?;
            // A confirmed delete runs right after the "deleting..." frame
            // is on screen, so a big folder never looks frozen.
            if let Some(index) = self.deleting.take() {
                self.perform_delete(index);
                continue;
            }
            if std::mem::take(&mut self.deleting_marks) {
                self.perform_marked_delete();
                continue;
            }
            if let Some(job) = self.job.take() {
                match job {
                    Job::ScanMount(index) => self.scan_mount(terminal, index)?,
                    Job::Rescan { reason } => self.rescan(terminal, reason)?,
                    Job::OpenRoot { path, reason } => self.open_root(terminal, path, reason)?,
                }
                continue;
            }
            // Wait for a key, but look at the mount table now and then.
            if event::poll(MOUNT_CHECK)? {
                self.handle_event(event::read()?);
            } else {
                self.check_mounts();
            }
        }
        Ok(())
    }

    /// Tells the user when a drive was mounted or unmounted while nudu runs,
    /// and where, because it is usually outside the folder on screen
    /// (USB sticks land in /run/media/<you>/...).
    fn check_mounts(&mut self) {
        let now_raw = read_mounts();
        if now_raw == self.mounts_seen {
            return;
        }
        let before = MountTable::parse(&self.mounts_seen);
        let now = MountTable::parse(&now_raw);
        self.mounts_seen = now_raw;

        if let Some(drive) = now.added_since(&before).into_iter().next() {
            let point = drive.mount.point.clone();
            self.note(
                &format!(
                    "Mounted {} ({}) · g to open it · D for all drives",
                    crate::units::shorten_home(&point),
                    drive.mount.fs_type
                ),
                false,
            );
            self.new_drive = Some(point);
        } else if let Some(drive) = now.removed_since(&before).into_iter().next() {
            let point = drive.mount.point;
            if self.new_drive.as_ref() == Some(&point) {
                self.new_drive = None;
            }
            self.settings.scan.include.retain(|p| p != &point);
            let reason = format!("{} was unmounted", crate::units::shorten_home(&point));
            if self.current_path().starts_with(&point) || self.root_path.starts_with(&point) {
                // We are inside the drive that just went away: step out.
                self.recover(reason);
            } else {
                self.note(&format!("{reason} · r to rescan"), false);
            }
        }
        // If the drives popup is open, keep it current.
        if self.drives.is_some() {
            self.open_drives();
        }
    }

    /// `O`: ranks the owners of everything in the open folder.
    fn open_owners(&mut self) {
        let dir = self.current_dir();
        let rows = owners::ranking(dir);
        if rows.is_empty() {
            self.note("Nothing here takes up space.", false);
            return;
        }
        let total = rows.iter().map(|(_, b)| b).sum();
        let mut list = ListState::default();
        list.select(Some(0));
        self.owners_view = Some(OwnersView {
            folder: self.display_path(),
            total,
            rows,
            list,
        });
    }

    /// `D`: lists every mounted drive.
    fn open_drives(&mut self) {
        let drives: Vec<(Drive, MountUsage)> = MountTable::load()
            .drives()
            .into_iter()
            .map(|d| {
                let usage = scan::mount_usage(&d.mount.point, &d.mount.fs_type);
                (d, usage)
            })
            .collect();
        let mut list = ListState::default();
        // Keep the cursor where it was if the popup was already open.
        let keep = self
            .drives
            .as_ref()
            .and_then(|v| v.list.selected())
            .unwrap_or(0);
        if !drives.is_empty() {
            list.select(Some(keep.min(drives.len() - 1)));
        }
        self.drives = Some(DrivesView { drives, list });
    }

    /// Keys while the drives popup is open.
    fn drives_key(&mut self, code: KeyCode) {
        let Some(view) = &mut self.drives else { return };
        let len = view.drives.len();
        let at = view.list.selected().unwrap_or(0);
        match code {
            KeyCode::Down | KeyCode::Char('j') if len > 0 => {
                view.list.select(Some((at + 1).min(len - 1)))
            }
            KeyCode::Up | KeyCode::Char('k') => view.list.select(Some(at.saturating_sub(1))),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                if let Some((drive, _)) = view.drives.get(at) {
                    self.job = Some(Job::OpenRoot {
                        path: drive.mount.point.clone(),
                        reason: None,
                    });
                }
                self.drives = None;
            }
            _ => self.drives = None,
        }
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
        // The drives popup takes the keys while it is open.
        if self.drives.is_some() {
            self.drives_key(key.code);
            return;
        }
        // So does the owners popup: arrows scroll, anything else closes.
        if let Some(view) = &mut self.owners_view {
            let at = view.list.selected().unwrap_or(0);
            match key.code {
                KeyCode::Down | KeyCode::Char('j') => view
                    .list
                    .select(Some((at + 1).min(view.rows.len().saturating_sub(1)))),
                KeyCode::Up | KeyCode::Char('k') => view.list.select(Some(at.saturating_sub(1))),
                _ => self.owners_view = None,
            }
            return;
        }
        self.message = None;
        // While a delete waits for confirmation, only `y` goes ahead.
        if let Some(index) = self.confirm.take() {
            if key.code == KeyCode::Char('y') {
                self.deleting = Some(index);
            }
            return;
        }
        // The same for a batch of marked entries. The marks stay on a "no".
        if std::mem::take(&mut self.confirm_marks) {
            if key.code == KeyCode::Char('y') {
                self.deleting_marks = true;
            }
            return;
        }
        // While typing a filter, keys go into it.
        if self.filter.as_ref().is_some_and(|f| f.typing) {
            self.filter_key(key);
            return;
        }
        let page = self.page_size as isize;
        match key.code {
            // Esc first drops a filter, and only quits when there is none.
            KeyCode::Esc if self.filter.is_some() => self.set_filter(None),
            // Then it drops the marks: quitting by accident would lose them.
            KeyCode::Esc if !self.marks.is_empty() => {
                self.marks.clear();
                self.note("Marks cleared", false);
            }
            KeyCode::Char(' ') => self.toggle_mark(),
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Char('/') => {
                // Edit the current filter, or start a new one.
                let mut filter = self.filter.clone().unwrap_or_default();
                filter.typing = true;
                self.filter = Some(filter);
            }
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
            KeyCode::Char('d') | KeyCode::Delete => self.request_delete(),
            KeyCode::Char('r') => self.job = Some(Job::Rescan { reason: None }),
            KeyCode::Char('D') => self.open_drives(),
            KeyCode::Char('o') => self.show_owners = !self.show_owners,
            KeyCode::Char('O') => self.open_owners(),
            KeyCode::Char('a') => self.heat = !self.heat,
            KeyCode::Char('A') => self.cycle_cold(),
            KeyCode::Char('x') => self.toggle_exclusive(),
            KeyCode::Char('g') => match self.new_drive.clone() {
                Some(path) => self.job = Some(Job::OpenRoot { path, reason: None }),
                None => self.note("No new drive yet · D lists all drives", false),
            },
            _ => {}
        }
    }

    /// Keys while typing after `/`: the list narrows with every letter.
    fn filter_key(&mut self, key: KeyEvent) {
        let Some(mut filter) = self.filter.clone() else {
            return;
        };
        let page = self.page_size as isize;
        match key.code {
            KeyCode::Esc => return self.set_filter(None),
            KeyCode::Enter => {
                // Keep the filter and go back to browsing (`/` edits it again).
                filter.typing = false;
                let keep = (!filter.is_empty()).then_some(filter);
                return self.set_filter(keep);
            }
            KeyCode::Backspace if filter.is_empty() => return self.set_filter(None),
            KeyCode::Backspace => {
                filter.query.pop();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                filter.query.clear();
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                filter.query.push(c);
            }
            // Moving works while typing, so you can pick a match right away.
            KeyCode::Down => return self.move_by(1),
            KeyCode::Up => return self.move_by(-1),
            KeyCode::PageDown => return self.move_by(page),
            KeyCode::PageUp => return self.move_by(-page),
            _ => return,
        }
        self.set_filter(Some(filter));
    }

    /// Applies a new filter (or none). The cursor stays on the same entry if
    /// it still matches, otherwise it goes to the first match.
    fn set_filter(&mut self, filter: Option<Filter>) {
        let selected = self.list.selected().and_then(|r| self.child_index(r));
        self.filter = filter;
        self.refresh_order();
        *self.list.offset_mut() = 0;
        let first = usize::from(self.has_parent_row() && !self.order.is_empty());
        let row = selected.and_then(|i| self.row_of(i)).unwrap_or(first);
        self.select_row(row);
    }

    /// The filter's query while it hides something, for the list title.
    pub fn active_filter(&self) -> Option<&Filter> {
        self.filter.as_ref().filter(|f| !f.is_empty())
    }

    /// Number of entries in the open folder, matches or not.
    pub fn entries_in_folder(&self) -> usize {
        self.current_dir().children.len()
    }

    /// Total size of the rows shown, for the filter's summary.
    pub fn shown_size(&self) -> u64 {
        let children = &self.current_dir().children;
        self.order.iter().map(|&i| self.size_of(&children[i])).sum()
    }

    /// Number of rows shown, not counting `..`.
    pub fn shown_count(&self) -> usize {
        self.order.len()
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
        self.list.select(if len == 0 {
            None
        } else {
            Some(row.min(len - 1))
        });
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

    /// `space`: mark or unmark the selected entry, then step down so a
    /// run of entries can be marked by holding the key.
    fn toggle_mark(&mut self) {
        if self.settings.read_only {
            self.note("Read-only mode: deleting is turned off.", true);
            return;
        }
        let Some(index) = self.list.selected().and_then(|r| self.child_index(r)) else {
            return;
        };
        if self.child(index).skipped.is_some() {
            self.note("Mount points and snapshots can't be deleted here.", true);
            return;
        }
        let path = self.current_path().join(&self.child(index).name);
        match self.marks.iter().position(|m| *m == path) {
            Some(at) => {
                self.marks.remove(at);
            }
            None => self.marks.push(path),
        }
        self.move_by(1);
    }

    /// Is the entry at this full path marked?
    pub fn is_marked(&self, path: &std::path::Path) -> bool {
        self.marks.iter().any(|m| m == path)
    }

    /// The marked entries that will really be deleted, with their paths:
    /// without the ones that vanished from the tree, and without the ones
    /// inside another marked folder (that folder takes them along).
    pub fn marked_entries(&self) -> Vec<(&PathBuf, &Entry)> {
        self.marks
            .iter()
            .filter(|path| {
                !self
                    .marks
                    .iter()
                    .any(|other| other != *path && path.starts_with(other))
            })
            .filter_map(|path| {
                let trail = tree::trail_to(&self.root, &self.root_path, path)?;
                // The scan root itself has no parent to delete it from.
                (!trail.is_empty()).then(|| (path, tree::entry_at(&self.root, &trail)))
            })
            .collect()
    }

    /// Deletes every marked entry, then puts the browser back on its feet:
    /// the open folder may be gone, or sit at another index now.
    fn perform_marked_delete(&mut self) {
        let here = self.current_path();
        let row = self.list.selected().unwrap_or(0);
        let targets: Vec<PathBuf> = self
            .marked_entries()
            .into_iter()
            .map(|(path, _)| path.clone())
            .collect();

        let (mut done, mut freed, mut shared) = (0usize, 0u64, 0u64);
        let mut failed: Option<(String, io::Error)> = None;
        for path in &targets {
            // Looked up again each time: every delete shifts indexes.
            let Some(mut trail) = tree::trail_to(&self.root, &self.root_path, path) else {
                continue;
            };
            let Some(index) = trail.pop() else { continue };
            let Some(dir_path) = path.parent() else {
                continue;
            };
            shared += tree::entry_at(&self.root, &trail).children[index].shared;
            let outcome =
                delete::delete(&mut self.root, &trail, dir_path, index, &self.settings.scan);
            freed += outcome.freed;
            match outcome.error {
                None => done += 1,
                Some(e) => {
                    let name = path.file_name().unwrap_or_default().to_string_lossy();
                    failed.get_or_insert((name.into_owned(), e));
                }
            }
        }
        self.marks.clear();

        // Back to the open folder, or the nearest one above it that is left.
        self.trail = here
            .ancestors()
            .find_map(|p| tree::trail_to(&self.root, &self.root_path, p))
            .unwrap_or_default();
        self.refresh_order();
        *self.list.offset_mut() = 0;
        self.select_row(row);
        self.fs = FsInfo::for_path(&self.root_path);

        let items = if done == 1 { "item" } else { "items" };
        let freed = if shared > 0 {
            self.shared_stale = true;
            format!(
                "about {} ({} was shared and stays on disk)",
                human_size(freed.saturating_sub(shared)),
                human_size(shared.min(freed))
            )
        } else {
            human_size(freed)
        };
        match failed {
            None => self.note(&format!("Deleted {done} {items} · freed {freed}"), false),
            Some((name, e)) => self.note(
                &format!("Deleted {done} {items}, but not all of {name}: {e} · freed {freed}"),
                true,
            ),
        }
    }

    /// `d`: ask before deleting the marked entries, or the selected one.
    fn request_delete(&mut self) {
        if self.settings.read_only {
            self.note("Read-only mode: deleting is turned off.", true);
            return;
        }
        if !self.marks.is_empty() {
            if self.marked_entries().is_empty() {
                self.marks.clear();
                self.note("The marked items are no longer there.", true);
            } else {
                self.confirm_marks = true;
            }
            return;
        }
        let Some(row) = self.list.selected() else {
            return;
        };
        let Some(index) = self.child_index(row) else {
            return;
        };
        if self.child(index).skipped.is_some() {
            self.note("Mount points and snapshots can't be deleted here.", true);
            return;
        }
        self.confirm = Some(index);
    }

    /// Deletes for real, then fixes totals, cursor and the header.
    fn perform_delete(&mut self, index: usize) {
        let name = self.child(index).name.to_string_lossy().into_owned();
        // Shared data stays on disk for whoever else uses it.
        let shared = self.child(index).shared;
        let row = self.list.selected().unwrap_or(0);
        let dir_path = self.current_path();

        let outcome = delete::delete(
            &mut self.root,
            &self.trail,
            &dir_path,
            index,
            &self.settings.scan,
        );

        self.refresh_order();
        self.select_row(row);
        self.fs = FsInfo::for_path(&self.root_path);

        let freed = if shared > 0 {
            self.shared_stale = true;
            format!(
                "about {} ({} was shared and stays on disk)",
                human_size(outcome.freed.saturating_sub(shared)),
                human_size(shared.min(outcome.freed))
            )
        } else {
            human_size(outcome.freed)
        };
        match outcome.error {
            None => self.note(&format!("Deleted {name} · freed {freed}"), false),
            Some(e) => self.note(
                &format!("Could not delete all of {name}: {e} · freed {freed}"),
                true,
            ),
        }
    }

    fn note(&mut self, text: &str, error: bool) {
        self.message = Some(Message {
            text: text.to_string(),
            error,
        });
    }

    /// Enter: descend into the selected directory, or go up on `..`.
    fn open_selected(&mut self) {
        let Some(row) = self.list.selected() else {
            return;
        };
        if row == 0 && self.has_parent_row() {
            self.go_up();
            return;
        }
        let Some(index) = self.child_index(row) else {
            return;
        };
        let entry = &self.current_dir().children[index];
        if entry.kind != Kind::Dir {
            return;
        }
        match &entry.skipped {
            None => {}
            // A drive we didn't scan yet: scan it now, then open it.
            Some(Skipped::OtherFs(_)) => {
                self.job = Some(Job::ScanMount(index));
                return;
            }
            Some(Skipped::Virtual(_)) => {
                self.note("Kernel filesystems hold no files to scan.", true);
                return;
            }
            Some(Skipped::Snapshots) => {
                self.note(
                    "Snapshots are not scanned: they share data with your files.",
                    true,
                );
                return;
            }
        }
        self.enter(index);
    }

    /// Opens the child folder at `index`.
    fn enter(&mut self, index: usize) {
        self.trail.push(index);
        self.filter = None;
        self.refresh_order();
        // Start on the first real entry, not on `..`.
        *self.list.offset_mut() = 0;
        self.select_row(1);
    }

    /// Scans a mounted drive that was skipped, puts it into the tree and
    /// opens it. Its size now counts toward the folders above it.
    fn scan_mount(&mut self, terminal: &mut DefaultTerminal, index: usize) -> io::Result<()> {
        let entry = self.child(index);
        let name = entry.name.clone();
        let fs_type = match &entry.skipped {
            Some(Skipped::OtherFs(usage)) => usage.fs_type.clone(),
            _ => return Ok(()),
        };
        let path = self.current_path().join(&name);

        // Stay on the drive itself; drives mounted inside it stay skipped.
        let opts = ScanOptions {
            one_filesystem: true,
            include: Vec::new(),
            extents: self.settings.scan.extents,
        };
        let mut scanned = match scanning::run(terminal, &path, opts)? {
            scanning::Outcome::Done(entry) => entry,
            scanning::Outcome::Cancelled => return Ok(()),
            scanning::Outcome::Failed(e) => {
                // Most likely unmounted in the meantime: refresh this folder.
                let shown = crate::units::shorten_home(&path);
                if path.exists() {
                    self.note(&format!("Could not scan {shown}: {e}"), true);
                } else {
                    self.job = Some(Job::Rescan {
                        reason: Some(format!("{shown} is no longer mounted")),
                    });
                }
                return Ok(());
            }
        };
        scanned.name = name;
        scanned.other_fs = Some(fs_type.clone());
        // Remember it, so `r` keeps this drive's contents.
        self.settings.scan.include.push(path.clone());
        let size = crate::units::human_size(scanned.disk);
        tree::replace_child(&mut self.root, &self.trail, index, scanned);

        self.enter(index);
        self.note(
            &format!("Scanned {} ({fs_type}) · {size}", path.display()),
            false,
        );
        Ok(())
    }

    /// Scans another place from scratch and browses it instead.
    fn open_root(
        &mut self,
        terminal: &mut DefaultTerminal,
        path: PathBuf,
        reason: Option<String>,
    ) -> io::Result<()> {
        let opts = ScanOptions {
            one_filesystem: self.settings.scan.one_filesystem,
            include: Vec::new(),
            extents: self.settings.scan.extents,
        };
        let root = match scanning::run(terminal, &path, opts.clone())? {
            scanning::Outcome::Done(root) => root,
            scanning::Outcome::Cancelled => return Ok(()), // stay where we were
            scanning::Outcome::Failed(e) => {
                let shown = crate::units::shorten_home(&path);
                self.note(&format!("Could not open {shown}: {e}"), true);
                return Ok(());
            }
        };
        self.root = root;
        self.root_path = path;
        self.settings.scan = opts;
        self.trail.clear();
        self.filter = None;
        self.marks.clear();
        self.fs = FsInfo::for_path(&self.root_path);
        self.mounts_seen = read_mounts();
        *self.list.offset_mut() = 0;
        self.refresh_order();
        self.select_row(0);
        let summary = format!(
            "Opened {} · {}",
            crate::units::shorten_home(&self.root_path),
            human_size(self.root.disk)
        );
        self.note(&with_reason(reason, summary), false);
        Ok(())
    }

    /// `r`: scans the open folder again, picking up new files and mounts.
    fn rescan(&mut self, terminal: &mut DefaultTerminal, reason: Option<String>) -> io::Result<()> {
        let path = self.current_path();
        self.shared_stale = false;
        let mut fresh = match scanning::run(terminal, &path, self.settings.scan.clone())? {
            scanning::Outcome::Done(entry) => entry,
            scanning::Outcome::Cancelled => return Ok(()),
            scanning::Outcome::Failed(e) => {
                let shown = crate::units::shorten_home(&path);
                if path.exists() {
                    self.note(&format!("Could not rescan {shown}: {e}"), true);
                } else {
                    self.recover(reason.unwrap_or_else(|| format!("{shown} is gone")));
                }
                return Ok(());
            }
        };
        let row = self.list.selected().unwrap_or(0);

        match self.trail.split_last() {
            // The scan root itself.
            None => {
                fresh.name = self.root.name.clone();
                self.root = fresh;
            }
            Some((&index, parent)) => {
                let old = self.current_dir();
                fresh.name = old.name.clone();
                fresh.other_fs = old.other_fs.clone();
                tree::replace_child(&mut self.root, parent, index, fresh);
            }
        }

        self.mounts_seen = read_mounts();
        self.fs = FsInfo::for_path(&self.root_path);
        self.refresh_order();
        self.select_row(row);
        let dir = self.current_dir();
        let summary = format!(
            "Rescanned · {} · {} items",
            human_size(dir.disk),
            crate::units::group_digits(dir.items)
        );
        self.note(&with_reason(reason, summary), false);
        Ok(())
    }

    /// The open folder no longer exists (its drive was unmounted, or it was
    /// deleted outside nudu). Climb to the nearest folder that still exists
    /// and rescan it. If even the scan root is gone, start over from its
    /// nearest existing parent.
    fn recover(&mut self, reason: String) {
        while !self.trail.is_empty() && !self.current_path().exists() {
            self.trail.pop();
        }
        self.filter = None;
        *self.list.offset_mut() = 0;
        self.refresh_order();
        self.select_row(0);
        self.job = Some(if self.current_path().exists() {
            Job::Rescan {
                reason: Some(reason),
            }
        } else {
            let path = self
                .root_path
                .ancestors()
                .find(|p| p.exists())
                .unwrap_or(std::path::Path::new("/"))
                .to_path_buf();
            Job::OpenRoot {
                path,
                reason: Some(reason),
            }
        });
    }

    /// Back to the parent directory, with the cursor on the folder we left.
    fn go_up(&mut self) {
        let Some(left) = self.trail.pop() else { return };
        self.filter = None;
        self.refresh_order();
        *self.list.offset_mut() = 0;
        let row = self.row_of(left).unwrap_or(0);
        self.select_row(row);
    }
}

/// The raw mount table, compared as a whole to notice any change.
fn read_mounts() -> String {
    std::fs::read_to_string("/proc/self/mountinfo").unwrap_or_default()
}

/// `"USB was unmounted · Rescanned · …"`, or just the summary.
fn with_reason(reason: Option<String>, summary: String) -> String {
    match reason {
        Some(reason) => format!("{reason} · {summary}"),
        None => summary,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyModifiers;

    fn file(name: &str, size: u64) -> Entry {
        Entry {
            name: name.into(),
            kind: Kind::File,
            apparent: size,
            disk: size,
            shared: 0,
            mtime: 0,
            items: 0,
            error: false,
            skipped: None,
            other_fs: None,
            types: None,
            ages: None,
            owner: Owner::None,
            estimated: false,
            children: Vec::new(),
        }
    }

    fn dir(name: &str, children: Vec<Entry>) -> Entry {
        let mut d = file(name, 0);
        d.kind = Kind::Dir;
        for c in &children {
            d.disk += c.disk;
            d.apparent += c.apparent;
            d.items += c.items + 1;
        }
        d.children = children;
        d
    }

    /// Sorted by size: Steam 60, Trash 30, notes.txt 5, steam.log 2.
    fn app() -> App {
        let root = dir(
            "root",
            vec![
                file("notes.txt", 5),
                dir("Steam", vec![file("game", 60)]),
                file("steam.log", 2),
                dir("Trash", vec![file("old", 30)]),
            ],
        );
        let settings = Settings {
            icons: false,
            read_only: true,
            scan: ScanOptions {
                one_filesystem: true,
                include: Vec::new(),
                extents: false,
            },
        };
        App::new(root, PathBuf::from("/tmp"), None, settings)
    }

    fn press(app: &mut App, code: KeyCode) {
        app.handle_event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            press(app, KeyCode::Char(c));
        }
    }

    fn shown(app: &App) -> Vec<String> {
        app.rows()
            .into_iter()
            .map(|r| match r {
                Row::Parent => "..".into(),
                Row::Entry(e) => e.name.to_string_lossy().into_owned(),
            })
            .collect()
    }

    fn selected(app: &App) -> String {
        match app.selected_row() {
            Some(Row::Entry(e)) => e.name.to_string_lossy().into_owned(),
            Some(Row::Parent) => "..".into(),
            None => String::new(),
        }
    }

    #[test]
    fn exclusive_view_counts_only_unshared_data() {
        let mut app = app();
        // Nothing shared: the view refuses to turn on.
        press(&mut app, KeyCode::Char('x'));
        assert!(!app.exclusive);

        // Steam (60) is fully shared, so Trash (30) becomes the biggest.
        app.root.children[1].shared = 60;
        app.root.shared = 60;
        press(&mut app, KeyCode::Char('x'));
        assert!(app.exclusive);
        assert_eq!(shown(&app), ["Trash", "notes.txt", "steam.log", "Steam"]);
        assert_eq!(app.size_of(&app.root), 37);

        // The cold view takes over, and x comes back to plain sizes.
        press(&mut app, KeyCode::Char('A'));
        assert!(!app.exclusive);
        press(&mut app, KeyCode::Char('x'));
        press(&mut app, KeyCode::Char('x'));
        assert!(!app.exclusive && app.cold.is_none());
        assert_eq!(shown(&app)[0], "Steam");
    }

    #[test]
    fn space_marks_and_steps_down() {
        let mut app = app();
        app.settings.read_only = false;
        press(&mut app, KeyCode::Char(' ')); // Steam
        press(&mut app, KeyCode::Char(' ')); // Trash
        assert_eq!(selected(&app), "notes.txt");
        assert!(app.is_marked(std::path::Path::new("/tmp/Steam")));
        assert_eq!(app.marked_entries().len(), 2);

        // Space on a marked entry takes the mark off again.
        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(app.marks, [PathBuf::from("/tmp/Steam")]);

        // d asks about the marked ones; another key keeps them marked.
        press(&mut app, KeyCode::Char('d'));
        assert!(app.confirm_marks && app.confirm.is_none());
        press(&mut app, KeyCode::Char('n'));
        assert!(!app.confirm_marks && !app.deleting_marks);
        assert_eq!(app.marks.len(), 1);

        // Esc drops the marks before it would quit.
        press(&mut app, KeyCode::Esc);
        assert!(app.marks.is_empty() && !app.should_quit);
    }

    #[test]
    fn marks_inside_a_marked_folder_are_not_counted_twice() {
        let mut app = app();
        app.marks = vec![
            PathBuf::from("/tmp/Steam"),
            PathBuf::from("/tmp/Steam/game"),
            PathBuf::from("/tmp/gone"),
            PathBuf::from("/tmp/notes.txt"),
        ];
        let names: Vec<_> = app
            .marked_entries()
            .into_iter()
            .map(|(_, e)| e.name.to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["Steam", "notes.txt"]);
    }

    #[test]
    fn marked_entries_are_deleted_across_folders() {
        use std::fs;
        let base = std::env::temp_dir().join(format!("nudu-marks-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("a")).unwrap();
        fs::create_dir_all(base.join("b")).unwrap();
        fs::write(base.join("a/x"), vec![0u8; 20_000]).unwrap();
        fs::write(base.join("a/y"), vec![0u8; 10_000]).unwrap();
        fs::write(base.join("b/z"), vec![0u8; 30_000]).unwrap();
        fs::write(base.join("keep"), vec![0u8; 5_000]).unwrap();

        let opts = ScanOptions {
            one_filesystem: true,
            include: Vec::new(),
            extents: false,
        };
        let root = scan::scan(&base, opts.clone(), &Default::default())
            .unwrap()
            .unwrap();
        let settings = Settings {
            icons: false,
            read_only: false,
            scan: opts,
        };
        let mut app = App::new(root, base.clone(), None, settings);

        // Open `a`, with marks in it and in another folder.
        let a = app
            .root
            .children
            .iter()
            .position(|c| c.name == "a")
            .unwrap();
        app.enter(a);
        app.marks = vec![base.join("a/x"), base.join("b")];
        let before = app.root.disk;
        let gone: u64 = app.marked_entries().iter().map(|(_, e)| e.disk).sum();
        app.perform_marked_delete();
        assert_eq!(app.current_path(), base.join("a"));
        assert_eq!(shown(&app), ["..", "y"]);
        assert!(!base.join("b").exists() && base.join("keep").exists());
        assert!(app.marks.is_empty());
        // Totals above the deleted entries shrank by what they held.
        assert_eq!(app.root.disk, before - gone);
        assert_eq!(app.root.items, 3); // a, a/y, keep

        // Deleting the open folder itself lands in its parent.
        app.marks = vec![base.join("a")];
        app.perform_marked_delete();
        assert_eq!(app.current_path(), base);
        assert_eq!(shown(&app), ["keep"]);

        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn read_only_mode_does_not_mark() {
        let mut app = app();
        press(&mut app, KeyCode::Char(' '));
        assert!(app.marks.is_empty());
    }

    #[test]
    fn typing_narrows_the_list() {
        let mut app = app();
        press(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "steam");
        assert_eq!(shown(&app), ["Steam", "steam.log"]);
        // Letters go into the query, not to the commands they usually are.
        assert!(!app.should_quit);
        assert_eq!(app.sort, Sort::default());
        assert_eq!(app.shown_size(), 62);
    }

    #[test]
    fn cursor_stays_on_a_matching_entry() {
        let mut app = app();
        press(&mut app, KeyCode::Down); // Trash
        press(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "t");
        assert_eq!(selected(&app), "Trash");
        // Trash no longer matches: the cursor goes to the first match.
        type_text(&mut app, "x");
        assert_eq!(selected(&app), "notes.txt");
    }

    #[test]
    fn enter_keeps_and_esc_clears_before_quitting() {
        let mut app = app();
        press(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "log");
        press(&mut app, KeyCode::Enter);
        assert_eq!(shown(&app), ["steam.log"]);
        // Browsing keys work again once the filter is kept.
        press(&mut app, KeyCode::Char('n'));
        assert_eq!(app.sort.key, SortKey::Name);
        press(&mut app, KeyCode::Esc);
        assert_eq!(shown(&app).len(), 4);
        assert_eq!(selected(&app), "steam.log");
        assert!(!app.should_quit);
        press(&mut app, KeyCode::Esc);
        assert!(app.should_quit);
    }

    #[test]
    fn backspace_on_empty_query_closes_it() {
        let mut app = app();
        press(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "zz");
        assert!(shown(&app).is_empty());
        press(&mut app, KeyCode::Backspace);
        press(&mut app, KeyCode::Backspace);
        assert!(app.filter.is_some());
        press(&mut app, KeyCode::Backspace);
        assert!(app.filter.is_none());
        assert_eq!(shown(&app).len(), 4);
    }

    #[test]
    fn changing_folder_drops_the_filter() {
        let mut app = app();
        press(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "Steam");
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Enter); // open Steam
        assert!(app.filter.is_none());
        assert_eq!(shown(&app), ["..", "game"]);
        press(&mut app, KeyCode::Left);
        assert_eq!(shown(&app).len(), 4);
        assert_eq!(selected(&app), "Steam");
    }
}
