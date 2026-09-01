//! On-demand "Check for updates", triggered from the About window (modeled on
//! InLook). Uses WinHTTP — the OS's own HTTPS stack (Schannel) — so certificate
//! validation is Windows' and no third-party TLS/HTTP library is compiled into
//! the binary. It performs a single redirect-suppressed GET to the public
//! `releases/latest` URL, reads the `Location` header to learn the newest tag,
//! and compares it to the running version. It never downloads or runs anything —
//! it only points the user at winget or the releases page.

use std::ffi::c_void;
use std::sync::{Arc, Mutex};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Networking::WinHttp::{
    WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryHeaders,
    WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetOption,
    WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_DISABLE_REDIRECTS, WINHTTP_FLAG_SECURE,
    WINHTTP_OPTION_DISABLE_FEATURE, WINHTTP_QUERY_LOCATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    MessageBoxW, PostMessageW, IDYES, MB_ICONINFORMATION, MB_ICONWARNING, MB_OK, MB_YESNO,
    MESSAGEBOX_STYLE,
};

use crate::scanner::wide;

use super::{SendHwnd, WM_APP_UPDATE_AVAILABLE};

const HOST: PCWSTR = w!("github.com");
const PATH: PCWSTR = w!("/StruisICT/ClutterCutter/releases/latest");
const RELEASES_URL: PCWSTR = w!("https://github.com/StruisICT/ClutterCutter/releases/latest");
const HTTPS_PORT: u16 = 443;
const APP: &str = "ClutterCutter";

/// On-demand update check. Runs the network call + result dialog on a background
/// thread so the UI stays responsive, and always reports something (up to date /
/// newer available / couldn't check). Clicking "Check for updates" is itself the
/// consent for this single check — nothing is stored and nothing auto-runs.
pub(crate) fn check_now() {
    std::thread::spawn(|| {
        let current = env!("CARGO_PKG_VERSION");
        match fetch_latest_tag() {
            Some(tag) if is_newer(&tag, current) => {
                let latest = tag.trim_start_matches('v');
                let msg = format!(
                    "ClutterCutter {latest} is available (you have {current}).\n\n\
                     To update:\n\
                     \u{2022} winget:  winget upgrade StruisICT.ClutterCutter\n\
                     \u{2022} or download it from the releases page.\n\n\
                     Open the releases page now?"
                );
                if msgbox_yesno(&msg, MB_ICONINFORMATION) {
                    open_releases_page();
                }
            }
            Some(_) => {
                msgbox_ok(&format!(
                    "You're on the latest version (ClutterCutter {current})."
                ));
            }
            None => {
                if msgbox_yesno(
                    "Couldn't check for updates right now.\n\n\
                     Open the releases page to check manually?",
                    MB_ICONWARNING,
                ) {
                    open_releases_page();
                }
            }
        }
    });
}

/// Silent background update check, kicked off once at startup (gated by the
/// `check_updates_on_launch` setting). Unlike [`check_now`] it shows no dialog:
/// if a newer release exists that the user hasn't already dismissed, it stashes
/// the bare version into `shared` and wakes the UI thread with
/// `WM_APP_UPDATE_AVAILABLE`, which raises the non-modal banner. Any network
/// failure is swallowed — a background check never nags.
///
/// `CC_FAKE_UPDATE=<ver>` forces the banner (skips the network + the
/// already-seen gate) so the chrome can be screenshotted without a real release.
pub(crate) fn check_in_background(
    hwnd: HWND,
    current: &'static str,
    last_seen: String,
    shared: Arc<Mutex<Option<String>>>,
) {
    let send = SendHwnd(hwnd.0 as isize);
    std::thread::spawn(move || {
        let (tag, forced) = match std::env::var("CC_FAKE_UPDATE") {
            Ok(v) if !v.is_empty() => {
                let t = if v.starts_with('v') {
                    v
                } else {
                    format!("v{v}")
                };
                (Some(t), true)
            }
            _ => (fetch_latest_tag(), false),
        };
        let Some(tag) = tag else {
            return;
        };
        let notify = if forced {
            is_newer(&tag, current)
        } else {
            should_notify(&tag, current, &last_seen)
        };
        if notify {
            let version = tag.trim_start_matches('v').to_string();
            if let Ok(mut s) = shared.lock() {
                *s = Some(version);
            }
            unsafe {
                let _ = PostMessageW(
                    send.to_hwnd(),
                    WM_APP_UPDATE_AVAILABLE,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
        }
    });
}

/// Whether the startup check should raise the banner: `latest` must be a newer
/// version than `current` AND not a version the user already saw/dismissed
/// (tracked in the `last_update_seen` setting). Pure, so it's unit-tested.
pub(crate) fn should_notify(latest: &str, current: &str, last_seen: &str) -> bool {
    is_newer(latest, current) && latest.trim_start_matches('v') != last_seen.trim_start_matches('v')
}

unsafe fn msgbox(
    text: &str,
    flags: windows::Win32::UI::WindowsAndMessaging::MESSAGEBOX_STYLE,
) -> i32 {
    let body = wide(text);
    let title = wide(APP);
    MessageBoxW(
        HWND::default(),
        PCWSTR(body.as_ptr()),
        PCWSTR(title.as_ptr()),
        flags,
    )
    .0
}

fn msgbox_ok(text: &str) {
    unsafe {
        let _ = msgbox(text, MB_OK | MB_ICONINFORMATION);
    }
}

fn msgbox_yesno(text: &str, icon: MESSAGEBOX_STYLE) -> bool {
    unsafe { msgbox(text, MB_YESNO | icon) == IDYES.0 }
}

/// Fetch the newest release tag by asking GitHub for the `releases/latest`
/// redirect and reading the `Location` header (e.g. `.../releases/tag/v0.8.0`).
/// Returns `None` on any failure — best-effort, never surfaces network errors.
fn fetch_latest_tag() -> Option<String> {
    // RAII so every early return closes its WinHTTP handles.
    struct Handle(*mut c_void);
    impl Drop for Handle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    let _ = WinHttpCloseHandle(self.0);
                }
            }
        }
    }

    unsafe {
        let session = Handle(WinHttpOpen(
            w!("ClutterCutter-update-check"),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        ));
        if session.0.is_null() {
            return None;
        }
        let connect = Handle(WinHttpConnect(session.0, HOST, HTTPS_PORT, 0));
        if connect.0.is_null() {
            return None;
        }
        let request = Handle(WinHttpOpenRequest(
            connect.0,
            w!("GET"),
            PATH,
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        ));
        if request.0.is_null() {
            return None;
        }

        // Suppress auto-redirect so we can read the 302's Location ourselves.
        WinHttpSetOption(
            Some(request.0),
            WINHTTP_OPTION_DISABLE_FEATURE,
            Some(&WINHTTP_DISABLE_REDIRECTS.to_le_bytes()),
        )
        .ok()?;

        WinHttpSendRequest(request.0, None, None, 0, 0, 0).ok()?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut()).ok()?;

        // Read the Location header into a fixed buffer (release URLs are short).
        let mut buf = [0u16; 512];
        let mut len = (buf.len() * std::mem::size_of::<u16>()) as u32;
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_LOCATION,
            PCWSTR::null(),
            Some(buf.as_mut_ptr() as *mut c_void),
            &mut len,
            std::ptr::null_mut(),
        )
        .ok()?;

        let n = (len as usize) / std::mem::size_of::<u16>();
        let location = String::from_utf16_lossy(&buf[..n]);
        tag_from_location(&location)
    }
}

