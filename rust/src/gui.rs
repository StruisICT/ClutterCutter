// Win32 GUI for ClutterCutter — raw window class + message loop + WndProc, no
// GUI framework.
//
//   [drive buttons] [Scan all] [Stop]
//   [TreeView] | [ListView] | [side panel: top files / oldest / temp]
//   [status bar]
//
// Drive buttons auto-pick MFT vs FindFirstFileEx walker (MFT when NTFS + admin);
// "Scan all" walks every drive into one synthetic root. Scans run on a worker
// thread; progress/results are posted back via WM_APP messages. The tree and
// the selected folder's list are always visible; the View menu picks an extra
// view for the side panel, which can detach into its own floating window.
// Right-click on a row/tile opens an Explorer/Copy/Cmd/Recycle menu; F5
// re-scans, Esc stops, Backspace goes to parent, Enter drills, Del recycles.

use crate::analysis::{oldest_n_files, top_n_files};
use crate::mft::{is_ntfs_drive_root, MftScanner};
use crate::scanner::{wide, wstr_to_string, ProgressFn, Scanner};
use crate::temp::{self, TempFileEntry};
use crate::types::{FileEntry, FolderNode, ScanProgress};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use windows::core::{w, PCSTR, PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    BOOL, COLORREF, FILETIME, HWND, LPARAM, LRESULT, POINT, RECT, SYSTEMTIME, WPARAM,
};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect, GetSysColorBrush,
    GetWindowDC, InvalidateRect, MapWindowPoints, RedrawWindow, ReleaseDC, ScreenToClient,
    SetBkMode, SetTextColor, UpdateWindow, COLOR_BTNFACE, DT_CENTER, DT_END_ELLIPSIS,
    DT_HIDEPREFIX, DT_LEFT, DT_SINGLELINE, DT_VCENTER, HBRUSH, HDC, PAINTSTRUCT, RDW_ALLCHILDREN,
    RDW_ERASE, RDW_FRAME, RDW_INVALIDATE, TRANSPARENT,
};
use windows::Win32::Storage::FileSystem::{
    GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW,
};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
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
    LVM_DELETECOLUMN, LVM_DELETEITEM, LVM_GETHEADER, LVM_GETITEMW, LVM_GETNEXTITEM,
    LVM_INSERTCOLUMNW, LVM_INSERTITEMW, LVM_SETBKCOLOR, LVM_SETCOLUMNWIDTH,
    LVM_SETEXTENDEDLISTVIEWSTYLE, LVM_SETITEMTEXTW, LVM_SETTEXTBKCOLOR, LVM_SETTEXTCOLOR,
    LVNI_SELECTED, LVS_EX_DOUBLEBUFFER, LVS_EX_FULLROWSELECT, LVS_EX_GRIDLINES, LVS_REPORT,
    LVS_SHOWSELALWAYS, NMHDR, NMITEMACTIVATE, NM_DBLCLK, NM_RCLICK, TVE_EXPAND, TVGN_CARET,
    TVGN_PARENT, TVIF_CHILDREN, TVIF_HANDLE, TVIF_PARAM, TVIF_TEXT, TVITEMW, TVI_ROOT,
    TVM_DELETEITEM, TVM_EXPAND, TVM_GETITEMW, TVM_GETNEXTITEM, TVM_INSERTITEMW, TVM_SELECTITEM,
    TVM_SETBKCOLOR, TVM_SETITEMW, TVM_SETTEXTCOLOR, TVN_ITEMEXPANDINGW, TVN_SELCHANGEDW,
    TVS_HASBUTTONS, TVS_HASLINES, TVS_LINESATROOT, TVS_SHOWSELALWAYS, TVS_TRACKSELECT,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    EnableWindow, GetCapture, GetFocus, ReleaseCapture, SetCapture,
};
use windows::Win32::UI::Shell::{
    IsUserAnAdmin, SHFileOperationW, ShellExecuteW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FO_DELETE,
    SHFILEOPSTRUCTW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CheckMenuRadioItem, CreateAcceleratorTableW, CreateMenu, CreatePopupMenu,
    CreateWindowExW, DefWindowProcW, DestroyMenu, DispatchMessageW, DrawMenuBar, GetClientRect,
    GetCursorPos, GetMenuBarInfo, GetMenuItemInfoW, GetMessageW, GetSystemMetrics,
    GetWindowLongPtrW, GetWindowRect, GetWindowTextW, IsDialogMessageW, LoadCursorW, LoadIconW,
    MessageBoxW, MoveWindow, PostMessageW, PostQuitMessage, RegisterClassExW, SendMessageW,
    SetCursor, SetForegroundWindow, SetMenu, SetParent, SetWindowLongPtrW, SetWindowPos,
    SetWindowTextW, ShowWindow, TrackPopupMenu, TranslateAcceleratorW, TranslateMessage, ACCEL,
    BS_PUSHBUTTON, CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, FVIRTKEY, GWLP_USERDATA,
    HMENU, IDC_ARROW, IDC_SIZEWE, IDI_APPLICATION, MB_ICONINFORMATION, MB_OK, MENUBARINFO,
    MENUITEMINFOW, MF_BYCOMMAND, MF_POPUP, MF_SEPARATOR, MF_STRING, MIIM_STRING, MSG, OBJID_MENU,
    SM_CXVSCROLL, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SW_HIDE,
    SW_NORMAL, SW_SHOW, TPM_LEFTALIGN, TPM_RIGHTBUTTON, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP,
    WM_CLOSE, WM_COMMAND, WM_CREATE, WM_DESTROY, WM_ERASEBKGND, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MOUSEMOVE, WM_NCACTIVATE, WM_NCCREATE, WM_NCPAINT, WM_NOTIFY, WM_PAINT, WM_SETCURSOR,
    WM_SIZE, WNDCLASSEXW, WS_BORDER, WS_CHILD, WS_EX_CLIENTEDGE, WS_EX_CONTROLPARENT,
    WS_OVERLAPPEDWINDOW, WS_TABSTOP, WS_VISIBLE,
};

// ---- Control ids ----
const ID_DRIVE_BASE: u16 = 1000;
const ID_STOP_BTN: u16 = 200;
const ID_SCAN_ALL_BTN: u16 = 201;
const ID_BTN_DETACH: u16 = 210;
const ID_BTN_RECYCLE_ALL: u16 = 211;
const ID_LIST: u16 = 300;
const ID_TREE: u16 = 301;
const ID_SIDE_LIST: u16 = 303;
const ID_PANEL: u16 = 304;
const ID_SPLITTER: u16 = 305;
const ID_STATUS: u16 = 400;

// Side panel geometry
const PANEL_W: i32 = 420;
const PANEL_HEADER_H: i32 = 30;
// Draggable divider between the main list and the side panel.
const SPLIT_W: i32 = 6;
// Header button metrics. Buttons are laid out right-to-left with a uniform gap
// and the title is clamped to the left of the leftmost button, so nothing
// overlaps at any panel width.
const PANEL_BTN_GAP: i32 = 6;
const PANEL_BTN_H: i32 = 24;
const DETACH_BTN_W: i32 = 74;
const RECYCLE_BTN_W: i32 = 100;

// Custom status strip height
const STATUS_H: i32 = 24;

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
const ID_MENU_VIEW_NONE: u16 = 5301;
const ID_MENU_VIEW_TOPFILES: u16 = 5302;
const ID_MENU_VIEW_OLDEST: u16 = 5303;
const ID_MENU_VIEW_TEMP: u16 = 5304;
const ID_MENU_VIEW_DETACH: u16 = 5310;

// Number of files shown in the file-based views (top largest / oldest).
const TOP_N_FILES: usize = 100;

// Custom messages
const WM_APP_PROGRESS: u32 = WM_APP + 1;
const WM_APP_DONE: u32 = WM_APP + 2;
const WM_APP_TEMP_DONE: u32 = WM_APP + 3;
// One drive of a scan-all finished; its result is waiting in `drive_inbox`.
const WM_APP_DRIVE_DONE: u32 = WM_APP + 4;
// A background recycle finished; its success flag is in `recycle_result`.
const WM_APP_RECYCLE_DONE: u32 = WM_APP + 5;

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

// What the side panel shows. The tree + selected-folder list are always
// visible; these are the optional extra views.
#[derive(Copy, Clone, Default, PartialEq)]
enum SideView {
    #[default]
    None,
    TopFiles,
    OldestFiles,
    TempFiles,
}

impl SideView {
    fn title(self) -> &'static str {
        match self {
            SideView::None => "",
            SideView::TopFiles => "Top largest files",
            SideView::OldestFiles => "Oldest files",
            SideView::TempFiles => "Safe-to-delete temp files",
        }
    }
}

// Which pane a context-menu / accelerator action targets.
#[derive(Copy, Clone, Default, PartialEq)]
enum CtxTarget {
    #[default]
    MainList,
    SideList,
}

// What F5 should re-run.
#[derive(Clone)]
enum ScanRequest {
    Single(String, bool), // path, use_mft
    AllDrives,
}

struct AppState {
    main_hwnd: HWND,
    drives: Vec<DriveInfo>,
    drive_buttons: Vec<HWND>,
    stop_btn: HWND,
    scan_all_btn: HWND,
    tree: HWND,
    list: HWND,
    status: HWND,

