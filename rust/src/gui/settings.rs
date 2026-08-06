// The themed, modal "Settings" window plus the persisted user-settings model.
// Like the About window, this is a custom popup (a native property sheet can't
// follow the dark/light theme) with its own modal message loop, painted with the
// shared palette. Toggling a row applies live and writes settings.cfg.

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{BOOL, COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, InvalidateRect, SelectObject, SetBkMode, SetTextColor, UpdateWindow,
    DT_CENTER, DT_LEFT, DT_SINGLELINE, DT_VCENTER, HGDIOBJ, PAINTSTRUCT, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect, GetMessageW,
    GetWindowLongPtrW, GetWindowRect, IsWindow, LoadCursorW, RegisterClassExW, SetForegroundWindow,
    SetWindowLongPtrW, ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW, GWLP_USERDATA, HMENU,
    IDC_ARROW, MSG, SW_SHOW, WINDOW_EX_STYLE, WM_CLOSE, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDOWN,
    WM_PAINT, WNDCLASSEXW, WS_CAPTION, WS_POPUP, WS_SYSMENU,
};

use crate::scanner::wide;

use super::gdi::{card_round, draw_text, fill_rect};
use super::palette::{palette, ThemeMode};
use super::{
    apply_side_view, apply_theme, layout, load_app_icon, populate_list_folders,
    populate_side_oldest_files, populate_side_top_files, AppState, SideView, VK_ESCAPE, VK_RETURN,
};

// ---------------------------------------------------------------------------
// Persisted model
// ---------------------------------------------------------------------------

pub(crate) struct Settings {
    pub theme: ThemeMode,
    pub units_binary: bool,
    pub default_side: SideView,
    pub scan_on_launch: bool,
    pub confirm_recycle: bool,
    pub show_sidebar: bool,
    pub col_visible: [bool; 7],
}

impl Default for Settings {
    fn default() -> Self {
        // Defaults mirror the app's historical hardcoded behaviour.
        Self {
            theme: ThemeMode::Auto,
            units_binary: true,
            default_side: SideView::TopFiles,
            scan_on_launch: true,
            confirm_recycle: true,
            show_sidebar: true,
            col_visible: [true; 7],
        }
    }
}

fn cfg_path() -> Option<std::path::PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(
        std::path::Path::new(&appdata)
            .join("ClutterCutter")
            .join("settings.cfg"),
    )
}

/// Read settings.cfg, falling back to defaults for any missing/garbled key.
pub(crate) fn load() -> Settings {
    let mut s = Settings::default();
    let Some(p) = cfg_path() else { return s };
    let Ok(text) = std::fs::read_to_string(p) else {
        return s;
    };
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let v = v.trim();
        match k.trim() {
            "theme" => {
                s.theme = match v {
                    "light" => ThemeMode::Light,
                    "dark" => ThemeMode::Dark,
                    _ => ThemeMode::Auto,
                }
            }
            "units" => s.units_binary = v != "decimal",
            "side" => {
                s.default_side = match v {
                    "top" => SideView::TopFiles,
                    "oldest" => SideView::OldestFiles,
                    "temp" => SideView::TempFiles,
                    _ => SideView::None,
                }
            }
            "scan_on_launch" => s.scan_on_launch = v != "0",
            "confirm_recycle" => s.confirm_recycle = v != "0",
            "show_sidebar" => s.show_sidebar = v != "0",
            "cols" => {
                // 7-char bitmask, one per logical column.
                for (i, ch) in v.chars().take(7).enumerate() {
                    s.col_visible[i] = ch != '0';
                }
                for &c in &super::ALWAYS_SHOWN_COLS {
                    s.col_visible[c] = true;
                }
            }
            _ => {}
        }
    }
    s
}

pub(crate) fn save(s: &Settings) {
    let Some(p) = cfg_path() else { return };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let theme = match s.theme {
        ThemeMode::Light => "light",
        ThemeMode::Dark => "dark",
        ThemeMode::Auto => "auto",
    };
    let side = match s.default_side {
        SideView::TopFiles => "top",
        SideView::OldestFiles => "oldest",
        SideView::TempFiles => "temp",
        SideView::None => "none",
    };
    let cols: String = s
        .col_visible
        .iter()
        .map(|&b| if b { '1' } else { '0' })
        .collect();
    let text = format!(
        "theme={theme}\nunits={}\nside={side}\nscan_on_launch={}\nconfirm_recycle={}\nshow_sidebar={}\ncols={cols}\n",
        if s.units_binary { "binary" } else { "decimal" },
        s.scan_on_launch as i32,
        s.confirm_recycle as i32,
        s.show_sidebar as i32,
    );
    let _ = std::fs::write(p, text);
}

