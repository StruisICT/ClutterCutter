// Win32 GUI for ClutterCutter — raw window class + message loop + WndProc, no
// GUI framework.
//
//   [drive buttons] [Stop]
//   [TreeView] | [ListView]
//   [status bar]
//
// Drive buttons auto-pick MFT vs FindFirstFileEx walker (MFT when NTFS + admin).
// Scans run on a worker thread; progress/results are posted back via WM_APP
// messages. Tree drives drill-in; listview shows the selected node's direct
// children sorted by size. Right-click on a row opens an Explorer/Copy/Cmd/Recycle
// menu; F5 re-scans, Esc stops, Backspace goes to parent, Enter drills, Del
// recycles.

use crate::analysis::{oldest_n_files, top_n_files};
use crate::mft::{is_ntfs_drive_root, MftScanner};
use crate::scanner::{wide, wstr_to_string, ProgressFn, Scanner};
use crate::temp::{self, TempFileEntry};
use crate::treemap::{self, Rectf};
use crate::types::{FileEntry, FolderNode, ScanProgress};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    BOOL, COLORREF, FILETIME, HWND, LPARAM, LRESULT, POINT, RECT, SYSTEMTIME, WPARAM,
};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateSolidBrush, DeleteDC,
    DeleteObject, EndPaint, FillRect, FrameRect, GetSysColorBrush, InvalidateRect, SelectObject,
    UpdateWindow, COLOR_BTNFACE, HBRUSH, PAINTSTRUCT, SRCCOPY,
};
use windows::Win32::Storage::FileSystem::{
    GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW,
};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::System::Registry::{
    RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ, REG_DWORD,
    REG_VALUE_TYPE,
};
use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
use windows::Win32::System::WindowsProgramming::{DRIVE_FIXED, DRIVE_REMOVABLE};
use windows::Win32::UI::Controls::{
    InitCommonControlsEx, SetWindowTheme, ICC_BAR_CLASSES, ICC_LISTVIEW_CLASSES,
    ICC_STANDARD_CLASSES, ICC_TREEVIEW_CLASSES, INITCOMMONCONTROLSEX, LVCFMT_LEFT, LVCFMT_RIGHT,
    LVCF_FMT, LVCF_TEXT, LVCF_WIDTH, LVCOLUMNW, LVIF_TEXT, LVITEMW, LVM_DELETEALLITEMS,
    LVM_DELETECOLUMN, LVM_GETITEMW, LVM_GETNEXTITEM, LVM_INSERTCOLUMNW, LVM_INSERTITEMW,
    LVM_SETBKCOLOR, LVM_SETEXTENDEDLISTVIEWSTYLE, LVM_SETITEMTEXTW, LVM_SETTEXTBKCOLOR,
    LVM_SETTEXTCOLOR, LVNI_SELECTED, LVS_EX_DOUBLEBUFFER, LVS_EX_FULLROWSELECT, LVS_EX_GRIDLINES,
    LVS_REPORT, LVS_SHOWSELALWAYS, NMHDR, NMITEMACTIVATE, NM_DBLCLK, NM_RCLICK, SBARS_SIZEGRIP,
    SB_SETTEXTW, TVE_EXPAND, TVGN_CARET, TVGN_PARENT, TVIF_CHILDREN, TVIF_PARAM, TVIF_TEXT,
    TVITEMW, TVI_ROOT, TVM_DELETEITEM, TVM_GETITEMW, TVM_GETNEXTITEM, TVM_INSERTITEMW,
    TVM_SELECTITEM, TVM_SETBKCOLOR, TVM_SETTEXTCOLOR, TVN_ITEMEXPANDINGW, TVN_SELCHANGEDW,
    TVS_HASBUTTONS, TVS_HASLINES, TVS_LINESATROOT, TVS_SHOWSELALWAYS, TVS_TRACKSELECT,
};
use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows::Win32::UI::Shell::{
    IsUserAnAdmin, SHFileOperationW, ShellExecuteW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FO_DELETE,
    SHFILEOPSTRUCTW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CheckMenuRadioItem, CreateAcceleratorTableW, CreateMenu, CreatePopupMenu,
    CreateWindowExW, DefWindowProcW, DestroyMenu, DispatchMessageW, DrawMenuBar, GetClientRect,
    GetCursorPos, GetMessageW, GetWindowLongPtrW, LoadCursorW, LoadIconW, MessageBoxW, MoveWindow,
    PostMessageW, PostQuitMessage, RegisterClassExW, SendMessageW, SetForegroundWindow, SetMenu,
    SetWindowLongPtrW, ShowWindow, TrackPopupMenu, TranslateAcceleratorW, TranslateMessage, ACCEL,
    BS_PUSHBUTTON, CREATESTRUCTW, CS_DBLCLKS, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, FVIRTKEY,
    GWLP_USERDATA, HMENU, IDC_ARROW, IDI_APPLICATION, MB_ICONINFORMATION, MB_OK, MF_BYCOMMAND,
    MF_POPUP, MF_SEPARATOR, MF_STRING, MSG, SW_HIDE, SW_NORMAL, SW_SHOW, TPM_LEFTALIGN,
    TPM_RIGHTBUTTON, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_COMMAND, WM_CREATE, WM_DESTROY,
    WM_ERASEBKGND, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_MOUSEMOVE, WM_NCCREATE, WM_NOTIFY,
    WM_PAINT, WM_RBUTTONDOWN, WM_SIZE, WNDCLASSEXW, WS_BORDER, WS_CHILD, WS_EX_CLIENTEDGE,
    WS_OVERLAPPEDWINDOW, WS_TABSTOP, WS_VISIBLE,
};

// ---- Control ids ----
const ID_DRIVE_BASE: u16 = 1000;
const ID_STOP_BTN: u16 = 200;
const ID_LIST: u16 = 300;
const ID_TREE: u16 = 301;
const ID_TREEMAP: u16 = 302;
const ID_STATUS: u16 = 400;

// Accelerator + context-menu IDs share the WM_COMMAND space.
const ID_ACC_REFRESH: u16 = 3001; // F5
const ID_ACC_STOP: u16 = 3002; // Esc
const ID_ACC_PARENT: u16 = 3003; // Backspace
const ID_ACC_DRILL: u16 = 3004; // Enter
const ID_ACC_DELETE: u16 = 3005; // Del

const ID_CTX_OPEN: u16 = 4001;
const ID_CTX_COPY: u16 = 4002;
const ID_CTX_CMD: u16 = 4003;
const ID_CTX_RECYCLE: u16 = 4004;

// Menu bar IDs
const ID_MENU_REFRESH: u16 = 5001;
const ID_MENU_EXIT: u16 = 5002;
const ID_MENU_RELAUNCH_ADMIN: u16 = 5003;
const ID_MENU_THEME_AUTO: u16 = 5101;
const ID_MENU_THEME_LIGHT: u16 = 5102;
const ID_MENU_THEME_DARK: u16 = 5103;
const ID_MENU_ABOUT: u16 = 5200;
const ID_MENU_VIEW_TREE: u16 = 5301;
const ID_MENU_VIEW_TOPFILES: u16 = 5302;
const ID_MENU_VIEW_OLDEST: u16 = 5303;
const ID_MENU_VIEW_TEMP: u16 = 5304;
const ID_MENU_VIEW_TREEMAP: u16 = 5305;

// Number of files shown in the file-based views (top largest / oldest).
const TOP_N_FILES: usize = 100;

// Treemap tiles thinner than this many pixels aren't emitted (and their
// subtrees aren't descended into) — they'd be invisible anyway.
const TREEMAP_MIN_PX: f64 = 3.0;
const TREEMAP_MAX_DEPTH: u32 = 24;

// Custom messages
const WM_APP_PROGRESS: u32 = WM_APP + 1;
const WM_APP_DONE: u32 = WM_APP + 2;
const WM_APP_TEMP_DONE: u32 = WM_APP + 3;

// Virtual key codes (avoid pulling another module just for these)
const VK_F5: u16 = 0x74;
const VK_ESCAPE: u16 = 0x1B;
const VK_BACK: u16 = 0x08;
const VK_RETURN: u16 = 0x0D;
const VK_DELETE: u16 = 0x2E;

// ---- Drive info ----
#[derive(Clone)]
struct DriveInfo {
    letter: char,
    root: String,
    label: String,
    #[allow(dead_code)]
    fs: String,
    total_bytes: u64,
    free_bytes: u64,
    is_ntfs: bool,
}

// ---- Shared scan state ----
#[derive(Default)]
struct ScanState {
    last_progress: ScanProgress,
    result: Option<Result<FolderNode, String>>,
}

#[derive(Copy, Clone, Default, PartialEq)]
enum ThemeMode {
    #[default]
    Auto,
    Light,
    Dark,
}

#[derive(Copy, Clone, Default, PartialEq)]
enum ViewMode {
    #[default]
    FolderTree,
    TopFiles,
    OldestFiles,
    TempFiles,
    Treemap,
}

// One painted tile of the treemap. Raw pointers into the pinned root_node
// tree — same lifetime argument as the tree/list lParams: the scan result is
// never mutated after completion, and the entries are cleared in start_scan
// before the old tree drops.
struct TreemapEntry {
    rect: RECT,
    // Owning folder: the folder itself for folder tiles, the containing
    // folder for file tiles.
    folder: *const FolderNode,
    file: *const FileEntry, // null for folder tiles
    depth: u32,
    hue_idx: usize,
}