    // Side panel: container (child of main or of the floating frame when
    // detached), its header buttons, the listview that hosts the file-based
    // side views, and the floating frame itself (created lazily).
    panel: HWND,
    side_list: HWND,
    btn_detach: HWND,
    btn_recycle_all: HWND,
    float_win: HWND,
    detached: bool,
    ctx_target: CtxTarget,
    // Draggable divider between the main list and the panel; `panel_frac` is
    // the panel's share of the width after the tree (so it grows on resize and
    // the user can drag it).
    splitter: HWND,
    panel_frac: f64,

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
    // Last scan request — remembered so F5 re-scans the same target.
    last_scan: Option<ScanRequest>,
    theme_mode: ThemeMode,
    is_dark: bool,
    menu: HMENU,
    side_view: SideView,

    // Rows of the file-based side views (top largest / oldest): (owning
    // folder, file) pointer pairs, indexed by the row's lParam. The scan tree
    // is pinned and never mutated after completion, so these stay valid;
    // cleared on the next scan.
    side_hits: Vec<(*const FolderNode, *const FileEntry)>,

    // Independent of the drive-scan tree: flat list of files discovered under
    // the known "safe-to-delete" temp locations. Populated by start_temp_scan.
    temp_entries: Vec<TempFileEntry>,
    temp_shared: Arc<Mutex<Option<Vec<TempFileEntry>>>>,

    // Incremental scan-all: drives are scanned on parallel worker threads and
    // appended to the synthetic root one at a time as they finish. The root's
    // children Vec is pre-reserved to the drive count so these pushes never
    // reallocate — keeping the raw child pointers the tree items hold valid.
    scan_all_active: bool,
    drives_expected: usize,
    drives_done: usize,
    scan_all_first_err: Option<String>,
    drive_inbox: Arc<Mutex<Vec<Result<FolderNode, String>>>>,
    // At-most-one-in-flight guard for WM_APP_PROGRESS so a fast scanner can't
    // flood the message queue and stall the UI.
    progress_pending: Arc<AtomicBool>,

