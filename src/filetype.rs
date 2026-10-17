//! Rough kinds of files, by extension, for the "By type" breakdown.

use std::ffi::OsStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Video,
    Audio,
    Images,
    Documents,
    Code,
    Archives,
    Other,
}

/// Bytes on disk per category, indexed by `Category as usize`.
pub type TypeTotals = [u64; Category::ALL.len()];

impl Category {
    pub const ALL: [Category; 7] = [
        Category::Video,
        Category::Audio,
        Category::Images,
        Category::Documents,
        Category::Code,
        Category::Archives,
        Category::Other,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Category::Video => "video",
            Category::Audio => "audio",
            Category::Images => "images",
            Category::Documents => "documents",
            Category::Code => "code",
            Category::Archives => "archives",
            Category::Other => "other",
        }
    }

    /// Nerd Font icon, and a plain fallback.
    pub fn icon(self, icons: bool) -> &'static str {
        match (self, icons) {
            (Category::Video, true) => "󰕧",
            (Category::Audio, true) => "󰝚",
            (Category::Images, true) => "󰋩",
            (Category::Documents, true) => "󰈙",
            (Category::Code, true) => "󰅩",
            (Category::Archives, true) => "󰗄",
            (Category::Other, true) => "󰈔",
            (_, false) => "·",
        }
    }

    /// The category of a file, from its name.
    pub fn of(name: &OsStr) -> Category {
        let name = name.to_string_lossy();
        let Some((_, ext)) = name.rsplit_once('.') else {
            return Category::Other;
        };
        match ext.to_ascii_lowercase().as_str() {
            "mp4" | "mkv" | "avi" | "mov" | "webm" | "flv" | "wmv" | "m4v" | "mpg" | "mpeg"
            | "3gp" => Category::Video,
            "mp3" | "flac" | "wav" | "ogg" | "opus" | "m4a" | "aac" | "wma" | "aiff" | "mid" => {
                Category::Audio
            }
            "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "tif" | "tiff" | "svg" | "heic"
            | "avif" | "ico" | "psd" | "xcf" | "raw" | "cr2" | "nef" | "arw" | "dng" => {
                Category::Images
            }
            "pdf" | "doc" | "docx" | "odt" | "xls" | "xlsx" | "ods" | "ppt" | "pptx" | "odp"
            | "txt" | "md" | "epub" | "rtf" | "csv" | "tex" => Category::Documents,
            "rs" | "py" | "js" | "mjs" | "ts" | "tsx" | "jsx" | "c" | "h" | "cpp" | "hpp"
            | "cc" | "go" | "java" | "kt" | "cs" | "rb" | "php" | "html" | "css" | "scss"
            | "json" | "toml" | "yaml" | "yml" | "sh" | "lua" | "sql" | "vue" | "svelte"
            | "zig" | "swift" | "dart" | "ipynb" => Category::Code,
            "zip" | "tar" | "gz" | "tgz" | "xz" | "bz2" | "zst" | "7z" | "rar" | "iso" | "deb"
            | "rpm" | "dmg" | "appimage" | "jar" | "whl" => Category::Archives,
            _ => Category::Other,
        }
    }
}

/// Adds `b` into `a`.
pub fn add(a: &mut TypeTotals, b: &TypeTotals) {
    for (x, y) in a.iter_mut().zip(b) {
        *x += y;
    }
}

/// Subtracts `b` from `a`, never going below zero.
pub fn sub(a: &mut TypeTotals, b: &TypeTotals) {
    for (x, y) in a.iter_mut().zip(b) {
        *x = x.saturating_sub(*y);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categorizes_by_extension() {
        assert_eq!(Category::of(OsStr::new("trip.MKV")), Category::Video);
        assert_eq!(Category::of(OsStr::new("song.flac")), Category::Audio);
        assert_eq!(Category::of(OsStr::new("main.rs")), Category::Code);
        assert_eq!(
            Category::of(OsStr::new("backup.tar.gz")),
            Category::Archives
        );
        assert_eq!(Category::of(OsStr::new("Makefile")), Category::Other);
        assert_eq!(Category::of(OsStr::new(".bashrc")), Category::Other);
    }

    #[test]
    fn adds_and_subtracts() {
        let mut a: TypeTotals = [1, 2, 3, 4, 5, 6, 7];
        add(&mut a, &[1; 7]);
        assert_eq!(a, [2, 3, 4, 5, 6, 7, 8]);
        sub(&mut a, &[10; 7]);
        assert_eq!(a, [0; 7]);
    }
}
