// The theme-aware "About ClutterCutter" modal window. A system TaskDialog can't
// follow the dark/light theme, so this is a small custom popup with its own
// modal message loop, painted with the shared palette.

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{BOOL, COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect, SelectObject,
    SetBkMode, SetTextColor, UpdateWindow, DT_CALCRECT, DT_LEFT, DT_SINGLELINE, HGDIOBJ,
    PAINTSTRUCT, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRect, CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyWindow,
    DispatchMessageW, DrawIconEx, GetClientRect, GetMessageW, GetWindowLongPtrW, GetWindowRect,
    IsWindow, LoadCursorW, RegisterClassExW, SetForegroundWindow, SetWindowLongPtrW, ShowWindow,
    TranslateMessage, CS_HREDRAW, CS_VREDRAW, DI_NORMAL, GWLP_USERDATA, HMENU, IDC_ARROW, MSG,
    SW_SHOW, WINDOW_EX_STYLE, WM_CLOSE, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDOWN, WM_PAINT,
    WNDCLASSEXW, WS_CAPTION, WS_POPUP, WS_SYSMENU,
};

use crate::scanner::wide;

use super::palette::palette;
use super::{load_app_icon, load_app_icon_sized, shell_exec, AppState, VK_ESCAPE, VK_RETURN};

const COFFEE_URL: &str = "https://buymeacoffee.com/struis112";
const GITHUB_URL: &str = "https://github.com/StruisICT/ClutterCutter";
const SITE_URL: &str = "https://struisict.com";

// A custom, theme-aware modal About window. Runs its own modal loop until closed.
pub(crate) unsafe fn show_about(parent: HWND, app: &mut AppState) {
    let hinstance = GetModuleHandleW(None).expect("hinst");
    let class = w!("ClutterCutterAbout");
    RegisterClassExW(&WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(about_proc),
        hInstance: hinstance.into(),
        hIcon: load_app_icon(),
        hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
        hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH::default(),
        lpszClassName: class,
        ..Default::default()
    });
    // (w, h) is the desired client area; grow the window so the title bar doesn't
    // clip the content (matches the Settings window).
    let style = WS_POPUP | WS_CAPTION | WS_SYSMENU;
    let mut wr = RECT {
        left: 0,
        top: 0,
        right: 460,
        bottom: 336,
    };
    let _ = AdjustWindowRect(&mut wr, style, false);
    let (w, h) = (wr.right - wr.left, wr.bottom - wr.top);
    let mut pr = RECT::default();
    let _ = GetWindowRect(parent, &mut pr);
    let x = pr.left + ((pr.right - pr.left) - w) / 2;
    let y = pr.top + ((pr.bottom - pr.top) - h) / 2;
    let title = wide("About ClutterCutter");
    let hwnd = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        class,
        PCWSTR(title.as_ptr()),
        style,
        x,
        y,
        w,
        h,
        parent,
        HMENU::default(),
        hinstance,
        None,
    )
    .expect("about window");
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, app as *mut AppState as isize);
    // Dark title bar to match the theme.
    let use_dark = BOOL(if app.is_dark { 1 } else { 0 });
    let _ = DwmSetWindowAttribute(
        hwnd,
        DWMWA_USE_IMMERSIVE_DARK_MODE,
        &use_dark as *const _ as *const _,
        std::mem::size_of::<BOOL>() as u32,
    );
    let _ = ShowWindow(hwnd, SW_SHOW);
    let _ = UpdateWindow(hwnd);

    // Modal: disable the parent and pump messages until the window is gone.
    let _ = EnableWindow(parent, false);
    let mut msg = MSG::default();
    while IsWindow(hwnd).as_bool() {
        if GetMessageW(&mut msg, None, 0, 0).0 <= 0 {
            break;
        }
        let _ = TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
    let _ = EnableWindow(parent, true);
    let _ = SetForegroundWindow(parent);
}

// Draws `s` left-aligned at (x, y) and returns the tight rect it occupies.
unsafe fn about_text(hdc: windows::Win32::Graphics::Gdi::HDC, s: &str, x: i32, y: i32) -> RECT {
    let mut v: Vec<u16> = s.encode_utf16().collect();
    let mut calc = RECT::default();
    DrawTextW(hdc, &mut v, &mut calc, DT_CALCRECT | DT_SINGLELINE);
    let tw = calc.right - calc.left;
    let th = calc.bottom - calc.top;
    let mut r = RECT {
        left: x,
        top: y,
        right: x + tw,
        bottom: y + th,
    };
    DrawTextW(hdc, &mut v, &mut r, DT_SINGLELINE | DT_LEFT);
    r
}