    // Recycling: folders/files are removed from the in-memory tree in place
    // (rather than triggering a full rescan) and the shell delete runs on a
    // background thread. `deleted_nodes` holds the raw pointers of every folder
    // that's been tombstoned — the nodes stay allocated (so all the other raw
    // pointers stay valid) but are skipped by every view. `recycle_result`
    // carries the background op's success flag back for a failure fallback.
    deleted_nodes: HashSet<isize>,
    recycle_result: Arc<Mutex<Option<bool>>>,
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
            main_hwnd: HWND::default(),
            drives: enumerate_drives(),
            drive_buttons: Vec::new(),
            stop_btn: HWND::default(),
            scan_all_btn: HWND::default(),
            tree: HWND::default(),
            list: HWND::default(),
            status: HWND::default(),
            panel: HWND::default(),
            side_list: HWND::default(),
            btn_detach: HWND::default(),
            btn_recycle_all: HWND::default(),
            float_win: HWND::default(),
            splitter: HWND::default(),
            panel_frac: 0.40,
            detached: false,
            ctx_target: CtxTarget::MainList,
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
            side_view: SideView::None,
            side_hits: Vec::new(),
            temp_entries: Vec::new(),
            temp_shared: Arc::new(Mutex::new(None)),
            scan_all_active: false,
            drives_expected: 0,
            drives_done: 0,
            scan_all_first_err: None,
            drive_inbox: Arc::new(Mutex::new(Vec::new())),
            progress_pending: Arc::new(AtomicBool::new(false)),
            deleted_nodes: HashSet::new(),
            recycle_result: Arc::new(Mutex::new(None)),
        });
        let app_ptr = Box::into_raw(app);

        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class_name,
            w!("ClutterCutter"),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1500,
            920,
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

        // Kick off a full scan of every drive right away (alphabetical, since
        // enumerate_drives walks A..Z). The worker posts results back once the
        // message loop below is pumping.
        start_scan_all(hwnd, &mut *app_ptr);

        let mut msg = MSG::default();
        loop {
            let r = GetMessageW(&mut msg, HWND::default(), 0, 0);
            if r.0 == 0 || r.0 == -1 {
                break;
            }
            if TranslateAcceleratorW(hwnd, haccel, &msg) != 0 {
                continue;
            }
            // Dialog navigation, so Tab moves focus between the controls. The
            // floating panel frame needs its own pass — its children aren't
            // under the main window.
            if IsDialogMessageW(hwnd, &msg).as_bool() {
                continue;
            }
            // Re-read the app pointer via the window: it's zeroed in
            // WM_DESTROY, so this never dereferences the freed state.
            let live = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState;
            if !live.is_null() {
                let float = (*live).float_win;
                if !float.is_invalid() && IsDialogMessageW(float, &msg).as_bool() {
                    continue;
                }
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
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
        WM_ERASEBKGND => erase_theme_bg(app, hwnd, HDC(wparam.0 as _)),
        // Dark menu bar: owner-draw via the undocumented UAH messages; light
        // mode falls through to the stock bar.
        m if m == WM_UAHDRAWMENU && app.is_dark => {
            uah_draw_menu_bar_bg(hwnd, &*(lparam.0 as *const UahMenu));
            LRESULT(1)
        }
        m if m == WM_UAHDRAWMENUITEM && app.is_dark => {
            uah_draw_menu_item(&*(lparam.0 as *const UahDrawMenuItem));
            LRESULT(1)
        }
        WM_NCPAINT | WM_NCACTIVATE => {
            let r = DefWindowProcW(hwnd, msg, wparam, lparam);
            if app.is_dark {
                uah_draw_menu_bottom_line(hwnd);
            }
            r
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
        m if m == WM_APP_DRIVE_DONE => {
            on_drive_done(app);
            LRESULT(0)
        }
        m if m == WM_APP_RECYCLE_DONE => {
            on_recycle_done(hwnd, app);
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

// The window-class background brush is fixed at registration, so dark mode
// fills the client area here instead.
unsafe fn erase_theme_bg(app: &AppState, hwnd: HWND, hdc: HDC) -> LRESULT {
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    if app.is_dark {
        let b = CreateSolidBrush(COLORREF(0x0020_2020));
        FillRect(hdc, &rc, b);
        let _ = DeleteObject(b);
    } else {
        FillRect(hdc, &rc, GetSysColorBrush(COLOR_BTNFACE));
    }
    LRESULT(1)
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
        ID_MENU_VIEW_NONE => apply_side_view(hwnd, app, SideView::None),
        ID_MENU_VIEW_TOPFILES => apply_side_view(hwnd, app, SideView::TopFiles),
        ID_MENU_VIEW_OLDEST => apply_side_view(hwnd, app, SideView::OldestFiles),
        ID_MENU_VIEW_TEMP => apply_side_view(hwnd, app, SideView::TempFiles),
        ID_MENU_VIEW_DETACH | ID_BTN_DETACH => toggle_detach(hwnd, app),
        ID_BTN_RECYCLE_ALL => recycle_all_temp(hwnd, app),
        ID_SCAN_ALL_BTN => {
            if !app.scanning {
                start_scan_all(hwnd, app);
            }
        }
        ID_MENU_REFRESH | ID_ACC_REFRESH => {
            if !app.scanning {
                match app.last_scan.clone() {
                    Some(ScanRequest::Single(path, use_mft)) => {
                        start_scan(hwnd, app, path, use_mft)
                    }
                    Some(ScanRequest::AllDrives) => start_scan_all(hwnd, app),
                    None => {
                        if app.side_view == SideView::TempFiles {
                            start_temp_scan(hwnd, app);
                        }
                    }
                }
            }
        }
        _ => on_command_more(hwnd, app, id),
    }
}

// Focus-based action target for keyboard accelerators (Enter/Del): the pane
// that has focus is the one the key should act on.
unsafe fn focus_target(app: &AppState) -> CtxTarget {
    let f = GetFocus();
    if f == app.side_list {
        CtxTarget::SideList
    } else {
        CtxTarget::MainList
    }
}

// Full path of the side list's selected row (file views + temp view).
unsafe fn side_selected_path(app: &AppState) -> Option<String> {
    let idx = selected_list_index(app.side_list);
    if idx < 0 {
        return None;
    }
    side_row_path(app, idx)
}

unsafe fn side_row_path(app: &AppState, row: i32) -> Option<String> {
    let lp = list_item_lparam(app.side_list, row);
    match app.side_view {
        SideView::TempFiles => app
            .temp_entries
            .get(lp as usize)
            .map(|e| e.full_path.clone()),
        SideView::TopFiles | SideView::OldestFiles => app
            .side_hits
            .get(lp as usize)
            .map(|&(folder, file)| join_path(&(*folder).full_path, &(*file).name)),
        _ => None,
    }
}

// Containing folder of the side list's selected row.
unsafe fn side_selected_folder(app: &AppState) -> Option<String> {
    let idx = selected_list_index(app.side_list);
    if idx < 0 {
        return None;
    }
    let lp = list_item_lparam(app.side_list, idx);
    match app.side_view {
        SideView::TempFiles => app.temp_entries.get(lp as usize).and_then(|e| {
            std::path::Path::new(&e.full_path)
                .parent()
                .map(|p| p.to_string_lossy().into_owned())
        }),
        SideView::TopFiles | SideView::OldestFiles => app
            .side_hits
            .get(lp as usize)
            .map(|&(folder, _)| (*folder).full_path.clone()),
        _ => None,
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
            // Enter acts on the focused pane; context-menu commands act on the
            // pane that opened the menu.
            let target = if id == ID_ACC_DRILL {
                focus_target(app)
            } else {
                app.ctx_target
            };
            match target {
                CtxTarget::SideList => {
                    // Side views hold files — open the containing folder.
                    if let Some(folder) = side_selected_folder(app) {
                        open_in_explorer(&folder);
                    }
                }
                CtxTarget::MainList => {
                    if let Some(node) = selected_list_node(app) {
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
                        } else if !node.full_path.is_empty() {
                            open_in_explorer(&node.full_path);
                        }
                    }
                }
            }
        }
        ID_ACC_DELETE | ID_CTX_RECYCLE => {
            let target = if id == ID_ACC_DELETE {
                focus_target(app)
            } else {
                app.ctx_target
            };
            handle_recycle(hwnd, app, target);
        }
        ID_CTX_COPY => {
            let path = match app.ctx_target {
                CtxTarget::SideList => side_selected_path(app),
                CtxTarget::MainList => selected_list_node(app).map(|n| n.full_path.clone()),
            };
            if let Some(path) = path {
                if !path.is_empty() {
                    copy_to_clipboard(hwnd, &path);
                }
            }
        }
        ID_CTX_CMD => {
            let folder = match app.ctx_target {
                CtxTarget::SideList => side_selected_folder(app),
                CtxTarget::MainList => selected_list_node(app).map(|n| n.full_path.clone()),
            };
            if let Some(folder) = folder {
                if !folder.is_empty() {
                    open_cmd_at(&folder);
                }
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
                app.ctx_target = CtxTarget::MainList;
                show_context_menu(hwnd, app);
            }
            _ => {}
        }
    } else if hdr.hwndFrom == app.side_list {
        match hdr.code {
            c if c == NM_DBLCLK => {
                // Side views hold files — open the containing folder.
                if let Some(folder) = side_selected_folder(app) {
                    open_in_explorer(&folder);
                }
            }
            c if c == NM_RCLICK => {
                app.ctx_target = CtxTarget::SideList;
                show_context_menu(hwnd, app);
            }
            _ => {}
        }
    }
    LRESULT(0)
}

unsafe fn create_children(hwnd: HWND, app: &mut AppState) {
    let hinstance = GetModuleHandleW(None).expect("GetModuleHandle");

    app.main_hwnd = hwnd;
    build_menu_bar(hwnd, app);

    // Button bar: [Scan all drives] [C:] [D:] ... [Stop] — one row, uniform
    // 60px height, laid out left to right.
    const BTN_Y: i32 = 10;
    const BTN_H: i32 = 60;
    const BTN_GAP: i32 = 10;
    let mut bar_x = 10;

    app.scan_all_btn = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        w!("BUTTON"),
        w!("Scan all\ndrives"),
        WS_CHILD
            | WS_VISIBLE
            | WS_TABSTOP
            | WINDOW_STYLE(BS_PUSHBUTTON as u32)
            | WINDOW_STYLE(0x0000_2000), // BS_MULTILINE
        bar_x,
        BTN_Y,
        110,
        BTN_H,
        hwnd,
        HMENU(ID_SCAN_ALL_BTN as isize as _),
        hinstance,
        None,
    )
    .expect("scan all btn");
    bar_x += 110 + BTN_GAP;

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
            bar_x,
            BTN_Y,
            160,
            BTN_H,
            hwnd,
            HMENU((ID_DRIVE_BASE + i as u16) as isize as _),
            hinstance,
            None,
        )
        .expect("drive button");
        app.drive_buttons.push(btn);
        bar_x += 160 + BTN_GAP;
    }

    app.stop_btn = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        w!("BUTTON"),
        w!("Stop"),
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_PUSHBUTTON as u32),
        bar_x,
        BTN_Y,
        80,
        BTN_H,
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

    // Side panel — container for the extra views (top files / oldest / temp).
    // Child of the main window while attached; re-parented into the floating
    // frame when detached. Every custom class here finds AppState via its own
    // GWLP_USERDATA.
    let app_lp = app as *mut AppState as isize;
    let panel_class = w!("ClutterCutterPanel");
    let panel_wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(panel_proc),
        hInstance: hinstance.into(),
        hCursor: LoadCursorW(None, IDC_ARROW).expect("cursor"),
        hbrBackground: GetSysColorBrush(COLOR_BTNFACE),
        lpszClassName: panel_class,
        ..Default::default()
    };
    RegisterClassExW(&panel_wc);
    // The floating frame class is registered up-front too; the window itself
    // is created lazily on first detach.
    let float_class = w!("ClutterCutterFloat");
    let float_wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: Default::default(),
        lpfnWndProc: Some(float_proc),
        hInstance: hinstance.into(),
        hIcon: LoadIconW(None, IDI_APPLICATION).unwrap_or_default(),
        hCursor: LoadCursorW(None, IDC_ARROW).expect("cursor"),
        hbrBackground: GetSysColorBrush(COLOR_BTNFACE),
        lpszClassName: float_class,
        ..Default::default()
    };
    RegisterClassExW(&float_wc);

    app.panel = CreateWindowExW(
        WS_EX_CONTROLPARENT, // Tab descends into the panel's controls
        panel_class,
        PCWSTR::null(),
        WS_CHILD, // hidden until a side view is activated
        680,
        80,
        PANEL_W,
        500,
        hwnd,
        HMENU(ID_PANEL as isize as _),
        hinstance,
        None,
    )
    .expect("panel");
    SetWindowLongPtrW(app.panel, GWLP_USERDATA, app_lp);

    // Draggable splitter between the main list and the panel.
    let split_class = w!("ClutterCutterSplitter");
    let split_wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(splitter_proc),
        hInstance: hinstance.into(),
        hCursor: LoadCursorW(None, IDC_SIZEWE).unwrap_or_default(),
        hbrBackground: GetSysColorBrush(COLOR_BTNFACE),
        lpszClassName: split_class,
        ..Default::default()
    };
    RegisterClassExW(&split_wc);
    app.splitter = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        split_class,
        PCWSTR::null(),
        WS_CHILD, // shown by layout() when the panel is attached
        0,
        80,
        SPLIT_W,
        500,
        hwnd,
        HMENU(ID_SPLITTER as isize as _),
        hinstance,
        None,
    )
    .expect("splitter");
    SetWindowLongPtrW(app.splitter, GWLP_USERDATA, app_lp);

    app.btn_detach = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        w!("BUTTON"),
        w!("Detach"),
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_PUSHBUTTON as u32),
        0,
        0,
        70,
        24,
        app.panel,
        HMENU(ID_BTN_DETACH as isize as _),
        hinstance,
        None,
    )
    .expect("detach btn");
    app.btn_recycle_all = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        w!("BUTTON"),
        w!("Recycle all"),
        WS_CHILD | WS_TABSTOP | WINDOW_STYLE(BS_PUSHBUTTON as u32), // temp view only
        0,
        0,
        90,
        24,
        app.panel,
        HMENU(ID_BTN_RECYCLE_ALL as isize as _),
        hinstance,
        None,
    )
    .expect("recycle all btn");

    // Listview hosting the file-based side views.
    app.side_list = CreateWindowExW(
        WS_EX_CLIENTEDGE,
        w!("SysListView32"),
        PCWSTR::null(),
        WS_CHILD | WS_TABSTOP | WINDOW_STYLE(LVS_REPORT) | WINDOW_STYLE(LVS_SHOWSELALWAYS),
        0,
        PANEL_HEADER_H,
        PANEL_W,
        400,
        app.panel,
        HMENU(ID_SIDE_LIST as isize as _),
        hinstance,
        None,
    )
    .expect("side listview");
    SendMessageW(
        app.side_list,
        LVM_SETEXTENDEDLISTVIEWSTYLE,
        WPARAM(0),
        LPARAM(ext),
    );

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
    // Custom status strip instead of msctls_statusbar32: the system status
    // bar has no dark theme part, so it stayed white in dark mode. The text
    // lives in the window text (WM_SETTEXT via set_status) and is painted
    // theme-aware in status_proc.
    let status_class = w!("ClutterCutterStatus");
    let status_wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW,
        lpfnWndProc: Some(status_proc),
        hInstance: hinstance.into(),
        hCursor: LoadCursorW(None, IDC_ARROW).expect("cursor"),
        hbrBackground: HBRUSH::default(), // fully painted in WM_PAINT
        lpszClassName: status_class,
        ..Default::default()
    };
    RegisterClassExW(&status_wc);
    app.status = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        status_class,
        PCWSTR(init_w.as_ptr()),
        WS_CHILD | WS_VISIBLE,
        0,
        0,
        0,
        STATUS_H,
        hwnd,
        HMENU(ID_STATUS as isize as _),
        hinstance,
        None,
    )
    .expect("status bar");
    SetWindowLongPtrW(app.status, GWLP_USERDATA, app_lp);

    // Apply initial theme.
    apply_theme(hwnd, app, ThemeMode::Auto);
}

