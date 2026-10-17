//! Application state and the main event loop.

use std::io;
use std::path::PathBuf;

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::widgets::ListState;

use crate::scan::Entry;
use crate::ui;

pub struct App {
    /// The root of the scanned tree.
    pub root: Entry,
    /// The path the user asked us to scan, as typed.
    pub root_path: PathBuf,
    /// Which row is highlighted, and how far the list is scrolled.
    pub list: ListState,
    /// Rows visible in the list, updated on every draw (used by PgUp/PgDn).
    pub page_size: usize,
    /// Set to true to leave the event loop.
    pub should_quit: bool,
}

impl App {
    pub fn new(root: Entry, root_path: PathBuf) -> Self {
        let mut list = ListState::default();
        if !root.children.is_empty() {
            list.select(Some(0));
        }
        Self {
            root,
            root_path,
            list,
            page_size: 1,
            should_quit: false,
        }
    }

    /// The entries shown in the list right now.
    pub fn current(&self) -> &[Entry] {
        &self.root.children
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
            _ => {}
        }
    }

    /// Moves the selection by `delta` rows, clamped to the list bounds.
    fn move_by(&mut self, delta: isize) {
        let len = self.current().len();
        if len == 0 {
            return;
        }
        let at = self.list.selected().unwrap_or(0) as isize;
        let next = at.saturating_add(delta).clamp(0, len as isize - 1);
        self.list.select(Some(next as usize));
    }
}