struct AppState {
    drives: Vec<DriveInfo>,
    drive_buttons: Vec<HWND>,
    stop_btn: HWND,
    tree: HWND,
    list: HWND,
    treemap: HWND,
    status: HWND,

    scanning: bool,
    cancel: Arc<AtomicBool>,
    shared: Arc<Mutex<ScanState>>,
    is_admin: bool,

    // Pinned by being inside AppState; never mutated after scan completes,
    // so &-pointers into its children Vec stay valid for the lifetime of
    // the scan result.
    root_node: Option<Box<FolderNode>>,
    // FolderNode pointer -> HTREEITEM handle, for selecting an item by node.
    item_by_node: HashMap<isize, isize>,
    // HTREEITEM handles that have had their children populated.
    populated: HashSet<isize>,
    // Path of the FolderNode currently selected in the tree (for context menu).
    selected_node: isize,
    // Last scan request — remembered so F5 re-scans the same drive.
    last_scan: Option<(String, bool)>,
    theme_mode: ThemeMode,
    is_dark: bool,
    menu: HMENU,
    view_mode: ViewMode,

    // Independent of the drive-scan tree: flat list of files discovered under
    // the known "safe-to-delete" temp locations. Populated by start_temp_scan.
    temp_entries: Vec<TempFileEntry>,
    temp_shared: Arc<Mutex<Option<Vec<TempFileEntry>>>>,

    // Treemap view: laid-out tiles in paint order (parents before children,
    // so reverse hit-testing finds the deepest tile), plus selection/hover
    // indices into that Vec (-1 = none).
    treemap_entries: Vec<TreemapEntry>,
    treemap_selected: i32,
    treemap_hover: i32,
}

#[derive(Copy, Clone)]
struct SendHwnd(isize);
unsafe impl Send for SendHwnd {}
unsafe impl Sync for SendHwnd {}
impl SendHwnd {
    fn to_hwnd(self) -> HWND {
        HWND(self.0 as _)
    }
}