// Centered variant; returns the drawn rect.
unsafe fn about_center(hdc: windows::Win32::Graphics::Gdi::HDC, s: &str, cw: i32, y: i32) -> RECT {
    let mut v: Vec<u16> = s.encode_utf16().collect();
    let mut calc = RECT::default();
    DrawTextW(hdc, &mut v, &mut calc, DT_CALCRECT | DT_SINGLELINE);
    about_text(hdc, s, (cw - (calc.right - calc.left)) / 2, y)
}

unsafe fn paint_about(hwnd: HWND, app_ptr: *mut AppState) {
    if app_ptr.is_null() {
        return;
    }
    let app = &mut *app_ptr;
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    let p = palette(app.is_dark);
    let cw = rc.right;
    let bg = CreateSolidBrush(COLORREF(p.card_bg));
    FillRect(hdc, &rc, bg);
    let _ = DeleteObject(bg);
    SetBkMode(hdc, TRANSPARENT);
    app.about_hit.clear();

    // Load the icon at the exact 48px draw size so it stays crisp (LoadIconW's
    // 32px default would be upscaled and look distorted). The sized icon is owned
    // and freed after drawing; the shared fallback must not be destroyed.
    let sized = load_app_icon_sized(48);
    let icon = if sized.is_invalid() {
        load_app_icon()
    } else {
        sized
    };
    let _ = DrawIconEx(hdc, (cw - 48) / 2, 22, icon, 48, 48, 0, None, DI_NORMAL);
    if !sized.is_invalid() {
        let _ = DestroyIcon(sized);
    }

    let old = SelectObject(hdc, HGDIOBJ(app.font_title.0));
    SetTextColor(hdc, COLORREF(p.text));
    about_center(hdc, "ClutterCutter", cw, 80);

    SelectObject(hdc, HGDIOBJ(app.font_small.0));
    SetTextColor(hdc, COLORREF(p.subtext));
    about_center(
        hdc,
        &format!(
            "Version {} \u{2014} Free Software from Struis ICT",
            env!("CARGO_PKG_VERSION")
        ),
        cw,
        114,
    );
    SetTextColor(hdc, COLORREF(p.text));
    about_center(hdc, "Lightweight Windows disk-usage browser.", cw, 144);
    about_center(hdc, "FindFirstFileEx walker + NTFS MFT fast path.", cw, 164);

    // Buy-me-a-coffee link.
    SetTextColor(hdc, COLORREF(p.blue));
    let coffee = about_center(hdc, "\u{2615} Buy me a coffee", cw, 198);
    app.about_hit.push((coffee, 0));

    // "GitHub  \u{00b7}  struisict.com" row, centred, with two links.
    let wpx = |s: &str| -> i32 {
        let mut v: Vec<u16> = s.encode_utf16().collect();
        let mut r = RECT::default();
        DrawTextW(hdc, &mut v, &mut r, DT_CALCRECT | DT_SINGLELINE);
        r.right - r.left
    };
    let (gh, sep, site) = ("GitHub", "   \u{00b7}   ", "struisict.com");
    let total = wpx(gh) + wpx(sep) + wpx(site);
    let mut lx = (cw - total) / 2;
    let y2 = 226;
    SetTextColor(hdc, COLORREF(p.blue));
    let ghr = about_text(hdc, gh, lx, y2);
    app.about_hit.push((ghr, 1));
    lx += wpx(gh);
    SetTextColor(hdc, COLORREF(p.subtext));
    about_text(hdc, sep, lx, y2);
    lx += wpx(sep);
    SetTextColor(hdc, COLORREF(p.blue));
    let sr = about_text(hdc, site, lx, y2);
    app.about_hit.push((sr, 2));

    // "Check for updates" link — an on-demand check against GitHub releases.
    SetTextColor(hdc, COLORREF(p.blue));
    let upd = about_center(hdc, "\u{21BB}  Check for updates", cw, 264);
    app.about_hit.push((upd, 3));

    // No OK button: like InLook's About card, the window closes via the
    // title-bar close (\u{00d7}) or Esc.
    SelectObject(hdc, old);
    let _ = EndPaint(hwnd, &ps);
}

unsafe extern "system" fn about_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let app_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState;
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            paint_about(hwnd, app_ptr);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            if !app_ptr.is_null() {
                let app = &*app_ptr;
                let x = (lparam.0 & 0xFFFF) as i16 as i32;
                let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
                for (r, action) in app.about_hit.clone() {
                    if x >= r.left && x < r.right && y >= r.top && y < r.bottom {
                        match action {
                            0 => open_url(COFFEE_URL),
                            1 => open_url(GITHUB_URL),
                            2 => open_url(SITE_URL),
                            3 => super::update::check_now(),
                            _ => {}
                        }
                        break;
                    }
                }
            }
            LRESULT(0)
        }
        WM_KEYDOWN if wparam.0 as u16 == VK_ESCAPE || wparam.0 as u16 == VK_RETURN => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn open_url(url: &str) {
    shell_exec("open", url, None, None);
}
