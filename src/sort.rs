//! Display order for the entries of a directory.
//!
//! Sorting never touches the scanned tree. Instead we compute an `order`:
//! a list of child indexes in the order they should appear on screen.
//! That keeps the indexes stored while navigating valid, whatever the sort.

use std::cmp::Ordering;

use crate::scan::Entry;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Size,
    Name,
    Items,
    Modified,
}

impl SortKey {
    pub fn label(self) -> &'static str {
        match self {
            SortKey::Size => "size",
            SortKey::Name => "name",
            SortKey::Items => "items",
            SortKey::Modified => "modified",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sort {
    pub key: SortKey,
    /// Flip the key's natural direction.
    pub reverse: bool,
}

impl Default for Sort {
    fn default() -> Self {
        Self {
            key: SortKey::Size,
            reverse: false,
        }
    }
}

impl Sort {
    /// Picking the active key again flips the direction; a new key starts
    /// in its natural direction.
    pub fn toggle(self, key: SortKey) -> Self {
        if self.key == key {
            Self {
                key,
                reverse: !self.reverse,
            }
        } else {
            Self {
                key,
                reverse: false,
            }
        }
    }

    /// True when the biggest / newest / last-alphabetically item is on top.
    pub fn descending(self) -> bool {
        let natural_desc = self.key != SortKey::Name;
        natural_desc != self.reverse
    }

    /// Child indexes of `entries`, in display order.
    pub fn order(self, entries: &[Entry]) -> Vec<usize> {
        let mut order: Vec<usize> = (0..entries.len()).collect();
        order.sort_by(|&a, &b| {
            let (a, b) = (&entries[a], &entries[b]);
            let primary = match self.key {
                SortKey::Size => a.disk.cmp(&b.disk),
                SortKey::Name => compare_names(a, b),
                SortKey::Items => a.items.cmp(&b.items),
                SortKey::Modified => a.mtime.cmp(&b.mtime),
            };
            let primary = if self.descending() {
                primary.reverse()
            } else {
                primary
            };
            // Ties are broken by name, so the order is always stable.
            primary.then_with(|| compare_names(a, b))
        });
        order
    }
}

/// Case-insensitive name comparison: `Music` sits between `docs` and `notes`.
fn compare_names(a: &Entry, b: &Entry) -> Ordering {
    let a = a.name.to_string_lossy();
    let b = b.name.to_string_lossy();
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| a.cmp(&b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::Kind;

    fn entry(name: &str, disk: u64, mtime: i64) -> Entry {
        Entry {
            name: name.into(),
            kind: Kind::File,
            apparent: disk,
            disk,
            mtime,
            items: 0,
            error: false,
            skipped: None,
            other_fs: None,
            children: Vec::new(),
        }
    }

    fn names(entries: &[Entry], sort: Sort) -> Vec<String> {
        sort.order(entries)
            .into_iter()
            .map(|i| entries[i].name.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn sorts_by_each_key() {
        let e = [entry("b", 10, 3), entry("C", 30, 1), entry("a", 20, 2)];
        let by = |key| Sort { key, reverse: false };
        assert_eq!(names(&e, by(SortKey::Size)), ["C", "a", "b"]);
        assert_eq!(names(&e, by(SortKey::Name)), ["a", "b", "C"]);
        assert_eq!(names(&e, by(SortKey::Modified)), ["b", "a", "C"]);
    }

    #[test]
    fn toggle_flips_then_resets() {
        let s = Sort::default().toggle(SortKey::Size);
        assert!(s.reverse);
        let s = s.toggle(SortKey::Name);
        assert_eq!(s, Sort { key: SortKey::Name, reverse: false });
    }
}
