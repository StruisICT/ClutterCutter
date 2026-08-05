// Drive-scan orchestration: the "scanning" concern lifted out of the main window
// module. begin_scan_ui flips the UI into scanning mode; scan_one dispatches the
// MFT fast path with a walker fallback; start_scan / start_scan_all kick off the
// background threads; and the WM_APP_* handlers (on_progress, on_drive_done,
// on_scan_done, finish_scan_all) fold results back into the tree and side views.
//
// This module is a child of `gui`, so `use super::*` pulls in AppState, the shared
// Win32 aliases, the WM_APP_* consts and the UI-update helpers it delegates to
// (insert_tree_item, populate_children, populate_side_*, set_status, ...).
#![allow(clippy::too_many_arguments)]

use super::*;

unsafe fn begin_scan_ui(app: &mut AppState, status_text: &str) {
    {
        let mut s = app.shared.lock().unwrap();
        *s = ScanState::default();
    }
    SendMessageW(app.list, LVM_DELETEALLITEMS, WPARAM(0), LPARAM(0));
    SendMessageW(app.tree, TVM_DELETEITEM, WPARAM(0), LPARAM(TVI_ROOT.0));
    app.root_node = None;
    app.item_by_node.clear();
    app.populated.clear();
    app.selected_node = 0;
    app.expanded.clear();
    // The old tree's items are about to be destroyed; a fresh scan is a fresh
    // navigation context, so drop the back/forward history (stale HTREEITEMs).
    app.nav_hist.clear();
    app.nav_pos = -1;
    // These point into the tree that's about to drop — clear before it does.
    app.side_hits.clear();
    // A fresh scan supersedes any in-place deletions.
    app.deleted_nodes.clear();
    app.deleted_files.clear();
    if app.side_view == SideView::TopFiles || app.side_view == SideView::OldestFiles {
        SendMessageW(app.side_list, LVM_DELETEALLITEMS, WPARAM(0), LPARAM(0));
    }
    set_status(app.status, status_text);
    app.cancel.store(false, Ordering::SeqCst);
    app.scanning = true;
    app.scan_start = Some(std::time::Instant::now());
    let _ = EnableWindow(app.stop_btn, true);
    let _ = EnableWindow(app.scan_all_btn, false);
    for b in &app.drive_buttons {
        let _ = EnableWindow(*b, false);
    }
}

// Progress callback shared by all drive scans. Posts WM_APP_PROGRESS only when
// none is already queued (coalesced via `pending`), so a fast scanner — or
// several parallel ones — can't outrun the UI thread and stall it.
fn make_progress(
    send_hwnd: SendHwnd,
    shared: Arc<Mutex<ScanState>>,
    pending: Arc<AtomicBool>,
) -> ProgressFn {
    Box::new(move |p| {
        if let Ok(mut s) = shared.lock() {
            s.last_progress = p.clone();
        }
        if pending
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            unsafe {
                let _ = PostMessageW(send_hwnd.to_hwnd(), WM_APP_PROGRESS, WPARAM(0), LPARAM(0));
            }
        }
    })
}

fn scan_one(
    path: &str,
    use_mft: bool,
    cancel: Arc<AtomicBool>,
    progress: ProgressFn,
) -> Result<FolderNode, String> {
    if use_mft {
        // Share one progress callback between the MFT attempt and the walker
        // fallback (ProgressFn is a Box and can't be cloned directly).
        let shared: Arc<ProgressFn> = Arc::new(progress);
        let p_mft: ProgressFn = {
            let s = shared.clone();
            Box::new(move |sp: &ScanProgress| (s)(sp))
        };
        match MftScanner::new()
            .with_cancel(cancel.clone())
            .with_progress(p_mft)
            .with_track_files(true)
            .scan(path)
        {
            Ok(node) => Ok(node),
            // The MFT fast path can fail on a volume that is NTFS yet has an
            // unexpected raw layout (dynamic disk, odd geometry, transient lock,
            // a removable NTFS stick that won't open raw). Rather than drop the
            // whole drive from the results, fall back to the ordinary walker —
            // unless the user cancelled, in which case honor that.
            Err(_) if !cancel.load(Ordering::Relaxed) => {
                let p_walk: ProgressFn = {
                    let s = shared.clone();
                    Box::new(move |sp: &ScanProgress| (s)(sp))
                };
                Scanner::new()
                    .with_cancel(cancel)
                    .with_progress(p_walk)
                    .with_track_files(true)
                    .scan(path)
                    .map_err(|e| e.to_string())
            }
            Err(e) => Err(e),
        }
    } else {
        Scanner::new()
            .with_cancel(cancel)
            .with_progress(progress)
            .with_track_files(true)
            .scan(path)
            .map_err(|e| e.to_string())
    }
}

