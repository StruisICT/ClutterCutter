// FindFirstFileExW-based recursive directory walker.
// Direct port of the `Scanner` class in ClutterCutter.cs — same FindExInfoBasic +
// LARGE_FETCH fast path, same parallel top-level fan-out, same progress throttling.

use crate::types::{FileEntry, FolderNode, ScanProgress};
use std::ffi::OsStr;
use std::iter::once;
use std::os::windows::ffi::OsStrExt;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{GetLastError, ERROR_ACCESS_DENIED, INVALID_HANDLE_VALUE};
use windows::Win32::Storage::FileSystem::{
    FindClose, FindExInfoBasic, FindExSearchNameMatch, FindFirstFileExW, FindNextFileW,
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FIND_FIRST_EX_LARGE_FETCH,
    WIN32_FIND_DATAW,
};

pub type ProgressFn = Box<dyn Fn(&ScanProgress) + Send + Sync>;

pub struct Scanner {
    cancel: Arc<AtomicBool>,
    progress: Option<Arc<ProgressFn>>,
    parallel_top_levels: i32,
    pub total_size_hint: i64,
    track_files: bool,

    total_size: AtomicI64,
    files_scanned: AtomicI64,
    last_report_ms: AtomicI64,
    start: Instant,
}

impl Default for Scanner {
    fn default() -> Self {
        Self::new()
    }
}

impl Scanner {
    pub fn new() -> Self {
        Self {
            cancel: Arc::new(AtomicBool::new(false)),
            progress: None,
            parallel_top_levels: 2,
            total_size_hint: 0,
            track_files: false,
            total_size: AtomicI64::new(0),
            files_scanned: AtomicI64::new(0),
            last_report_ms: AtomicI64::new(0),
            start: Instant::now(),
        }
    }

    pub fn with_cancel(mut self, c: Arc<AtomicBool>) -> Self {
        self.cancel = c;
        self
    }

    pub fn with_progress(mut self, p: ProgressFn) -> Self {
        self.progress = Some(Arc::new(p));
        self
    }

    pub fn with_track_files(mut self, b: bool) -> Self {
        self.track_files = b;
        self
    }

    #[allow(dead_code)] // exposed for the eventual GUI to tune fan-out at runtime
    pub fn with_parallelism(mut self, depth: i32) -> Self {
        self.parallel_top_levels = depth;
        self
    }

