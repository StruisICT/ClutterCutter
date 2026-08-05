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
use windows::Win32::UI::Controls::{TVGN_CARET, TVGN_PARENT, TVM_GETNEXTITEM, TVM_SELECTITEM};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetCapture, ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, GetClientRect, GetCursorPos, GetWindowLongPtrW, GetWindowTextW, LoadCursorW,
    MoveWindow, SendMessageW, SetCursor, GWLP_USERDATA, IDC_SIZEWE, WM_CLOSE, WM_ERASEBKGND,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT, WM_SETCURSOR, WM_SIZE,
};

use crate::types::FolderNode;

use super::gdi::{card_round, fill_rect, fill_round};
use super::geometry::{delete_button_rect, nav_button_rects, pill_rect};
use super::palette::{palette, ThemeMode};
use super::{
    apply_theme, delete_selected, erase_theme_bg, layout, nav_back, nav_forward, nav_parent_hti,
    nav_up, toggle_detach, tree_item_lparam, AppState, SIDEBAR_W, SPLIT_W,
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
            let btns = nav_button_rects(&rc);
            let enabled = [
                nav_parent_hti(app) != 0,
                app.nav_pos > 0,
                app.nav_pos >= 0 && (app.nav_pos as usize) < app.nav_hist.len().saturating_sub(1),
            ];
            let glyphs = ["\u{E80F}", "\u{E72B}", "\u{E72A}"]; // Home (top level), Back, Forward
            let old = SelectObject(hdc, HGDIOBJ(app.font_icon.0));
            for (i, br) in btns.iter().enumerate() {
                card_round(hdc, br, 6, p.card_bg, p.hairline, 1);
                let col = if enabled[i] { p.text } else { p.subtext };
                SetTextColor(hdc, COLORREF(col));
                let mut g: Vec<u16> = glyphs[i].encode_utf16().collect();
                let mut grc = *br;
                DrawTextW(
                    hdc,
                    &mut g,
                    &mut grc,
                    DT_SINGLELINE | DT_VCENTER | DT_CENTER,
                );
            }

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

            // Theme toggle: a bordered pill with a sliding knob. Both glyphs sit
            // on the track; a rounded knob slides over the active side.
            let pr = pill_rect(&rc);
            let pill_r = (pr.bottom - pr.top) / 2;
            card_round(hdc, &pr, pill_r, p.track, p.subtext, 1);
            let mid = (pr.left + pr.right) / 2;
            let light_active = !app.is_dark;
            SelectObject(hdc, HGDIOBJ(app.font_icon.0));
            let mut sun: Vec<u16> = "\u{E706}".encode_utf16().collect(); // Brightness
            let mut moon: Vec<u16> = "\u{E708}".encode_utf16().collect(); // QuietHours
            let mut lrc = RECT {
                left: pr.left,
                right: mid,
                ..pr
            };
            let mut rrc = RECT {
                left: mid,
                right: pr.right,
                ..pr
            };
            SetTextColor(hdc, COLORREF(p.subtext));
            DrawTextW(
                hdc,
                &mut sun,
                &mut lrc,
                DT_SINGLELINE | DT_VCENTER | DT_CENTER,
            );
            DrawTextW(
                hdc,
                &mut moon,
                &mut rrc,
                DT_SINGLELINE | DT_VCENTER | DT_CENTER,
            );
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
            fill_round(hdc, &knob, knob_r, knob_col);
            SetTextColor(hdc, COLORREF(on_knob));
            if light_active {
                let mut s2: Vec<u16> = "\u{E706}".encode_utf16().collect();
                let mut kr = knob;
                DrawTextW(
                    hdc,
                    &mut s2,
                    &mut kr,
                    DT_SINGLELINE | DT_VCENTER | DT_CENTER,
                );
            } else {
                let mut m2: Vec<u16> = "\u{E708}".encode_utf16().collect();
                let mut kr = knob;
                DrawTextW(
                    hdc,
                    &mut m2,
                    &mut kr,
                    DT_SINGLELINE | DT_VCENTER | DT_CENTER,
                );
            }

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
            let btns = nav_button_rects(&rc);
            if hit(&btns[0]) {
                nav_up(app);
            } else if hit(&btns[1]) {
                nav_back(app);
            } else if hit(&btns[2]) {
                nav_forward(app);
            } else if hit(&delete_button_rect(&rc)) {
                delete_selected(app.main_hwnd, app);
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
