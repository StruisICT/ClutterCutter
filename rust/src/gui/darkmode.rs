// Dark/light theming: apply_theme (retheme every control), the undocumented
// uxtheme opt-in used to make Win32 common controls render dark, and the WM_UAH*
// owner-draw for the dark menu bar. Also the theme-aware background erase and the
// cached list fill brushes.

use windows::core::PWSTR;
use windows::core::{w, PCSTR, PCWSTR};
use windows::Win32::Foundation::{BOOL, COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};
use windows::Win32::Graphics::Gdi::{
    CreateSolidBrush, DeleteObject, DrawTextW, FillRect, GetSysColorBrush, GetWindowDC,
    InvalidateRect, MapWindowPoints, RedrawWindow, ReleaseDC, SetBkMode, SetTextColor,
    COLOR_BTNFACE, DT_CENTER, DT_HIDEPREFIX, DT_SINGLELINE, DT_VCENTER, HDC, RDW_ALLCHILDREN,
    RDW_ERASE, RDW_FRAME, RDW_INVALIDATE, RDW_UPDATENOW, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Registry::{
    RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ, REG_DWORD,
    REG_VALUE_TYPE,
};
use windows::Win32::UI::Controls::{
    SetWindowTheme, DRAWITEMSTRUCT, LVM_GETHEADER, LVM_SETBKCOLOR, LVM_SETTEXTBKCOLOR,
    LVM_SETTEXTCOLOR, TVM_SETBKCOLOR, TVM_SETTEXTCOLOR,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CheckMenuRadioItem, DrawMenuBar, GetClientRect, GetMenuBarInfo, GetMenuItemInfoW,
    GetWindowRect, IsZoomed, SendMessageW, SetWindowPos, HMENU, MENUBARINFO, MENUITEMINFOW,
    MF_BYCOMMAND, MIIM_STRING, OBJID_MENU, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, SWP_NOZORDER,
};

use super::gdi::fill_rect;
use super::palette::{palette, ThemeMode};
use super::{AppState, ID_MENU_THEME_AUTO, ID_MENU_THEME_DARK, ID_MENU_THEME_LIGHT};

pub(crate) const WM_UAHDRAWMENU: u32 = 0x0091;
pub(crate) const WM_UAHDRAWMENUITEM: u32 = 0x0092;

// Raw ODS_* bits (DRAWITEMSTRUCT.itemState).
const ODS_RAW_SELECTED: u32 = 0x0001;
const ODS_RAW_GRAYED: u32 = 0x0002;
const ODS_RAW_HOTLIGHT: u32 = 0x0040;
const ODS_RAW_NOACCEL: u32 = 0x0100;

const MENUBAR_DARK_BG: u32 = 0x0020_2020;
const MENUBAR_DARK_HOT: u32 = 0x003E_3E3E;
const MENUBAR_DARK_FG: u32 = 0x00E0_E0E0;
const MENUBAR_DARK_GRAY: u32 = 0x0080_8080;

// Theme-aware WM_ERASEBKGND fill, shared by the main window and the chrome frames.
pub(crate) unsafe fn erase_theme_bg(app: &AppState, hwnd: HWND, hdc: HDC) -> LRESULT {
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    if app.is_dark {
        fill_rect(hdc, &rc, 0x0020_2020);
    } else {
        FillRect(hdc, &rc, GetSysColorBrush(COLOR_BTNFACE));
    }
    LRESULT(1)
}

// (Re)create the cached list fill brushes for the current theme.
pub(crate) unsafe fn rebuild_theme_brushes(app: &mut AppState) {
    let p = palette(app.is_dark);
    if !app.brush_card.is_invalid() {
        let _ = DeleteObject(app.brush_card);
    }
    if !app.brush_panel.is_invalid() {
        let _ = DeleteObject(app.brush_panel);
    }
    app.brush_card = CreateSolidBrush(COLORREF(p.card_bg));
    app.brush_panel = CreateSolidBrush(COLORREF(p.panel_bg));
}

fn read_system_uses_light_theme() -> bool {
    unsafe {
        let mut hkey = HKEY::default();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
            0,
            KEY_READ,
            &mut hkey,
        )
        .is_err()
        {
            return true; // default to light if registry read fails
        }
        let mut value: u32 = 1;
        let mut size: u32 = std::mem::size_of::<u32>() as u32;
        let mut vtype = REG_VALUE_TYPE(0);
        let _ = RegQueryValueExW(
            hkey,
            w!("SystemUsesLightTheme"),
            None,
            Some(&mut vtype),
            Some(&mut value as *mut u32 as *mut u8),
            Some(&mut size),
        );
        let _ = RegCloseKey(hkey);
        if vtype == REG_DWORD {
            value != 0
        } else {
            true
        }
    }
}