    pub fn scan(&self, root: &str) -> Result<FolderNode, &'static str> {
        let mut path = root.trim().to_string();
        if path.len() > 3 && path.ends_with('\\') {
            path = path.trim_end_matches('\\').to_string();
        }
        let node = self.scan_folder(&path, true, self.parallel_top_levels);
        if self.cancel.load(Ordering::Relaxed) {
            return Err("cancelled");
        }
        Ok(node)
    }

    fn scan_folder(&self, path: &str, is_root: bool, parallel_depth: i32) -> FolderNode {
        if self.cancel.load(Ordering::Relaxed) {
            return FolderNode::default();
        }

        let mut node = FolderNode {
            full_path: path.to_string(),
            name: if is_root {
                path.to_string()
            } else {
                std::path::Path::new(path)
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            },
            ..Default::default()
        };

        let find_path = if path.ends_with('\\') {
            format!("{path}*")
        } else {
            format!("{path}\\*")
        };
        let find_path = if find_path.len() > 240 {
            to_long_path(&find_path)
        } else {
            find_path
        };
        let find_path_w = wide(&find_path);

        let mut fd: WIN32_FIND_DATAW = unsafe { std::mem::zeroed() };
        let h = unsafe {
            FindFirstFileExW(
                PCWSTR(find_path_w.as_ptr()),
                FindExInfoBasic,
                &mut fd as *mut _ as *mut _,
                FindExSearchNameMatch,
                None,
                FIND_FIRST_EX_LARGE_FETCH,
            )
        };
        let h = match h {
            Ok(h) if !h.is_invalid() && h != INVALID_HANDLE_VALUE => h,
            _ => {
                let err = unsafe { GetLastError() };
                if err == ERROR_ACCESS_DENIED {
                    node.is_access_denied = true;
                }
                return node;
            }
        };

        let mut subdirs: Vec<String> = Vec::new();
        loop {
            let name = wstr_to_string(&fd.cFileName);
            if name != "." && name != ".." {
                let is_dir = (fd.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0) != 0;
                let is_reparse = (fd.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT.0) != 0;
                if is_dir {
                    if !is_reparse {
                        let child_path = if path.ends_with('\\') {
                            format!("{path}{name}")
                        } else {
                            format!("{path}\\{name}")
                        };
                        subdirs.push(child_path);
                    }
                } else {
                    let size = ((fd.nFileSizeHigh as i64) << 32) | (fd.nFileSizeLow as i64);
                    node.own_size += size;
                    node.size += size;
                    node.direct_file_count += 1;
                    node.file_count += 1;
                    self.files_scanned.fetch_add(1, Ordering::Relaxed);
                    self.total_size.fetch_add(size, Ordering::Relaxed);
                    if self.track_files {
                        let mtime = ((fd.ftLastWriteTime.dwHighDateTime as i64) << 32)
                            | (fd.ftLastWriteTime.dwLowDateTime as i64);
                        node.files.push(FileEntry {
                            name: name.clone(),
                            size,
                            last_modified_ft: mtime,
                        });
                    }
                }
            }
            let next = unsafe { FindNextFileW(h, &mut fd as *mut _ as *mut _) };
            if next.is_err() {
                break;
            }
        }
        unsafe {
            let _ = FindClose(h);
        }

        if self.cancel.load(Ordering::Relaxed) {
            return node;
        }

        if !subdirs.is_empty() {
            if parallel_depth > 0 && subdirs.len() > 1 {
                // Bounded fan-out: round-robin the subdirs across a fixed pool of
                // worker threads (≈ CPU count), each scanning its share
                // sequentially. Spawning one thread per subdir instead would
                // explode into thousands of threads on a wide tree — and on a
                // slow NAS/DAS that is thread thrash, not throughput. A fixed
                // pool also naturally throttles concurrent network I/O.
                let workers = std::thread::available_parallelism()
                    .map(|n| n.get())
                    .unwrap_or(4)
                    .clamp(1, subdirs.len());
                let mut buckets: Vec<Vec<String>> = (0..workers).map(|_| Vec::new()).collect();
                for (i, p) in subdirs.iter().enumerate() {
                    buckets[i % workers].push(p.clone());
                }
                let children: Vec<FolderNode> = std::thread::scope(|s| {
                    let handles: Vec<_> = buckets
                        .into_iter()
                        .map(|bucket| {
                            let me = self;
                            s.spawn(move || {
                                bucket
                                    .iter()
                                    .map(|p| me.scan_folder(p, false, 0))
                                    .collect::<Vec<_>>()
                            })
                        })
                        .collect();
                    handles
                        .into_iter()
                        .flat_map(|h| h.join().unwrap_or_default())
                        .collect()
                });
                for c in children {
                    node.size += c.size;
                    node.file_count += c.file_count;
                    node.folder_count += c.folder_count + 1;
                    node.children.push(c);
                }
            } else {
                for p in &subdirs {
                    if self.cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    let c = self.scan_folder(p, false, 0);
                    node.size += c.size;
                    node.file_count += c.file_count;
                    node.folder_count += c.folder_count + 1;
                    node.children.push(c);
                }
            }
        }

        self.report_progress(path);
        node
    }

    fn report_progress(&self, path: &str) {
        let progress = match &self.progress {
            Some(p) => p,
            None => return,
        };
        let now_ms = self.start.elapsed().as_millis() as i64;
        let last = self.last_report_ms.load(Ordering::Relaxed);
        // throttle to ~12 reports/sec across all threads
        if now_ms - last < 80 {
            return;
        }
        if self
            .last_report_ms
            .compare_exchange(last, now_ms, Ordering::SeqCst, Ordering::Relaxed)
            .is_err()
        {
            return;
        }
        let total = self.total_size.load(Ordering::Relaxed);
        let files = self.files_scanned.load(Ordering::Relaxed);
        let percent = if self.total_size_hint > 0 {
            let pct = 100.0 * (total as f64) / (self.total_size_hint as f64);
            pct.clamp(0.0, 99.5)
        } else {
            -1.0
        };
        progress(&ScanProgress {
            total_size: total,
            files_scanned: files,
            current_path: path.to_string(),
            percent,
        });
    }
}

pub(crate) fn wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(once(0)).collect()
}