pub(crate) unsafe fn start_scan(hwnd: HWND, app: &mut AppState, path: String, use_mft: bool) {
    begin_scan_ui(
        app,
        &format!(
            "Scanning {} ({})...",
            path,
            if use_mft { "MFT" } else { "walker" }
        ),
    );
    app.last_scan = Some(ScanRequest::Single(path.clone(), use_mft));

    let send_hwnd = SendHwnd(hwnd.0 as isize);
    let shared = app.shared.clone();
    let cancel = app.cancel.clone();
    let progress = make_progress(send_hwnd, shared.clone(), app.progress_pending.clone());

    std::thread::spawn(move || {
        let result = scan_one(&path, use_mft, cancel, progress);
        if let Ok(mut s) = shared.lock() {
            s.result = Some(result);
        }
        unsafe {
            let _ = PostMessageW(send_hwnd.to_hwnd(), WM_APP_DONE, WPARAM(0), LPARAM(0));
        }
    });
}

// Scans every enumerated drive on its own worker thread (volumes are
// independent, so this is safe and roughly as fast as the slowest single
// drive) and appends each into a synthetic "All drives" root as it finishes.
// The tree/list update per drive, so results appear progressively and in
// alphabetical order; the root is auto-expanded. Drives that fail are skipped.
pub(crate) unsafe fn start_scan_all(hwnd: HWND, app: &mut AppState) {
    // Sort targets alphabetically by drive letter.
    let mut targets: Vec<(String, bool)> = app
        .drives
        .iter()
        .map(|d| (d.root.clone(), d.is_ntfs && app.is_admin))
        .collect();
    targets.sort_by(|a, b| a.0.cmp(&b.0));
    if targets.is_empty() {
        return;
    }
    let n = targets.len();
    begin_scan_ui(app, &format!("Scanning {n} drives..."));
    app.last_scan = Some(ScanRequest::AllDrives);

    // Create the synthetic root now with a placeholder child per drive, so every
    // drive shows in the list immediately (each with an animated "scanning" bar)
    // and gets filled in place as its scan finishes. Capacity is exactly the
    // drive count, and finished drives replace their slot in place, so the Vec
    // never reallocates — keeping the raw child pointers the tree items hold valid.
    let mut root = FolderNode {
        full_path: String::new(), // synthetic — shell actions no-op on it
        name: "All drives".to_string(),
        ..Default::default()
    };
    root.children = Vec::with_capacity(n);
    for (path, _) in &targets {
        root.children.push(FolderNode {
            full_path: path.clone(),
            name: path.clone(),
            ..Default::default()
        });
    }
    app.root_node = Some(Box::new(root));
    let root_ptr = app.root_node.as_deref().unwrap() as *const FolderNode;
    let hti = insert_tree_item(app.tree, 0, &*root_ptr, false);
    set_tree_item_has_children(app.tree, hti);
    app.item_by_node.insert(root_ptr as isize, hti);
    app.populated.insert(hti); // drives are added by hand, not lazily
    app.selected_node = root_ptr as isize;
    SendMessageW(
        app.tree,
        TVM_SELECTITEM,
        WPARAM(TVGN_CARET as usize),
        LPARAM(hti),
    );

    // Insert a tree item for each placeholder drive and mark it pending.
    app.pending_drives.clear();
    let drive_ptrs: Vec<isize> = {
        let root = app.root_node.as_deref().unwrap();
        root.children
            .iter()
            .map(|c| c as *const FolderNode as isize)
            .collect()
    };
    for &dp in &drive_ptrs {
        let child = &*(dp as *const FolderNode);
        let dhti = insert_tree_item(app.tree, hti, child, true);
        app.item_by_node.insert(dp, dhti);
        app.pending_drives.insert(dp);
    }
    SendMessageW(
        app.tree,
        TVM_EXPAND,
        WPARAM(TVE_EXPAND.0 as usize),
        LPARAM(hti),
    );
    populate_list_folders(app, &*root_ptr);

    app.scan_all_active = true;
    app.drives_expected = n;
    app.drives_done = 0;
    app.scan_all_first_err = None;
    app.marquee_phase = 0;
    let inbox = Arc::new(Mutex::new(Vec::new()));
    app.drive_inbox = inbox.clone();
    // Animate the pending bars until every drive is in.
    SetTimer(hwnd, DRIVE_MARQUEE_TIMER, 60, None);

    let send_hwnd = SendHwnd(hwnd.0 as isize);
    for (path, use_mft) in targets {
        let inbox = inbox.clone();
        let cancel = app.cancel.clone();
        let progress = make_progress(send_hwnd, app.shared.clone(), app.progress_pending.clone());
        std::thread::spawn(move || {
            let res =
                scan_one(&path, use_mft, cancel, progress).map_err(|e| format!("{path}: {e}"));
            if let Ok(mut q) = inbox.lock() {
                q.push((path, res));
            }
            unsafe {
                let _ = PostMessageW(send_hwnd.to_hwnd(), WM_APP_DRIVE_DONE, WPARAM(0), LPARAM(0));
            }
        });
    }
}