/// Extract the version tag from a `.../releases/tag/<tag>` URL. Pure so it can be
/// unit-tested without any network.
fn tag_from_location(location: &str) -> Option<String> {
    let tag = location.trim_end_matches('/').rsplit('/').next()?;
    if tag.is_empty() || !tag.starts_with('v') {
        return None;
    }
    Some(tag.to_string())
}

/// True if `latest` (a `vX.Y.Z` tag) is a newer version than `current`. Compares
/// the first three dotted numeric components; missing/garbled parts count as 0.
fn is_newer(latest: &str, current: &str) -> bool {
    fn parts(s: &str) -> [u32; 3] {
        let mut out = [0u32; 3];
        for (i, seg) in s
            .trim_start_matches('v')
            .split(['.', '-', '+'])
            .take(3)
            .enumerate()
        {
            out[i] = seg.parse().unwrap_or(0);
        }
        out
    }
    parts(latest) > parts(current)
}

/// Open the releases page in the default browser. Fixed literal URL — nothing
/// user- or network-derived reaches the shell.
pub(crate) fn open_releases_page() {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    unsafe {
        ShellExecuteW(
            HWND::default(),
            w!("open"),
            RELEASES_URL,
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{is_newer, should_notify, tag_from_location};

    #[test]
    fn extracts_tag_from_github_redirect() {
        assert_eq!(
            tag_from_location("https://github.com/StruisICT/ClutterCutter/releases/tag/v0.8.0"),
            Some("v0.8.0".to_string())
        );
        assert_eq!(
            tag_from_location("https://github.com/StruisICT/ClutterCutter/releases/tag/v1.0.0/"),
            Some("v1.0.0".to_string())
        );
    }

    #[test]
    fn rejects_unexpected_locations() {
        assert_eq!(tag_from_location("https://github.com/login"), None);
        assert_eq!(tag_from_location(""), None);
        assert_eq!(
            tag_from_location("https://github.com/StruisICT/ClutterCutter/releases"),
            None
        );
    }

    #[test]
    fn version_comparison() {
        assert!(is_newer("v0.8.0", "0.7.0"));
        assert!(is_newer("v0.7.1", "0.7.0"));
        assert!(is_newer("v1.0.0", "0.9.9"));
        assert!(!is_newer("v0.7.0", "0.7.0"));
        assert!(!is_newer("v0.6.9", "0.7.0"));
        // Garbled parts count as zero, never panic.
        assert!(!is_newer("vnope", "0.7.0"));
    }

    #[test]
    fn notify_only_for_newer_unseen_versions() {
        // Newer and never seen -> notify.
        assert!(should_notify("v0.12.0", "0.11.0", ""));
        // Newer but already dismissed (stored bare) -> stay quiet.
        assert!(!should_notify("v0.12.0", "0.11.0", "0.12.0"));
        // A tag-form last_seen also matches (both sides strip the leading v).
        assert!(!should_notify("v0.12.0", "0.11.0", "v0.12.0"));
        // Not newer -> never notify, regardless of last_seen.
        assert!(!should_notify("v0.11.0", "0.11.0", ""));
        assert!(!should_notify("v0.10.0", "0.11.0", ""));
        // Seen an older version, but an even newer one appears -> notify again.
        assert!(should_notify("v0.13.0", "0.11.0", "0.12.0"));
    }
}