pub(crate) unsafe fn apply_theme(hwnd: HWND, app: &mut AppState, mode: ThemeMode) {
    let is_dark = match mode {
        ThemeMode::Light => false,
        ThemeMode::Dark => true,
        ThemeMode::Auto => !read_system_uses_light_theme(),
    };

    // Title bar (Windows 10 2004+ and Windows 11)
    let use_dark = BOOL(if is_dark { 1 } else { 0 });
    let _ = DwmSetWindowAttribute(
        hwnd,
        DWMWA_USE_IMMERSIVE_DARK_MODE,
        &use_dark as *const _ as *const _,
        std::mem::size_of::<BOOL>() as u32,
    );

    // ListView + TreeView themes
    let theme_w: Vec<u16> = if is_dark {
        "DarkMode_Explorer\0".encode_utf16().collect()
    } else {
        "Explorer\0".encode_utf16().collect()
    };
    // Process-wide dark mode (undocumented uxtheme, guarded): without this,
    // DarkMode_* window themes don't actually render dark and popup menus stay
    // white. Must run before the SetWindowTheme calls below.
    set_preferred_app_mode(is_dark);
    for w in [hwnd, app.float_win, app.panel, app.status] {
        if !w.is_invalid() {
            allow_dark_mode_for_window(w, is_dark);
        }
    }

    allow_dark_mode_for_window(app.list, is_dark);
    allow_dark_mode_for_window(app.tree, is_dark);
    allow_dark_mode_for_window(app.side_list, is_dark);
    let _ = SetWindowTheme(app.list, PCWSTR(theme_w.as_ptr()), PCWSTR::null());
    let _ = SetWindowTheme(app.tree, PCWSTR(theme_w.as_ptr()), PCWSTR::null());
    let _ = SetWindowTheme(app.side_list, PCWSTR(theme_w.as_ptr()), PCWSTR::null());
    // Listview column headers have their own theme part ("ItemsView").
    let header_theme_w: Vec<u16> = "ItemsView\0".encode_utf16().collect();
    for list in [app.list, app.side_list] {
        let header = SendMessageW(list, LVM_GETHEADER, WPARAM(0), LPARAM(0));
        if header.0 != 0 {
            let header = HWND(header.0 as _);
            allow_dark_mode_for_window(header, is_dark);
            let _ = SetWindowTheme(header, PCWSTR(header_theme_w.as_ptr()), PCWSTR::null());
        }
    }
    // Dark push buttons (Win10 1809+); "Explorer" restores the standard look.
    let buttons: Vec<HWND> = app
        .drive_buttons
        .iter()
        .copied()
        .chain([
            app.stop_btn,
            app.scan_all_btn,
            app.btn_detach,
            app.btn_recycle_all,
        ])
        .collect();
    for b in buttons {
        allow_dark_mode_for_window(b, is_dark);
        let _ = SetWindowTheme(b, PCWSTR(theme_w.as_ptr()), PCWSTR::null());
    }
    if !app.float_win.is_invalid() {
        let _ = DwmSetWindowAttribute(
            app.float_win,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &use_dark as *const _ as *const _,
            std::mem::size_of::<BOOL>() as u32,
        );
    }

    let (bg, fg): (u32, u32) = if is_dark {
        // COLORREF is 0x00BBGGRR
        (0x00202020, 0x00E0E0E0)
    } else {
        (0x00FFFFFF, 0x00000000)
    };
    SendMessageW(app.list, LVM_SETBKCOLOR, WPARAM(0), LPARAM(bg as isize));
    SendMessageW(app.list, LVM_SETTEXTCOLOR, WPARAM(0), LPARAM(fg as isize));
    SendMessageW(app.list, LVM_SETTEXTBKCOLOR, WPARAM(0), LPARAM(bg as isize));
    let side_bg = palette(is_dark).panel_bg as isize;
    SendMessageW(app.side_list, LVM_SETBKCOLOR, WPARAM(0), LPARAM(side_bg));
    SendMessageW(
        app.side_list,
        LVM_SETTEXTCOLOR,
        WPARAM(0),
        LPARAM(fg as isize),
    );
    SendMessageW(
        app.side_list,
        LVM_SETTEXTBKCOLOR,
        WPARAM(0),
        LPARAM(side_bg),
    );
    SendMessageW(app.tree, TVM_SETBKCOLOR, WPARAM(0), LPARAM(bg as isize));
    SendMessageW(app.tree, TVM_SETTEXTCOLOR, WPARAM(0), LPARAM(fg as isize));

    if !app.menu.is_invalid() {
        let id = match mode {
            ThemeMode::Auto => ID_MENU_THEME_AUTO,
            ThemeMode::Light => ID_MENU_THEME_LIGHT,
            ThemeMode::Dark => ID_MENU_THEME_DARK,
        } as u32;
        let _ = CheckMenuRadioItem(
            app.menu,
            ID_MENU_THEME_AUTO as u32,
            ID_MENU_THEME_DARK as u32,
            id,
            MF_BYCOMMAND.0,
        );
    }

    app.theme_mode = mode;
    app.is_dark = is_dark;
    rebuild_theme_brushes(app); // refresh cached list fill brushes for the new theme
                                // Force a full frame + children repaint: the title/menu bar live in the
                                // non-client area and don't pick up a client-area invalidate alone.
    nudge_caption_repaint(hwnd);
    let _ = DrawMenuBar(hwnd);
    let _ = RedrawWindow(
        hwnd,
        None,
        None,
        RDW_INVALIDATE | RDW_ERASE | RDW_FRAME | RDW_ALLCHILDREN | RDW_UPDATENOW,
    );
    if !app.float_win.is_invalid() {
        nudge_caption_repaint(app.float_win);
        let _ = RedrawWindow(
            app.float_win,
            None,
            None,
            RDW_INVALIDATE | RDW_ERASE | RDW_FRAME | RDW_ALLCHILDREN,
        );
    }
    let _ = InvalidateRect(app.panel, None, true);
    let _ = InvalidateRect(app.status, None, true);
    let _ = InvalidateRect(app.topbar, None, false);
    let _ = InvalidateRect(app.sidebar, None, false);
    let _ = InvalidateRect(app.crumb, None, false);
}

