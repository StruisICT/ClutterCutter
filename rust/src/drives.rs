// Portable volume enumeration for the disk-usage frontends. Returns each
// mountable root with its total/free bytes so the UI can draw usage bars and
// pick a scan root. Windows lists fixed/removable drive letters; Unix lists
// real (device-backed) mount points via statvfs.

#[derive(Clone, Debug)]
pub struct DriveInfo {
    /// Root path to scan, e.g. `C:\` or `/`.
    pub path: String,
    /// Short display label, e.g. `C:` or `/home`.
    pub label: String,
    pub total: u64,
    pub free: u64,
}

impl DriveInfo {
    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.free)
    }
    /// Used fraction in `0.0..=1.0` (0 when total is unknown).
    pub fn used_fraction(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            (self.used() as f64 / self.total as f64) as f32
        }
    }
}

/// Enumerate the machine's volumes. Never panics; returns an empty vec if the
/// platform query fails.
pub fn list_drives() -> Vec<DriveInfo> {
    imp::list_drives()
}

#[cfg(windows)]
mod imp {
    use super::DriveInfo;
    use std::ffi::OsStr;
    use std::iter::once;
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{
        GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives,
    };

    // GetDriveTypeW return values (winbase.h).
    const DRIVE_REMOVABLE: u32 = 2;
    const DRIVE_FIXED: u32 = 3;

    fn wide(s: &str) -> Vec<u16> {
        OsStr::new(s).encode_wide().chain(once(0)).collect()
    }

    pub fn list_drives() -> Vec<DriveInfo> {
        let mask = unsafe { GetLogicalDrives() };
        let mut out = Vec::new();
        for i in 0..26u32 {
            if mask & (1 << i) == 0 {
                continue;
            }
            let letter = (b'A' + i as u8) as char;
            let root = format!("{letter}:\\");
            let w = wide(&root);
            let dtype = unsafe { GetDriveTypeW(PCWSTR(w.as_ptr())) };
            if dtype != DRIVE_FIXED && dtype != DRIVE_REMOVABLE {
                continue;
            }
            let mut free_avail: u64 = 0;
            let mut total: u64 = 0;
            let ok = unsafe {
                GetDiskFreeSpaceExW(
                    PCWSTR(w.as_ptr()),
                    Some(&mut free_avail),
                    Some(&mut total),
                    None,
                )
            };
            // Skip volumes we can't query (e.g. empty removable slots).
            if ok.is_err() || total == 0 {
                continue;
            }
            out.push(DriveInfo {
                path: root,
                label: format!("{letter}:"),
                total,
                free: free_avail,
            });
        }
        out
    }
}

#[cfg(not(windows))]
mod imp {
    use super::DriveInfo;

    fn statvfs(path: &str) -> Option<(u64, u64)> {
        use std::ffi::CString;
        let c = CString::new(path).ok()?;
        // SAFETY: zeroed statvfs is a valid all-fields-set target; we only read
        // scalar fields the kernel populates on success.
        let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
        let rc = unsafe { libc::statvfs(c.as_ptr(), &mut s) };
        if rc != 0 {
            return None;
        }
        let bs = if s.f_frsize != 0 {
            s.f_frsize as u64
        } else {
            s.f_bsize as u64
        };
        Some((s.f_blocks as u64 * bs, s.f_bavail as u64 * bs))
    }

    // Parse /proc/mounts for device-backed mount points; fall back to `/`.
    #[cfg(target_os = "linux")]
    fn mount_points() -> Vec<(String, String)> {
        use std::collections::HashSet;
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        if let Ok(text) = std::fs::read_to_string("/proc/mounts") {
            for line in text.lines() {
                let mut cols = line.split_whitespace();
                let dev = cols.next().unwrap_or("");
                let mnt = cols.next().unwrap_or("");
                // Only real block devices; skip pseudo/virtual filesystems.
                if !dev.starts_with("/dev/") || mnt.is_empty() {
                    continue;
                }
                let mnt = mnt.replace("\\040", " ");
                if seen.insert(mnt.clone()) {
                    let label = if mnt == "/" {
                        "/".to_string()
                    } else {
                        mnt.clone()
                    };
                    out.push((mnt, label));
                }
            }
        }
        if out.is_empty() {
            out.push(("/".to_string(), "/".to_string()));
        }
        out
    }

    #[cfg(not(target_os = "linux"))]
    fn mount_points() -> Vec<(String, String)> {
        vec![("/".to_string(), "/".to_string())]
    }

    pub fn list_drives() -> Vec<DriveInfo> {
        let mut out = Vec::new();
        for (path, label) in mount_points() {
            if let Some((total, free)) = statvfs(&path) {
                if total == 0 {
                    continue;
                }
                out.push(DriveInfo {
                    path,
                    label,
                    total,
                    free,
                });
            }
        }
        out
    }
}
