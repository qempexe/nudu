//! Runs a scan in the background while showing live progress.

use std::io;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::{Duration, Instant};

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};

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

/// Scans `path` on worker threads and draws progress until it finishes.
/// Returns `Ok(None)` if the user cancelled.
pub fn run(
    terminal: &mut DefaultTerminal,
    path: &Path,
    opts: ScanOptions,
) -> io::Result<Option<Entry>> {
    let progress = Progress::default();
    let started = Instant::now();
    let display_path = crate::units::shorten_home(path);

    // A scoped thread may borrow `progress` and `path` from this function,
    // because the scope guarantees the thread ends before they do.
    thread::scope(|s| {
        let worker = s.spawn(|| scan::scan(path, opts, &progress));

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

        worker.join().expect("scan thread panicked")
    })
}
