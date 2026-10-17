//! Runs a scan in the background while showing live progress.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::thread;
use std::time::{Duration, Instant};

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};

use crate::owners;
use crate::scan::{self, Entry, Progress, ScanOptions};
use crate::ui;

/// How often the progress screen redraws.
const TICK: Duration = Duration::from_millis(100);

/// A snapshot of the counters, handed to the UI for one frame.
pub struct Snapshot {
    pub path: String,
    pub current: String,
    pub items: u64,
    pub bytes: u64,
    pub errors: u64,
    pub elapsed: Duration,
}

/// How a scan ended.
pub enum Outcome {
    Done(Entry),
    /// The user pressed Esc.
    Cancelled,
    /// The scan could not start, usually because the folder is gone
    /// (a drive was unmounted). Not fatal: the browser recovers.
    Failed(io::Error),
}

/// Scans `path` on worker threads and draws progress until it finishes.
///
/// The outer `io::Result` is only for terminal errors, which are fatal.
/// Problems with the folder itself come back as `Outcome::Failed`.
pub fn run(terminal: &mut DefaultTerminal, path: &Path, opts: ScanOptions) -> io::Result<Outcome> {
    let progress = Progress::default();
    let started = Instant::now();
    let display_path = crate::units::shorten_home(path);

    // A scoped thread may borrow `progress` and `path` from this function,
    // because the scope guarantees the thread ends before they do.
    thread::scope(|s| {
        // Read the package database while the disk is being scanned.
        s.spawn(|| owners::preload(path));
        let worker = s.spawn(|| {
            let mut root = match scan::scan(path, opts, &progress) {
                Ok(Some(root)) => root,
                other => return other,
            };
            // Second, quick pass: who owns what (packages, apps, repos).
            if let Ok(mut current) = progress.current.lock() {
                *current = PathBuf::from("matching files to their owners…");
            }
            owners::assign(&mut root, path);
            Ok(Some(root))
        });

        while !worker.is_finished() {
            let snapshot = Snapshot {
                path: display_path.clone(),
                current: progress
                    .current
                    .lock()
                    .map(|p| crate::units::shorten_home(&p))
                    .unwrap_or_default(),
                items: progress.items.load(Ordering::Relaxed),
                bytes: progress.bytes.load(Ordering::Relaxed),
                errors: progress.errors.load(Ordering::Relaxed),
                elapsed: started.elapsed(),
            };
            terminal.draw(|frame| ui::draw_scanning(frame, &snapshot))?;

            // Wait for a key, but never longer than one tick.
            if event::poll(TICK)?
                && let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
                && matches!(key.code, KeyCode::Esc | KeyCode::Char('q'))
            {
                progress.cancel.store(true, Ordering::Relaxed);
            }
        }

        Ok(match worker.join().expect("scan thread panicked") {
            Ok(Some(root)) => Outcome::Done(root),
            Ok(None) => Outcome::Cancelled,
            Err(e) => Outcome::Failed(e),
        })
    })
}
