// Portable std::fs recursive directory walker. Cross-platform counterpart to
// the Win32 `FindFirstFileExW` `Scanner` in scanner.rs — same FolderNode shape,
// same own_size / direct_file_count / access-denied / symlink-skip semantics,
// same bounded parallel top-level fan-out and throttled progress. This is what
// the Linux (and macOS) builds use; on Windows the native Scanner/MFT paths are
// faster and preferred, but this walker compiles and runs there too.

use crate::datetime::filetime_from_system_time;
use crate::types::{FileEntry, FolderNode, ScanProgress};
use std::fs;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Instant;

pub type ProgressFn = Box<dyn Fn(&ScanProgress) + Send + Sync>;

pub struct WalkScanner {
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

impl Default for WalkScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl WalkScanner {
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

    /// Set an expected total byte size so progress can report a real percentage.
    /// 0 keeps progress indeterminate.
    pub fn with_size_hint(mut self, bytes: i64) -> Self {
        self.total_size_hint = bytes;
        self
    }

    #[allow(dead_code)] // exposed so the GUI can tune fan-out at runtime
    pub fn with_parallelism(mut self, depth: i32) -> Self {
        self.parallel_top_levels = depth;
        self
    }

    pub fn scan(&self, root: &str) -> Result<FolderNode, &'static str> {
        let path = root.trim().to_string();
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

        let rd = match fs::read_dir(path) {
            Ok(rd) => rd,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::PermissionDenied {
                    node.is_access_denied = true;
                }
                return node;
            }
        };

        let mut subdirs: Vec<String> = Vec::new();
        for entry in rd.flatten() {
            // DirEntry::metadata() does NOT traverse symlinks (symlink_metadata
            // semantics) — mirrors the Win32 reparse-point skip so we never
            // double-count or loop through links.
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            let ft = meta.file_type();
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                subdirs.push(entry.path().to_string_lossy().into_owned());
            } else if ft.is_file() {
                let size = meta.len() as i64;
                node.own_size += size;
                node.size += size;
                node.direct_file_count += 1;
                node.file_count += 1;
                self.files_scanned.fetch_add(1, Ordering::Relaxed);
                self.total_size.fetch_add(size, Ordering::Relaxed);
                if self.track_files {
                    let mtime = meta.modified().map(filetime_from_system_time).unwrap_or(0);
                    node.files.push(FileEntry {
                        name: entry.file_name().to_string_lossy().into_owned(),
                        size,
                        last_modified_ft: mtime,
                    });
                }
            }
        }

        if self.cancel.load(Ordering::Relaxed) {
            return node;
        }

        if !subdirs.is_empty() {
            if parallel_depth > 0 && subdirs.len() > 1 {
                // Bounded fan-out: round-robin subdirs across ~CPU-count workers,
                // each scanning its share sequentially. A fixed pool avoids the
                // thread explosion of one-thread-per-subdir on a wide tree.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_a_temp_tree() {
        // Build a small tree under the OS temp dir and verify aggregation.
        let base = std::env::temp_dir().join(format!("cc_walk_test_{}", std::process::id()));
        let sub = base.join("sub");
        fs::create_dir_all(&sub).unwrap();
        fs::write(base.join("a.bin"), vec![0u8; 100]).unwrap();
        fs::write(sub.join("b.bin"), vec![0u8; 250]).unwrap();

        let root = WalkScanner::new()
            .with_track_files(true)
            .scan(&base.to_string_lossy())
            .unwrap();

        assert_eq!(root.size, 350);
        assert_eq!(root.own_size, 100);
        assert_eq!(root.file_count, 2);
        assert_eq!(root.direct_file_count, 1);
        assert_eq!(root.folder_count, 1);
        assert_eq!(root.children.len(), 1);
        assert_eq!(root.children[0].size, 250);

        let _ = fs::remove_dir_all(&base);
    }
}