pub(crate) fn wstr_to_string(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

fn to_long_path(p: &str) -> String {
    if p.is_empty() {
        return p.to_string();
    }
    if p.starts_with(r"\\?\") {
        return p.to_string();
    }
    if let Some(rest) = p.strip_prefix(r"\\") {
        return format!(r"\\?\UNC\{rest}");
    }
    format!(r"\\?\{p}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_is_nul_terminated() {
        let w = wide("AB");
        assert_eq!(w, vec![b'A' as u16, b'B' as u16, 0]);
    }

    #[test]
    fn wstr_to_string_stops_at_first_nul() {
        let buf = [b'H' as u16, b'i' as u16, 0, b'X' as u16];
        assert_eq!(wstr_to_string(&buf), "Hi");
        // No NUL: whole buffer is used.
        let buf2 = [b'O' as u16, b'k' as u16];
        assert_eq!(wstr_to_string(&buf2), "Ok");
    }

    #[test]
    fn to_long_path_prefixes_local_and_unc() {
        assert_eq!(to_long_path(r"C:\a\b"), r"\\?\C:\a\b");
        assert_eq!(to_long_path(r"\\server\share\f"), r"\\?\UNC\server\share\f");
        // Already-prefixed and empty are passed through unchanged.
        assert_eq!(to_long_path(r"\\?\C:\x"), r"\\?\C:\x");
        assert_eq!(to_long_path(""), "");
    }

    #[test]
    fn scan_counts_sizes_files_and_folders() {
        // Build a throwaway tree under the system temp dir.
        let base = std::env::temp_dir().join("cc_scanner_test_a1b2");
        let _ = std::fs::remove_dir_all(&base);
        let sub = base.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(base.join("a.txt"), b"12345").unwrap(); // 5 bytes
        std::fs::write(base.join("b.txt"), b"1234567890").unwrap(); // 10 bytes
        std::fs::write(sub.join("c.bin"), b"xyz").unwrap(); // 3 bytes

        let root = Scanner::new()
            .with_track_files(true)
            .scan(base.to_str().unwrap())
            .expect("scan should succeed");

        assert_eq!(root.size, 18, "total bytes across the tree");
        assert_eq!(root.file_count, 3, "all files counted recursively");
        assert_eq!(root.direct_file_count, 2, "only the 2 files in the root");
        assert_eq!(root.folder_count, 1, "one subdirectory");
        // track_files retained the two direct entries.
        let mut names: Vec<&str> = root.files.iter().map(|f| f.name.as_str()).collect();
        names.sort();
        assert_eq!(names, vec!["a.txt", "b.txt"]);

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn scan_parallel_fanout_aggregates_all_children() {
        // >1 top-level subdir + parallelism > 0 takes the bounded worker-pool
        // branch. Verify the aggregation across worker threads is correct.
        let base = std::env::temp_dir().join("cc_scanner_test_par9x");
        let _ = std::fs::remove_dir_all(&base);
        for d in ["one", "two", "three", "four"] {
            let sub = base.join(d);
            std::fs::create_dir_all(&sub).unwrap();
            std::fs::write(sub.join("f.bin"), vec![0u8; 100]).unwrap();
        }
        let root = Scanner::new()
            .with_parallelism(2)
            .scan(base.to_str().unwrap())
            .expect("scan should succeed");
        assert_eq!(root.size, 400, "4 dirs × 100 bytes");
        assert_eq!(root.file_count, 4);
        assert_eq!(root.folder_count, 4);
        assert_eq!(root.children.len(), 4);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn scan_reports_progress() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let base = std::env::temp_dir().join("cc_scanner_test_prog7");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("a"), b"hello").unwrap();

        // The reporter throttles to ~12/s, so a sub-80ms scan may emit zero
        // callbacks — we're exercising the with_progress + throttle path, not
        // asserting a specific count.
        let calls = Arc::new(AtomicUsize::new(0));
        let c2 = calls.clone();
        let root = Scanner::new()
            .with_progress(Box::new(move |_p| {
                c2.fetch_add(1, Ordering::Relaxed);
            }))
            .scan(base.to_str().unwrap())
            .unwrap();
        assert_eq!(root.size, 5);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn scan_of_missing_path_is_empty_not_error() {
        let root = Scanner::new()
            .scan(r"Z:\definitely\not\here\cc_missing")
            .expect("missing path yields an empty node, not Err");
        assert_eq!(root.size, 0);
        assert_eq!(root.file_count, 0);
    }
}