// One or more drives finished: drain the inbox, fill each drive's placeholder
// row in place, and refresh the visible views. Runs on the UI thread.
pub(crate) unsafe fn on_drive_done(app: &mut AppState) {
    if !app.scan_all_active {
        return;
    }
    let drained: Vec<(String, Result<FolderNode, String>)> = {
        let mut q = app.drive_inbox.lock().unwrap();
        std::mem::take(&mut *q)
    };
    for (path, res) in drained {
        app.drives_done += 1;
        match res {
            Ok(node) => fill_drive(app, &path, node),
            Err(e) => {
                stop_drive_pending(app, &path);
                if app.scan_all_first_err.is_none() {
                    app.scan_all_first_err = Some(e);
                }
            }
        }
    }

    // Recompute the root totals from the (small) set of drive children.
    if let Some(root) = app.root_node.as_deref_mut() {
        let (mut size, mut fc, mut folc) = (0i64, 0i64, 0i64);
        for c in &root.children {
            size += c.size;
            fc += c.file_count;
            folc += c.folder_count + 1;
        }
        root.size = size;
        root.file_count = fc;
        root.folder_count = folc;
    }

    // Repaint the drive rows (filled numbers replace the animated bars).
    if let Some(root_ptr) = app.root_node.as_deref().map(|r| r as *const FolderNode) {
        if app.selected_node == root_ptr as isize {
            populate_list_folders(app, &*root_ptr);
        }
    }

    if app.pending_drives.is_empty() {
        finish_scan_all(app);
    } else if let Some(root) = app.root_node.as_deref() {
        set_status(
            app.status,
            &format!(
                "Scanned {}/{} drives — {} ({} files) so far...",
                app.drives_done,
                app.drives_expected,
                format_bytes(root.size),
                format_count(root.file_count),
            ),
        );
    }
}

// Replace a drive's placeholder FolderNode with its scan result, in place (the
// slot address — and thus the tree item's pointer — is preserved).
unsafe fn fill_drive(app: &mut AppState, path: &str, node: FolderNode) {
    let root = match app.root_node.as_deref_mut() {
        Some(r) => r,
        None => return,
    };
    if let Some(slot) = root.children.iter_mut().find(|c| c.full_path == path) {
        let ptr = slot as *const FolderNode as isize;
        *slot = node;
        app.pending_drives.remove(&ptr);
    }
}

// A drive failed: stop animating its row (it will show as an empty/0-byte entry
// and the status line notes the skip).
fn stop_drive_pending(app: &mut AppState, path: &str) {
    if let Some(root) = app.root_node.as_deref() {
        if let Some(slot) = root.children.iter().find(|c| c.full_path == path) {
            let ptr = slot as *const FolderNode as isize;
            app.pending_drives.remove(&ptr);
        }
    }
}