pub fn run() {
    unsafe {
        let icex = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_LISTVIEW_CLASSES
                | ICC_TREEVIEW_CLASSES
                | ICC_BAR_CLASSES
                | ICC_STANDARD_CLASSES,
        };
        let _ = InitCommonControlsEx(&icex);

        let hinstance = GetModuleHandleW(None).expect("GetModuleHandle");

        let class_name = w!("ClutterCutterMain");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: Default::default(),
            lpfnWndProc: Some(wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinstance.into(),
            hIcon: LoadIconW(None, IDI_APPLICATION).unwrap_or_default(),
            hCursor: LoadCursorW(None, IDC_ARROW).expect("cursor"),
            hbrBackground: GetSysColorBrush(COLOR_BTNFACE),
            lpszMenuName: PCWSTR::null(),
            lpszClassName: class_name,
            hIconSm: LoadIconW(None, IDI_APPLICATION).unwrap_or_default(),
        };
        if RegisterClassExW(&wc) == 0 {
            return;
        }

        let app = Box::new(AppState {
            drives: enumerate_drives(),
            drive_buttons: Vec::new(),
            stop_btn: HWND::default(),
            tree: HWND::default(),
            list: HWND::default(),
            treemap: HWND::default(),
            status: HWND::default(),
            scanning: false,
            cancel: Arc::new(AtomicBool::new(false)),
            shared: Arc::new(Mutex::new(ScanState::default())),
            is_admin: IsUserAnAdmin().as_bool(),
            root_node: None,
            item_by_node: HashMap::new(),
            populated: HashSet::new(),
            selected_node: 0,
            last_scan: None,
            theme_mode: ThemeMode::Auto,
            is_dark: false,
            menu: HMENU::default(),
            view_mode: ViewMode::FolderTree,
            temp_entries: Vec::new(),
            temp_shared: Arc::new(Mutex::new(None)),
            treemap_entries: Vec::new(),
            treemap_selected: -1,
            treemap_hover: -1,
        });
        let app_ptr = Box::into_raw(app);

        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class_name,
            w!("ClutterCutter"),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1100,
            720,
            HWND::default(),
            HMENU::default(),
            hinstance,
            Some(app_ptr as _),
        )
        .expect("CreateWindowExW");

        // Accelerator table
        let accels: [ACCEL; 5] = [
            accel(VK_F5, ID_ACC_REFRESH),
            accel(VK_ESCAPE, ID_ACC_STOP),
            accel(VK_BACK, ID_ACC_PARENT),
            accel(VK_RETURN, ID_ACC_DRILL),
            accel(VK_DELETE, ID_ACC_DELETE),
        ];
        let haccel = CreateAcceleratorTableW(&accels).unwrap_or_default();

        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = UpdateWindow(hwnd);

        let mut msg = MSG::default();
        loop {
            let r = GetMessageW(&mut msg, HWND::default(), 0, 0);
            if r.0 == 0 || r.0 == -1 {
                break;
            }
            if TranslateAcceleratorW(hwnd, haccel, &msg) == 0 {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

fn accel(vk: u16, cmd: u16) -> ACCEL {
    ACCEL {
        fVirt: windows::Win32::UI::WindowsAndMessaging::ACCEL_VIRT_FLAGS(FVIRTKEY.0),
        key: vk,
        cmd,
    }
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_NCCREATE {
        let cs = lparam.0 as *const CREATESTRUCTW;
        let app_ptr = (*cs).lpCreateParams as isize;
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, app_ptr);
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }

    let app_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState;
    if app_ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let app = &mut *app_ptr;

    match msg {
        WM_CREATE => {
            create_children(hwnd, app);
            LRESULT(0)
        }
        WM_SIZE => {
            layout(hwnd, app);
            LRESULT(0)
        }
        WM_COMMAND => {
            on_command(hwnd, app, (wparam.0 & 0xFFFF) as u16);
            LRESULT(0)
        }
        WM_NOTIFY => on_notify(hwnd, app, lparam),
        m if m == WM_APP_PROGRESS => {
            on_progress(app);
            LRESULT(0)
        }
        m if m == WM_APP_DONE => {
            on_scan_done(app);
            LRESULT(0)
        }
        m if m == WM_APP_TEMP_DONE => {
            on_temp_scan_done(app);
            LRESULT(0)
        }
        WM_DESTROY => {
            app.cancel.store(true, Ordering::SeqCst);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            drop(Box::from_raw(app_ptr));
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn on_command(hwnd: HWND, app: &mut AppState, id: u16) {
    match id {
        ID_STOP_BTN | ID_ACC_STOP => {
            if app.scanning {
                app.cancel.store(true, Ordering::SeqCst);
            }
        }
        ID_MENU_EXIT => {
            let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(hwnd);
        }
        ID_MENU_RELAUNCH_ADMIN => {
            relaunch_elevated();
        }
        ID_MENU_ABOUT => {
            show_about(hwnd);
        }
        ID_MENU_THEME_AUTO => apply_theme(hwnd, app, ThemeMode::Auto),
        ID_MENU_THEME_LIGHT => apply_theme(hwnd, app, ThemeMode::Light),
        ID_MENU_THEME_DARK => apply_theme(hwnd, app, ThemeMode::Dark),
        ID_MENU_VIEW_TREE => apply_view_mode(hwnd, app, ViewMode::FolderTree),
        ID_MENU_VIEW_TOPFILES => apply_view_mode(hwnd, app, ViewMode::TopFiles),
        ID_MENU_VIEW_OLDEST => apply_view_mode(hwnd, app, ViewMode::OldestFiles),
        ID_MENU_VIEW_TEMP => apply_view_mode(hwnd, app, ViewMode::TempFiles),
        ID_MENU_VIEW_TREEMAP => apply_view_mode(hwnd, app, ViewMode::Treemap),
        ID_MENU_REFRESH | ID_ACC_REFRESH => {
            if !app.scanning {
                match app.view_mode {
                    ViewMode::TempFiles => start_temp_scan(hwnd, app),
                    _ => {
                        if let Some((path, use_mft)) = app.last_scan.clone() {
                            start_scan(hwnd, app, path, use_mft);
                        }
                    }
                }
            }
        }
        _ => on_command_more(hwnd, app, id),
    }
}

unsafe fn on_command_more(hwnd: HWND, app: &mut AppState, id: u16) {
    match id {
        ID_ACC_PARENT => {
            // Select parent in tree
            let cur = SendMessageW(
                app.tree,
                TVM_GETNEXTITEM,
                WPARAM(TVGN_CARET as usize),
                LPARAM(0),
            );
            if cur.0 != 0 {
                let parent = SendMessageW(
                    app.tree,
                    TVM_GETNEXTITEM,
                    WPARAM(TVGN_PARENT as usize),
                    LPARAM(cur.0),
                );
                if parent.0 != 0 {
                    SendMessageW(
                        app.tree,
                        TVM_SELECTITEM,
                        WPARAM(TVGN_CARET as usize),
                        LPARAM(parent.0),
                    );
                }
            }
        }
        ID_ACC_DRILL | ID_CTX_OPEN => {
            if app.view_mode == ViewMode::Treemap {
                if let Some((folder, file)) = treemap_selected_ptrs(app) {
                    if id == ID_ACC_DRILL {
                        if file.is_null() {
                            select_tree_node(app, folder);
                        }
                    } else {
                        // Files open their containing folder; folders open themselves.
                        open_in_explorer(&(*folder).full_path);
                    }
                }
            } else if let Some(node) = selected_list_node(app) {
                // Drill into the selected list row by selecting its tree item
                if id == ID_ACC_DRILL {
                    let p = node as *const _ as isize;
                    if let Some(&hti) = app.item_by_node.get(&p) {
                        SendMessageW(
                            app.tree,
                            TVM_SELECTITEM,
                            WPARAM(TVGN_CARET as usize),
                            LPARAM(hti),
                        );
                    }
                } else {
                    open_in_explorer(&node.full_path);
                }
            }
        }
        ID_ACC_DELETE | ID_CTX_RECYCLE => {
            handle_recycle(hwnd, app);
        }
        ID_CTX_COPY => {
            if app.view_mode == ViewMode::Treemap {
                if let Some(path) = treemap_selected_path(app) {
                    copy_to_clipboard(hwnd, &path);
                }
            } else if let Some(node) = selected_list_node(app) {
                copy_to_clipboard(hwnd, &node.full_path);
            }
        }
        ID_CTX_CMD => {
            if app.view_mode == ViewMode::Treemap {
                if let Some((folder, _)) = treemap_selected_ptrs(app) {
                    open_cmd_at(&(*folder).full_path);
                }
            } else if let Some(node) = selected_list_node(app) {
                open_cmd_at(&node.full_path);
            }
        }
        id if id >= ID_DRIVE_BASE && !app.scanning => {
            let idx = (id - ID_DRIVE_BASE) as usize;
            if let Some(drive) = app.drives.get(idx).cloned() {
                let use_mft = drive.is_ntfs && app.is_admin;
                start_scan(hwnd, app, drive.root, use_mft);
            }
        }
        _ => {}
    }
}

unsafe fn on_notify(hwnd: HWND, app: &mut AppState, lparam: LPARAM) -> LRESULT {
    let hdr = &*(lparam.0 as *const NMHDR);
    if hdr.hwndFrom == app.tree {
        match hdr.code {
            c if c == TVN_SELCHANGEDW => {
                on_tree_select(app);
            }
            c if c == TVN_ITEMEXPANDINGW => {
                let info = &*(lparam.0 as *const windows::Win32::UI::Controls::NMTREEVIEWW);
                if info.action == TVE_EXPAND {
                    on_tree_expand(app, info.itemNew.hItem.0);
                }
            }
            _ => {}
        }
    } else if hdr.hwndFrom == app.list {
        match hdr.code {
            c if c == NM_DBLCLK => {
                let act = &*(lparam.0 as *const NMITEMACTIVATE);
                if act.iItem >= 0 {
                    if let Some(node) = nth_visible_node(app, act.iItem) {
                        let p = node as *const _ as isize;
                        if let Some(&hti) = app.item_by_node.get(&p) {
                            SendMessageW(
                                app.tree,
                                TVM_SELECTITEM,
                                WPARAM(TVGN_CARET as usize),
                                LPARAM(hti),
                            );
                        }
                    }
                }
            }
            c if c == NM_RCLICK => {
                show_context_menu(hwnd, app);
            }
            _ => {}
        }
    }
    LRESULT(0)
}

unsafe fn create_children(hwnd: HWND, app: &mut AppState) {
    let hinstance = GetModuleHandleW(None).expect("GetModuleHandle");

    build_menu_bar(hwnd, app);

    for (i, drive) in app.drives.iter().enumerate() {
        let label = format!(
            "{}:\\  {}\n{} / {}",
            drive.letter,
            if drive.label.is_empty() {
                "(no label)"
            } else {
                &drive.label
            },
            format_bytes((drive.total_bytes - drive.free_bytes) as i64),
            format_bytes(drive.total_bytes as i64),
        );
        let label_w: Vec<u16> = label.encode_utf16().chain(std::iter::once(0)).collect();
        let btn = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("BUTTON"),
            PCWSTR(label_w.as_ptr()),
            WS_CHILD
                | WS_VISIBLE
                | WS_TABSTOP
                | WINDOW_STYLE(BS_PUSHBUTTON as u32)
                | WINDOW_STYLE(0x0000_2000), // BS_MULTILINE
            10 + (i as i32) * 170,
            10,
            160,
            60,
            hwnd,
            HMENU((ID_DRIVE_BASE + i as u16) as isize as _),
            hinstance,
            None,
        )
        .expect("drive button");
        app.drive_buttons.push(btn);
    }

    let stop_x = 10 + (app.drives.len() as i32) * 170 + 10;
    app.stop_btn = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        w!("BUTTON"),
        w!("Stop"),
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_PUSHBUTTON as u32),
        stop_x,
        20,
        80,
        40,
        hwnd,
        HMENU(ID_STOP_BTN as isize as _),
        hinstance,
        None,
    )
    .expect("stop btn");
    let _ = EnableWindow(app.stop_btn, false);

    app.tree = CreateWindowExW(
        WS_EX_CLIENTEDGE,
        w!("SysTreeView32"),
        PCWSTR::null(),
        WS_CHILD
            | WS_VISIBLE
            | WS_TABSTOP
            | WS_BORDER
            | WINDOW_STYLE(
                TVS_HASBUTTONS
                    | TVS_HASLINES
                    | TVS_LINESATROOT
                    | TVS_SHOWSELALWAYS
                    | TVS_TRACKSELECT,
            ),
        0,
        80,
        320,
        500,
        hwnd,
        HMENU(ID_TREE as isize as _),
        hinstance,
        None,
    )
    .expect("treeview");

    app.list = CreateWindowExW(
        WS_EX_CLIENTEDGE,
        w!("SysListView32"),
        PCWSTR::null(),
        WS_CHILD
            | WS_VISIBLE
            | WS_TABSTOP
            | WINDOW_STYLE(LVS_REPORT)
            | WINDOW_STYLE(LVS_SHOWSELALWAYS),
        320,
        80,
        780,
        500,
        hwnd,
        HMENU(ID_LIST as isize as _),
        hinstance,
        None,
    )
    .expect("listview");

    let ext = (LVS_EX_FULLROWSELECT | LVS_EX_GRIDLINES | LVS_EX_DOUBLEBUFFER) as isize;
    SendMessageW(
        app.list,
        LVM_SETEXTENDEDLISTVIEWSTYLE,
        WPARAM(0),
        LPARAM(ext),
    );

    // Treemap canvas — custom-painted sibling of the listview, shown only in
    // Treemap view. Its WndProc finds AppState via its own GWLP_USERDATA.
    let tm_class = w!("ClutterCutterTreemap");
    let tm_wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW | CS_DBLCLKS,
        lpfnWndProc: Some(treemap_proc),
        hInstance: hinstance.into(),
        hCursor: LoadCursorW(None, IDC_ARROW).expect("cursor"),
        hbrBackground: HBRUSH::default(), // fully painted in WM_PAINT
        lpszClassName: tm_class,
        ..Default::default()
    };
    RegisterClassExW(&tm_wc);
    app.treemap = CreateWindowExW(
        WS_EX_CLIENTEDGE,
        tm_class,
        PCWSTR::null(),
        WS_CHILD, // hidden until the Treemap view is activated
        320,
        80,
        780,
        500,
        hwnd,
        HMENU(ID_TREEMAP as isize as _),
        hinstance,
        None,
    )
    .expect("treemap");
    SetWindowLongPtrW(app.treemap, GWLP_USERDATA, app as *mut AppState as isize);
    insert_column(app.list, 0, "Name", 320, false);
    insert_column(app.list, 1, "Size", 130, true);
    insert_column(app.list, 2, "Files", 100, true);
    insert_column(app.list, 3, "Folders", 100, true);

    let status_initial = if app.is_admin {
        "Ready (Administrator — MFT fast path available on NTFS drives)"
    } else {
        "Ready (not elevated — FindFirstFile walker on all drives)"
    };
    let init_w: Vec<u16> = status_initial
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    app.status = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        w!("msctls_statusbar32"),
        PCWSTR(init_w.as_ptr()),
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(SBARS_SIZEGRIP),
        0,
        0,
        0,
        0,
        hwnd,
        HMENU(ID_STATUS as isize as _),
        hinstance,
        None,
    )
    .expect("status bar");

    // Apply initial theme.
    apply_theme(hwnd, app, ThemeMode::Auto);
}

