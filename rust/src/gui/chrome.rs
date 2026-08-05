// Custom-drawn chrome window procedures. Each is a self-contained WNDPROC that
// paints one piece of the branded frame; they read AppState via GWLP_USERDATA
// and the shared palette. (The larger, more AppState-coupled procs — top bar,
// sidebar, panel — still live in gui.rs for now.)

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect, ScreenToClient,
    SelectObject, SetBkMode, SetTextColor, DT_END_ELLIPSIS, DT_LEFT, DT_RIGHT, DT_SINGLELINE,
    DT_VCENTER, HDC, HGDIOBJ, PAINTSTRUCT, TRANSPARENT,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetCapture, ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, GetClientRect, GetCursorPos, GetWindowLongPtrW, GetWindowTextW, LoadCursorW,
    MoveWindow, SetCursor, GWLP_USERDATA, IDC_SIZEWE, WM_CLOSE, WM_ERASEBKGND, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT, WM_SETCURSOR, WM_SIZE,
};

use super::palette::palette;
use super::{erase_theme_bg, layout, toggle_detach, AppState, SIDEBAR_W, SPLIT_W};

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
