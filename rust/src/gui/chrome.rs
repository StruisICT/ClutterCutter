// Custom-drawn chrome window procedures. Each is a self-contained WNDPROC that
// paints one piece of the branded frame; they read AppState via GWLP_USERDATA
// and the shared palette. (The larger, more AppState-coupled procs — top bar,
// sidebar, panel — still live in gui.rs for now.)

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect, ScreenToClient,
    SelectObject, SetBkMode, SetTextColor, DT_CALCRECT, DT_CENTER, DT_END_ELLIPSIS, DT_LEFT,
    DT_RIGHT, DT_SINGLELINE, DT_VCENTER, HDC, HGDIOBJ, PAINTSTRUCT, TRANSPARENT,
};
use windows::Win32::UI::Controls::{
    DRAWITEMSTRUCT, TVGN_CARET, TVGN_PARENT, TVM_GETNEXTITEM, TVM_SELECTITEM,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetCapture, ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, GetClientRect, GetCursorPos, GetWindowLongPtrW, GetWindowTextW, LoadCursorW,
    MoveWindow, PostMessageW, SendMessageW, SetCursor, GWLP_USERDATA, IDC_SIZEWE, WM_CLOSE,
    WM_COMMAND, WM_DRAWITEM, WM_ERASEBKGND, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_NOTIFY,
    WM_PAINT, WM_SETCURSOR, WM_SIZE,
};

use crate::format::format_bytes;
use crate::types::FolderNode;

use super::darkmode::{apply_theme, erase_theme_bg};
use super::gdi::{card_round, draw_pill_aa, fill_rect, fill_round};
use super::geometry::{delete_button_rect, home_button_rect, pill_rect};
use super::palette::{palette, ThemeMode};
use super::{
    apply_side_view, delete_selected, draw_flat_button, layout, nav_up, on_command, on_notify,
    paint_panel_header, panel_layout, panel_view_buttons, toggle_detach, tree_item_lparam,
    AppState, DRIVE_CARD_GAP, DRIVE_CARD_H, ID_BANNER_DISMISS, ID_BANNER_GET, ID_DRIVE_BASE,
    PANEL_VIEW_BUTTONS, SIDEBAR_W, SPLIT_W,
};

// Bottom status strip: window-bg fill, a top hairline, a dark message on the
// left and muted stats on the right (the two halves are split on a tab).
pub(crate) unsafe extern "system" fn status_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let app_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState;
    if app_ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let app = &*app_ptr;
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            let p = palette(app.is_dark);
            let brush = CreateSolidBrush(COLORREF(p.win_bg));
            FillRect(hdc, &rc, brush);
            let _ = DeleteObject(brush);
            let accent = RECT {
                bottom: rc.top + 1,
                ..rc
            };
            let accent_brush = CreateSolidBrush(COLORREF(p.hairline));
            FillRect(hdc, &accent, accent_brush);
            let _ = DeleteObject(accent_brush);
            let mut buf = [0u16; 1024];
            let len = GetWindowTextW(hwnd, &mut buf) as usize;
            if len > 0 {
                SetBkMode(hdc, TRANSPARENT);
                SelectObject(hdc, HGDIOBJ(app.font_small.0));
                // A tab separates the left message from the right-aligned stats
                // block; either part may be empty. Draw them independently.
                let tab = buf[..len].iter().position(|&c| c == b'\t' as u16);
                let (left, right): (&[u16], &[u16]) = match tab {
                    Some(i) => (&buf[..i], &buf[i + 1..len]),
                    None => (&buf[..len], &[]),
                };
                if !left.is_empty() {
                    SetTextColor(hdc, COLORREF(p.text));
                    let mut lrc = RECT {
                        left: 14,
                        top: 0,
                        right: rc.right - 8,
                        bottom: rc.bottom,
                    };
                    let mut lbuf = left.to_vec();
                    DrawTextW(
                        hdc,
                        &mut lbuf,
                        &mut lrc,
                        DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_END_ELLIPSIS,
                    );
                }
                if !right.is_empty() {
                    SetTextColor(hdc, COLORREF(p.subtext));
                    let mut rrc = RECT {
                        left: 8,
                        top: 0,
                        right: rc.right - 14,
                        bottom: rc.bottom,
                    };
                    let mut rbuf = right.to_vec();
                    DrawTextW(
                        hdc,
                        &mut rbuf,
                        &mut rrc,
                        DT_SINGLELINE | DT_VCENTER | DT_RIGHT | DT_END_ELLIPSIS,
                    );
                }
            }
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

