//! How old data is: age buckets, their colors, and short labels.
//!
//! Age is measured from the last modification time. Access times would be
//! nicer in theory, but most Linux systems mount with `relatime` or
//! `noatime`, so they are rarely accurate.

use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::style::Color;

const HOUR: i64 = 3_600;
const DAY: i64 = 24 * HOUR;
const MONTH: i64 = 30 * DAY;
const YEAR: i64 = 365 * DAY;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Bucket {
    Day,
    Week,
    Month,
    HalfYear,
    Year,
    Older,
}

/// Bytes on disk per age bucket, indexed by `Bucket as usize`.
pub type AgeTotals = [u64; Bucket::ALL.len()];

impl Bucket {
    pub const ALL: [Bucket; 6] = [
        Bucket::Day,
        Bucket::Week,
        Bucket::Month,
        Bucket::HalfYear,
        Bucket::Year,
        Bucket::Older,
    ];

    /// Which bucket something last changed at `mtime` falls in.
    pub fn of(mtime: i64) -> Bucket {
        match now().saturating_sub(mtime) {
            age if age < DAY => Bucket::Day,
            age if age < 7 * DAY => Bucket::Week,
            age if age < MONTH => Bucket::Month,
            age if age < 6 * MONTH => Bucket::HalfYear,
            age if age < YEAR => Bucket::Year,
            _ => Bucket::Older,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Bucket::Day => "today",
            Bucket::Week => "this week",
            Bucket::Month => "this month",
            Bucket::HalfYear => "< 6 months",
            Bucket::Year => "< 1 year",
            Bucket::Older => "older",
        }
    }

    /// Hot to cold, using the terminal's own palette.
    pub fn color(self) -> Color {
        match self {
            Bucket::Day => Color::Red,
            Bucket::Week => Color::Yellow,
            Bucket::Month => Color::Green,
            Bucket::HalfYear => Color::Cyan,
            Bucket::Year => Color::Blue,
            Bucket::Older => Color::Magenta,
        }
    }
}

/// How old data must be to count as cold, for the cold view (`A`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cold {
    Year,
    HalfYear,
}

impl Cold {
    /// `A` cycles: one year, then six months, then off.
    pub fn next(current: Option<Cold>) -> Option<Cold> {
        match current {
            None => Some(Cold::Year),
            Some(Cold::Year) => Some(Cold::HalfYear),
            Some(Cold::HalfYear) => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Cold::Year => "1 year",
            Cold::HalfYear => "6 months",
        }
    }

    /// Bytes in `totals` that are at least this old.
    pub fn bytes(self, totals: &AgeTotals) -> u64 {
        let from = match self {
            Cold::Year => Bucket::Older,
            Cold::HalfYear => Bucket::Year,
        };
        totals[from as usize..].iter().sum()
    }
}

/// "Now", fixed once per run so every age is measured from the same moment.
pub fn now() -> i64 {
    static NOW: OnceLock<i64> = OnceLock::new();
    *NOW.get_or_init(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    })
}

/// Compact age for the list: `now`, `5m`, `3h`, `4d`, `2mo`, `3y`.
pub fn short(mtime: i64) -> String {
    let age = now().saturating_sub(mtime).max(0);
    match age {
        a if a < 60 => "now".to_string(),
        a if a < HOUR => format!("{}m", a / 60),
        a if a < DAY => format!("{}h", a / HOUR),
        a if a < MONTH => format!("{}d", a / DAY),
        a if a < YEAR => format!("{}mo", a / MONTH),
        a => format!("{}y", a / YEAR),
    }
}

pub fn add(a: &mut AgeTotals, b: &AgeTotals) {
    for (x, y) in a.iter_mut().zip(b) {
        *x += y;
    }
}

pub fn sub(a: &mut AgeTotals, b: &AgeTotals) {
    for (x, y) in a.iter_mut().zip(b) {
        *x = x.saturating_sub(*y);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets_by_age() {
        let n = now();
        assert_eq!(Bucket::of(n - 60), Bucket::Day);
        assert_eq!(Bucket::of(n - 3 * DAY), Bucket::Week);
        assert_eq!(Bucket::of(n - 20 * DAY), Bucket::Month);
        assert_eq!(Bucket::of(n - 4 * MONTH), Bucket::HalfYear);
        assert_eq!(Bucket::of(n - 9 * MONTH), Bucket::Year);
        assert_eq!(Bucket::of(n - 3 * YEAR), Bucket::Older);
    }

    #[test]
    fn short_labels() {
        let n = now();
        assert_eq!(short(n), "now");
        assert_eq!(short(n - 120), "2m");
        assert_eq!(short(n - 5 * HOUR), "5h");
        assert_eq!(short(n - 4 * DAY), "4d");
        assert_eq!(short(n - 2 * MONTH), "2mo");
        assert_eq!(short(n - 3 * YEAR), "3y");
    }

    #[test]
    fn cold_counts_the_right_buckets() {
        let totals: AgeTotals = [1, 2, 3, 4, 5, 6];
        assert_eq!(Cold::Year.bytes(&totals), 6);
        assert_eq!(Cold::HalfYear.bytes(&totals), 11);
        assert_eq!(Cold::next(None), Some(Cold::Year));
        assert_eq!(Cold::next(Some(Cold::HalfYear)), None);
    }
}