unsafe fn build_menu_bar(hwnd: HWND, app: &mut AppState) {
    let menu = CreateMenu().expect("CreateMenu");

    let file_pop = CreatePopupMenu().expect("CreatePopupMenu file");
    let _ = AppendMenuW(
        file_pop,
        MF_STRING,
        ID_MENU_REFRESH as usize,
        w!("&Refresh\tF5"),
    );
    if !app.is_admin {
        let _ = AppendMenuW(
            file_pop,
            MF_STRING,
            ID_MENU_RELAUNCH_ADMIN as usize,
            w!("Restart as &Administrator..."),
        );
    }
    let _ = AppendMenuW(file_pop, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(file_pop, MF_STRING, ID_MENU_EXIT as usize, w!("E&xit"));
    let _ = AppendMenuW(menu, MF_POPUP, file_pop.0 as usize, w!("&File"));

    let view_pop = CreatePopupMenu().expect("CreatePopupMenu view");
    let _ = AppendMenuW(
        view_pop,
        MF_STRING,
        ID_MENU_VIEW_TREE as usize,
        w!("&Folder tree"),
    );
    let _ = AppendMenuW(
        view_pop,
        MF_STRING,
        ID_MENU_VIEW_TREEMAP as usize,
        w!("Tree&map"),
    );
    let _ = AppendMenuW(
        view_pop,
        MF_STRING,
        ID_MENU_VIEW_TOPFILES as usize,
        w!("&Top largest files"),
    );
    let _ = AppendMenuW(
        view_pop,
        MF_STRING,
        ID_MENU_VIEW_OLDEST as usize,
        w!("&Oldest files (by date modified)"),
    );
    let _ = AppendMenuW(
        view_pop,
        MF_STRING,
        ID_MENU_VIEW_TEMP as usize,
        w!("&Safe-to-delete temp files"),
    );
    let _ = AppendMenuW(view_pop, MF_SEPARATOR, 0, PCWSTR::null());
    let theme_pop = CreatePopupMenu().expect("CreatePopupMenu theme");
    let _ = AppendMenuW(
        theme_pop,
        MF_STRING,
        ID_MENU_THEME_AUTO as usize,
        w!("&Auto (system)"),
    );
    let _ = AppendMenuW(
        theme_pop,
        MF_STRING,
        ID_MENU_THEME_LIGHT as usize,
        w!("&Light"),
    );
    let _ = AppendMenuW(
        theme_pop,
        MF_STRING,
        ID_MENU_THEME_DARK as usize,
        w!("&Dark"),
    );
    let _ = AppendMenuW(view_pop, MF_POPUP, theme_pop.0 as usize, w!("T&heme"));
    let _ = AppendMenuW(menu, MF_POPUP, view_pop.0 as usize, w!("&View"));

    let help_pop = CreatePopupMenu().expect("CreatePopupMenu help");
    let _ = AppendMenuW(
        help_pop,
        MF_STRING,
        ID_MENU_ABOUT as usize,
        w!("&About ClutterCutter"),
    );
    let _ = AppendMenuW(menu, MF_POPUP, help_pop.0 as usize, w!("&Help"));

    let _ = SetMenu(hwnd, menu);
    let _ = DrawMenuBar(hwnd);
    app.menu = menu;

    // Initially check Auto theme + FolderTree view.
    let _ = CheckMenuRadioItem(
        menu,
        ID_MENU_THEME_AUTO as u32,
        ID_MENU_THEME_DARK as u32,
        ID_MENU_THEME_AUTO as u32,
        MF_BYCOMMAND.0,
    );
    let _ = CheckMenuRadioItem(
        menu,
        ID_MENU_VIEW_TREE as u32,
        ID_MENU_VIEW_TREEMAP as u32,
        ID_MENU_VIEW_TREE as u32,
        MF_BYCOMMAND.0,
    );
}

unsafe fn layout(hwnd: HWND, app: &mut AppState) {
    SendMessageW(app.status, WM_SIZE, WPARAM(0), LPARAM(0));

    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    let mut status_rc = RECT::default();
    let _ = GetClientRect(app.status, &mut status_rc);
    let status_h = status_rc.bottom - status_rc.top;

    let top = 80;
    let body_h = (rc.bottom - top - status_h).max(0);
    let tree_w = 320;
    let _ = MoveWindow(app.tree, 0, top, tree_w, body_h, true);
    let _ = MoveWindow(app.list, tree_w, top, rc.right - tree_w, body_h, true);
    let _ = MoveWindow(app.treemap, tree_w, top, rc.right - tree_w, body_h, true);
}

unsafe fn start_scan(hwnd: HWND, app: &mut AppState, path: String, use_mft: bool) {
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
    // Entries point into the tree that's about to drop — clear before it does.
    app.treemap_entries.clear();
    app.treemap_selected = -1;
    app.treemap_hover = -1;
    let _ = InvalidateRect(app.treemap, None, true);
    set_status(
        app.status,
        &format!(
            "Scanning {} ({})...",
            path,
            if use_mft { "MFT" } else { "walker" }
        ),
    );
    app.last_scan = Some((path.clone(), use_mft));
    app.cancel.store(false, Ordering::SeqCst);
    app.scanning = true;
    let _ = EnableWindow(app.stop_btn, true);
    for b in &app.drive_buttons {
        let _ = EnableWindow(*b, false);
    }

    let send_hwnd = SendHwnd(hwnd.0 as isize);
    let shared = app.shared.clone();
    let cancel = app.cancel.clone();
    let progress_shared = shared.clone();
    let progress: ProgressFn = Box::new(move |p| {
        if let Ok(mut s) = progress_shared.lock() {
            s.last_progress = p.clone();
        }
        unsafe {
            let _ = PostMessageW(send_hwnd.to_hwnd(), WM_APP_PROGRESS, WPARAM(0), LPARAM(0));
        }
    });

    std::thread::spawn(move || {
        let result = if use_mft {
            MftScanner::new()
                .with_cancel(cancel)
                .with_progress(progress)
                .with_track_files(true)
                .scan(&path)
        } else {
            Scanner::new()
                .with_cancel(cancel)
                .with_progress(progress)
                .with_track_files(true)
                .scan(&path)
                .map_err(|e| e.to_string())
        };
        if let Ok(mut s) = shared.lock() {
            s.result = Some(result);
        }
        unsafe {
            let _ = PostMessageW(send_hwnd.to_hwnd(), WM_APP_DONE, WPARAM(0), LPARAM(0));
        }
    });
}

fn on_progress(app: &AppState) {
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

unsafe fn on_scan_done(app: &mut AppState) {
    let result = {
        let mut s = app.shared.lock().unwrap();
        s.result.take()
    };
    app.scanning = false;
    let _ = EnableWindow(app.stop_btn, false);
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

    let summary = format!(
        "{} — {} ({} files, {} folders)",
        node.name,
        format_bytes(node.size),
        format_count(node.file_count),
        format_count(node.folder_count),
    );

    app.root_node = Some(Box::new(node));
    // Insert root item; lazy-populate children as the user expands.
    // Use a raw pointer so we drop the &-borrow before calling &mut methods.
    let root_ptr: *const FolderNode = app
        .root_node
        .as_deref()
        .map(|r| r as *const _)
        .unwrap_or(std::ptr::null());
    if !root_ptr.is_null() {
        let root: &FolderNode = &*root_ptr;
        let hti = insert_tree_item(app.tree, 0, root);
        app.item_by_node.insert(root_ptr as isize, hti);
        populate_children(app, hti, root);
        SendMessageW(
            app.tree,
            TVM_SELECTITEM,
            WPARAM(TVGN_CARET as usize),
            LPARAM(hti),
        );
    }
    // The tree-selection above repopulates the listview (FolderTree) or the
    // treemap canvas (Treemap) via on_tree_select. File-based views ignore
    // tree selection; populate the global ranking directly. TempFiles is
    // independent of drive scans entirely.
    match app.view_mode {
        ViewMode::FolderTree | ViewMode::TempFiles | ViewMode::Treemap => {}
        ViewMode::TopFiles => populate_list_top_files(app),
        ViewMode::OldestFiles => populate_list_oldest_files(app),
    }

    set_status(app.status, &summary);
}

unsafe fn populate_children(app: &mut AppState, parent_hti: isize, parent: &FolderNode) {
    if !app.populated.insert(parent_hti) {
        return;
    }
    let mut kids: Vec<&FolderNode> = parent.children.iter().collect();
    kids.sort_by_key(|n| std::cmp::Reverse(n.size));
    for c in kids {
        // Only insert subdirectories as tree items; leaf-like nodes (no children)
        // still appear because every FolderNode here is a directory.
        let hti = insert_tree_item(app.tree, parent_hti, c);
        let p = c as *const _ as isize;
        app.item_by_node.insert(p, hti);
    }
}

unsafe fn insert_tree_item(tree: HWND, parent_hti: isize, node: &FolderNode) -> isize {
    let mut name_w: Vec<u16> = node.name.encode_utf16().chain(std::iter::once(0)).collect();
    let has_children = if node.children.is_empty() { 0 } else { 1 };
    let item = TVITEMW {
        mask: TVIF_TEXT | TVIF_PARAM | TVIF_CHILDREN,
        pszText: PWSTR(name_w.as_mut_ptr()),
        cchTextMax: name_w.len() as i32,
        cChildren: windows::Win32::UI::Controls::TVITEMEXW_CHILDREN(has_children),
        lParam: LPARAM(node as *const _ as isize),
        ..Default::default()
    };
    let ins = windows::Win32::UI::Controls::TVINSERTSTRUCTW {
        hParent: windows::Win32::UI::Controls::HTREEITEM(parent_hti as _),
        hInsertAfter: windows::Win32::UI::Controls::HTREEITEM(
            windows::Win32::UI::Controls::TVI_LAST.0 as _,
        ),
        Anonymous: windows::Win32::UI::Controls::TVINSERTSTRUCTW_0 { item },
    };
    let r = SendMessageW(
        tree,
        TVM_INSERTITEMW,
        WPARAM(0),
        LPARAM(&ins as *const _ as isize),
    );
    r.0 as isize
}

unsafe fn on_tree_expand(app: &mut AppState, hti: isize) {
    // Look up the FolderNode for this item and populate its children if not yet done.
    let lparam = tree_item_lparam(app.tree, hti);
    if lparam == 0 {
        return;
    }
    let node: &FolderNode = &*(lparam as *const FolderNode);
    if !app.populated.contains(&hti) {
        populate_children(app, hti, node);
    }
}

unsafe fn on_tree_select(app: &mut AppState) {
    let hti = SendMessageW(
        app.tree,
        TVM_GETNEXTITEM,
        WPARAM(TVGN_CARET as usize),
        LPARAM(0),
    )
    .0 as isize;
    if hti == 0 {
        return;
    }
    let lparam = tree_item_lparam(app.tree, hti);
    if lparam == 0 {
        return;
    }
    app.selected_node = lparam;
    if app.view_mode == ViewMode::Treemap {
        rebuild_treemap(app);
    } else {
        let node: &FolderNode = &*(lparam as *const FolderNode);
        populate_list(app, node);
    }
}

unsafe fn populate_list(app: &AppState, node: &FolderNode) {
    match app.view_mode {
        ViewMode::FolderTree => populate_list_folders(app, node),
        ViewMode::TopFiles | ViewMode::OldestFiles | ViewMode::TempFiles => {
            // File-based and temp views show global rankings independent of the
            // tree selection; repopulating on tree-select would be wasteful and
            // would clobber the temp view entirely.
        }
        ViewMode::Treemap => {
            // Handled in on_tree_select via rebuild_treemap (needs &mut).
        }
    }
}

unsafe fn populate_list_folders(app: &AppState, node: &FolderNode) {
    SendMessageW(app.list, LVM_DELETEALLITEMS, WPARAM(0), LPARAM(0));
    let mut kids: Vec<&FolderNode> = node.children.iter().collect();
    kids.sort_by_key(|n| std::cmp::Reverse(n.size));
    for (i, k) in kids.iter().enumerate() {
        insert_row_with_param(
            app.list,
            i as i32,
            &k.name,
            &[
                format_bytes(k.size),
                format_count(k.file_count),
                format_count(k.folder_count),
            ],
            *k as *const _ as isize,
        );
    }
}

unsafe fn populate_list_top_files(app: &AppState) {
    populate_list_from_hits(app, |root| top_n_files(root, TOP_N_FILES));
}

unsafe fn populate_list_oldest_files(app: &AppState) {
    populate_list_from_hits(app, |root| oldest_n_files(root, TOP_N_FILES));
}

unsafe fn populate_list_temp(app: &AppState) {
    SendMessageW(app.list, LVM_DELETEALLITEMS, WPARAM(0), LPARAM(0));
    for (i, e) in app.temp_entries.iter().enumerate() {
        let p = std::path::Path::new(&e.full_path);
        let leaf = p
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| e.full_path.clone());
        let folder = p
            .parent()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        // lParam carries the index into app.temp_entries so multi-select
        // recycle can recover the full path without re-parsing the listview.
        insert_row_with_param(
            app.list,
            i as i32,
            &leaf,
            &[
                format_bytes(e.size),
                format_filetime(e.last_modified_ft),
                e.source.label().to_string(),
                folder,
            ],
            i as isize,
        );
    }
}

unsafe fn start_temp_scan(hwnd: HWND, app: &mut AppState) {
    if app.scanning {
        return;
    }
    let locations = temp::discover_locations();
    if locations.is_empty() {
        set_status(app.status, "No known temp locations found on this system.");
        return;
    }

    {
        let mut s = app.temp_shared.lock().unwrap();
        *s = None;
    }
    SendMessageW(app.list, LVM_DELETEALLITEMS, WPARAM(0), LPARAM(0));
    app.temp_entries.clear();

    let summary = locations
        .iter()
        .map(|(s, _)| s.label())
        .collect::<Vec<_>>()
        .join(", ");
    set_status(
        app.status,
        &format!("Scanning temp locations: {summary}..."),
    );
    app.cancel.store(false, Ordering::SeqCst);
    app.scanning = true;
    let _ = EnableWindow(app.stop_btn, true);
    for b in &app.drive_buttons {
        let _ = EnableWindow(*b, false);
    }

    let send_hwnd = SendHwnd(hwnd.0 as isize);
    let shared = app.temp_shared.clone();
    let cancel = app.cancel.clone();
    std::thread::spawn(move || {
        let entries = temp::scan_locations(&locations, cancel);
        if let Ok(mut s) = shared.lock() {
            *s = Some(entries);
        }
        unsafe {
            let _ = PostMessageW(send_hwnd.to_hwnd(), WM_APP_TEMP_DONE, WPARAM(0), LPARAM(0));
        }
    });
}

unsafe fn on_temp_scan_done(app: &mut AppState) {
    let entries = {
        let mut s = app.temp_shared.lock().unwrap();
        s.take()
    };
    app.scanning = false;
    let _ = EnableWindow(app.stop_btn, false);
    for b in &app.drive_buttons {
        let _ = EnableWindow(*b, true);
    }

    let Some(entries) = entries else {
        set_status(app.status, "Temp scan cancelled.");
        return;
    };

    let total: i64 = entries.iter().map(|e| e.size).sum();
    let count = entries.len();
    app.temp_entries = entries;

    if app.view_mode == ViewMode::TempFiles {
        populate_list_temp(app);
    }
    set_status(
        app.status,
        &format!(
            "{} temp files — {} reclaimable. Ctrl/Shift-click to multi-select, then Del to recycle.",
            format_count(count as i64),
            format_bytes(total),
        ),
    );
}

unsafe fn handle_recycle(hwnd: HWND, app: &mut AppState) {
    if app.view_mode == ViewMode::Treemap {
        if let Some(path) = treemap_selected_path(app) {
            recycle(&path);
            if let Some((scan_path, use_mft)) = app.last_scan.clone() {
                if !app.scanning {
                    start_scan(hwnd, app, scan_path, use_mft);
                }
            }
        }
        return;
    }

    if app.view_mode == ViewMode::TempFiles {
        let indices = selected_indices(app.list);
        if indices.is_empty() {
            return;
        }
        // Borrow paths out so we can drop the borrow before mutating app.
        let paths: Vec<String> = indices
            .iter()
            .filter_map(|&i| app.temp_entries.get(i as usize))
            .map(|e| e.full_path.clone())
            .collect();
        if paths.is_empty() {
            return;
        }
        let path_refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        recycle_many(&path_refs);
        if !app.scanning {
            start_temp_scan(hwnd, app);
        }
        return;
    }

    if let Some(node) = selected_list_node(app) {
        recycle(&node.full_path);
        if let Some((path, use_mft)) = app.last_scan.clone() {
            if !app.scanning {
                start_scan(hwnd, app, path, use_mft);
            }
        }
    }
}

unsafe fn populate_list_from_hits<F>(app: &AppState, query: F)
where
    F: for<'a> FnOnce(&'a FolderNode) -> Vec<crate::analysis::FileHit<'a>>,
{
    SendMessageW(app.list, LVM_DELETEALLITEMS, WPARAM(0), LPARAM(0));
    let root_ptr = match app.root_node.as_deref() {
        Some(r) => r as *const FolderNode,
        None => return,
    };
    let root: &FolderNode = &*root_ptr;
    let hits = query(root);
    for (i, h) in hits.iter().enumerate() {
        let full_path = join_path(&h.folder.full_path, &h.file.name);
        insert_row_with_param(
            app.list,
            i as i32,
            &h.file.name,
            &[
                format_bytes(h.file.size),
                format_filetime(h.file.last_modified_ft),
                full_path,
            ],
            0, // No FolderNode ptr for files; double-click navigation is folder-only
        );
    }
}

