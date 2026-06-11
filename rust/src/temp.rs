// Discovery + flat enumeration of "safe-to-delete" temp/cache locations.
//
// The list is intentionally conservative: only places Windows / the user's
// installed apps treat as throwaway caches. We do NOT touch user-data
// directories. Deleting everything here is safe in the sense that the OS or
// the owning app will recreate what it needs on next use (browsers will
// repopulate caches, %TEMP% is rebuilt by installers/apps as needed,
// Windows.old is what Disk Cleanup removes when you reclaim post-upgrade
// space).
//
// The walker reuses `Scanner` with `with_track_files(true)` so we get the
// same long-path + access-denied behavior the rest of the app uses.

use crate::scanner::Scanner;
use crate::types::FolderNode;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub enum TempSource {
    UserTemp,
    WindowsTemp,
    ChromeCache,
    EdgeCache,
    FirefoxCache,
    WindowsOld,
}

impl TempSource {
    pub fn label(&self) -> &'static str {
        match self {
            TempSource::UserTemp => "User Temp",
            TempSource::WindowsTemp => "Windows Temp",
            TempSource::ChromeCache => "Chrome cache",
            TempSource::EdgeCache => "Edge cache",
            TempSource::FirefoxCache => "Firefox cache",
            TempSource::WindowsOld => "Windows.old",
        }
    }
}

pub struct TempFileEntry {
    pub full_path: String,
    pub size: i64,
    pub last_modified_ft: i64,
    pub source: TempSource,
}

// Build the list of (source, root path) pairs to scan, after deduping any
// paths that collapse to the same folder (TEMP and LOCALAPPDATA\Temp usually
// do) and dropping anything that doesn't exist.
pub fn discover_locations() -> Vec<(TempSource, String)> {
    let mut out: Vec<(TempSource, String)> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    let mut push = |src: TempSource, path: String| {
        if path.is_empty() {
            return;
        }
        // canonicalize() expands 8.3 short names and resolves symlinks, so
        // %TEMP% (which often points to C:\Users\ADMINI~1\...) and
        // %LOCALAPPDATA%\Temp (C:\Users\Administrator\...) collapse to the
        // same key. Strip the \\?\ prefix for a friendlier displayed path.
        let canon_path = match std::fs::canonicalize(&path) {
            Ok(p) => p,
            Err(_) => return,
        };
        if !canon_path.is_dir() {
            return;
        }
        let display = {
            let s = canon_path.to_string_lossy().into_owned();
            s.strip_prefix(r"\\?\")
                .map(str::to_string)
                .unwrap_or(s)
                .trim_end_matches('\\')
                .to_string()
        };
        if !seen.insert(display.to_ascii_lowercase()) {
            return;
        }
        out.push((src, display));
    };

    if let Ok(p) = std::env::var("TEMP") {
        push(TempSource::UserTemp, p);
    }
    if let Ok(lad) = std::env::var("LOCALAPPDATA") {
        push(TempSource::UserTemp, format!(r"{lad}\Temp"));
    }
    if let Ok(wd) = std::env::var("WINDIR") {
        push(TempSource::WindowsTemp, format!(r"{wd}\Temp"));
    }

    if let Ok(lad) = std::env::var("LOCALAPPDATA") {
        push(
            TempSource::ChromeCache,
            format!(r"{lad}\Google\Chrome\User Data\Default\Cache"),
        );
        push(
            TempSource::EdgeCache,
            format!(r"{lad}\Microsoft\Edge\User Data\Default\Cache"),
        );
        // Firefox profile folders are randomly named (e.g.
        // "abc123.default-release"), so enumerate Profiles\*\cache2.
        let fx_root = format!(r"{lad}\Mozilla\Firefox\Profiles");
        if let Ok(entries) = std::fs::read_dir(&fx_root) {
            for e in entries.flatten() {
                let cache2 = e.path().join("cache2");
                if cache2.is_dir() {
                    push(
                        TempSource::FirefoxCache,
                        cache2.to_string_lossy().into_owned(),
                    );
                }
            }
        }
    }

    if let Ok(sd) = std::env::var("SystemDrive") {
        push(TempSource::WindowsOld, format!(r"{sd}\Windows.old"));
    }

    out
}

// Walk every location and produce a flat, sorted-by-size list of files.
// Cancellation is honored between locations and inside the per-location
// Scanner.
pub fn scan_locations(
    locations: &[(TempSource, String)],
    cancel: Arc<AtomicBool>,
) -> Vec<TempFileEntry> {
    let mut out: Vec<TempFileEntry> = Vec::new();
    for (src, path) in locations {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let scanner = Scanner::new()
            .with_cancel(cancel.clone())
            .with_track_files(true);
        let root = match scanner.scan(path) {
            Ok(r) => r,
            Err(_) => continue, // cancelled or invalid root — skip silently
        };
        collect(&root, *src, &mut out);
    }
    out.sort_by_key(|e| std::cmp::Reverse(e.size));
    out
}

fn collect(node: &FolderNode, src: TempSource, out: &mut Vec<TempFileEntry>) {
    let sep = if node.full_path.ends_with('\\') {
        ""
    } else {
        "\\"
    };
    for f in &node.files {
        out.push(TempFileEntry {
            full_path: format!("{}{}{}", node.full_path, sep, f.name),
            size: f.size,
            last_modified_ft: f.last_modified_ft,
            source: src,
        });
    }
    for c in &node.children {
        collect(c, src, out);
    }
}