unsafe extern "system" fn status_proc(
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
            let (bg, fg): (u32, u32) = if app.is_dark {
                (0x002B_2B2B, 0x00E0_E0E0)
            } else {
                (0x00F0_F0F0, 0x0000_0000)
            };
            let brush = CreateSolidBrush(COLORREF(bg));
            FillRect(hdc, &rc, brush);
            let _ = DeleteObject(brush);
            let mut buf = [0u16; 1024];
            let len = GetWindowTextW(hwnd, &mut buf) as usize;
            if len > 0 {
                SetBkMode(hdc, TRANSPARENT);
                SetTextColor(hdc, COLORREF(fg));
                let mut text_rc = RECT {
                    left: 8,
                    top: 0,
                    right: rc.right - 8,
                    bottom: rc.bottom,
                };
                DrawTextW(
                    hdc,
                    &mut buf[..len],
                    &mut text_rc,
                    DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_END_ELLIPSIS,
                );
            }
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
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

    // The tree + selected-folder list are always visible; the View menu picks
    // what the (detachable) side panel shows.
    let view_pop = CreatePopupMenu().expect("CreatePopupMenu view");
    let _ = AppendMenuW(view_pop, MF_STRING, ID_MENU_VIEW_NONE as usize, w!("&None"));
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
    let _ = AppendMenuW(
        view_pop,
        MF_STRING,
        ID_MENU_VIEW_DETACH as usize,
        w!("&Detach side panel"),
    );
    let _ = AppendMenuW(menu, MF_POPUP, view_pop.0 as usize, w!("&View"));

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
    let _ = AppendMenuW(menu, MF_POPUP, theme_pop.0 as usize, w!("&Theme"));

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

    // Initially check Auto theme + no side view.
    let _ = CheckMenuRadioItem(
        menu,
        ID_MENU_THEME_AUTO as u32,
        ID_MENU_THEME_DARK as u32,
        ID_MENU_THEME_AUTO as u32,
        MF_BYCOMMAND.0,
    );
    let _ = CheckMenuRadioItem(
        menu,
        ID_MENU_VIEW_NONE as u32,
        ID_MENU_VIEW_TEMP as u32,
        ID_MENU_VIEW_NONE as u32,
        MF_BYCOMMAND.0,
    );
}

unsafe fn layout(hwnd: HWND, app: &mut AppState) {
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    let _ = MoveWindow(
        app.status,
        0,
        rc.bottom - STATUS_H,
        rc.right,
        STATUS_H,
        true,
    );

    let top = 80;
    let body_h = (rc.bottom - top - STATUS_H).max(0);
    let tree_w = 320;
    let _ = MoveWindow(app.tree, 0, top, tree_w, body_h, true);
    // The side panel takes `panel_frac` of the width after the tree, with a
    // draggable splitter between it and the main list. Because it's a fraction,
    // the panel grows with the window and the user can drag the split.
    let panel_here = app.side_view != SideView::None && !app.detached;
    let split_w = if panel_here { SPLIT_W } else { 0 };
    let avail = (rc.right - tree_w).max(0);
    let panel_w = if panel_here {
        let raw = (avail as f64 * app.panel_frac).round() as i32;
        // Keep both panes usable.
        raw.clamp(180, (avail - split_w - 180).max(180))
    } else {
        0
    };
    let list_w = (rc.right - tree_w - panel_w - split_w).max(0);
    let _ = MoveWindow(app.list, tree_w, top, list_w, body_h, true);
    if panel_here {
        let _ = MoveWindow(app.splitter, tree_w + list_w, top, split_w, body_h, true);
        let _ = ShowWindow(app.splitter, SW_SHOW);
        let _ = MoveWindow(
            app.panel,
            tree_w + list_w + split_w,
            top,
            panel_w,
            body_h,
            true,
        );
    } else {
        let _ = ShowWindow(app.splitter, SW_HIDE);
    }
    // Stretch the Name column so the folder list's columns always fill the
    // list width — otherwise widening the window (which slides the flush-right
    // panel over) leaves a growing block of empty list to the right of the
    // last column. The other three columns are fixed (130 + 100 + 100).
    let vscroll = GetSystemMetrics(SM_CXVSCROLL);
    let name_w = (list_w - 130 - 100 - 100 - vscroll - 4).max(120);
    SendMessageW(
        app.list,
        LVM_SETCOLUMNWIDTH,
        WPARAM(0),
        LPARAM(name_w as isize),
    );
}

// Shared prologue for drive scans: reset all views/state that point into the
// old tree, flip the UI into "scanning" mode.
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
    // These point into the tree that's about to drop — clear before it does.
    app.side_hits.clear();
    // A fresh scan supersedes any in-place deletions.
    app.deleted_nodes.clear();
    if app.side_view == SideView::TopFiles || app.side_view == SideView::OldestFiles {
        SendMessageW(app.side_list, LVM_DELETEALLITEMS, WPARAM(0), LPARAM(0));
    }
    set_status(app.status, status_text);
    app.cancel.store(false, Ordering::SeqCst);
    app.scanning = true;
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
        MftScanner::new()
            .with_cancel(cancel)
            .with_progress(progress)
            .with_track_files(true)
            .scan(path)
    } else {
        Scanner::new()
            .with_cancel(cancel)
            .with_progress(progress)
            .with_track_files(true)
            .scan(path)
            .map_err(|e| e.to_string())
    }
}