/// Snapshot the live AppState settings and persist them.
pub(crate) fn save_from(app: &AppState) {
    save(&Settings {
        theme: app.theme_mode,
        units_binary: app.units_binary,
        default_side: app.default_side,
        scan_on_launch: app.scan_on_launch,
        confirm_recycle: app.confirm_recycle,
        show_sidebar: app.show_sidebar,
        col_visible: app.col_visible,
    });
}

// ---------------------------------------------------------------------------
// Modal window
// ---------------------------------------------------------------------------

// Hit-region action ids recorded on paint, consumed on click.
const A_THEME_AUTO: i32 = 10;
const A_THEME_LIGHT: i32 = 11;
const A_THEME_DARK: i32 = 12;
const A_UNITS_BIN: i32 = 20;
const A_UNITS_DEC: i32 = 21;
const A_SIDE_TOP: i32 = 30;
const A_SIDE_OLD: i32 = 31;
const A_SIDE_TEMP: i32 = 32;
const A_SIDE_NONE: i32 = 33;
const A_TOG_SCAN: i32 = 40;
const A_TOG_CONFIRM: i32 = 41;
const A_TOG_SIDEBAR: i32 = 42;
// Column-visibility toggles carry the logical column id (1,3,4,5,6) as 100 + id.
const A_COL_BASE: i32 = 100;

const WIN_W: i32 = 460;
const WIN_H: i32 = 636;

pub(crate) unsafe fn show_settings(parent: HWND, app: &mut AppState) {
    let hinstance = GetModuleHandleW(None).expect("hinst");
    let class = w!("ClutterCutterSettings");
    RegisterClassExW(&WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(settings_proc),
        hInstance: hinstance.into(),
        hIcon: load_app_icon(),
        hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
        hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH::default(),
        lpszClassName: class,
        ..Default::default()
    });
    let mut pr = RECT::default();
    let _ = GetWindowRect(parent, &mut pr);
    let x = pr.left + ((pr.right - pr.left) - WIN_W) / 2;
    let y = pr.top + ((pr.bottom - pr.top) - WIN_H) / 2;
    let title = wide("Settings");
    let hwnd = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        class,
        PCWSTR(title.as_ptr()),
        WS_POPUP | WS_CAPTION | WS_SYSMENU,
        x,
        y,
        WIN_W,
        WIN_H,
        parent,
        HMENU::default(),
        hinstance,
        None,
    )
    .expect("settings window");
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, app as *mut AppState as isize);
    set_dark_titlebar(hwnd, app.is_dark);
    let _ = ShowWindow(hwnd, SW_SHOW);
    let _ = UpdateWindow(hwnd);

    // Modal: disable the parent and pump until the window is gone.
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

unsafe fn set_dark_titlebar(hwnd: HWND, dark: bool) {
    let use_dark = BOOL(if dark { 1 } else { 0 });
    let _ = DwmSetWindowAttribute(
        hwnd,
        DWMWA_USE_IMMERSIVE_DARK_MODE,
        &use_dark as *const _ as *const _,
        std::mem::size_of::<BOOL>() as u32,
    );
}

// A left-aligned label + a segmented control on the right; records one hit per
// segment. Returns nothing (hits pushed into app.settings_hit).
#[allow(clippy::too_many_arguments)]
unsafe fn row_segmented(
    hdc: windows::Win32::Graphics::Gdi::HDC,
    app: &mut AppState,
    label: &str,
    y: i32,
    segs: &[(&str, i32, bool)],
) {
    let p = palette(app.is_dark);
    SelectObject(hdc, HGDIOBJ(app.font_small.0));
    SetTextColor(hdc, COLORREF(p.text));
    let lrc = RECT {
        left: 24,
        top: y,
        right: 190,
        bottom: y + 30,
    };
    draw_text(hdc, label, &lrc, DT_SINGLELINE | DT_VCENTER | DT_LEFT);

    let right = WIN_W - 24;
    let left = 196;
    let n = segs.len() as i32;
    let seg_w = (right - left) / n;
    let top = y;
    let bot = y + 30;
    // Container track.
    let track = RECT {
        left,
        top,
        right: left + seg_w * n,
        bottom: bot,
    };
    card_round(hdc, &track, 8, p.track, p.hairline, 1);
    for (i, (text, action, active)) in segs.iter().enumerate() {
        let sx = left + seg_w * i as i32;
        let sr = RECT {
            left: sx,
            top,
            right: sx + seg_w,
            bottom: bot,
        };
        if *active {
            let fill = RECT {
                left: sx + 2,
                top: top + 2,
                right: sx + seg_w - 2,
                bottom: bot - 2,
            };
            card_round(hdc, &fill, 7, p.blue, p.blue, 1);
            SetTextColor(hdc, COLORREF(0x00FF_FFFF));
        } else {
            SetTextColor(hdc, COLORREF(p.subtext));
        }
        draw_text(hdc, text, &sr, DT_SINGLELINE | DT_VCENTER | DT_CENTER);
        app.settings_hit.push((sr, *action));
    }
}

