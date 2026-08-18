// Portable date helpers so the scan core doesn't need Win32 time APIs.
//
// The whole codebase stores timestamps as a Windows FILETIME i64 (100-ns ticks
// since 1601-01-01 UTC) — that's what the MFT/FindFirstFileEx paths produce, so
// the portable walker converts std SystemTime into the same representation and
// everything downstream (display, sorting) stays uniform across platforms.

use std::time::{SystemTime, UNIX_EPOCH};

// Seconds between 1601-01-01 and 1970-01-01, times 10^7 (100-ns ticks).
const EPOCH_DIFF_TICKS: i64 = 116_444_736_000_000_000;
// Whole days between 1601-01-01 and 1970-01-01 (11_644_473_600 / 86_400).
const EPOCH_DIFF_DAYS: i64 = 134_774;

/// Convert a `SystemTime` (e.g. a file's mtime) into a Windows FILETIME i64.
pub fn filetime_from_system_time(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => {
            EPOCH_DIFF_TICKS + (d.as_secs() as i64) * 10_000_000 + (d.subsec_nanos() as i64) / 100
        }
        Err(e) => {
            let d = e.duration();
            EPOCH_DIFF_TICKS - ((d.as_secs() as i64) * 10_000_000 + (d.subsec_nanos() as i64) / 100)
        }
    }
}

/// Break a Windows FILETIME i64 into UTC `(year, month, day, hour, min, sec)`.
/// Returns all-zero for a zero/invalid timestamp. UTC, no timezone shift —
/// callers that need local time apply the offset themselves.
pub fn ymdhms_from_filetime(ft: i64) -> (i32, u32, u32, u32, u32, u32) {
    if ft <= 0 {
        return (0, 0, 0, 0, 0, 0);
    }
    let secs_since_1601 = ft / 10_000_000;
    let days_since_1601 = secs_since_1601 / 86_400;
    let secs_of_day = secs_since_1601 % 86_400;
    let hour = (secs_of_day / 3_600) as u32;
    let min = ((secs_of_day % 3_600) / 60) as u32;
    let sec = (secs_of_day % 60) as u32;
    let (y, m, d) = civil_from_days(days_since_1601 - EPOCH_DIFF_DAYS);
    (y, m, d, hour, min, sec)
}

/// `YYYY-MM-DD` for a FILETIME, or blank spaces for a zero timestamp.
pub fn short_date(ft: i64) -> String {
    if ft <= 0 {
        return "          ".into();
    }
    let (y, m, d, _, _, _) = ymdhms_from_filetime(ft);
    format!("{y:04}-{m:02}-{d:02}")
}

// Howard Hinnant's civil-from-days: days since 1970-01-01 -> (year, month, day).
// Valid across the full proleptic Gregorian range; no lookup tables.
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { (y + 1) as i32 } else { y as i32 }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_filetime_maps_to_expected_utc() {
        // 2007-02-01 12:30:45 UTC. Unix 1170288000 == 2007-02-01 00:00:00Z.
        let secs = 1_170_288_000i64 + 12 * 3600 + 30 * 60 + 45;
        let ft = EPOCH_DIFF_TICKS + secs * 10_000_000;
        let (y, m, d, h, mi, s) = ymdhms_from_filetime(ft);
        assert_eq!((y, m, d, h, mi, s), (2007, 2, 1, 12, 30, 45));
    }

    #[test]
    fn unix_epoch_roundtrips_to_1970() {
        let ft = filetime_from_system_time(UNIX_EPOCH);
        assert_eq!(ft, EPOCH_DIFF_TICKS);
        let (y, m, d, _, _, _) = ymdhms_from_filetime(ft);
        assert_eq!((y, m, d), (1970, 1, 1));
    }

    #[test]
    fn zero_is_blank() {
        assert_eq!(short_date(0).trim(), "");
    }

    #[test]
    fn leap_day_2028() {
        // 2028-02-29 UTC.
        let secs = 1_835_395_200i64; // 2028-02-29 00:00:00 UTC
        let ft = EPOCH_DIFF_TICKS + secs * 10_000_000;
        let (y, m, d, _, _, _) = ymdhms_from_filetime(ft);
        assert_eq!((y, m, d), (2028, 2, 29));
    }
}