unsafe fn start_scan(hwnd: HWND, app: &mut AppState, path: String, use_mft: bool) {
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
unsafe fn start_scan_all(hwnd: HWND, app: &mut AppState) {
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

    // Create the synthetic root now, with capacity reserved for every drive so
    // the incremental pushes in on_drive_done never reallocate the Vec (which
    // would dangle the raw child pointers the tree items hold).
    let mut root = FolderNode {
        full_path: String::new(), // synthetic — shell actions no-op on it
        name: "All drives".to_string(),
        ..Default::default()
    };
    root.children = Vec::with_capacity(n);
    app.root_node = Some(Box::new(root));
    let root_ptr = app.root_node.as_deref().unwrap() as *const FolderNode;
    let hti = insert_tree_item(app.tree, 0, &*root_ptr, false);
    // Root is inserted while its children Vec is still empty, so the tree would
    // treat it as a leaf; force the has-children flag so drives appended later
    // show under an expandable node.
    set_tree_item_has_children(app.tree, hti);
    app.item_by_node.insert(root_ptr as isize, hti);
    app.populated.insert(hti); // drives are appended by hand, not lazily
    app.selected_node = root_ptr as isize;
    SendMessageW(
        app.tree,
        TVM_SELECTITEM,
        WPARAM(TVGN_CARET as usize),
        LPARAM(hti),
    );

    app.scan_all_active = true;
    app.drives_expected = n;
    app.drives_done = 0;
    app.scan_all_first_err = None;
    let inbox = Arc::new(Mutex::new(Vec::new()));
    app.drive_inbox = inbox.clone();

    let send_hwnd = SendHwnd(hwnd.0 as isize);
    for (path, use_mft) in targets {
        let inbox = inbox.clone();
        let cancel = app.cancel.clone();
        let progress = make_progress(send_hwnd, app.shared.clone(), app.progress_pending.clone());
        std::thread::spawn(move || {
            let res =
                scan_one(&path, use_mft, cancel, progress).map_err(|e| format!("{path}: {e}"));
            if let Ok(mut q) = inbox.lock() {
                q.push(res);
            }
            unsafe {
                let _ = PostMessageW(send_hwnd.to_hwnd(), WM_APP_DRIVE_DONE, WPARAM(0), LPARAM(0));
            }
        });
    }
}

// One or more drives finished: drain the inbox, append each into the root, and
// refresh the visible views. Runs on the UI thread.
unsafe fn on_drive_done(app: &mut AppState) {
    if !app.scan_all_active {
        return;
    }
    let drained: Vec<Result<FolderNode, String>> = {
        let mut q = app.drive_inbox.lock().unwrap();
        std::mem::take(&mut *q)
    };
    for res in drained {
        app.drives_done += 1;
        match res {
            Ok(node) => append_drive(app, node),
            Err(e) => {
                if app.scan_all_first_err.is_none() {
                    app.scan_all_first_err = Some(e);
                }
            }
        }
    }

    if app.drives_done >= app.drives_expected {
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

// Pushes a finished drive into the root and inserts its (alphabetically
// sorted) tree item, keeping the root expanded so drives stay visible.
unsafe fn append_drive(app: &mut AppState, node: FolderNode) {
    let root_ptr = match app.root_node.as_deref() {
        Some(r) => r as *const FolderNode,
        None => return,
    };
    let root = app.root_node.as_deref_mut().unwrap();
    root.size += node.size;
    root.file_count += node.file_count;
    root.folder_count += node.folder_count + 1;
    root.children.push(node); // capacity reserved in start_scan_all — no realloc
    let drive_ptr = root.children.last().unwrap() as *const FolderNode;

    if let Some(&root_hti) = app.item_by_node.get(&(root_ptr as isize)) {
        let hti = insert_tree_item(app.tree, root_hti, &*drive_ptr, true);
        app.item_by_node.insert(drive_ptr as isize, hti);
        SendMessageW(
            app.tree,
            TVM_EXPAND,
            WPARAM(TVE_EXPAND.0 as usize),
            LPARAM(root_hti),
        );
    }
    // Keep the main list (showing the root's drives) current if root is selected.
    if app.selected_node == root_ptr as isize {
        populate_list_folders(app, &*root_ptr);
    }
}

// Final housekeeping once every drive thread has reported.
unsafe fn finish_scan_all(app: &mut AppState) {
    app.scan_all_active = false;
    app.scanning = false;
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
    let mut summary = format!(
        "All drives — {} ({} files, {} folders)",
        format_bytes(root.size),
        format_count(root.file_count),
        format_count(root.folder_count),
    );
    if let Some(err) = &app.scan_all_first_err {
        summary.push_str(&format!("  [some skipped: {err}]"));
    }
    set_status(app.status, &summary);
}

fn on_progress(app: &AppState) {
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

unsafe fn on_scan_done(app: &mut AppState) {
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

unsafe fn populate_children(app: &mut AppState, parent_hti: isize, parent: &FolderNode) {
    if !app.populated.insert(parent_hti) {
        return;
    }
    let mut kids: Vec<&FolderNode> = parent
        .children
        .iter()
        .filter(|c| !app.deleted_nodes.contains(&(*c as *const _ as isize)))
        .collect();
    // Alphabetical, matching the folder list.
    kids.sort_by_key(|n| n.name.to_lowercase());
    for c in kids {
        // Only insert subdirectories as tree items; leaf-like nodes (no children)
        // still appear because every FolderNode here is a directory.
        let hti = insert_tree_item(app.tree, parent_hti, c, false);
        let p = c as *const _ as isize;
        app.item_by_node.insert(p, hti);
    }
}

unsafe fn insert_tree_item(
    tree: HWND,
    parent_hti: isize,
    node: &FolderNode,
    sorted: bool,
) -> isize {
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
    // `sorted` inserts the item in alphabetical position among its siblings
    // (TVI_SORT); otherwise it's appended (TVI_LAST) and the caller controls
    // order via pre-sorting.
    let after = if sorted {
        windows::Win32::UI::Controls::TVI_SORT.0
    } else {
        windows::Win32::UI::Controls::TVI_LAST.0
    };
    let ins = windows::Win32::UI::Controls::TVINSERTSTRUCTW {
        hParent: windows::Win32::UI::Controls::HTREEITEM(parent_hti as _),
        hInsertAfter: windows::Win32::UI::Controls::HTREEITEM(after as _),
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

// Force a tree item to display as having children (an expand button), used
// for the "All drives" root which is created before any drive is appended.
unsafe fn set_tree_item_has_children(tree: HWND, hti: isize) {
    let item = TVITEMW {
        mask: TVIF_HANDLE | TVIF_CHILDREN,
        hItem: windows::Win32::UI::Controls::HTREEITEM(hti as _),
        cChildren: windows::Win32::UI::Controls::TVITEMEXW_CHILDREN(1),
        ..Default::default()
    };
    SendMessageW(
        tree,
        TVM_SETITEMW,
        WPARAM(0),
        LPARAM(&item as *const _ as isize),
    );
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
    let node: &FolderNode = &*(lparam as *const FolderNode);
    populate_list_folders(app, node);
    // Make sure this node's children exist as tree items (not just on expand),
    // so double-clicking a folder row can keep drilling level after level —
    // the list double-click selects a child's tree item, which must be present.
    // populate_children is idempotent (guards on the `populated` set).
    populate_children(app, hti, node);
}

unsafe fn populate_list_folders(app: &AppState, node: &FolderNode) {
    SendMessageW(app.list, LVM_DELETEALLITEMS, WPARAM(0), LPARAM(0));

    // Subfolders first, then the folder's own files — each group sorted
    // case-insensitively by name. Tombstoned (recycled-in-place) folders are
    // skipped.
    let mut folders: Vec<&FolderNode> = node
        .children
        .iter()
        .filter(|c| !app.deleted_nodes.contains(&(*c as *const _ as isize)))
        .collect();
    folders.sort_by_key(|n| n.name.to_lowercase());

    let mut row = 0i32;
    for k in &folders {
        // lParam = the FolderNode pointer, so double-click drills into it and
        // the context menu can act on it.
        insert_row_with_param(
            app.list,
            row,
            &k.name,
            &[
                format_bytes(k.size),
                format_count(k.file_count),
                format_count(k.folder_count),
            ],
            *k as *const _ as isize,
        );
        row += 1;
    }

    let mut files: Vec<&FileEntry> = node.files.iter().collect();
    files.sort_by_key(|f| f.name.to_lowercase());
    for f in &files {
        // lParam 0 marks a file row: not drillable, and folder-only actions
        // (drill / open-in-explorer) skip it.
        insert_row_with_param(
            app.list,
            row,
            &f.name,
            &[format_bytes(f.size), String::new(), String::new()],
            0,
        );
        row += 1;
    }
}

unsafe fn populate_side_top_files(app: &mut AppState) {
    populate_side_from_hits(app, |root| top_n_files(root, TOP_N_FILES));
}

unsafe fn populate_side_oldest_files(app: &mut AppState) {
    populate_side_from_hits(app, |root| oldest_n_files(root, TOP_N_FILES));
}

unsafe fn populate_side_temp(app: &AppState) {
    SendMessageW(app.side_list, LVM_DELETEALLITEMS, WPARAM(0), LPARAM(0));
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
            app.side_list,
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
    SendMessageW(app.side_list, LVM_DELETEALLITEMS, WPARAM(0), LPARAM(0));
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
    let _ = EnableWindow(app.scan_all_btn, false);
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
    let _ = EnableWindow(app.scan_all_btn, true);
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

    if app.side_view == SideView::TempFiles {
        populate_side_temp(app);
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

// Re-runs the last drive scan after something was recycled out of the tree.
unsafe fn rescan_after_recycle(hwnd: HWND, app: &mut AppState) {
    if app.scanning {
        return;
    }
    match app.last_scan.clone() {
        Some(ScanRequest::Single(path, use_mft)) => start_scan(hwnd, app, path, use_mft),
        Some(ScanRequest::AllDrives) => start_scan_all(hwnd, app),
        None => {}
    }
}

unsafe fn handle_recycle(hwnd: HWND, app: &mut AppState, target: CtxTarget) {
    match target {
        CtxTarget::SideList => {
            let indices = selected_indices(app.side_list);
            if indices.is_empty() {
                return;
            }
            let mut paths: Vec<String> = Vec::new();
            // For file-ranking views, decrement the owning folders' totals.
            if app.side_view == SideView::TopFiles || app.side_view == SideView::OldestFiles {
                for &i in &indices {
                    let lp = list_item_lparam(app.side_list, i);
                    if let Some(&(folder, file)) = app.side_hits.get(lp as usize) {
                        let folder_ref: &FolderNode = &*folder;
                        let file_ref: &FileEntry = &*file;
                        paths.push(join_path(&folder_ref.full_path, &file_ref.name));
                        adjust_ancestors(app, folder, file_ref.size, 1, 0, true);
                    }
                }
            } else {
                for &i in &indices {
                    if let Some(p) = side_row_path(app, i) {
                        paths.push(p);
                    }
                }
            }
            if paths.is_empty() {
                return;
            }
            recycle_in_background(hwnd, app, paths);
            remove_side_rows(app.side_list, &indices);
            refresh_after_delete(app);
        }
        CtxTarget::MainList => {
            if let Some(node) = selected_list_node(app) {
                if !node.full_path.is_empty() {
                    let node_ptr = node as *const FolderNode;
                    recycle_in_background(hwnd, app, vec![node.full_path.clone()]);
                    delete_folder_node(app, node_ptr);
                }
            }
        }
    }
}

// "Recycle all" panel button: every temp entry in one undoable shell op, in
// the background; the list clears immediately.
unsafe fn recycle_all_temp(hwnd: HWND, app: &mut AppState) {
    if app.side_view != SideView::TempFiles || app.temp_entries.is_empty() {
        return;
    }
    let paths: Vec<String> = app
        .temp_entries
        .iter()
        .map(|e| e.full_path.clone())
        .collect();
    recycle_in_background(hwnd, app, paths);
    app.temp_entries.clear();
    SendMessageW(app.side_list, LVM_DELETEALLITEMS, WPARAM(0), LPARAM(0));
    set_status(app.status, "Recycling temp files in the background...");
}

// Runs the shell delete on a worker thread so the UI never blocks on it; the
// in-memory tree/views are updated optimistically by the caller. Reports back
// via WM_APP_RECYCLE_DONE (used only for the failure fallback).
unsafe fn recycle_in_background(hwnd: HWND, app: &AppState, paths: Vec<String>) {
    if paths.is_empty() {
        return;
    }
    {
        *app.recycle_result.lock().unwrap() = None;
    }
    let send = SendHwnd(hwnd.0 as isize);
    let result = app.recycle_result.clone();
    std::thread::spawn(move || {
        let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        let ok = recycle_many(&refs);
        *result.lock().unwrap() = Some(ok);
        unsafe {
            let _ = PostMessageW(send.to_hwnd(), WM_APP_RECYCLE_DONE, WPARAM(0), LPARAM(0));
        }
    });
}

// A background recycle finished. On success we've already updated the views;
// on failure, fall back to a full rescan so the display resyncs with disk.
unsafe fn on_recycle_done(hwnd: HWND, app: &mut AppState) {
    let ok = app.recycle_result.lock().unwrap().take();
    if ok == Some(false) {
        set_status(app.status, "Recycle failed — rescanning to resync.");
        rescan_after_recycle(hwnd, app);
    }
}

// Tombstones a folder subtree and removes it from the tree control, then
// updates ancestors and the visible views — no rescan.
unsafe fn delete_folder_node(app: &mut AppState, node_ptr: *const FolderNode) {
    let node = &*node_ptr;
    let (dsize, dfiles, dfolders) = (node.size, node.file_count, node.folder_count + 1);

    // Decrement every ancestor (not the node itself — it's going away).
    adjust_ancestors(app, node_ptr, dsize, dfiles, dfolders, false);

    // Tombstone the whole subtree so every view skips it, and drop its tree
    // bookkeeping (its tree items get removed below).
    mark_subtree_deleted(app, node);

    // Remove the node's tree item (and, with it, its descendants), selecting
    // the parent so the caret stays valid.
    if let Some(&hti) = app.item_by_node.get(&(node_ptr as isize)) {
        let parent = SendMessageW(
            app.tree,
            TVM_GETNEXTITEM,
            WPARAM(TVGN_PARENT as usize),
            LPARAM(hti),
        );
        SendMessageW(app.tree, TVM_DELETEITEM, WPARAM(0), LPARAM(hti));
        app.item_by_node.remove(&(node_ptr as isize));
        if parent.0 != 0 {
            SendMessageW(
                app.tree,
                TVM_SELECTITEM,
                WPARAM(TVGN_CARET as usize),
                LPARAM(parent.0),
            );
        }
    }
    refresh_after_delete(app);
}

// Adds every folder pointer in `node`'s subtree to `deleted_nodes` (so the
// folder list and file-ranking views skip anything under it) and drops their
// tree bookkeeping. Iterative to avoid deep recursion on deep trees.
unsafe fn mark_subtree_deleted(app: &mut AppState, node: &FolderNode) {
    let mut ptrs: Vec<*const FolderNode> = Vec::new();
    collect_folder_ptrs(node, &mut ptrs);
    for p in ptrs {
        app.deleted_nodes.insert(p as isize);
        app.item_by_node.remove(&(p as isize));
    }
}

// Subtracts a deleted item's contribution from its ancestor folders' running
// totals. `include_self` decrements the leaf folder too (for a file delete,
// where the leaf is the containing folder that survives).
unsafe fn adjust_ancestors(
    app: &mut AppState,
    leaf_ptr: *const FolderNode,
    dsize: i64,
    dfiles: i64,
    dfolders: i64,
    include_self: bool,
) {
    let root = match app.root_node.as_deref() {
        Some(r) => r,
        None => return,
    };
    subtract_along_ancestors(root, leaf_ptr, (dsize, dfiles, dfolders), include_self);
}

// Subtracts a deleted item's (size, files, folders) from every folder on the
// path root..=target. `include_self` also decrements the target folder (for a
// file delete, where the target is the surviving container). Pure over the
// in-memory tree — no Win32 — so it's unit-tested.
unsafe fn subtract_along_ancestors(
    root: &FolderNode,
    target: *const FolderNode,
    d: (i64, i64, i64),
    include_self: bool,
) {
    let mut path: Vec<*const FolderNode> = Vec::new();
    if !find_node_path(root, target, &mut path) {
        return;
    }
    let upto = if include_self {
        path.len()
    } else {
        path.len().saturating_sub(1)
    };
    for &anc in &path[..upto] {
        let a = anc as *mut FolderNode;
        (*a).size -= d.0;
        (*a).file_count -= d.1;
        (*a).folder_count -= d.2;
    }
}

// Collects the pointers of `node` and every folder beneath it (iterative).
fn collect_folder_ptrs(node: &FolderNode, out: &mut Vec<*const FolderNode>) {
    let mut stack: Vec<*const FolderNode> = vec![node as *const _];
    while let Some(p) = stack.pop() {
        out.push(p);
        // SAFETY: pointers come from the borrowed tree and outlive this call.
        let f = unsafe { &*p };
        for c in &f.children {
            stack.push(c as *const _);
        }
    }
}

// Deletes the given (ascending) listview row indices, bottom-up so the
// remaining indices stay valid.
unsafe fn remove_side_rows(list: HWND, indices: &[i32]) {
    for &i in indices.iter().rev() {
        SendMessageW(list, LVM_DELETEITEM, WPARAM(i as usize), LPARAM(0));
    }
}

// Repaints the views after an in-place deletion, using the tree's current
// selection. No disk access.
unsafe fn refresh_after_delete(app: &mut AppState) {
    let hti = SendMessageW(
        app.tree,
        TVM_GETNEXTITEM,
        WPARAM(TVGN_CARET as usize),
        LPARAM(0),
    )
    .0 as isize;
    if hti != 0 {
        let lp = tree_item_lparam(app.tree, hti);
        if lp != 0 {
            app.selected_node = lp;
            populate_list_folders(app, &*(lp as *const FolderNode));
            let summary = {
                let n = &*(lp as *const FolderNode);
                format!(
                    "{} — {} ({} files, {} folders)",
                    if n.full_path.is_empty() {
                        "All drives"
                    } else {
                        &n.name
                    },
                    format_bytes(n.size),
                    format_count(n.file_count),
                    format_count(n.folder_count),
                )
            };
            set_status(app.status, &summary);
        }
    }
    match app.side_view {
        SideView::TopFiles => populate_side_top_files(app),
        SideView::OldestFiles => populate_side_oldest_files(app),
        SideView::None | SideView::TempFiles => {}
    }
}

unsafe fn populate_side_from_hits<F>(app: &mut AppState, query: F)
where
    F: for<'a> FnOnce(&'a FolderNode) -> Vec<crate::analysis::FileHit<'a>>,
{
    SendMessageW(app.side_list, LVM_DELETEALLITEMS, WPARAM(0), LPARAM(0));
    app.side_hits.clear();
    let root_ptr = match app.root_node.as_deref() {
        Some(r) => r as *const FolderNode,
        None => return,
    };
    let root: &FolderNode = &*root_ptr;
    let hits = query(root);
    let mut row = 0i32;
    for h in hits.iter() {
        // Skip files under a folder that's been recycled in place.
        if app.deleted_nodes.contains(&(h.folder as *const _ as isize)) {
            continue;
        }
        let full_path = join_path(&h.folder.full_path, &h.file.name);
        // lParam is the index into side_hits so context actions can recover
        // the (folder, file) pair.
        let idx = app.side_hits.len() as isize;
        app.side_hits
            .push((h.folder as *const _, h.file as *const _));
        insert_row_with_param(
            app.side_list,
            row,
            &h.file.name,
            &[
                format_bytes(h.file.size),
                format_filetime(h.file.last_modified_ft),
                full_path,
            ],
            idx,
        );
        row += 1;
    }
}

// ---- Side panel ----

unsafe fn apply_side_view(hwnd: HWND, app: &mut AppState, view: SideView) {
    if app.side_view == view {
        return;
    }
    app.side_view = view;

    // Reconfigure the side list's columns for the incoming view. Views differ
    // in column count, so loop until LVM_DELETECOLUMN returns 0.
    while SendMessageW(app.side_list, LVM_DELETECOLUMN, WPARAM(0), LPARAM(0)).0 != 0 {}
    SendMessageW(app.side_list, LVM_DELETEALLITEMS, WPARAM(0), LPARAM(0));
    app.side_hits.clear();

    match view {
        SideView::None => {}
        SideView::TopFiles => {
            insert_column(app.side_list, 0, "Name", 160, false);
            insert_column(app.side_list, 1, "Size", 80, true);
            insert_column(app.side_list, 2, "Modified", 110, false);
            insert_column(app.side_list, 3, "Path", 400, false);
            populate_side_top_files(app);
        }
        SideView::OldestFiles => {
            insert_column(app.side_list, 0, "Name", 160, false);
            insert_column(app.side_list, 1, "Size", 80, true);
            insert_column(app.side_list, 2, "Modified", 110, false);
            insert_column(app.side_list, 3, "Path", 400, false);
            populate_side_oldest_files(app);
        }
        SideView::TempFiles => {
            insert_column(app.side_list, 0, "Name", 150, false);
            insert_column(app.side_list, 1, "Size", 75, true);
            insert_column(app.side_list, 2, "Modified", 105, false);
            insert_column(app.side_list, 3, "Source", 90, false);
            insert_column(app.side_list, 4, "Folder", 350, false);
            if app.temp_entries.is_empty() && !app.scanning {
                start_temp_scan(hwnd, app);
            } else {
                populate_side_temp(app);
            }
        }
    }

    let _ = ShowWindow(
        app.side_list,
        if view == SideView::None {
            SW_HIDE
        } else {
            SW_SHOW
        },
    );
    let _ = ShowWindow(
        app.btn_recycle_all,
        if view == SideView::TempFiles {
            SW_SHOW
        } else {
            SW_HIDE
        },
    );

    // Show the panel where it currently lives (main window or floating frame).
    let visible = view != SideView::None;
    if app.detached {
        if !app.float_win.is_invalid() {
            let _ = ShowWindow(app.float_win, if visible { SW_SHOW } else { SW_HIDE });
            update_float_title(app);
        }
        let _ = ShowWindow(app.panel, if visible { SW_SHOW } else { SW_HIDE });
    } else {
        let _ = ShowWindow(app.panel, if visible { SW_SHOW } else { SW_HIDE });
        layout(hwnd, app);
    }
    // Reposition the header buttons/content for the new view: `layout` above
    // only sends the panel a WM_SIZE when its size actually changes, so on a
    // same-size view switch the just-shown Recycle-all button would otherwise
    // stay at its (0,0) creation spot, on top of the title.
    if visible {
        panel_layout(app, app.panel);
    }
    let _ = InvalidateRect(app.panel, None, true);

    if !app.menu.is_invalid() {
        let id = match view {
            SideView::None => ID_MENU_VIEW_NONE,
            SideView::TopFiles => ID_MENU_VIEW_TOPFILES,
            SideView::OldestFiles => ID_MENU_VIEW_OLDEST,
            SideView::TempFiles => ID_MENU_VIEW_TEMP,
        } as u32;
        let _ = CheckMenuRadioItem(
            app.menu,
            ID_MENU_VIEW_NONE as u32,
            ID_MENU_VIEW_TEMP as u32,
            id,
            MF_BYCOMMAND.0,
        );
    }
}

unsafe fn update_float_title(app: &AppState) {
    if app.float_win.is_invalid() {
        return;
    }
    let title = format!("ClutterCutter — {}", app.side_view.title());
    let title_w: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    let _ = SetWindowTextW(app.float_win, PCWSTR(title_w.as_ptr()));
}

// Moves the side panel between the main window and its floating frame.
unsafe fn toggle_detach(hwnd: HWND, app: &mut AppState) {
    if !app.detached {
        if app.float_win.is_invalid() {
            let hinstance = GetModuleHandleW(None).expect("GetModuleHandle");
            app.float_win = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("ClutterCutterFloat"),
                w!("ClutterCutter"),
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                PANEL_W + 40,
                640,
                hwnd, // owned by the main window: stays above it, closes with it
                HMENU::default(),
                hinstance,
                None,
            )
            .expect("float win");
            SetWindowLongPtrW(app.float_win, GWLP_USERDATA, app as *mut AppState as isize);
            // The float frame is created lazily, so apply the current theme's
            // dark title bar now (apply_theme only reaches it once it exists).
            let use_dark = BOOL(if app.is_dark { 1 } else { 0 });
            let _ = DwmSetWindowAttribute(
                app.float_win,
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                &use_dark as *const _ as *const _,
                std::mem::size_of::<BOOL>() as u32,
            );
            allow_dark_mode_for_window(app.float_win, app.is_dark);
        }
        app.detached = true;
        let _ = SetParent(app.panel, app.float_win);
        update_float_title(app);
        if app.side_view != SideView::None {
            let _ = ShowWindow(app.float_win, SW_SHOW);
            let _ = ShowWindow(app.panel, SW_SHOW);
        }
        // Fit the panel to the frame's current client area.
        let mut rc = RECT::default();
        let _ = GetClientRect(app.float_win, &mut rc);
        let _ = MoveWindow(app.panel, 0, 0, rc.right, rc.bottom, true);
    } else {
        app.detached = false;
        let _ = SetParent(app.panel, hwnd);
        if !app.float_win.is_invalid() {
            let _ = ShowWindow(app.float_win, SW_HIDE);
        }
        let _ = ShowWindow(
            app.panel,
            if app.side_view != SideView::None {
                SW_SHOW
            } else {
                SW_HIDE
            },
        );
    }
    // Re-flow the main window either way, and update the button/menu labels.
    layout(hwnd, app);
    let label = if app.detached {
        w!("Attach")
    } else {
        w!("Detach")
    };
    let _ = SetWindowTextW(app.btn_detach, label);
}

// Positions the panel header (title strip + buttons) and the content view.
// X of the leftmost header button for a given panel width — the title clamps
// to its left. Buttons pin to the right edge, so this stays valid at any width.
fn header_buttons_left_x(app: &AppState, panel_w: i32) -> i32 {
    let detach_x = panel_w - PANEL_BTN_GAP - DETACH_BTN_W;
    if app.side_view == SideView::TempFiles {
        detach_x - PANEL_BTN_GAP - RECYCLE_BTN_W
    } else {
        detach_x
    }
}

unsafe fn panel_layout(app: &AppState, panel: HWND) {
    let mut rc = RECT::default();
    let _ = GetClientRect(panel, &mut rc);
    let w = rc.right - rc.left;
    let h = rc.bottom - rc.top;
    let btn_y = (PANEL_HEADER_H - PANEL_BTN_H) / 2;
    // Detach is rightmost; Recycle-all (temp view only) sits to its left.
    let detach_x = w - PANEL_BTN_GAP - DETACH_BTN_W;
    let _ = MoveWindow(
        app.btn_detach,
        detach_x,
        btn_y,
        DETACH_BTN_W,
        PANEL_BTN_H,
        true,
    );
    if app.side_view == SideView::TempFiles {
        let recycle_x = detach_x - PANEL_BTN_GAP - RECYCLE_BTN_W;
        let _ = MoveWindow(
            app.btn_recycle_all,
            recycle_x,
            btn_y,
            RECYCLE_BTN_W,
            PANEL_BTN_H,
            true,
        );
    }
    let content_h = (h - PANEL_HEADER_H).max(0);
    let _ = MoveWindow(app.side_list, 0, PANEL_HEADER_H, w, content_h, true);
    // Buttons moved — repaint the header so the title re-clamps.
    let _ = InvalidateRect(panel, None, false);
}

unsafe fn paint_panel_header(app: &AppState, panel: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(panel, &mut ps);
    let mut rc = RECT::default();
    let _ = GetClientRect(panel, &mut rc);
    let header = RECT {
        bottom: PANEL_HEADER_H,
        ..rc
    };
    let (bg, fg): (u32, u32) = if app.is_dark {
        (0x002B_2B2B, 0x00E0_E0E0)
    } else {
        (0x00F0_F0F0, 0x0000_0000)
    };
    let brush = CreateSolidBrush(COLORREF(bg));
    FillRect(hdc, &header, brush);
    let _ = DeleteObject(brush);

    SetBkMode(hdc, TRANSPARENT);
    SetTextColor(hdc, COLORREF(fg));
    // Clamp the title to the left of the leftmost button so they never overlap,
    // whatever the panel width or which buttons are shown.
    let title_right = (header_buttons_left_x(app, rc.right) - PANEL_BTN_GAP).max(8);
    let mut text_rc = RECT {
        left: 8,
        top: 0,
        right: title_right,
        bottom: PANEL_HEADER_H,
    };
    let mut title_w: Vec<u16> = app.side_view.title().encode_utf16().collect();
    DrawTextW(
        hdc,
        &mut title_w,
        &mut text_rc,
        DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_END_ELLIPSIS,
    );
    let _ = EndPaint(panel, &ps);
}

unsafe extern "system" fn panel_proc(
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

// The draggable divider between the main list and the side panel. Dragging it
// updates `panel_frac` and re-flows the layout live.
unsafe extern "system" fn splitter_proc(
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
                let tree_w = 320;
                let avail = (rc.right - tree_w).max(1);
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

unsafe extern "system" fn float_proc(
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

unsafe fn list_item_lparam(list: HWND, idx: i32) -> isize {
    let mut item = LVITEMW {
        mask: windows::Win32::UI::Controls::LVIF_PARAM,
        iItem: idx,
        ..Default::default()
    };
    let r = SendMessageW(
        list,
        LVM_GETITEMW,
        WPARAM(0),
        LPARAM(&mut item as *mut _ as isize),
    );
    if r.0 == 0 {
        return 0;
    }
    item.lParam.0
}

unsafe fn nth_visible_node(app: &AppState, idx: i32) -> Option<&'static FolderNode> {
    let p = list_item_lparam(app.list, idx);
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
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    show_context_menu_at(hwnd, pt);
}

unsafe fn show_context_menu_at(hwnd: HWND, pt: POINT) {
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
    // Process-wide dark mode (undocumented uxtheme, guarded): without this,
    // DarkMode_* window themes don't actually render dark and popup menus
    // stay white. Must run before the SetWindowTheme calls below, and every
    // themed control additionally needs the per-window allow call.
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
    // Listview column headers have their own theme part ("ItemsView", which
    // follows the allow-dark state); without both they stay white in dark mode.
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
    SendMessageW(
        app.side_list,
        LVM_SETBKCOLOR,
        WPARAM(0),
        LPARAM(bg as isize),
    );
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
        LPARAM(bg as isize),
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
    // Force a full frame + children repaint: the title/menu bar live in the
    // non-client area and don't pick up theme changes from a client-area
    // invalidate alone.
    let _ = SetWindowPos(
        hwnd,
        HWND::default(),
        0,
        0,
        0,
        0,
        SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
    );
    let _ = DrawMenuBar(hwnd);
    let _ = RedrawWindow(
        hwnd,
        None,
        None,
        RDW_INVALIDATE | RDW_ERASE | RDW_FRAME | RDW_ALLCHILDREN,
    );
    if !app.float_win.is_invalid() {
        let _ = RedrawWindow(
            app.float_win,
            None,
            None,
            RDW_INVALIDATE | RDW_ERASE | RDW_FRAME | RDW_ALLCHILDREN,
        );
    }
    let _ = InvalidateRect(app.panel, None, true);
    let _ = InvalidateRect(app.status, None, true);
}

// ---- Undocumented dark-mode plumbing ----
//
// Windows exposes no public API to render Win32 common controls dark; the
// DarkMode_* window themes only take effect after the app opts in through
// unexported uxtheme entry points (looked up by ordinal — the technique
// Notepad++, Windows Terminal-era tools, etc. use). Everything here degrades
// to a silent no-op if an ordinal is missing, leaving light-on-dark controls
// that still pass WCAG contrast.
//
//   ordinal 104: RefreshImmersiveColorPolicyState()
//   ordinal 133: AllowDarkModeForWindow(hwnd, allow) — required per control
//                before DarkMode_*/ItemsView themes actually render dark
//   ordinal 135: SetPreferredAppMode(mode) — 1903+; 0=Default 1=AllowDark
//                2=ForceDark 3=ForceLight
//   ordinal 136: FlushMenuThemes() — re-themes popup menus

unsafe fn uxtheme_ordinal(ordinal: u16) -> Option<unsafe extern "system" fn() -> isize> {
    let lib = LoadLibraryW(w!("uxtheme.dll")).ok()?;
    GetProcAddress(lib, PCSTR(ordinal as usize as *const u8))
}

unsafe fn allow_dark_mode_for_window(hwnd: HWND, allow: bool) {
    if let Some(f) = uxtheme_ordinal(133) {
        let allow_fn: unsafe extern "system" fn(HWND, BOOL) -> BOOL = std::mem::transmute(f);
        let _ = allow_fn(hwnd, BOOL(allow as i32));
    }
}

// ---- Dark menu bar (WM_UAH* owner-draw) ----
//
// FlushMenuThemes darkens popup menus but never the menu *bar*. The bar can
// be painted via the undocumented WM_UAHDRAWMENU / WM_UAHDRAWMENUITEM
// messages Windows sends when a window is UAH-subclassed — which it is by
// default for any themed top-level window. This is the same technique
// Notepad++ uses. When the theme is light we pass everything to
// DefWindowProc, giving the stock menu bar.

const WM_UAHDRAWMENU: u32 = 0x0091;
const WM_UAHDRAWMENUITEM: u32 = 0x0092;

// Raw ODS_* bits (DRAWITEMSTRUCT.itemState).
const ODS_RAW_SELECTED: u32 = 0x0001;
const ODS_RAW_GRAYED: u32 = 0x0002;
const ODS_RAW_HOTLIGHT: u32 = 0x0040;
const ODS_RAW_NOACCEL: u32 = 0x0100;

const MENUBAR_DARK_BG: u32 = 0x0020_2020;
const MENUBAR_DARK_HOT: u32 = 0x003E_3E3E;
const MENUBAR_DARK_FG: u32 = 0x00E0_E0E0;
const MENUBAR_DARK_GRAY: u32 = 0x0080_8080;

#[repr(C)]
struct UahMenu {
    hmenu: HMENU,
    hdc: windows::Win32::Graphics::Gdi::HDC,
    dw_flags: u32,
}

#[repr(C)]
struct UahMenuItemMetrics {
    // Union of bar/popup size pairs; 4 (cx, cy) pairs cover both variants.
    rgsize: [[u32; 2]; 4],
}

#[repr(C)]
struct UahMenuPopupMetrics {
    rgcx: [u32; 4],
    bitfield: u32, // fUpdateMaxWidths : 2
}

#[repr(C)]
struct UahMenuItem {
    i_position: i32,
    umim: UahMenuItemMetrics,
    umpm: UahMenuPopupMetrics,
}

#[repr(C)]
struct UahDrawMenuItem {
    dis: windows::Win32::UI::Controls::DRAWITEMSTRUCT,
    um: UahMenu,
    umi: UahMenuItem,
}

// Menu bar background (the strip behind the items).
unsafe fn uah_draw_menu_bar_bg(hwnd: HWND, udm: &UahMenu) {
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

unsafe fn uah_draw_menu_item(pudmi: &UahDrawMenuItem) {
    // Item caption.
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
unsafe fn uah_draw_menu_bottom_line(hwnd: HWND) {
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

// Bulk recycle via SHFileOperationW. pFrom is a double-null-terminated list
// of single-null-terminated wide paths — one syscall regardless of count, so
// the user sees one undoable operation in the Recycle Bin. Returns whether the
// operation succeeded without being aborted.
fn recycle_many(paths: &[&str]) -> bool {
    if paths.is_empty() {
        return true;
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
        let rc = SHFileOperationW(&mut op);
        rc == 0 && !op.fAnyOperationsAborted.as_bool()
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
    let w: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let _ = SetWindowTextW(status, PCWSTR(w.as_ptr()));
    let _ = InvalidateRect(status, None, true);
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

#[cfg(test)]
mod tests {
    use super::{collect_folder_ptrs, find_node_path, subtract_along_ancestors};
    use crate::types::{FileEntry, FolderNode};

    // Builds root{ a{ a1{} }, b{} } with known sizes/counts.
    fn sample_tree() -> FolderNode {
        let file = |sz: i64| FileEntry {
            name: "f".into(),
            size: sz,
            last_modified_ft: 0,
        };
        let mut a1 = FolderNode {
            name: "a1".into(),
            full_path: r"C:\a\a1".into(),
            size: 30,
            file_count: 3,
            folder_count: 0,
            files: vec![file(10), file(10), file(10)],
            ..Default::default()
        };
        a1.direct_file_count = 3;
        let a = FolderNode {
            name: "a".into(),
            full_path: r"C:\a".into(),
            size: 30,
            file_count: 3,
            folder_count: 1, // a1
            children: vec![a1],
            ..Default::default()
        };
        let b = FolderNode {
            name: "b".into(),
            full_path: r"C:\b".into(),
            size: 5,
            file_count: 1,
            folder_count: 0,
            files: vec![file(5)],
            direct_file_count: 1,
            ..Default::default()
        };
        FolderNode {
            name: r"C:\".into(),
            full_path: r"C:\".into(),
            size: 35,
            file_count: 4,
            folder_count: 2, // a, b (a1 is under a)
            children: vec![a, b],
            ..Default::default()
        }
    }

    // Deleting folder `a` (size 30, 3 files, 2 folders incl. itself) must
    // subtract exactly that from the root, leaving b's contribution.
    #[test]
    fn delete_folder_updates_ancestor_totals() {
        let root = sample_tree();
        let a_ptr = &root.children[0] as *const FolderNode;
        // a contributes: size 30, files 3, folders (a.folder_count + 1) = 2.
        unsafe { subtract_along_ancestors(&root, a_ptr, (30, 3, 2), false) };
        assert_eq!(root.size, 5, "root size after deleting a");
        assert_eq!(root.file_count, 1, "root files after deleting a");
        assert_eq!(root.folder_count, 0, "root folders after deleting a");
        // a itself is untouched (it's tombstoned, not mutated).
        assert_eq!(root.children[0].size, 30);
    }

    // Deleting a file decrements the containing folder and every ancestor.
    #[test]
    fn delete_file_updates_folder_and_ancestors() {
        let root = sample_tree();
        let a1_ptr = &root.children[0].children[0] as *const FolderNode;
        // Remove one 10-byte file from a1: size 10, 1 file, 0 folders, self too.
        unsafe { subtract_along_ancestors(&root, a1_ptr, (10, 1, 0), true) };
        assert_eq!(root.children[0].children[0].size, 20); // a1
        assert_eq!(root.children[0].children[0].file_count, 2);
        assert_eq!(root.children[0].size, 20); // a
        assert_eq!(root.size, 25); // root
        assert_eq!(root.file_count, 3);
    }

    // Tombstoning a subtree must collect the folder itself and all descendants.
    #[test]
    fn collect_folder_ptrs_covers_whole_subtree() {
        let root = sample_tree();
        let a = &root.children[0];
        let mut ptrs = Vec::new();
        collect_folder_ptrs(a, &mut ptrs);
        // a + a1 = 2 folders.
        assert_eq!(ptrs.len(), 2);
        assert!(ptrs.contains(&(a as *const FolderNode)));
        assert!(ptrs.contains(&(&a.children[0] as *const FolderNode)));
    }

    // find_node_path returns root..=target; a missing target returns false.
    #[test]
    fn find_node_path_locates_and_rejects() {
        let root = sample_tree();
        let a1 = &root.children[0].children[0] as *const FolderNode;
        let mut path = Vec::new();
        assert!(find_node_path(&root, a1, &mut path));
        assert_eq!(path.len(), 3); // root, a, a1
        let mut none = Vec::new();
        assert!(!find_node_path(&root, std::ptr::null(), &mut none));
    }
}
