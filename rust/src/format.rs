// Pure, Win32-free formatting helpers shared by the GUI and CLI. Kept in their
// own module so they can be unit-tested without a window/message loop.

use std::sync::atomic::{AtomicBool, Ordering};

// Global unit mode read by format_bytes, so the Settings toggle doesn't have to
// be threaded through the dozens of call sites. Default true = binary (1024).
static BINARY_UNITS: AtomicBool = AtomicBool::new(true);

/// Switch byte formatting between binary (1024) and decimal (1000) divisors.
pub fn set_binary_units(binary: bool) {
    BINARY_UNITS.store(binary, Ordering::Relaxed);
}

/// Whether byte formatting currently uses binary (1024) units.
pub fn binary_units() -> bool {
    BINARY_UNITS.load(Ordering::Relaxed)
}

/// Join a directory and a leaf name with a single backslash, tolerating a
/// directory that already ends in one (e.g. a drive root `C:\`).
pub fn join_path(dir: &str, leaf: &str) -> String {
    if dir.ends_with('\\') {
        format!("{dir}{leaf}")
    } else {
        format!("{dir}\\{leaf}")
    }
}

/// Humanize a byte count: `B` under 1 KiB, then KB/MB/GB/TB/PB with 2/1/0
/// decimals as the value grows (so it stays ~3-4 significant digits wide).
/// Binary (1024) units, matching Explorer/TreeSize conventions.
pub fn format_bytes(n: i64) -> String {
    let base = if binary_units() { 1024.0 } else { 1000.0 };
    let threshold = base as i64;
    if n < threshold {
        return format!("{n} B");
    }
    let mut v = n as f64 / base;
    let units = ["KB", "MB", "GB", "TB", "PB"];
    let mut i = 0;
    while v >= base && i < units.len() - 1 {
        v /= base;
        i += 1;
    }
    if v >= 100.0 {
        format!("{v:.0} {}", units[i])
    } else if v >= 10.0 {
        format!("{v:.1} {}", units[i])
    } else {
        format!("{v:.2} {}", units[i])
    }
}

/// Group a (possibly negative) integer with thousands separators: `1234567`
/// -> `1,234,567`. Works purely on the decimal digits, no locale.
pub fn format_count(n: i64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let neg = bytes.first() == Some(&b'-');
    let digits = if neg { &bytes[1..] } else { bytes };
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    if neg {
        out.push('-');
    }
    let first_chunk = digits.len() % 3;
    if first_chunk > 0 {
        // Safe: `to_string` on an integer only ever yields ASCII bytes.
        out.push_str(std::str::from_utf8(&digits[..first_chunk]).unwrap());
    }
    for (i, c) in digits[first_chunk..].iter().enumerate() {
        if i % 3 == 0 && !(first_chunk == 0 && i == 0) {
            out.push(',');
        }
        out.push(*c as char);
    }
    out
}

/// Render an already-localized calendar time as `YYYY-MM-DD HH:MM`. The Win32
/// FILETIME -> local SYSTEMTIME conversion lives in the GUI layer; this is the
/// pure string-layout half so the format is testable.
pub fn format_ymdhm(year: u16, month: u16, day: u16, hour: u16, minute: u16) -> String {
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_path_adds_separator() {
        assert_eq!(join_path(r"C:\Users", "file.txt"), r"C:\Users\file.txt");
    }

    #[test]
    fn join_path_respects_existing_trailing_slash() {
        assert_eq!(join_path(r"C:\", "Windows"), r"C:\Windows");
        assert_eq!(join_path(r"D:\a\", "b"), r"D:\a\b");
    }

    #[test]
    fn format_bytes_below_kib_is_raw_bytes() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1), "1 B");
        assert_eq!(format_bytes(1023), "1023 B");
    }

    #[test]
    fn format_bytes_unit_boundaries() {
        assert_eq!(format_bytes(1024), "1.00 KB");
        assert_eq!(format_bytes(1024 * 1024), "1.00 MB");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.00 GB");
        assert_eq!(format_bytes(1024i64.pow(4)), "1.00 TB");
        assert_eq!(format_bytes(1024i64.pow(5)), "1.00 PB");
    }

    #[test]
    fn format_bytes_decimal_tiers() {
        // < 10 -> 2 decimals, < 100 -> 1 decimal, >= 100 -> 0 decimals.
        assert_eq!(format_bytes(1536), "1.50 KB"); // 1.5
        assert_eq!(format_bytes(1024 * 15), "15.0 KB"); // 15.0
        assert_eq!(format_bytes(1024 * 500), "500 KB"); // 500
    }

    #[test]
    fn format_bytes_caps_at_petabytes() {
        // Even absurdly large values stay in PB (no overflow into a 6th unit).
        assert!(format_bytes(i64::MAX).ends_with(" PB"));
    }

    #[test]
    fn format_count_groups_thousands() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(1), "1");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1000), "1,000");
        assert_eq!(format_count(12345), "12,345");
        assert_eq!(format_count(1234567), "1,234,567");
    }

    #[test]
    fn format_count_handles_negatives() {
        assert_eq!(format_count(-5), "-5");
        assert_eq!(format_count(-1000), "-1,000");
        assert_eq!(format_count(-1234567), "-1,234,567");
    }

    #[test]
    fn format_count_extremes() {
        assert_eq!(format_count(i64::MAX), "9,223,372,036,854,775,807");
        assert_eq!(format_count(i64::MIN), "-9,223,372,036,854,775,808");
    }

    #[test]
    fn format_ymdhm_pads_fields() {
        assert_eq!(format_ymdhm(2026, 8, 4, 9, 5), "2026-08-04 09:05");
        assert_eq!(format_ymdhm(999, 12, 31, 23, 59), "0999-12-31 23:59");
    }
}