// A full-width clickable checkbox row.
unsafe fn row_checkbox(
    hdc: windows::Win32::Graphics::Gdi::HDC,
    app: &mut AppState,
    label: &str,
    y: i32,
    checked: bool,
    action: i32,
) {
    let p = palette(app.is_dark);
    let box_rc = RECT {
        left: 24,
        top: y + 4,
        right: 44,
        bottom: y + 24,
    };
    if checked {
        card_round(hdc, &box_rc, 5, p.blue, p.blue, 1);
        SelectObject(hdc, HGDIOBJ(app.font_icon.0));
        SetTextColor(hdc, COLORREF(0x00FF_FFFF));
        draw_text(
            hdc,
            "\u{E73E}",
            &box_rc,
            DT_SINGLELINE | DT_VCENTER | DT_CENTER,
        );
    } else {
        card_round(hdc, &box_rc, 5, p.card_bg, p.subtext, 1);
    }
    SelectObject(hdc, HGDIOBJ(app.font_small.0));
    SetTextColor(hdc, COLORREF(p.text));
    let lrc = RECT {
        left: 54,
        top: y,
        right: WIN_W - 24,
        bottom: y + 28,
    };
    draw_text(hdc, label, &lrc, DT_SINGLELINE | DT_VCENTER | DT_LEFT);
    // Whole row is clickable.
    let row = RECT {
        left: 24,
        top: y,
        right: WIN_W - 24,
        bottom: y + 28,
    };
    app.settings_hit.push((row, action));
}

unsafe fn section(hdc: windows::Win32::Graphics::Gdi::HDC, app: &AppState, title: &str, y: i32) {
    let p = palette(app.is_dark);
    SelectObject(hdc, HGDIOBJ(app.font_small.0));
    SetTextColor(hdc, COLORREF(p.subtext));
    let rc = RECT {
        left: 24,
        top: y,
        right: WIN_W - 24,
        bottom: y + 20,
    };
    draw_text(hdc, title, &rc, DT_SINGLELINE | DT_VCENTER | DT_LEFT);
    // Hairline under the section title.
    let hair = RECT {
        left: 24,
        top: y + 22,
        right: WIN_W - 24,
        bottom: y + 23,
    };
    fill_rect(hdc, &hair, p.hairline);
}

unsafe fn paint_settings(hwnd: HWND, app_ptr: *mut AppState) {
    if app_ptr.is_null() {
        return;
    }
    let app = &mut *app_ptr;
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    let p = palette(app.is_dark);
    fill_rect(hdc, &rc, p.card_bg);
    SetBkMode(hdc, TRANSPARENT);
    app.settings_hit.clear();

    // Heading.
    SelectObject(hdc, HGDIOBJ(app.font_title.0));
    SetTextColor(hdc, COLORREF(p.text));
    let hrc = RECT {
        left: 24,
        top: 16,
        right: WIN_W - 24,
        bottom: 48,
    };
    draw_text(hdc, "Settings", &hrc, DT_SINGLELINE | DT_LEFT);

    section(hdc, app, "APPEARANCE", 58);
    let tm = app.theme_mode;
    row_segmented(
        hdc,
        app,
        "Theme",
        86,
        &[
            ("Auto", A_THEME_AUTO, matches!(tm, ThemeMode::Auto)),
            ("Light", A_THEME_LIGHT, matches!(tm, ThemeMode::Light)),
            ("Dark", A_THEME_DARK, matches!(tm, ThemeMode::Dark)),
        ],
    );
    let bin = app.units_binary;
    row_segmented(
        hdc,
        app,
        "Size units",
        124,
        &[("Binary", A_UNITS_BIN, bin), ("Decimal", A_UNITS_DEC, !bin)],
    );

    section(hdc, app, "STARTUP", 172);
    let ds = app.default_side;
    row_segmented(
        hdc,
        app,
        "Default panel",
        200,
        &[
            ("Top", A_SIDE_TOP, matches!(ds, SideView::TopFiles)),
            ("Oldest", A_SIDE_OLD, matches!(ds, SideView::OldestFiles)),
            ("Temp", A_SIDE_TEMP, matches!(ds, SideView::TempFiles)),
            ("None", A_SIDE_NONE, matches!(ds, SideView::None)),
        ],
    );
    row_checkbox(
        hdc,
        app,
        "Scan all drives on launch",
        240,
        app.scan_on_launch,
        A_TOG_SCAN,
    );

    section(hdc, app, "BEHAVIOUR", 288);
    row_checkbox(
        hdc,
        app,
        "Confirm before recycling",
        316,
        app.confirm_recycle,
        A_TOG_CONFIRM,
    );
    row_checkbox(
        hdc,
        app,
        "Show drive sidebar",
        350,
        app.show_sidebar,
        A_TOG_SIDEBAR,
    );

    section(hdc, app, "COLUMNS  (Name and Size always shown)", 396);
    let cols = [
        ("% of parent", 1),
        ("Own size", 3),
        ("Files", 4),
        ("Folders", 5),
        ("Modified", 6),
    ];
    let mut cy = 424;
    for (label, logical) in cols {
        row_checkbox(
            hdc,
            app,
            label,
            cy,
            app.col_visible[logical as usize],
            A_COL_BASE + logical,
        );
        cy += 32;
    }

    // Footer hint.
    SelectObject(hdc, HGDIOBJ(app.font_small.0));
    SetTextColor(hdc, COLORREF(p.subtext));
    let frc = RECT {
        left: 24,
        top: WIN_H - 52,
        right: WIN_W - 24,
        bottom: WIN_H - 30,
    };
    draw_text(
        hdc,
        "Changes save automatically. Close with \u{00d7} or Esc.",
        &frc,
        DT_SINGLELINE | DT_VCENTER | DT_LEFT,
    );

    let _ = EndPaint(hwnd, &ps);
}

