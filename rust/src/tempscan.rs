// Portable "temp / cache" location discovery for the reclaimable-space view of
// the cross-platform frontends. The Windows-only `temp` module does a deeper,
// Explorer-style enumeration; this one is a simple, conservative list of
// throwaway-cache roots that exist on the current platform, each browsable with
// the normal walker. Never returns a path that doesn't exist.

use std::collections::HashSet;
use std::path::PathBuf;

pub struct TempRoot {
    pub label: String,
    pub path: String,
}

pub fn temp_locations() -> Vec<TempRoot> {
    let mut out: Vec<TempRoot> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut add = |label: &str, path: PathBuf| {
        if path.is_dir() {
            let s = path.to_string_lossy().into_owned();
            if seen.insert(s.clone()) {
                out.push(TempRoot {
                    label: label.to_string(),
                    path: s,
                });
            }
        }
    };
    let env_path = |k: &str| std::env::var_os(k).map(PathBuf::from);

    #[cfg(windows)]
    {
        if let Some(p) = env_path("TEMP") {
            add("User Temp", p);
        }
        if let Some(la) = env_path("LOCALAPPDATA") {
            add("Local Temp", la.join("Temp"));
            add(
                "Chrome cache",
                la.join(r"Google\Chrome\User Data\Default\Cache"),
            );
            add(
                "Edge cache",
                la.join(r"Microsoft\Edge\User Data\Default\Cache"),
            );
        }
        if let Some(sd) = env_path("SystemDrive") {
            add(
                "Windows.old",
                PathBuf::from(format!("{}\\Windows.old", sd.to_string_lossy())),
            );
        }
    }
    #[cfg(target_os = "linux")]
    {
        add("/tmp", PathBuf::from("/tmp"));
        add("/var/tmp", PathBuf::from("/var/tmp"));
        if let Some(c) = env_path("XDG_CACHE_HOME") {
            add("Cache", c);
        } else if let Some(h) = env_path("HOME") {
            add("~/.cache", h.join(".cache"));
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(t) = env_path("TMPDIR") {
            add("Temp", t);
        }
        if let Some(h) = env_path("HOME") {
            add("Caches", h.join("Library/Caches"));
        }
    }

    out
}
