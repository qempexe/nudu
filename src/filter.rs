//! `/`: narrow the open folder down to names containing some text.
//!
//! Smart case, like most search boxes in terminal tools: an all-lowercase
//! query ignores case (`steam` finds `Steam`), a query with a capital
//! letter in it matches case exactly (`Steam` does not find `steamapps`).

/// The filter of the open folder.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    /// What the user typed after `/`.
    pub query: String,
    /// True while keys go into the query; false once `Enter` kept it.
    pub typing: bool,
}

impl Filter {
    /// True when the query hides nothing.
    pub fn is_empty(&self) -> bool {
        self.query.is_empty()
    }

    /// Does `name` pass the filter? An empty query lets everything through.
    pub fn matches(&self, name: &str) -> bool {
        self.find(name).is_some()
    }

    /// Where the query first appears in `name`, as a range of characters
    /// (not bytes), so the list can highlight it.
    pub fn find(&self, name: &str) -> Option<(usize, usize)> {
        let query: Vec<char> = self.query.chars().collect();
        if query.is_empty() {
            return Some((0, 0));
        }
        let exact = query.iter().any(|c| c.is_uppercase());
        let same = |a: char, b: char| {
            if exact {
                a == b
            } else {
                a == b || a.to_lowercase().eq(b.to_lowercase())
            }
        };
        let name: Vec<char> = name.chars().collect();
        (0..=name.len().checked_sub(query.len())?)
            .find(|&start| {
                query
                    .iter()
                    .enumerate()
                    .all(|(i, &q)| same(name[start + i], q))
            })
            .map(|start| (start, start + query.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter(query: &str) -> Filter {
        Filter {
            query: query.into(),
            typing: false,
        }
    }

    #[test]
    fn lowercase_ignores_case() {
        assert!(filter("steam").matches("Steam"));
        assert!(filter("cache").matches(".CACHE"));
        assert!(!filter("steam").matches("Trash"));
    }

    #[test]
    fn capitals_match_exactly() {
        assert!(filter("Steam").matches("Steam"));
        assert!(!filter("Steam").matches("steamapps"));
    }

    #[test]
    fn empty_query_matches_everything() {
        assert!(filter("").matches("anything"));
        assert!(filter("").is_empty());
    }

    #[test]
    fn finds_character_range() {
        assert_eq!(filter("box").find("Dropbox"), Some((4, 7)));
        // Counted in characters, not bytes: `é` is two bytes.
        assert_eq!(filter("tes").find("Vidéotest"), Some((5, 8)));
        assert_eq!(filter("longer than name").find("short"), None);
    }
}