unsafe fn apply_view_mode(hwnd: HWND, app: &mut AppState, mode: ViewMode) {
    if app.view_mode == mode {
        return;
    }
    app.view_mode = mode;

    // Drain all existing columns. Views differ in column count (folder = 4,
    // file views = 4, temp = 5), so loop until LVM_DELETECOLUMN returns 0.
    while SendMessageW(app.list, LVM_DELETECOLUMN, WPARAM(0), LPARAM(0)).0 != 0 {}
    SendMessageW(app.list, LVM_DELETEALLITEMS, WPARAM(0), LPARAM(0));

    match mode {
        ViewMode::FolderTree => {
            insert_column(app.list, 0, "Name", 320, false);
            insert_column(app.list, 1, "Size", 130, true);
            insert_column(app.list, 2, "Files", 100, true);
            insert_column(app.list, 3, "Folders", 100, true);
            if let Some(root) = app.root_node.as_deref() {
                let sel_ptr = if app.selected_node != 0 {
                    app.selected_node as *const FolderNode
                } else {
                    root as *const FolderNode
                };
                populate_list_folders(app, &*sel_ptr);
            }
        }
        ViewMode::TopFiles => {
            insert_column(app.list, 0, "Name", 280, false);
            insert_column(app.list, 1, "Size", 110, true);
            insert_column(app.list, 2, "Modified", 130, false);
            insert_column(app.list, 3, "Path", 600, false);
            populate_list_top_files(app);
        }
        ViewMode::OldestFiles => {
            insert_column(app.list, 0, "Name", 280, false);
            insert_column(app.list, 1, "Size", 110, true);
            insert_column(app.list, 2, "Modified", 130, false);
            insert_column(app.list, 3, "Path", 600, false);
            populate_list_oldest_files(app);
        }
        ViewMode::TempFiles => {
            insert_column(app.list, 0, "Name", 240, false);
            insert_column(app.list, 1, "Size", 100, true);
            insert_column(app.list, 2, "Modified", 120, false);
            insert_column(app.list, 3, "Source", 110, false);
            insert_column(app.list, 4, "Folder", 550, false);
            if app.temp_entries.is_empty() && !app.scanning {
                start_temp_scan(hwnd, app);
            } else {
                populate_list_temp(app);
            }
        }
        ViewMode::Treemap => {
            // No listview columns — the treemap canvas replaces the listview.
            rebuild_treemap(app);
        }
    }

    // The treemap canvas and the listview swap places depending on the view.
    let treemap_mode = mode == ViewMode::Treemap;
    let _ = ShowWindow(app.list, if treemap_mode { SW_HIDE } else { SW_SHOW });
    let _ = ShowWindow(app.treemap, if treemap_mode { SW_SHOW } else { SW_HIDE });

    if !app.menu.is_invalid() {
        let id = match mode {
            ViewMode::FolderTree => ID_MENU_VIEW_TREE,
            ViewMode::TopFiles => ID_MENU_VIEW_TOPFILES,
            ViewMode::OldestFiles => ID_MENU_VIEW_OLDEST,
            ViewMode::TempFiles => ID_MENU_VIEW_TEMP,
            ViewMode::Treemap => ID_MENU_VIEW_TREEMAP,
        } as u32;
        let _ = CheckMenuRadioItem(
            app.menu,
            ID_MENU_VIEW_TREE as u32,
            ID_MENU_VIEW_TREEMAP as u32,
            id,
            MF_BYCOMMAND.0,
        );
    }
}