// Final housekeeping once every drive thread has reported.
unsafe fn finish_scan_all(app: &mut AppState) {
    app.scan_all_active = false;
    app.scanning = false;
    app.pending_drives.clear();
    app.tree_version = app.tree_version.wrapping_add(1); // fresh tree — drop stale side caches
    let _ = KillTimer(app.main_hwnd, DRIVE_MARQUEE_TIMER);
    let _ = EnableWindow(app.stop_btn, false);
    let _ = EnableWindow(app.scan_all_btn, true);
    for b in &app.drive_buttons {
        let _ = EnableWindow(*b, true);
    }
    // The global side views were empty during the scan; populate them now.
    match app.side_view {
        SideView::None | SideView::TempFiles => {}
        SideView::TopFiles => populate_side_top_files(app),
        SideView::OldestFiles => populate_side_oldest_files(app),
    }

    let root = match app.root_node.as_deref() {
        Some(r) => r,
        None => return,
    };
    if root.children.is_empty() {
        let msg = app
            .scan_all_first_err
            .clone()
            .unwrap_or_else(|| "no drives scanned".to_string());
        set_status(app.status, &format!("Scan failed: {msg}"));
        return;
    }
    let elapsed = app
        .scan_start
        .map(|t| format!("  ·  {:.1}s", t.elapsed().as_secs_f64()))
        .unwrap_or_default();
    let mut left = "All drives scanned".to_string();
    if let Some(err) = &app.scan_all_first_err {
        left.push_str(&format!("  [some skipped: {err}]"));
    }
    let summary = format!(
        "{}\tSize: {}  ·  Files: {}  ·  Folders: {}{}",
        left,
        format_bytes(root.size),
        format_count(root.file_count),
        format_count(root.folder_count),
        elapsed,
    );
    set_status(app.status, &summary);
}

pub(crate) fn on_progress(app: &AppState) {
    // Allow the next progress post through now that we're servicing this one.
    app.progress_pending.store(false, Ordering::Release);
    let p = {
        let s = app.shared.lock().unwrap();
        s.last_progress.clone()
    };
    let text = if p.percent < 0.0 {
        format!(
            "Scanning... {} files, {}  {}",
            format_count(p.files_scanned),
            format_bytes(p.total_size),
            p.current_path
        )
    } else {
        format!(
            "Scanning... {} files, {} ({:.1}%)",
            format_count(p.files_scanned),
            format_bytes(p.total_size),
            p.percent
        )
    };
    unsafe { set_status(app.status, &text) };
}

pub(crate) unsafe fn on_scan_done(app: &mut AppState) {
    let result = {
        let mut s = app.shared.lock().unwrap();
        s.result.take()
    };
    app.scanning = false;
    let _ = EnableWindow(app.stop_btn, false);
    let _ = EnableWindow(app.scan_all_btn, true);
    for b in &app.drive_buttons {
        let _ = EnableWindow(*b, true);
    }

    let node = match result {
        Some(Ok(n)) => n,
        Some(Err(e)) => {
            set_status(app.status, &format!("Scan failed: {e}"));
            return;
        }
        None => return,
    };

    // Two-part status: a left-aligned message and a right-aligned stats block
    // (split on a tab in status_proc). Matches the branded status strip design.
    let elapsed = app
        .scan_start
        .map(|t| format!("  ·  {:.1}s", t.elapsed().as_secs_f64()))
        .unwrap_or_default();
    let summary = format!(
        "{} scanned\tSize: {}  ·  Files: {}  ·  Folders: {}{}",
        node.name,
        format_bytes(node.size),
        format_count(node.file_count),
        format_count(node.folder_count),
        elapsed,
    );

    app.root_node = Some(Box::new(node));
    app.tree_version = app.tree_version.wrapping_add(1); // new tree — invalidate side caches
                                                         // Insert root item; lazy-populate children as the user expands.
                                                         // Use a raw pointer so we drop the &-borrow before calling &mut methods.
    let root_ptr: *const FolderNode = app
        .root_node
        .as_deref()
        .map(|r| r as *const _)
        .unwrap_or(std::ptr::null());
    if !root_ptr.is_null() {
        let root: &FolderNode = &*root_ptr;
        let hti = insert_tree_item(app.tree, 0, root, false);
        app.item_by_node.insert(root_ptr as isize, hti);
        populate_children(app, hti, root);
        SendMessageW(
            app.tree,
            TVM_SELECTITEM,
            WPARAM(TVGN_CARET as usize),
            LPARAM(hti),
        );
    }
    // The tree-selection above repopulates the main list via on_tree_select.
    // The file-ranking side views are global over the new tree; refresh them
    // directly. TempFiles is independent of drive scans entirely.
    match app.side_view {
        SideView::None | SideView::TempFiles => {}
        SideView::TopFiles => populate_side_top_files(app),
        SideView::OldestFiles => populate_side_oldest_files(app),
    }

    set_status(app.status, &summary);
}