// The drag handle between the main content and the side panel. Dragging it
// updates AppState::panel_frac and re-lays-out the window.
pub(crate) unsafe extern "system" fn splitter_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let app_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState;
    if app_ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let app = &mut *app_ptr;
    match msg {
        WM_SETCURSOR => {
            let _ = SetCursor(LoadCursorW(None, IDC_SIZEWE).unwrap_or_default());
            LRESULT(1)
        }
        WM_LBUTTONDOWN => {
            SetCapture(hwnd);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            if GetCapture() == hwnd {
                let main = app.main_hwnd;
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                let _ = ScreenToClient(main, &mut pt);
                let mut rc = RECT::default();
                let _ = GetClientRect(main, &mut rc);
                let avail = (rc.right - SIDEBAR_W).max(1);
                // The splitter's left edge tracks the cursor; the panel is
                // everything to its right (minus the splitter width).
                let panel_w = (rc.right - pt.x - SPLIT_W).clamp(180, (avail - 180).max(180));
                app.panel_frac = (panel_w as f64 / avail as f64).clamp(0.1, 0.9);
                layout(main, app);
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let _ = ReleaseCapture();
            LRESULT(0)
        }
        WM_ERASEBKGND => erase_theme_bg(app, hwnd, HDC(wparam.0 as _)),
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

// The frame of the detached ("float") side panel. Resizing keeps the reparented
// panel filling it; closing re-attaches the panel rather than destroying it.
pub(crate) unsafe extern "system" fn float_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let app_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState;
    if app_ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let app = &mut *app_ptr;
    match msg {
        WM_SIZE => {
            if app.detached {
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                let _ = MoveWindow(app.panel, 0, 0, rc.right, rc.bottom, true);
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => erase_theme_bg(app, hwnd, HDC(wparam.0 as _)),
        WM_CLOSE => {
            // Closing the frame re-attaches the panel instead of destroying it.
            toggle_detach(app.main_hwnd, app);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

// The breadcrumb bar. Paints the path from the hidden tree's caret up to the
// root (clickable segments), plus a right-aligned hint; a click selects the
// corresponding tree item.
pub(crate) unsafe extern "system" fn crumb_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let app_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState;
    if app_ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let app = &mut *app_ptr;
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            let p = palette(app.is_dark);
            let (bg, fg) = (p.card_bg, p.text);
            let b = CreateSolidBrush(COLORREF(bg));
            FillRect(hdc, &rc, b);
            let _ = DeleteObject(b);

            app.crumb_segs.clear();
            // Walk the hidden tree from the current caret up to the root.
            let caret = SendMessageW(
                app.tree,
                TVM_GETNEXTITEM,
                WPARAM(TVGN_CARET as usize),
                LPARAM(0),
            )
            .0 as isize;
            let mut chain: Vec<(String, isize)> = Vec::new();
            let mut hti = caret;
            while hti != 0 {
                let lp = tree_item_lparam(app.tree, hti);
                if lp != 0 {
                    let n = &*(lp as *const FolderNode);
                    chain.push((n.name.clone(), hti));
                }
                hti = SendMessageW(
                    app.tree,
                    TVM_GETNEXTITEM,
                    WPARAM(TVGN_PARENT as usize),
                    LPARAM(hti),
                )
                .0 as isize;
            }
            chain.reverse();

            SetBkMode(hdc, TRANSPARENT);
            SelectObject(hdc, HGDIOBJ(app.font_small.0));
            let brand = p.blue;
            let mut x = 14;
            for (i, (name, hti)) in chain.iter().enumerate() {
                if i > 0 {
                    let mut sep: Vec<u16> = "  \u{203A}  ".encode_utf16().collect();
                    let mut calc = RECT::default();
                    DrawTextW(hdc, &mut sep, &mut calc, DT_CALCRECT | DT_SINGLELINE);
                    let sw = calc.right - calc.left;
                    let mut src = RECT {
                        left: x,
                        top: 0,
                        right: x + sw,
                        bottom: rc.bottom,
                    };
                    SetTextColor(hdc, COLORREF(0x0090_9090));
                    DrawTextW(
                        hdc,
                        &mut sep,
                        &mut src,
                        DT_SINGLELINE | DT_VCENTER | DT_LEFT,
                    );
                    x += sw;
                }
                let last = i == chain.len() - 1;
                let mut seg: Vec<u16> = name.encode_utf16().collect();
                let mut calc = RECT::default();
                DrawTextW(hdc, &mut seg, &mut calc, DT_CALCRECT | DT_SINGLELINE);
                let segw = (calc.right - calc.left).max(8);
                let mut drc = RECT {
                    left: x,
                    top: 0,
                    right: x + segw,
                    bottom: rc.bottom,
                };
                SetTextColor(hdc, COLORREF(if last { fg } else { brand }));
                DrawTextW(
                    hdc,
                    &mut seg,
                    &mut drc,
                    DT_SINGLELINE | DT_VCENTER | DT_LEFT,
                );
                app.crumb_segs.push((x, x + segw, *hti));
                x += segw;
            }
            // Right-aligned muted hint, matching the mockup.
            let mut hint: Vec<u16> = "Folders (sorted by size)  ·  double-click to drill in"
                .encode_utf16()
                .collect();
            let mut hrc = RECT {
                left: x + 20,
                top: 0,
                right: rc.right - 14,
                bottom: rc.bottom,
            };
            SetTextColor(hdc, COLORREF(p.subtext));
            DrawTextW(
                hdc,
                &mut hint,
                &mut hrc,
                DT_SINGLELINE | DT_VCENTER | DT_RIGHT | DT_END_ELLIPSIS,
            );
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let target = app
                .crumb_segs
                .iter()
                .find(|(l, r, _)| x >= *l && x < *r)
                .map(|(_, _, hti)| *hti);
            if let Some(hti) = target {
                SendMessageW(
                    app.tree,
                    TVM_SELECTITEM,
                    WPARAM(TVGN_CARET as usize),
                    LPARAM(hti),
                );
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

// The "update available" banner strip below the top bar. A blue left accent +
// hairlines frame a message ("ClutterCutter X is available — winget upgrade …")
// with a blue "Get update" link and a "✕" dismiss on the right. Both hotspots
// are recorded on paint and forwarded to the main window as WM_COMMAND. Only
// shown (via layout()) when the startup check surfaced a newer version.
pub(crate) unsafe extern "system" fn banner_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let app_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState;
    if app_ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let app = &mut *app_ptr;
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            let p = palette(app.is_dark);

            // Background + a blue left accent bar + top/bottom hairlines.
            let bgb = CreateSolidBrush(COLORREF(p.card_bg));
            FillRect(hdc, &rc, bgb);
            let _ = DeleteObject(bgb);
            let r = |l, t, ri, b| RECT {
                left: l,
                top: t,
                right: ri,
                bottom: b,
            };
            fill_rect(hdc, &r(0, 0, 4, rc.bottom), p.blue);
            fill_rect(hdc, &r(0, 0, rc.right, 1), p.hairline);
            fill_rect(hdc, &r(0, rc.bottom - 1, rc.right, rc.bottom), p.hairline);

            SetBkMode(hdc, TRANSPARENT);
            SelectObject(hdc, HGDIOBJ(app.font_small.0));
            app.update_banner_hit.clear();

            // Text width helper (single line).
            let wpx = |s: &str| -> i32 {
                let mut v: Vec<u16> = s.encode_utf16().collect();
                let mut r = RECT::default();
                DrawTextW(hdc, &mut v, &mut r, DT_CALCRECT | DT_SINGLELINE);
                r.right - r.left
            };
            let draw_at = |s: &str, x: i32, w: i32, color: u32| {
                let mut v: Vec<u16> = s.encode_utf16().collect();
                let mut r = RECT {
                    left: x,
                    top: 0,
                    right: x + w,
                    bottom: rc.bottom,
                };
                SetTextColor(hdc, COLORREF(color));
                DrawTextW(
                    hdc,
                    &mut v,
                    &mut r,
                    DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_END_ELLIPSIS,
                );
            };

            // Right-aligned actions: "Get update" then a "✕", laid out from the
            // right so they never overlap the message.
            let (get, dismiss) = ("Get update", "\u{2715}");
            let (gw, dw) = (wpx(get), wpx(dismiss));
            let gap = 18;
            let dismiss_x = rc.right - 14 - dw;
            let get_x = dismiss_x - gap - gw;
            draw_at(get, get_x, gw, p.blue);
            app.update_banner_hit.push((
                RECT {
                    left: get_x - 6,
                    top: 0,
                    right: get_x + gw + 6,
                    bottom: rc.bottom,
                },
                ID_BANNER_GET as i32,
            ));
            draw_at(dismiss, dismiss_x, dw, p.subtext);
            app.update_banner_hit.push((
                RECT {
                    left: dismiss_x - 6,
                    top: 0,
                    right: dismiss_x + dw + 8,
                    bottom: rc.bottom,
                },
                ID_BANNER_DISMISS as i32,
            ));

            // Message on the left, clamped to the left of the actions.
            let msg_x = 16;
            let msg_w = (get_x - 20 - msg_x).max(40);
            let text = format!(
                "ClutterCutter {} is available  \u{2014}  winget upgrade StruisICT.ClutterCutter",
                app.update_available_version
            );
            draw_at(&text, msg_x, msg_w, p.text);

            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            for (r, id) in app.update_banner_hit.clone() {
                if x >= r.left && x < r.right && y >= r.top && y < r.bottom {
                    let _ = PostMessageW(
                        app.main_hwnd,
                        WM_COMMAND,
                        WPARAM(id as u16 as usize),
                        LPARAM(0),
                    );
                    break;
                }
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

// The branded top bar: nav buttons (Home/Back/Forward), a red Delete-selected
// button, and the sliding light/dark theme pill. Clicks route to nav_*,
// delete_selected, or apply_theme.
pub(crate) unsafe extern "system" fn topbar_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let app_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState;
    if app_ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let app = &mut *app_ptr;
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            let p = palette(app.is_dark);

            fill_rect(hdc, &rc, p.win_bg);
            // Bottom hairline.
            let hair = RECT {
                top: rc.bottom - 1,
                ..rc
            };
            fill_rect(hdc, &hair, p.hairline);

            // Navigation buttons (Home / Back / Forward) on the left. Each is a
            // rounded button with a Segoe MDL2 glyph; unavailable actions dim.
            SetBkMode(hdc, TRANSPARENT);
            // Home button (returns to the All-drives overview).
            let old = SelectObject(hdc, HGDIOBJ(app.font_icon.0));
            let home = home_button_rect(&rc);
            card_round(hdc, &home, 6, p.card_bg, p.hairline, 1);
            SetTextColor(hdc, COLORREF(p.text));
            let mut hg: Vec<u16> = "\u{E80F}".encode_utf16().collect();
            let mut hrc = home;
            DrawTextW(
                hdc,
                &mut hg,
                &mut hrc,
                DT_SINGLELINE | DT_VCENTER | DT_CENTER,
            );

            // Delete-selected button (trash glyph, red).
            let del = delete_button_rect(&rc);
            card_round(hdc, &del, 6, p.card_bg, p.hairline, 1);
            SetTextColor(hdc, COLORREF(0x0040_40E0));
            let mut dg: Vec<u16> = "\u{E74D}".encode_utf16().collect();
            let mut drc = del;
            DrawTextW(
                hdc,
                &mut dg,
                &mut drc,
                DT_SINGLELINE | DT_VCENTER | DT_CENTER,
            );

            // Theme toggle: a bordered pill with a sliding knob. The track + knob
            // are drawn anti-aliased (supersampled) for smooth edges; the inactive
            // side shows its glyph on the track, the active side shows it on the
            // knob.
            let pr = pill_rect(&rc);
            let pill_r = (pr.bottom - pr.top) / 2;
            let mid = (pr.left + pr.right) / 2;
            let light_active = !app.is_dark;
            let inset = 3;
            let knob = RECT {
                left: if light_active {
                    pr.left + inset
                } else {
                    mid + inset / 2
                },
                right: if light_active {
                    mid - inset / 2
                } else {
                    pr.right - inset
                },
                top: pr.top + inset,
                bottom: pr.bottom - inset,
            };
            let knob_r = (knob.bottom - knob.top) / 2;
            let (knob_col, on_knob) = if app.is_dark {
                (0x00E4_E4E4u32, 0x0020_2020u32) // light knob, dark glyph
            } else {
                (0x001E_1E1Eu32, 0x00FF_FFFFu32) // dark knob, white glyph
            };
            draw_pill_aa(
                hdc, &pr, pill_r, p.track, p.subtext, 1, &knob, knob_r, knob_col, p.win_bg,
            );

            // Glyphs (kept crisp at 1x): the inactive side's glyph on the track,
            // the active side's glyph on the knob.
            SelectObject(hdc, HGDIOBJ(app.font_icon.0));
            let mut sun: Vec<u16> = "\u{E706}".encode_utf16().collect(); // Brightness
            let mut moon: Vec<u16> = "\u{F0CE}".encode_utf16().collect(); // ClearNight (crescent)
            let lrc = RECT {
                left: pr.left,
                right: mid,
                ..pr
            };
            let rrc = RECT {
                left: mid,
                right: pr.right,
                ..pr
            };
            SetTextColor(hdc, COLORREF(p.subtext));
            let mut inactive_rc = if light_active { rrc } else { lrc };
            DrawTextW(
                hdc,
                if light_active { &mut moon } else { &mut sun },
                &mut inactive_rc,
                DT_SINGLELINE | DT_VCENTER | DT_CENTER,
            );
            SetTextColor(hdc, COLORREF(on_knob));
            let mut kr = knob;
            DrawTextW(
                hdc,
                if light_active { &mut sun } else { &mut moon },
                &mut kr,
                DT_SINGLELINE | DT_VCENTER | DT_CENTER,
            );

            SelectObject(hdc, old);
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            let hit = |r: &RECT| x >= r.left && x < r.right && y >= r.top && y < r.bottom;
            if hit(&home_button_rect(&rc)) {
                nav_up(app);
            } else if hit(&delete_button_rect(&rc)) {
                delete_selected(app.main_hwnd, app, false);
            } else {
                let pr = pill_rect(&rc);
                if hit(&pr) {
                    let mid = (pr.left + pr.right) / 2;
                    let mode = if x < mid {
                        ThemeMode::Light
                    } else {
                        ThemeMode::Dark
                    };
                    apply_theme(app.main_hwnd, app, mode);
                }
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

// The left DRIVES column: a "DRIVES" header, an owner-drawn usage-bar card per
// drive (active card gets a blue border), and it hosts the reparented Scan-all
// button (owner-drawn via draw_flat_button). A card click scans that drive.
pub(crate) unsafe extern "system" fn sidebar_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let app_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState;
    if app_ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let app = &mut *app_ptr;
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        // The Scan-all button lives here now; bubble its click up to the main window.
        WM_COMMAND => {
            SendMessageW(app.main_hwnd, WM_COMMAND, wparam, lparam);
            LRESULT(0)
        }
        // Flat-style Scan-all button (owner-drawn, accent primary).
        WM_DRAWITEM => {
            draw_flat_button(
                app,
                lparam.0 as *const DRAWITEMSTRUCT,
                true,
                palette(app.is_dark).win_bg,
            );
            LRESULT(1)
        }
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            let p = palette(app.is_dark);
            let b = CreateSolidBrush(COLORREF(p.win_bg));
            FillRect(hdc, &rc, b);
            let _ = DeleteObject(b);

            SetBkMode(hdc, TRANSPARENT);
            SelectObject(hdc, HGDIOBJ(app.font_small.0));
            // "DRIVES" section header, muted grey.
            let mut hdr: Vec<u16> = "DRIVES".encode_utf16().collect();
            let mut hrc = RECT {
                left: 18,
                top: 10,
                right: rc.right - 12,
                bottom: 30,
            };
            SetTextColor(hdc, COLORREF(p.subtext));
            DrawTextW(
                hdc,
                &mut hdr,
                &mut hrc,
                DT_SINGLELINE | DT_VCENTER | DT_LEFT,
            );

            let top0 = 36;
            for (i, d) in app.drives.iter().enumerate() {
                let cy = top0 + i as i32 * (DRIVE_CARD_H + DRIVE_CARD_GAP);
                let card_rc = RECT {
                    left: 12,
                    top: cy,
                    right: rc.right - 12,
                    bottom: cy + DRIVE_CARD_H,
                };
                let is_active = app.active_drive == i as i32;
                let (border, bw) = if is_active {
                    (p.blue, 2)
                } else {
                    (p.hairline, 1)
                };
                card_round(hdc, &card_rc, 10, p.card_bg, border, bw);

                // Drive glyph (Segoe MDL2 "Hard drive").
                let gx = card_rc.left + 14;
                SelectObject(hdc, HGDIOBJ(app.font_icon.0));
                let mut glyph: Vec<u16> = "\u{EDA2}".encode_utf16().collect();
                let mut grc = RECT {
                    left: gx,
                    top: cy + 8,
                    right: gx + 22,
                    bottom: cy + 30,
                };
                SetTextColor(hdc, COLORREF(p.text));
                DrawTextW(
                    hdc,
                    &mut glyph,
                    &mut grc,
                    DT_SINGLELINE | DT_VCENTER | DT_CENTER,
                );

                let lx = gx + 30;
                let name = if d.label.is_empty() {
                    format!("{}:", d.letter)
                } else {
                    format!("{}: — {}", d.letter, d.label)
                };
                let mut nw: Vec<u16> = name.encode_utf16().collect();
                SelectObject(hdc, HGDIOBJ(app.font_small.0));
                let mut nrc = RECT {
                    left: lx,
                    top: cy + 8,
                    right: card_rc.right - 12,
                    bottom: cy + 28,
                };
                SetTextColor(hdc, COLORREF(p.text));
                DrawTextW(
                    hdc,
                    &mut nw,
                    &mut nrc,
                    DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_END_ELLIPSIS,
                );

                // Usage bar (rounded, blue fill on a light track).
                let total = d.total_bytes.max(1);
                let used = total.saturating_sub(d.free_bytes);
                let frac = (used as f64 / total as f64).clamp(0.0, 1.0);
                let bl = card_rc.left + 14;
                let br_x = card_rc.right - 14;
                let bar = RECT {
                    left: bl,
                    top: cy + 32,
                    right: br_x,
                    bottom: cy + 40,
                };
                fill_round(hdc, &bar, 4, p.track);
                let fw = ((bar.right - bar.left) as f64 * frac).round() as i32;
                if fw >= 4 {
                    let fill = RECT {
                        right: bar.left + fw,
                        ..bar
                    };
                    fill_round(hdc, &fill, 4, p.blue);
                }

                // "216 GB / 238 GB · 91%".
                let usage = format!(
                    "{} / {}  ·  {:.0}%",
                    format_bytes(used as i64),
                    format_bytes(total as i64),
                    frac * 100.0
                );
                let mut uw: Vec<u16> = usage.encode_utf16().collect();
                let mut urc = RECT {
                    left: bl,
                    top: cy + 42,
                    right: card_rc.right - 12,
                    bottom: cy + 58,
                };
                SetTextColor(hdc, COLORREF(p.subtext));
                DrawTextW(
                    hdc,
                    &mut uw,
                    &mut urc,
                    DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_END_ELLIPSIS,
                );
            }
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            if app.scanning {
                return LRESULT(0);
            }
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let top0 = 32;
            let stride = DRIVE_CARD_H + DRIVE_CARD_GAP;
            if y >= top0 {
                let i = (y - top0) / stride;
                let within = (y - top0) % stride;
                if within < DRIVE_CARD_H && (i as usize) < app.drives.len() {
                    SendMessageW(
                        app.main_hwnd,
                        WM_COMMAND,
                        WPARAM((ID_DRIVE_BASE + i as u16) as usize),
                        LPARAM(0),
                    );
                }
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

// The side-panel frame: paints its header (title + view-switch toolbar +
// Detach/Recycle-all owner-draw buttons), switches views on toolbar clicks, and
// routes its children's commands/notifications to the main window's handlers.
pub(crate) unsafe extern "system" fn panel_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let app_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState;
    if app_ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let app = &mut *app_ptr;
    match msg {
        WM_SIZE => {
            panel_layout(app, hwnd);
            LRESULT(0)
        }
        WM_ERASEBKGND => erase_theme_bg(app, hwnd, HDC(wparam.0 as _)),
        WM_PAINT => {
            paint_panel_header(app, hwnd);
            LRESULT(0)
        }
        // Flat-style Detach / Recycle-all buttons (owner-drawn, secondary).
        WM_DRAWITEM => {
            draw_flat_button(
                app,
                lparam.0 as *const DRAWITEMSTRUCT,
                false,
                palette(app.is_dark).panel_bg,
            );
            LRESULT(1)
        }
        // Clicks on the view-switch toolbar buttons switch the side view.
        WM_LBUTTONDOWN => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            for (i, br) in panel_view_buttons().iter().enumerate() {
                if x >= br.left && x < br.right && y >= br.top && y < br.bottom {
                    let view = PANEL_VIEW_BUTTONS[i].0;
                    if view != app.side_view {
                        apply_side_view(app.main_hwnd, app, view);
                    }
                    break;
                }
            }
            LRESULT(0)
        }
        // The header buttons and the side list are children of the panel, so
        // their commands/notifications land here — route them to the shared
        // handlers on the main window.
        WM_COMMAND => {
            on_command(app.main_hwnd, app, (wparam.0 & 0xFFFF) as u16);
            LRESULT(0)
        }
        WM_NOTIFY => on_notify(app.main_hwnd, app, lparam),
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