// ---- Treemap view ----

// Recomputes the tile layout for the current tree selection (or the scan root)
// and repaints. Cheap enough to run on tree-select and resize: pure math over
// the already-scanned tree, pruned at TREEMAP_MIN_PX.
unsafe fn rebuild_treemap(app: &mut AppState) {
    app.treemap_entries.clear();
    app.treemap_selected = -1;
    app.treemap_hover = -1;
    let root_ptr: *const FolderNode = if app.selected_node != 0 {
        app.selected_node as *const FolderNode
    } else {
        match app.root_node.as_deref() {
            Some(r) => r,
            None => {
                let _ = InvalidateRect(app.treemap, None, true);
                return;
            }
        }
    };
    let mut rc = RECT::default();
    let _ = GetClientRect(app.treemap, &mut rc);
    let bounds = Rectf {
        x: rc.left as f64,
        y: rc.top as f64,
        w: (rc.right - rc.left) as f64,
        h: (rc.bottom - rc.top) as f64,
    };
    build_treemap_level(&mut app.treemap_entries, &*root_ptr, bounds, 0, None);
    let _ = InvalidateRect(app.treemap, None, true);
}

// Emits tiles for one folder's contents (subfolders + direct files) into
// `bounds`, then recurses into each subfolder tile. `hue` is None only at the
// top level, where each item founds its own color family.
fn build_treemap_level(
    entries: &mut Vec<TreemapEntry>,
    folder: &FolderNode,
    bounds: Rectf,
    depth: u32,
    hue: Option<usize>,
) {
    if depth > TREEMAP_MAX_DEPTH || bounds.w < TREEMAP_MIN_PX || bounds.h < TREEMAP_MIN_PX {
        return;
    }
    enum Item<'a> {
        Folder(&'a FolderNode),
        File(&'a FileEntry),
    }
    let mut items: Vec<(i64, Item)> = folder
        .children
        .iter()
        .map(|c| (c.size, Item::Folder(c)))
        .chain(folder.files.iter().map(|f| (f.size, Item::File(f))))
        .collect();
    // Descending size gives the squarified layout its best aspect ratios.
    items.sort_by_key(|(s, _)| std::cmp::Reverse(*s));
    let sizes: Vec<i64> = items.iter().map(|(s, _)| *s).collect();
    let rects = treemap::layout(&sizes, bounds);
    for (i, ((_, item), r)) in items.iter().zip(&rects).enumerate() {
        if r.w < TREEMAP_MIN_PX || r.h < TREEMAP_MIN_PX {
            continue;
        }
        let hue_idx = hue.unwrap_or(i);
        let rect = RECT {
            left: r.x.round() as i32,
            top: r.y.round() as i32,
            right: (r.x + r.w).round() as i32,
            bottom: (r.y + r.h).round() as i32,
        };
        match item {
            Item::Folder(c) => {
                entries.push(TreemapEntry {
                    rect,
                    folder: *c as *const _,
                    file: std::ptr::null(),
                    depth,
                    hue_idx,
                });
                // Inset children by 1px so the folder's own border stays visible.
                let inner = Rectf {
                    x: r.x + 1.0,
                    y: r.y + 1.0,
                    w: r.w - 2.0,
                    h: r.h - 2.0,
                };
                build_treemap_level(entries, c, inner, depth + 1, Some(hue_idx));
            }
            Item::File(f) => {
                entries.push(TreemapEntry {
                    rect,
                    folder: folder as *const _,
                    file: *f as *const _,
                    depth,
                    hue_idx,
                });
            }
        }
    }
}

unsafe extern "system" fn treemap_proc(
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
        WM_ERASEBKGND => LRESULT(1), // fully painted in WM_PAINT (flicker-free)
        WM_PAINT => {
            paint_treemap(hwnd, app);
            LRESULT(0)
        }
        WM_SIZE => {
            if app.view_mode == ViewMode::Treemap {
                rebuild_treemap(app);
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let (x, y) = lparam_xy(lparam);
            let hit = treemap_hit_test(app, x, y);
            if hit != app.treemap_hover {
                app.treemap_hover = hit;
                if hit >= 0 {
                    let text = treemap_entry_status(&app.treemap_entries[hit as usize]);
                    set_status(app.status, &text);
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let (x, y) = lparam_xy(lparam);
            let hit = treemap_hit_test(app, x, y);
            if hit != app.treemap_selected {
                app.treemap_selected = hit;
                let _ = InvalidateRect(hwnd, None, false);
            }
            LRESULT(0)
        }
        WM_LBUTTONDBLCLK => {
            let (x, y) = lparam_xy(lparam);
            let hit = treemap_hit_test(app, x, y);
            if hit >= 0 {
                let e = &app.treemap_entries[hit as usize];
                let (folder, file) = (e.folder, e.file);
                if file.is_null() {
                    select_tree_node(app, folder);
                }
            }
            LRESULT(0)
        }
        WM_RBUTTONDOWN => {
            let (x, y) = lparam_xy(lparam);
            let hit = treemap_hit_test(app, x, y);
            app.treemap_selected = hit;
            let _ = InvalidateRect(hwnd, None, false);
            if hit >= 0 {
                // Route the shared context menu through the main window so its
                // WM_COMMAND handlers fire there.
                show_context_menu(main_window_of(app), app);
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

// The treemap canvas is a direct child of the main window; commands from the
// shared context menu must go to the parent's WndProc.
unsafe fn main_window_of(app: &AppState) -> HWND {
    windows::Win32::UI::WindowsAndMessaging::GetParent(app.treemap).unwrap_or_default()
}

fn lparam_xy(lparam: LPARAM) -> (i32, i32) {
    let x = (lparam.0 & 0xFFFF) as u16 as i16 as i32;
    let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
    (x, y)
}

// Entries are stored parents-before-children, so scanning backwards returns
// the deepest tile under the cursor (siblings never overlap).
fn treemap_hit_test(app: &AppState, x: i32, y: i32) -> i32 {
    for (i, e) in app.treemap_entries.iter().enumerate().rev() {
        if x >= e.rect.left && x < e.rect.right && y >= e.rect.top && y < e.rect.bottom {
            return i as i32;
        }
    }
    -1
}

unsafe fn treemap_selected_ptrs(app: &AppState) -> Option<(*const FolderNode, *const FileEntry)> {
    if app.treemap_selected < 0 {
        return None;
    }
    app.treemap_entries
        .get(app.treemap_selected as usize)
        .map(|e| (e.folder, e.file))
}

unsafe fn treemap_selected_path(app: &AppState) -> Option<String> {
    treemap_selected_ptrs(app).map(|(folder, file)| {
        if file.is_null() {
            (*folder).full_path.clone()
        } else {
            join_path(&(*folder).full_path, &(*file).name)
        }
    })
}

unsafe fn treemap_entry_status(e: &TreemapEntry) -> String {
    let folder = &*e.folder;
    if e.file.is_null() {
        format!(
            "{} — {} ({} files)",
            folder.full_path,
            format_bytes(folder.size),
            format_count(folder.file_count),
        )
    } else {
        let f = &*e.file;
        format!(
            "{} — {}",
            join_path(&folder.full_path, &f.name),
            format_bytes(f.size),
        )
    }
}

// Expands/populates the tree down to `target` (tree items are lazily created,
// so ancestors may not have items yet), then selects it — which triggers
// on_tree_select and re-roots the treemap.
unsafe fn select_tree_node(app: &mut AppState, target: *const FolderNode) {
    let root_ptr: *const FolderNode = match app.root_node.as_deref() {
        Some(r) => r,
        None => return,
    };
    let mut path: Vec<*const FolderNode> = Vec::new();
    if !find_node_path(&*root_ptr, target, &mut path) {
        return;
    }
    for win in path.windows(2) {
        let (parent, child) = (win[0], win[1]);
        if !app.item_by_node.contains_key(&(child as isize)) {
            if let Some(&phti) = app.item_by_node.get(&(parent as isize)) {
                populate_children(app, phti, &*parent);
            }
        }
    }
    if let Some(&hti) = app.item_by_node.get(&(target as isize)) {
        SendMessageW(
            app.tree,
            TVM_SELECTITEM,
            WPARAM(TVGN_CARET as usize),
            LPARAM(hti),
        );
    }
}

// DFS for the pointer-path root..=target. Recursion depth = folder nesting
// depth, which Windows caps well below any stack concern.
fn find_node_path(
    cur: &FolderNode,
    target: *const FolderNode,
    path: &mut Vec<*const FolderNode>,
) -> bool {
    path.push(cur as *const _);
    if std::ptr::eq(cur, target) {
        return true;
    }
    for c in &cur.children {
        if find_node_path(c, target, path) {
            return true;
        }
    }
    path.pop();
    false
}

// ---- Treemap painting ----

const PALETTE_HUES: [f64; 12] = [
    210.0, 30.0, 130.0, 275.0, 55.0, 0.0, 180.0, 315.0, 95.0, 240.0, 160.0, 340.0,
];

fn tile_color(hue_idx: usize, depth: u32, is_file: bool, is_dark: bool) -> u32 {
    let hue = PALETTE_HUES[hue_idx % PALETTE_HUES.len()];
    let sat = if is_file { 0.18 } else { 0.50 };
    let dl = 0.035 * depth.min(8) as f64;
    // Deeper tiles drift toward the theme's foreground so nesting reads.
    let l = if is_dark {
        (0.28 + dl).min(0.55)
    } else {
        (0.62 - dl).max(0.35)
    };
    hsl_to_colorref(hue, sat, l)
}

// COLORREF is 0x00BBGGRR.
fn hsl_to_colorref(h: f64, s: f64, l: f64) -> u32 {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h.rem_euclid(360.0) / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r1, g1, b1) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    let to8 = |v: f64| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u32;
    (to8(b1) << 16) | (to8(g1) << 8) | to8(r1)
}

unsafe fn paint_treemap(hwnd: HWND, app: &AppState) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    let (w, h) = (rc.right - rc.left, rc.bottom - rc.top);
    if w <= 0 || h <= 0 {
        let _ = EndPaint(hwnd, &ps);
        return;
    }

    // Off-screen buffer: tiles overdraw their parents, so painting direct
    // would flicker badly.
    let mem = CreateCompatibleDC(hdc);
    let bmp = CreateCompatibleBitmap(hdc, w, h);
    let old = SelectObject(mem, bmp);

    let bg = if app.is_dark {
        0x0020_2020
    } else {
        0x00FF_FFFF
    };
    let bg_brush = CreateSolidBrush(COLORREF(bg));
    FillRect(mem, &rc, bg_brush);
    let _ = DeleteObject(bg_brush);

    let border = if app.is_dark {
        0x0010_1010
    } else {
        0x00D0_D0D0
    };
    let border_brush = CreateSolidBrush(COLORREF(border));
    // Few distinct colors (hue × depth × kind), many tiles — cache brushes.
    let mut brushes: HashMap<u32, windows::Win32::Graphics::Gdi::HBRUSH> = HashMap::new();
    for e in &app.treemap_entries {
        let color = tile_color(e.hue_idx, e.depth, !e.file.is_null(), app.is_dark);
        let brush = *brushes
            .entry(color)
            .or_insert_with(|| CreateSolidBrush(COLORREF(color)));
        FillRect(mem, &e.rect, brush);
        FrameRect(mem, &e.rect, border_brush);
    }
    for (_, b) in brushes {
        let _ = DeleteObject(b);
    }
    let _ = DeleteObject(border_brush);

    if app.treemap_selected >= 0 {
        if let Some(e) = app.treemap_entries.get(app.treemap_selected as usize) {
            // 2px accent frame (#3399FF) so the selection reads on any tile color.
            let sel = CreateSolidBrush(COLORREF(0x00FF_9933));
            FrameRect(mem, &e.rect, sel);
            let inner = RECT {
                left: e.rect.left + 1,
                top: e.rect.top + 1,
                right: (e.rect.right - 1).max(e.rect.left + 1),
                bottom: (e.rect.bottom - 1).max(e.rect.top + 1),
            };
            FrameRect(mem, &inner, sel);
            let _ = DeleteObject(sel);
        }
    }

    let _ = BitBlt(hdc, 0, 0, w, h, mem, 0, 0, SRCCOPY);
    SelectObject(mem, old);
    let _ = DeleteObject(bmp);
    let _ = DeleteDC(mem);
    let _ = EndPaint(hwnd, &ps);
}

fn format_filetime(raw: i64) -> String {
    if raw == 0 {
        return String::new();
    }
    let ft = FILETIME {
        dwLowDateTime: raw as u32,
        dwHighDateTime: (raw >> 32) as u32,
    };
    let mut utc = SYSTEMTIME::default();
    let mut local = SYSTEMTIME::default();
    unsafe {
        if FileTimeToSystemTime(&ft, &mut utc).is_err() {
            return String::new();
        }
        if SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).is_err() {
            return String::new();
        }
    }
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        local.wYear, local.wMonth, local.wDay, local.wHour, local.wMinute
    )
}

unsafe fn tree_item_lparam(tree: HWND, hti: isize) -> isize {
    let mut item = TVITEMW {
        mask: TVIF_PARAM,
        hItem: windows::Win32::UI::Controls::HTREEITEM(hti as _),
        ..Default::default()
    };
    SendMessageW(
        tree,
        TVM_GETITEMW,
        WPARAM(0),
        LPARAM(&mut item as *mut _ as isize),
    );
    item.lParam.0
}

unsafe fn selected_indices(list: HWND) -> Vec<i32> {
    let mut out = Vec::new();
    let mut idx: i32 = -1;
    loop {
        let r = SendMessageW(
            list,
            LVM_GETNEXTITEM,
            WPARAM(idx as usize),
            LPARAM(LVNI_SELECTED as isize),
        );
        let next = r.0 as i32;
        if next < 0 {
            break;
        }
        out.push(next);
        idx = next;
    }
    out
}

unsafe fn selected_list_index(list: HWND) -> i32 {
    let r = SendMessageW(
        list,
        LVM_GETNEXTITEM,
        WPARAM((-1isize) as usize),
        LPARAM(LVNI_SELECTED as isize),
    );
    r.0 as i32
}

unsafe fn nth_visible_node(app: &AppState, idx: i32) -> Option<&'static FolderNode> {
    let mut item = LVITEMW {
        mask: windows::Win32::UI::Controls::LVIF_PARAM,
        iItem: idx,
        ..Default::default()
    };
    let r = SendMessageW(
        app.list,
        LVM_GETITEMW,
        WPARAM(0),
        LPARAM(&mut item as *mut _ as isize),
    );
    if r.0 == 0 {
        return None;
    }
    let p = item.lParam.0;
    if p == 0 {
        return None;
    }
    Some(&*(p as *const FolderNode))
}

unsafe fn selected_list_node(app: &AppState) -> Option<&'static FolderNode> {
    let idx = selected_list_index(app.list);
    if idx < 0 {
        return None;
    }
    nth_visible_node(app, idx)
}

unsafe fn show_context_menu(hwnd: HWND, _app: &AppState) {
    let menu = match CreatePopupMenu() {
        Ok(m) => m,
        Err(_) => return,
    };
    let _ = AppendMenuW(
        menu,
        MF_STRING,
        ID_CTX_OPEN as usize,
        w!("Open in Explorer"),
    );
    let _ = AppendMenuW(menu, MF_STRING, ID_CTX_COPY as usize, w!("Copy path"));
    let _ = AppendMenuW(
        menu,
        MF_STRING,
        ID_CTX_CMD as usize,
        w!("Open Command Prompt here"),
    );
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(
        menu,
        MF_STRING,
        ID_CTX_RECYCLE as usize,
        w!("Move to Recycle Bin"),
    );

    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let _ = SetForegroundWindow(hwnd);
    let _ = TrackPopupMenu(
        menu,
        TPM_LEFTALIGN | TPM_RIGHTBUTTON,
        pt.x,
        pt.y,
        0,
        hwnd,
        None,
    );
    let _ = DestroyMenu(menu);
}

// ---- Theme ----

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

unsafe fn apply_theme(hwnd: HWND, app: &mut AppState, mode: ThemeMode) {
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
    let _ = SetWindowTheme(app.list, PCWSTR(theme_w.as_ptr()), PCWSTR::null());
    let _ = SetWindowTheme(app.tree, PCWSTR(theme_w.as_ptr()), PCWSTR::null());

    let (bg, fg): (u32, u32) = if is_dark {
        // COLORREF is 0x00BBGGRR
        (0x00202020, 0x00E0E0E0)
    } else {
        (0x00FFFFFF, 0x00000000)
    };
    SendMessageW(app.list, LVM_SETBKCOLOR, WPARAM(0), LPARAM(bg as isize));
    SendMessageW(app.list, LVM_SETTEXTCOLOR, WPARAM(0), LPARAM(fg as isize));
    SendMessageW(app.list, LVM_SETTEXTBKCOLOR, WPARAM(0), LPARAM(bg as isize));
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
    let _ = InvalidateRect(hwnd, None, true);
    let _ = InvalidateRect(app.treemap, None, true);
}

// ---- About dialog + admin relaunch ----

unsafe fn show_about(hwnd: HWND) {
    let text = w!("ClutterCutter — Rust port\n\
         Version 0.0.1\n\
         © Struis ICT\n\
         \n\
         Lightweight Windows disk-usage browser.\n\
         FindFirstFileEx walker + NTFS MFT fast path.");
    let _ = MessageBoxW(
        hwnd,
        text,
        w!("About ClutterCutter"),
        MB_OK | MB_ICONINFORMATION,
    );
}

fn relaunch_elevated() {
    if let Ok(exe) = std::env::current_exe() {
        let exe_str = exe.to_string_lossy().into_owned();
        let exe_w: Vec<u16> = exe_str.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            let _ = ShellExecuteW(
                HWND::default(),
                w!("runas"),
                PCWSTR(exe_w.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_NORMAL,
            );
        }
        std::process::exit(0);
    }
}

// ---- Shell actions ----

fn open_in_explorer(path: &str) {
    unsafe {
        let path_w = wide(path);
        let _ = ShellExecuteW(
            HWND::default(),
            w!("open"),
            PCWSTR(path_w.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_NORMAL,
        );
    }
}

fn open_cmd_at(path: &str) {
    unsafe {
        let path_w = wide(path);
        let _ = ShellExecuteW(
            HWND::default(),
            w!("open"),
            w!("cmd.exe"),
            PCWSTR::null(),
            PCWSTR(path_w.as_ptr()),
            SW_NORMAL,
        );
    }
}

fn recycle(path: &str) {
    recycle_many(&[path]);
}

// Bulk recycle via SHFileOperationW. pFrom is a double-null-terminated list
// of single-null-terminated wide paths — one syscall regardless of count, so
// the user sees one undoable operation in the Recycle Bin.
fn recycle_many(paths: &[&str]) {
    if paths.is_empty() {
        return;
    }
    unsafe {
        let mut buf: Vec<u16> = Vec::new();
        for p in paths {
            buf.extend(p.encode_utf16());
            buf.push(0);
        }
        buf.push(0);
        let mut op = SHFILEOPSTRUCTW {
            hwnd: HWND::default(),
            wFunc: FO_DELETE,
            pFrom: PCWSTR(buf.as_ptr()),
            pTo: PCWSTR::null(),
            fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION).0 as u16,
            fAnyOperationsAborted: false.into(),
            hNameMappings: std::ptr::null_mut(),
            lpszProgressTitle: PCWSTR::null(),
        };
        let _ = SHFileOperationW(&mut op);
    }
}

fn copy_to_clipboard(hwnd: HWND, text: &str) {
    unsafe {
        if OpenClipboard(hwnd).is_err() {
            return;
        }
        let _ = EmptyClipboard();
        let utf16: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let bytes = utf16.len() * 2;
        if let Ok(h) = GlobalAlloc(GMEM_MOVEABLE, bytes) {
            let ptr = GlobalLock(h) as *mut u16;
            if !ptr.is_null() {
                std::ptr::copy_nonoverlapping(utf16.as_ptr(), ptr, utf16.len());
                let _ = GlobalUnlock(h);
                let _ = SetClipboardData(
                    CF_UNICODETEXT.0 as u32,
                    windows::Win32::Foundation::HANDLE(h.0),
                );
            }
        }
        let _ = CloseClipboard();
    }
}

// ---- Drive enumeration ----

fn enumerate_drives() -> Vec<DriveInfo> {
    let mask = unsafe { GetLogicalDrives() };
    let mut out = Vec::new();
    for i in 0..26 {
        if mask & (1u32 << i) == 0 {
            continue;
        }
        let letter = (b'A' + i as u8) as char;
        let root = format!("{letter}:\\");
        let root_w = wide(&root);
        let drive_type = unsafe { GetDriveTypeW(PCWSTR(root_w.as_ptr())) };
        if drive_type != DRIVE_FIXED && drive_type != DRIVE_REMOVABLE {
            continue;
        }
        let mut total: u64 = 0;
        let mut free: u64 = 0;
        let _ = unsafe {
            GetDiskFreeSpaceExW(
                PCWSTR(root_w.as_ptr()),
                None,
                Some(&mut total as *mut u64 as *mut _),
                Some(&mut free as *mut u64 as *mut _),
            )
        };
        let mut label_buf = [0u16; 64];
        let mut fs_buf = [0u16; 20];
        let mut serial = 0u32;
        let mut max_len = 0u32;
        let mut flags = 0u32;
        let _ = unsafe {
            GetVolumeInformationW(
                PCWSTR(root_w.as_ptr()),
                Some(&mut label_buf),
                Some(&mut serial),
                Some(&mut max_len),
                Some(&mut flags),
                Some(&mut fs_buf),
            )
        };
        let label = wstr_to_string(&label_buf);
        let fs = wstr_to_string(&fs_buf);
        let is_ntfs = is_ntfs_drive_root(&root) || fs.eq_ignore_ascii_case("NTFS");
        out.push(DriveInfo {
            letter,
            root,
            label,
            fs,
            total_bytes: total,
            free_bytes: free,
            is_ntfs,
        });
    }
    out
}

// ---- ListView helpers ----

unsafe fn insert_column(list: HWND, idx: i32, title: &str, width: i32, right_align: bool) {
    let mut text: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    let col = LVCOLUMNW {
        mask: LVCF_TEXT | LVCF_WIDTH | LVCF_FMT,
        fmt: if right_align {
            LVCFMT_RIGHT
        } else {
            LVCFMT_LEFT
        },
        cx: width,
        pszText: PWSTR(text.as_mut_ptr()),
        ..Default::default()
    };
    SendMessageW(
        list,
        LVM_INSERTCOLUMNW,
        WPARAM(idx as usize),
        LPARAM(&col as *const _ as isize),
    );
}

unsafe fn insert_row_with_param(
    list: HWND,
    idx: i32,
    name: &str,
    subitems: &[String],
    lparam: isize,
) {
    let mut name_w: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let item = LVITEMW {
        mask: LVIF_TEXT | windows::Win32::UI::Controls::LVIF_PARAM,
        iItem: idx,
        iSubItem: 0,
        pszText: PWSTR(name_w.as_mut_ptr()),
        lParam: LPARAM(lparam),
        ..Default::default()
    };
    SendMessageW(
        list,
        LVM_INSERTITEMW,
        WPARAM(0),
        LPARAM(&item as *const _ as isize),
    );
    for (si, text) in subitems.iter().enumerate() {
        let mut sub_w: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let sub = LVITEMW {
            mask: LVIF_TEXT,
            iItem: idx,
            iSubItem: (si + 1) as i32,
            pszText: PWSTR(sub_w.as_mut_ptr()),
            ..Default::default()
        };
        SendMessageW(
            list,
            LVM_SETITEMTEXTW,
            WPARAM(idx as usize),
            LPARAM(&sub as *const _ as isize),
        );
    }
}

unsafe fn set_status(status: HWND, text: &str) {
    let mut w: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    SendMessageW(
        status,
        SB_SETTEXTW,
        WPARAM(0),
        LPARAM(w.as_mut_ptr() as isize),
    );
}

// ---- Formatting ----

fn join_path(dir: &str, leaf: &str) -> String {
    if dir.ends_with('\\') {
        format!("{dir}{leaf}")
    } else {
        format!("{dir}\\{leaf}")
    }
}

fn format_bytes(n: i64) -> String {
    if n < 1024 {
        return format!("{n} B");
    }
    let mut v = n as f64 / 1024.0;
    let units = ["KB", "MB", "GB", "TB", "PB"];
    let mut i = 0;
    while v >= 1024.0 && i < units.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if v >= 100.0 {
        format!("{v:.0} {}", units[i])
    } else if v >= 10.0 {
        format!("{v:.1} {}", units[i])
    } else {
        format!("{v:.2} {}", units[i])
    }
}

fn format_count(n: i64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let neg = bytes.first() == Some(&b'-');
    let digits = if neg { &bytes[1..] } else { bytes };
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    if neg {
        out.push('-');
    }
    let first_chunk = digits.len() % 3;
    if first_chunk > 0 {
        out.push_str(std::str::from_utf8(&digits[..first_chunk]).unwrap());
    }
    for (i, c) in digits[first_chunk..].iter().enumerate() {
        if i % 3 == 0 && !(first_chunk == 0 && i == 0) {
            out.push(',');
        }
        out.push(*c as char);
    }
    out
}
