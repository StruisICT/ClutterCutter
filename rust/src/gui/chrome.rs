// Custom-drawn chrome window procedures. Each is a self-contained WNDPROC that
// paints one piece of the branded frame; they read AppState via GWLP_USERDATA
// and the shared palette. (The larger, more AppState-coupled procs — top bar,
// sidebar, panel — still live in gui.rs for now.)

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect, SelectObject,
    SetBkMode, SetTextColor, DT_END_ELLIPSIS, DT_LEFT, DT_RIGHT, DT_SINGLELINE, DT_VCENTER,
    HGDIOBJ, PAINTSTRUCT, TRANSPARENT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, GetClientRect, GetWindowLongPtrW, GetWindowTextW, GWLP_USERDATA, WM_ERASEBKGND,
    WM_PAINT,
};

use super::palette::palette;
use super::AppState;

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
