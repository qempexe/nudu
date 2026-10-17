//! Human-readable formatting helpers.

const UNITS: [&str; 7] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];

/// Formats a byte count: `12 B`, `512.0 KiB`, `1.4 GiB`.
pub fn human_size(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// Groups digits in threes with a thin space: `412204` -> `412 204`.
pub fn group_digits(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push('\u{2009}');
        }
        out.push(ch);
    }
    out
}

/// How long ago `then` was, relative to `now` (both Unix seconds).
pub fn relative_time(now: i64, then: i64) -> String {
    let secs = now - then;
    if secs < 0 {
        return "in the future".into();
    }
    let (amount, unit) = match secs {
        0..60 => return "just now".into(),
        60..3_600 => (secs / 60, "minute"),
        3_600..86_400 => (secs / 3_600, "hour"),
        86_400..2_592_000 => (secs / 86_400, "day"),
        2_592_000..31_536_000 => (secs / 2_592_000, "month"),
        _ => (secs / 31_536_000, "year"),
    };
    let plural = if amount == 1 { "" } else { "s" };
    format!("{amount} {unit}{plural} ago")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_sizes() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(1023), "1023 B");
        assert_eq!(human_size(1024), "1.0 KiB");
        assert_eq!(human_size(1536 * 1024 * 1024), "1.5 GiB");
    }

    #[test]
    fn groups_digits() {
        assert_eq!(group_digits(7), "7");
        assert_eq!(group_digits(1000), "1\u{2009}000");
        assert_eq!(group_digits(412204), "412\u{2009}204");
    }

    #[test]
    fn relative_times() {
        assert_eq!(relative_time(100, 90), "just now");
        assert_eq!(relative_time(7_200, 0), "2 hours ago");
        assert_eq!(relative_time(86_400 * 3, 0), "3 days ago");
        assert_eq!(relative_time(31_536_000, 0), "1 year ago");
    }
}