// Apply a clicked action to AppState, live-refresh the main window, and persist.
unsafe fn apply_action(hwnd: HWND, app: &mut AppState, action: i32) {
    let main = app.main_hwnd;
    match action {
        A_THEME_AUTO => apply_theme(main, app, ThemeMode::Auto),
        A_THEME_LIGHT => apply_theme(main, app, ThemeMode::Light),
        A_THEME_DARK => apply_theme(main, app, ThemeMode::Dark),
        A_UNITS_BIN | A_UNITS_DEC => {
            app.units_binary = action == A_UNITS_BIN;
            crate::format::set_binary_units(app.units_binary);
            refresh_after_units(app);
        }
        A_SIDE_TOP => set_default_side(app, SideView::TopFiles),
        A_SIDE_OLD => set_default_side(app, SideView::OldestFiles),
        A_SIDE_TEMP => set_default_side(app, SideView::TempFiles),
        A_SIDE_NONE => set_default_side(app, SideView::None),
        A_TOG_SCAN => app.scan_on_launch = !app.scan_on_launch,
        A_TOG_CONFIRM => app.confirm_recycle = !app.confirm_recycle,
        A_TOG_SIDEBAR => {
            app.show_sidebar = !app.show_sidebar;
            layout(main, app);
            let _ = InvalidateRect(main, None, true);
        }
        a if a >= A_COL_BASE => {
            let logical = (a - A_COL_BASE) as usize;
            if logical < 7 && !super::ALWAYS_SHOWN_COLS.contains(&logical) {
                app.col_visible[logical] = !app.col_visible[logical];
                super::rebuild_columns(app);
            } else {
                return;
            }
        }
        _ => return,
    }
    // Theme changes re-tint this window too.
    if (A_THEME_AUTO..=A_THEME_DARK).contains(&action) {
        set_dark_titlebar(hwnd, app.is_dark);
    }
    save_from(app);
    let _ = InvalidateRect(hwnd, None, true);
}

// The default panel is also applied to the current view for immediate feedback.
unsafe fn set_default_side(app: &mut AppState, side: SideView) {
    app.default_side = side;
    apply_side_view(app.main_hwnd, app, side);
}

// Units changed: rebuild the cached formatted strings in the main list and the
// side panel, and repaint the sidebar cards + status bar.
unsafe fn refresh_after_units(app: &mut AppState) {
    if app.selected_node != 0 {
        populate_list_folders(app, &*(app.selected_node as *const super::FolderNode));
    }
    // The side-view cache holds pre-formatted strings keyed by tree version, which
    // hasn't changed — clear it and re-populate the current view (apply_side_view
    // no-ops when the view is unchanged, so call the populate fns directly).
    app.side_cache.clear();
    match app.side_view {
        SideView::TopFiles => populate_side_top_files(app),
        SideView::OldestFiles => populate_side_oldest_files(app),
        SideView::None | SideView::TempFiles => {}
    }
    let _ = InvalidateRect(app.sidebar, None, false);
    let _ = InvalidateRect(app.status, None, false);
}

unsafe extern "system" fn settings_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let app_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState;
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            paint_settings(hwnd, app_ptr);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            if !app_ptr.is_null() {
                let x = (lparam.0 & 0xFFFF) as i16 as i32;
                let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
                let hits = (*app_ptr).settings_hit.clone();
                for (r, action) in hits {
                    if x >= r.left && x < r.right && y >= r.top && y < r.bottom {
                        apply_action(hwnd, &mut *app_ptr, action);
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