// DWM caches the title bar, so flipping the dark-mode attribute with only an
// SWP_FRAMECHANGED doesn't reliably recomposite the caption. A 1px size wobble
// forces DWM to repaint it. Skipped when maximized.
unsafe fn nudge_caption_repaint(hwnd: HWND) {
    let flags = SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED;
    if IsZoomed(hwnd).as_bool() {
        let _ = SetWindowPos(hwnd, HWND::default(), 0, 0, 0, 0, flags | SWP_NOSIZE);
        return;
    }
    let mut wr = RECT::default();
    if GetWindowRect(hwnd, &mut wr).is_err() {
        return;
    }
    let (w, h) = (wr.right - wr.left, wr.bottom - wr.top);
    let _ = SetWindowPos(hwnd, HWND::default(), 0, 0, w, h + 1, flags);
    let _ = SetWindowPos(hwnd, HWND::default(), 0, 0, w, h, flags);
}

// ---- Undocumented dark-mode plumbing ----
//
// Windows exposes no public API to render Win32 common controls dark; the
// DarkMode_* window themes only take effect after opting in through unexported
// uxtheme entry points (looked up by ordinal). Everything degrades to a silent
// no-op if an ordinal is missing.
//   ordinal 104 RefreshImmersiveColorPolicyState; 133 AllowDarkModeForWindow;
//   135 SetPreferredAppMode; 136 FlushMenuThemes.
unsafe fn uxtheme_ordinal(ordinal: u16) -> Option<unsafe extern "system" fn() -> isize> {
    let lib = LoadLibraryW(w!("uxtheme.dll")).ok()?;
    GetProcAddress(lib, PCSTR(ordinal as usize as *const u8))
}

pub(crate) unsafe fn allow_dark_mode_for_window(hwnd: HWND, allow: bool) {
    if let Some(f) = uxtheme_ordinal(133) {
        let allow_fn: unsafe extern "system" fn(HWND, BOOL) -> BOOL = std::mem::transmute(f);
        let _ = allow_fn(hwnd, BOOL(allow as i32));
    }
}

unsafe fn set_preferred_app_mode(dark: bool) {
    if let Some(f) = uxtheme_ordinal(135) {
        let set_mode: unsafe extern "system" fn(i32) -> i32 = std::mem::transmute(f);
        set_mode(if dark { 2 } else { 3 }); // ForceDark / ForceLight
    }
    if let Some(f) = uxtheme_ordinal(104) {
        f(); // RefreshImmersiveColorPolicyState
    }
    if let Some(f) = uxtheme_ordinal(136) {
        f(); // FlushMenuThemes
    }
}

// ---- Dark menu bar (WM_UAH* owner-draw) ----
// FlushMenuThemes darkens popup menus but never the menu *bar*; the bar is
// painted via the undocumented WM_UAHDRAWMENU / WM_UAHDRAWMENUITEM messages.

#[repr(C)]
pub(crate) struct UahMenu {
    hmenu: HMENU,
    hdc: HDC,
    dw_flags: u32,
}

#[repr(C)]
struct UahMenuItemMetrics {
    rgsize: [[u32; 2]; 4],
}

