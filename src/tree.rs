//! Editing the scanned tree in place.
//!
//! Folders store the summed sizes of everything below them. When one entry
//! changes (deleted, rescanned, or a drive scanned on demand), these
//! functions fix that entry's folder and every folder above it, so totals
//! stay right without rescanning the whole disk.
//!
//! A `trail` is the list of child indexes from the root down to a folder,
//! the same one app.rs keeps for navigation.

use crate::scan::Entry;

/// The folder at the end of `trail`, mutably.
pub fn folder_mut<'a>(root: &'a mut Entry, trail: &[usize]) -> &'a mut Entry {
    trail.iter().fold(root, |dir, &i| &mut dir.children[i])
}

/// Removes a child and subtracts it from every folder on the way down.
pub fn remove_child(root: &mut Entry, trail: &[usize], index: usize) -> Entry {
    let gone = folder_mut(root, trail).children.remove(index);
    adjust(root, trail, |dir| {
        dir.apparent = dir.apparent.saturating_sub(gone.apparent);
        dir.disk = dir.disk.saturating_sub(gone.disk);
        dir.items = dir.items.saturating_sub(gone.items + 1);
    });
    gone
}

/// Swaps a child for a freshly scanned version and corrects the totals.
pub fn replace_child(root: &mut Entry, trail: &[usize], index: usize, new: Entry) {
    let old = std::mem::replace(&mut folder_mut(root, trail).children[index], new);
    let new = &folder_mut(root, trail).children[index];
    let (new_apparent, new_disk, new_items) = (new.apparent, new.disk, new.items);
    adjust(root, trail, |dir| {
        dir.apparent = dir.apparent.saturating_sub(old.apparent) + new_apparent;
        dir.disk = dir.disk.saturating_sub(old.disk) + new_disk;
        dir.items = dir.items.saturating_sub(old.items) + new_items;
    });
}

/// Applies `f` to the root and to every folder along `trail`.
fn adjust(root: &mut Entry, trail: &[usize], mut f: impl FnMut(&mut Entry)) {
    let mut dir = root;
    f(dir);
    for &i in trail {
        dir = &mut dir.children[i];
        f(dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::Kind;

    fn file(name: &str, size: u64) -> Entry {
        Entry {
            name: name.into(),
            kind: Kind::File,
            apparent: size,
            disk: size,
            mtime: 0,
            items: 0,
            error: false,
            skipped: None,
            other_fs: None,
            children: Vec::new(),
        }
    }

    fn dir(name: &str, children: Vec<Entry>) -> Entry {
        let mut d = file(name, 0);
        d.kind = Kind::Dir;
        for c in &children {
            d.apparent += c.apparent;
            d.disk += c.disk;
            d.items += c.items + 1;
        }
        d.children = children;
        d
    }

    /// root/{a/{x 10, y 20}, b 5}
    fn tree() -> Entry {
        dir("root", vec![dir("a", vec![file("x", 10), file("y", 20)]), file("b", 5)])
    }

    #[test]
    fn removing_updates_every_parent() {
        let mut root = tree();
        let gone = remove_child(&mut root, &[0], 1); // a/y
        assert_eq!(gone.name, "y");
        assert_eq!(root.children[0].disk, 10);
        assert_eq!(root.children[0].items, 1);
        assert_eq!(root.disk, 15);
        assert_eq!(root.items, 3);
    }

    #[test]
    fn removing_a_folder_removes_its_contents_from_totals() {
        let mut root = tree();
        remove_child(&mut root, &[], 0); // a and everything in it
        assert_eq!(root.disk, 5);
        assert_eq!(root.items, 1);
    }

    #[test]
    fn replacing_applies_the_difference() {
        let mut root = tree();
        replace_child(&mut root, &[], 0, dir("a", vec![file("x", 10)]));
        assert_eq!(root.disk, 15);
        assert_eq!(root.items, 3);
    }
}
