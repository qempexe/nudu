//! Application state and the main event loop.

use std::io;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::widgets::ListState;

use crate::delete;
use crate::fsinfo::FsInfo;
use crate::mounts::{Drive, MountTable};
use crate::scan::{self, Entry, Kind, MountUsage, ScanOptions, Skipped};
use crate::scanning;
use crate::tree;
use crate::units::human_size;
use crate::sort::{Sort, SortKey};
use crate::ui;

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
    /// Scan the open folder again.
    Rescan,
    /// Start over in another place, like a drive picked from the list.
    OpenRoot(PathBuf),
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
    /// Child index waiting for the user to confirm its deletion.
    pub confirm: Option<usize>,
    /// Child index being deleted right now (drawn as "deleting...").
    pub deleting: Option<usize>,
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
}

impl App {
    pub fn new(root: Entry, root_path: PathBuf, fs: Option<FsInfo>, settings: Settings) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let mut app = Self {
            fs,
            settings,
            now,
            confirm: None,
            deleting: None,
            message: None,
            job: None,
            mounts_seen: read_mounts(),
            new_drive: None,
            drives: None,
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
            if let Some(job) = self.job.take() {
                match job {
                    Job::ScanMount(index) => self.scan_mount(terminal, index)?,
                    Job::Rescan => self.rescan(terminal)?,
                    Job::OpenRoot(path) => self.open_root(terminal, path)?,
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
            self.note(
                &format!("Unmounted {} · r to rescan", crate::units::shorten_home(&point)),
                false,
            );
        }
        // If the drives popup is open, keep it current.
        if self.drives.is_some() {
            self.open_drives();
        }
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
        let keep = self.drives.as_ref().and_then(|v| v.list.selected()).unwrap_or(0);
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
            KeyCode::Down | KeyCode::Char('j') if len > 0 => view.list.select(Some((at + 1).min(len - 1))),
            KeyCode::Up | KeyCode::Char('k') => view.list.select(Some(at.saturating_sub(1))),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                if let Some((drive, _)) = view.drives.get(at) {
                    self.job = Some(Job::OpenRoot(drive.mount.point.clone()));
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
        self.message = None;
        // While a delete waits for confirmation, only `y` goes ahead.
        if let Some(index) = self.confirm.take() {
            if key.code == KeyCode::Char('y') {
                self.deleting = Some(index);
            }
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
            KeyCode::Char('d') | KeyCode::Delete => self.request_delete(),
            KeyCode::Char('r') => self.job = Some(Job::Rescan),
            KeyCode::Char('D') => self.open_drives(),
            KeyCode::Char('g') => match self.new_drive.clone() {
                Some(path) => self.job = Some(Job::OpenRoot(path)),
                None => self.note("No new drive yet · D lists all drives", false),
            },
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

    /// `d`: ask before deleting the selected entry.
    fn request_delete(&mut self) {
        if self.settings.read_only {
            self.note("Read-only mode: deleting is turned off.", true);
            return;
        }
        let Some(row) = self.list.selected() else { return };
        let Some(index) = self.child_index(row) else { return };
        if self.child(index).skipped.is_some() {
            self.note("Mount points and snapshots can't be deleted here.", true);
            return;
        }
        self.confirm = Some(index);
    }

    /// Deletes for real, then fixes totals, cursor and the header.
    fn perform_delete(&mut self, index: usize) {
        let name = self.child(index).name.to_string_lossy().into_owned();
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

        let freed = human_size(outcome.freed);
        match outcome.error {
            None => self.note(&format!("Deleted {name} · freed {freed}"), false),
            Some(e) => self.note(&format!("Could not delete all of {name}: {e} · freed {freed}"), true),
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
        let Some(row) = self.list.selected() else { return };
        if row == 0 && self.has_parent_row() {
            self.go_up();
            return;
        }
        let Some(index) = self.child_index(row) else { return };
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
                self.note("Snapshots are not scanned: they share data with your files.", true);
                return;
            }
        }
        self.enter(index);
    }

    /// Opens the child folder at `index`.
    fn enter(&mut self, index: usize) {
        self.trail.push(index);
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
        };
        let Some(mut scanned) = scanning::run(terminal, &path, opts)? else {
            return Ok(()); // cancelled
        };
        scanned.name = name;
        scanned.other_fs = Some(fs_type.clone());
        // Remember it, so `r` keeps this drive's contents.
        self.settings.scan.include.push(path.clone());
        let size = crate::units::human_size(scanned.disk);
        tree::replace_child(&mut self.root, &self.trail, index, scanned);

        self.enter(index);
        self.note(&format!("Scanned {} ({fs_type}) · {size}", path.display()), false);
        Ok(())
    }

    /// Scans another place from scratch and browses it instead.
    fn open_root(&mut self, terminal: &mut DefaultTerminal, path: PathBuf) -> io::Result<()> {
        let opts = ScanOptions {
            one_filesystem: self.settings.scan.one_filesystem,
            include: Vec::new(),
        };
        let Some(root) = scanning::run(terminal, &path, opts.clone())? else {
            return Ok(()); // cancelled: stay where we were
        };
        self.root = root;
        self.root_path = path;
        self.settings.scan = opts;
        self.trail.clear();
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
        self.note(&summary, false);
        Ok(())
    }

    /// `r`: scans the open folder again, picking up new files and mounts.
    fn rescan(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        let path = self.current_path();
        let Some(mut fresh) = scanning::run(terminal, &path, self.settings.scan.clone())? else {
            return Ok(()); // cancelled
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
        self.note(&summary, false);
        Ok(())
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

/// The raw mount table, compared as a whole to notice any change.
fn read_mounts() -> String {
    std::fs::read_to_string("/proc/self/mountinfo").unwrap_or_default()
}