#[repr(C)]
struct UahMenuPopupMetrics {
    rgcx: [u32; 4],
    bitfield: u32,
}

#[repr(C)]
struct UahMenuItem {
    i_position: i32,
    umim: UahMenuItemMetrics,
    umpm: UahMenuPopupMetrics,
}

#[repr(C)]
pub(crate) struct UahDrawMenuItem {
    dis: DRAWITEMSTRUCT,
    um: UahMenu,
    umi: UahMenuItem,
}

// Menu bar background (the strip behind the items).
pub(crate) unsafe fn uah_draw_menu_bar_bg(hwnd: HWND, udm: &UahMenu) {
    let mut mbi = MENUBARINFO {
        cbSize: std::mem::size_of::<MENUBARINFO>() as u32,
        ..Default::default()
    };
    if GetMenuBarInfo(hwnd, OBJID_MENU, 0, &mut mbi).is_err() {
        return;
    }
    let mut rc_win = RECT::default();
    let _ = GetWindowRect(hwnd, &mut rc_win);
    // rcBar is in screen coords; the UAH DC is a window DC.
    let rc = RECT {
        left: mbi.rcBar.left - rc_win.left,
        top: mbi.rcBar.top - rc_win.top,
        right: mbi.rcBar.right - rc_win.left,
        bottom: mbi.rcBar.bottom - rc_win.top,
    };
    let brush = CreateSolidBrush(COLORREF(MENUBAR_DARK_BG));
    FillRect(udm.hdc, &rc, brush);
    let _ = DeleteObject(brush);
}

pub(crate) unsafe fn uah_draw_menu_item(pudmi: &UahDrawMenuItem) {
    let mut buf = [0u16; 256];
    let mut mii = MENUITEMINFOW {
        cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
        fMask: MIIM_STRING,
        dwTypeData: PWSTR(buf.as_mut_ptr()),
        cch: (buf.len() - 1) as u32,
        ..Default::default()
    };
    let _ = GetMenuItemInfoW(pudmi.um.hmenu, pudmi.umi.i_position as u32, true, &mut mii);
    let len = mii.cch as usize;

    let state = pudmi.dis.itemState.0;
    let bg = if state & (ODS_RAW_HOTLIGHT | ODS_RAW_SELECTED) != 0 {
        MENUBAR_DARK_HOT
    } else {
        MENUBAR_DARK_BG
    };
    let fg = if state & ODS_RAW_GRAYED != 0 {
        MENUBAR_DARK_GRAY
    } else {
        MENUBAR_DARK_FG
    };
    let brush = CreateSolidBrush(COLORREF(bg));
    FillRect(pudmi.dis.hDC, &pudmi.dis.rcItem, brush);
    let _ = DeleteObject(brush);
    if len == 0 {
        return;
    }
    SetBkMode(pudmi.dis.hDC, TRANSPARENT);
    SetTextColor(pudmi.dis.hDC, COLORREF(fg));
    let mut fmt = DT_CENTER | DT_SINGLELINE | DT_VCENTER;
    if state & ODS_RAW_NOACCEL != 0 {
        fmt |= DT_HIDEPREFIX;
    }
    let mut rc = pudmi.dis.rcItem;
    DrawTextW(pudmi.dis.hDC, &mut buf[..len], &mut rc, fmt);
}

// Windows draws a 1px light line between the menu bar and the client area
// during non-client painting; overpaint it to match the dark bar.
pub(crate) unsafe fn uah_draw_menu_bottom_line(hwnd: HWND) {
    let mut mbi = MENUBARINFO {
        cbSize: std::mem::size_of::<MENUBARINFO>() as u32,
        ..Default::default()
    };
    if GetMenuBarInfo(hwnd, OBJID_MENU, 0, &mut mbi).is_err() {
        return;
    }
    let mut rc_client = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc_client);
    let mut pts = [
        POINT {
            x: rc_client.left,
            y: rc_client.top,
        },
        POINT {
            x: rc_client.right,
            y: rc_client.bottom,
        },
    ];
    MapWindowPoints(hwnd, HWND::default(), &mut pts);
    let mut rc_win = RECT::default();
    let _ = GetWindowRect(hwnd, &mut rc_win);
    let line = RECT {
        left: pts[0].x - rc_win.left,
        top: pts[0].y - rc_win.top - 1,
        right: pts[1].x - rc_win.left,
        bottom: pts[0].y - rc_win.top,
    };
    let hdc = GetWindowDC(hwnd);
    let brush = CreateSolidBrush(COLORREF(MENUBAR_DARK_BG));
    FillRect(hdc, &line, brush);
    let _ = DeleteObject(brush);
    let _ = ReleaseDC(hwnd, hdc);
}
