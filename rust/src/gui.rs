// Win32 GUI for ClutterCutter — raw window class + message loop + WndProc, no
// GUI framework.
//
//   [drive buttons] [Scan all] [Stop]
//   [TreeView] | [ListView] | [side panel: top files / oldest / temp / treemap]
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
use crate::treemap::{self, Rectf};
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
    BeginPaint, BitBlt, ClientToScreen, CreateCompatibleBitmap, CreateCompatibleDC,
    CreateSolidBrush, DeleteDC, DeleteObject, DrawTextW, EndPaint, FillRect, FrameRect,
    GetSysColorBrush, GetWindowDC, InvalidateRect, MapWindowPoints, RedrawWindow, ReleaseDC,
    SelectObject, SetBkMode, SetTextColor, UpdateWindow, COLOR_BTNFACE, DT_CENTER, DT_END_ELLIPSIS,
    DT_HIDEPREFIX, DT_LEFT, DT_SINGLELINE, DT_VCENTER, HBRUSH, HDC, PAINTSTRUCT, RDW_ALLCHILDREN,
    RDW_ERASE, RDW_FRAME, RDW_INVALIDATE, SRCCOPY, TRANSPARENT,
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
    LVM_DELETECOLUMN, LVM_GETHEADER, LVM_GETITEMW, LVM_GETNEXTITEM, LVM_INSERTCOLUMNW,
    LVM_INSERTITEMW, LVM_SETBKCOLOR, LVM_SETEXTENDEDLISTVIEWSTYLE, LVM_SETITEMTEXTW,
    LVM_SETTEXTBKCOLOR, LVM_SETTEXTCOLOR, LVNI_SELECTED, LVS_EX_DOUBLEBUFFER, LVS_EX_FULLROWSELECT,
    LVS_EX_GRIDLINES, LVS_REPORT, LVS_SHOWSELALWAYS, NMHDR, NMITEMACTIVATE, NM_DBLCLK, NM_RCLICK,
    TVE_EXPAND, TVGN_CARET, TVGN_PARENT, TVIF_CHILDREN, TVIF_PARAM, TVIF_TEXT, TVITEMW, TVI_ROOT,
    TVM_DELETEITEM, TVM_GETITEMW, TVM_GETNEXTITEM, TVM_INSERTITEMW, TVM_SELECTITEM, TVM_SETBKCOLOR,
    TVM_SETTEXTCOLOR, TVN_ITEMEXPANDINGW, TVN_SELCHANGEDW, TVS_HASBUTTONS, TVS_HASLINES,
    TVS_LINESATROOT, TVS_SHOWSELALWAYS, TVS_TRACKSELECT,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, GetFocus, SetFocus};
use windows::Win32::UI::Shell::{
    IsUserAnAdmin, SHFileOperationW, ShellExecuteW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FO_DELETE,
    SHFILEOPSTRUCTW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CheckMenuRadioItem, CreateAcceleratorTableW, CreateMenu, CreatePopupMenu,
    CreateWindowExW, DefWindowProcW, DestroyMenu, DispatchMessageW, DrawMenuBar, GetClientRect,
    GetCursorPos, GetMenuBarInfo, GetMenuItemInfoW, GetMessageW, GetWindowLongPtrW, GetWindowRect,
    GetWindowTextW, IsDialogMessageW, LoadCursorW, LoadIconW, MessageBoxW, MoveWindow,
    PostMessageW, PostQuitMessage, RegisterClassExW, SendMessageW, SetForegroundWindow, SetMenu,
    SetParent, SetWindowLongPtrW, SetWindowPos, SetWindowTextW, ShowWindow, TrackPopupMenu,
    TranslateAcceleratorW, TranslateMessage, ACCEL, BS_PUSHBUTTON, CREATESTRUCTW, CS_DBLCLKS,
    CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, DLGC_WANTARROWS, FVIRTKEY, GWLP_USERDATA, HMENU,
    IDC_ARROW, IDI_APPLICATION, MB_ICONINFORMATION, MB_OK, MENUBARINFO, MENUITEMINFOW,
    MF_BYCOMMAND, MF_POPUP, MF_SEPARATOR, MF_STRING, MIIM_STRING, MSG, OBJID_MENU,
    SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SW_HIDE, SW_NORMAL,
    SW_SHOW, TPM_LEFTALIGN, TPM_RIGHTBUTTON, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_CLOSE,
    WM_COMMAND, WM_CREATE, WM_DESTROY, WM_ERASEBKGND, WM_GETDLGCODE, WM_KEYDOWN, WM_KILLFOCUS,
    WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_MOUSEMOVE, WM_NCACTIVATE, WM_NCCREATE, WM_NCPAINT,
    WM_NOTIFY, WM_PAINT, WM_RBUTTONDOWN, WM_SETFOCUS, WM_SIZE, WNDCLASSEXW, WS_BORDER, WS_CHILD,
    WS_EX_CLIENTEDGE, WS_EX_CONTROLPARENT, WS_OVERLAPPEDWINDOW, WS_TABSTOP, WS_VISIBLE,
};

// ---- Control ids ----
const ID_DRIVE_BASE: u16 = 1000;
const ID_STOP_BTN: u16 = 200;
const ID_SCAN_ALL_BTN: u16 = 201;
const ID_BTN_DETACH: u16 = 210;
const ID_BTN_RECYCLE_ALL: u16 = 211;
const ID_LIST: u16 = 300;
const ID_TREE: u16 = 301;
const ID_TREEMAP: u16 = 302;
const ID_SIDE_LIST: u16 = 303;
const ID_PANEL: u16 = 304;
const ID_STATUS: u16 = 400;

// Side panel geometry
const PANEL_W: i32 = 420;
const PANEL_HEADER_H: i32 = 30;

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
const ID_MENU_VIEW_TREEMAP: u16 = 5305;
const ID_MENU_VIEW_DETACH: u16 = 5310;

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
const VK_LEFT: u16 = 0x25;
const VK_UP: u16 = 0x26;
const VK_RIGHT: u16 = 0x27;
const VK_DOWN: u16 = 0x28;
const VK_APPS: u16 = 0x5D;

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
    Treemap,
}

impl SideView {
    fn title(self) -> &'static str {
        match self {
            SideView::None => "",
            SideView::TopFiles => "Top largest files",
            SideView::OldestFiles => "Oldest files",
            SideView::TempFiles => "Safe-to-delete temp files",
            SideView::Treemap => "Treemap",
        }
    }
}

// Which pane a context-menu / accelerator action targets.
#[derive(Copy, Clone, Default, PartialEq)]
enum CtxTarget {
    #[default]
    MainList,
    SideList,
    Treemap,
}

// What F5 should re-run.
#[derive(Clone)]
enum ScanRequest {
    Single(String, bool), // path, use_mft
    AllDrives,
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
    hue_idx: usize,
    // No child tiles rendered inside this one — parents get overdrawn, so
    // only leaves carry a body label.
    is_leaf: bool,
    // Title-strip height reserved at the top of a folder tile (0 = none);
    // children are laid out below it and the folder name is drawn in it.
    header_h: i32,
}

struct AppState {
    main_hwnd: HWND,
    drives: Vec<DriveInfo>,
    drive_buttons: Vec<HWND>,
    stop_btn: HWND,
    scan_all_btn: HWND,
    tree: HWND,
    list: HWND,
    treemap: HWND,
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
    // folder, file) pointer pairs, indexed by the row's lParam. Same pinning
    // rules as treemap_entries; cleared in start_scan.
    side_hits: Vec<(*const FolderNode, *const FileEntry)>,

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
            main_hwnd: HWND::default(),
            drives: enumerate_drives(),
            drive_buttons: Vec::new(),
            stop_btn: HWND::default(),
            scan_all_btn: HWND::default(),
            tree: HWND::default(),
            list: HWND::default(),
            treemap: HWND::default(),
            status: HWND::default(),
            panel: HWND::default(),
            side_list: HWND::default(),
            btn_detach: HWND::default(),
            btn_recycle_all: HWND::default(),
            float_win: HWND::default(),
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
        ID_MENU_VIEW_TREEMAP => apply_side_view(hwnd, app, SideView::Treemap),
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
    if f == app.treemap {
        CtxTarget::Treemap
    } else if f == app.side_list {
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
                CtxTarget::Treemap => {
                    if let Some((folder, file)) = treemap_selected_ptrs(app) {
                        if id == ID_ACC_DRILL {
                            if file.is_null() {
                                select_tree_node(app, folder);
                            }
                        } else {
                            // Files open their containing folder; folders themselves.
                            let folder: &FolderNode = &*folder;
                            if !folder.full_path.is_empty() {
                                open_in_explorer(&folder.full_path);
                            }
                        }
                    }
                }
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
                CtxTarget::Treemap => treemap_selected_path(app),
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
                CtxTarget::Treemap => {
                    treemap_selected_ptrs(app).map(|(folder, _)| (*folder).full_path.clone())
                }
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

    // Side panel — container for the extra views (top files / oldest / temp /
    // treemap). Child of the main window while attached; re-parented into the
    // floating frame when detached. Every custom class here finds AppState via
    // its own GWLP_USERDATA.
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

    // Treemap canvas — custom-painted child of the panel.
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
        WS_CHILD | WS_TABSTOP, // shown only for the Treemap side view
        0,
        PANEL_HEADER_H,
        PANEL_W,
        400,
        app.panel,
        HMENU(ID_TREEMAP as isize as _),
        hinstance,
        None,
    )
    .expect("treemap");
    SetWindowLongPtrW(app.treemap, GWLP_USERDATA, app_lp);
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
        ID_MENU_VIEW_TREEMAP as u32,
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
    // The side panel takes a fixed strip on the right while attached; the
    // main list gets whatever is left.
    let panel_here = app.side_view != SideView::None && !app.detached;
    let panel_w = if panel_here { PANEL_W } else { 0 };
    let list_w = (rc.right - tree_w - panel_w).max(0);
    let _ = MoveWindow(app.list, tree_w, top, list_w, body_h, true);
    if panel_here {
        let _ = MoveWindow(app.panel, tree_w + list_w, top, panel_w, body_h, true);
    }
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
    app.treemap_entries.clear();
    app.treemap_selected = -1;
    app.treemap_hover = -1;
    app.side_hits.clear();
    if app.side_view == SideView::TopFiles || app.side_view == SideView::OldestFiles {
        SendMessageW(app.side_list, LVM_DELETEALLITEMS, WPARAM(0), LPARAM(0));
    }
    let _ = InvalidateRect(app.treemap, None, true);
    set_status(app.status, status_text);
    app.cancel.store(false, Ordering::SeqCst);
    app.scanning = true;
    let _ = EnableWindow(app.stop_btn, true);
    let _ = EnableWindow(app.scan_all_btn, false);
    for b in &app.drive_buttons {
        let _ = EnableWindow(*b, false);
    }
}

// Progress callback shared by all drive scans.
fn make_progress(send_hwnd: SendHwnd, shared: Arc<Mutex<ScanState>>) -> ProgressFn {
    Box::new(move |p| {
        if let Ok(mut s) = shared.lock() {
            s.last_progress = p.clone();
        }
        unsafe {
            let _ = PostMessageW(send_hwnd.to_hwnd(), WM_APP_PROGRESS, WPARAM(0), LPARAM(0));
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
    let progress = make_progress(send_hwnd, shared.clone());

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

// Scans every enumerated drive sequentially and composes the results under a
// synthetic "All drives" root. Drives that fail (ejected, access denied) are
// skipped; the scan only errors if *no* drive produced a result.
unsafe fn start_scan_all(hwnd: HWND, app: &mut AppState) {
    let targets: Vec<(String, bool)> = app
        .drives
        .iter()
        .map(|d| (d.root.clone(), d.is_ntfs && app.is_admin))
        .collect();
    if targets.is_empty() {
        return;
    }
    begin_scan_ui(
        app,
        &format!("Scanning all drives ({} volumes)...", targets.len()),
    );
    app.last_scan = Some(ScanRequest::AllDrives);

    let send_hwnd = SendHwnd(hwnd.0 as isize);
    let shared = app.shared.clone();
    let cancel = app.cancel.clone();
    let progress_shared = shared.clone();

    std::thread::spawn(move || {
        let mut root = FolderNode {
            full_path: String::new(), // synthetic — shell actions no-op on it
            name: "All drives".to_string(),
            ..Default::default()
        };
        let mut first_err: Option<String> = None;
        for (path, use_mft) in &targets {
            if cancel.load(Ordering::SeqCst) {
                break;
            }
            let progress = make_progress(send_hwnd, progress_shared.clone());
            match scan_one(path, *use_mft, cancel.clone(), progress) {
                Ok(node) => {
                    root.size += node.size;
                    root.file_count += node.file_count;
                    root.folder_count += node.folder_count + 1;
                    root.children.push(node);
                }
                Err(e) => {
                    if first_err.is_none() {
                        first_err = Some(format!("{path}: {e}"));
                    }
                }
            }
        }
        let result = if root.children.is_empty() {
            Err(first_err.unwrap_or_else(|| "no drives scanned".to_string()))
        } else {
            Ok(root)
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
    // The tree-selection above repopulates the main list and (for Treemap)
    // the canvas via on_tree_select. The file-ranking side views are global
    // over the new tree; refresh them directly. TempFiles is independent of
    // drive scans entirely.
    match app.side_view {
        SideView::None | SideView::TempFiles | SideView::Treemap => {}
        SideView::TopFiles => populate_side_top_files(app),
        SideView::OldestFiles => populate_side_oldest_files(app),
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
    let node: &FolderNode = &*(lparam as *const FolderNode);
    populate_list_folders(app, node);
    // The treemap is rooted at the tree selection; the file-ranking side
    // views are global and ignore it.
    if app.side_view == SideView::Treemap {
        rebuild_treemap(app);
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
        CtxTarget::Treemap => {
            if let Some(path) = treemap_selected_path(app) {
                if !path.is_empty() {
                    recycle(&path);
                    rescan_after_recycle(hwnd, app);
                }
            }
        }
        CtxTarget::SideList => {
            // Multi-select recycle for all file-based side views.
            let indices = selected_indices(app.side_list);
            let paths: Vec<String> = indices
                .iter()
                .filter_map(|&i| side_row_path(app, i))
                .collect();
            if paths.is_empty() {
                return;
            }
            let path_refs: Vec<&str> = paths.iter().map(String::as_str).collect();
            recycle_many(&path_refs);
            if app.side_view == SideView::TempFiles {
                if !app.scanning {
                    start_temp_scan(hwnd, app);
                }
            } else {
                rescan_after_recycle(hwnd, app);
            }
        }
        CtxTarget::MainList => {
            if let Some(node) = selected_list_node(app) {
                if !node.full_path.is_empty() {
                    recycle(&node.full_path);
                    rescan_after_recycle(hwnd, app);
                }
            }
        }
    }
}

// "Recycle all" panel button: every temp entry in one undoable shell op.
unsafe fn recycle_all_temp(hwnd: HWND, app: &mut AppState) {
    if app.side_view != SideView::TempFiles || app.temp_entries.is_empty() {
        return;
    }
    let paths: Vec<&str> = app
        .temp_entries
        .iter()
        .map(|e| e.full_path.as_str())
        .collect();
    recycle_many(&paths);
    if !app.scanning {
        start_temp_scan(hwnd, app);
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
    for (i, h) in hits.iter().enumerate() {
        let full_path = join_path(&h.folder.full_path, &h.file.name);
        // lParam is the index into side_hits so context actions can recover
        // the (folder, file) pair.
        app.side_hits
            .push((h.folder as *const _, h.file as *const _));
        insert_row_with_param(
            app.side_list,
            i as i32,
            &h.file.name,
            &[
                format_bytes(h.file.size),
                format_filetime(h.file.last_modified_ft),
                full_path,
            ],
            i as isize,
        );
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
        SideView::Treemap => {
            rebuild_treemap(app);
        }
    }

    // Inside the panel, the treemap canvas and the side list swap places.
    let treemap_mode = view == SideView::Treemap;
    let _ = ShowWindow(app.side_list, if treemap_mode { SW_HIDE } else { SW_SHOW });
    let _ = ShowWindow(app.treemap, if treemap_mode { SW_SHOW } else { SW_HIDE });
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
    let _ = InvalidateRect(app.panel, None, true);

    if !app.menu.is_invalid() {
        let id = match view {
            SideView::None => ID_MENU_VIEW_NONE,
            SideView::TopFiles => ID_MENU_VIEW_TOPFILES,
            SideView::OldestFiles => ID_MENU_VIEW_OLDEST,
            SideView::TempFiles => ID_MENU_VIEW_TEMP,
            SideView::Treemap => ID_MENU_VIEW_TREEMAP,
        } as u32;
        let _ = CheckMenuRadioItem(
            app.menu,
            ID_MENU_VIEW_NONE as u32,
            ID_MENU_VIEW_TREEMAP as u32,
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
unsafe fn panel_layout(app: &AppState, panel: HWND) {
    let mut rc = RECT::default();
    let _ = GetClientRect(panel, &mut rc);
    let w = rc.right - rc.left;
    let h = rc.bottom - rc.top;
    let btn_y = (PANEL_HEADER_H - 24) / 2;
    let mut x = w - 75;
    let _ = MoveWindow(app.btn_detach, x, btn_y, 70, 24, true);
    if app.side_view == SideView::TempFiles {
        x -= 95;
        let _ = MoveWindow(app.btn_recycle_all, x, btn_y, 90, 24, true);
    }
    let content_h = (h - PANEL_HEADER_H).max(0);
    let _ = MoveWindow(app.side_list, 0, PANEL_HEADER_H, w, content_h, true);
    let _ = MoveWindow(app.treemap, 0, PANEL_HEADER_H, w, content_h, true);
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
    let mut text_rc = RECT {
        left: 8,
        top: 0,
        right: (rc.right - 180).max(8),
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
                // Reserve a title strip when the tile is big enough to show
                // one; children lay out below it (the WinDirStat-style header).
                let header_h = if r.w >= 60.0 && r.h >= 34.0 {
                    TREEMAP_HEADER_H
                } else {
                    0
                };
                let idx = entries.len();
                entries.push(TreemapEntry {
                    rect,
                    folder: *c as *const _,
                    file: std::ptr::null(),
                    hue_idx,
                    is_leaf: false, // fixed up after recursion
                    header_h,
                });
                // Inset children by 1px (+ the header strip) so the folder's
                // own border and title stay visible.
                let inner = Rectf {
                    x: r.x + 1.0,
                    y: r.y + 1.0 + header_h as f64,
                    w: r.w - 2.0,
                    h: r.h - 2.0 - header_h as f64,
                };
                build_treemap_level(entries, c, inner, depth + 1, Some(hue_idx));
                // A leaf if no child tiles were emitted below it.
                entries[idx].is_leaf = entries.len() == idx + 1;
            }
            Item::File(f) => {
                entries.push(TreemapEntry {
                    rect,
                    folder: folder as *const _,
                    file: *f as *const _,
                    hue_idx,
                    is_leaf: true,
                    header_h: 0,
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
            if app.side_view == SideView::Treemap {
                rebuild_treemap(app);
            }
            LRESULT(0)
        }
        // Keep arrow keys out of the dialog navigator — they move the tile
        // selection (keyboard equivalent of clicking).
        WM_GETDLGCODE => LRESULT(DLGC_WANTARROWS as isize),
        WM_SETFOCUS | WM_KILLFOCUS => {
            // Focused state is painted (ring around the canvas edge).
            let _ = InvalidateRect(hwnd, None, false);
            LRESULT(0)
        }
        WM_KEYDOWN => {
            match wparam.0 as u16 {
                VK_LEFT => treemap_move_selection(app, -1, 0),
                VK_RIGHT => treemap_move_selection(app, 1, 0),
                VK_UP => treemap_move_selection(app, 0, -1),
                VK_DOWN => treemap_move_selection(app, 0, 1),
                VK_APPS => {
                    if let Some(e) = app
                        .treemap_entries
                        .get(app.treemap_selected.max(0) as usize)
                    {
                        // Open the shared context menu at the tile's center.
                        let mut pt = POINT {
                            x: (e.rect.left + e.rect.right) / 2,
                            y: (e.rect.top + e.rect.bottom) / 2,
                        };
                        let _ = ClientToScreen(hwnd, &mut pt);
                        app.ctx_target = CtxTarget::Treemap;
                        show_context_menu_at(app.main_hwnd, pt);
                    }
                }
                _ => return DefWindowProcW(hwnd, msg, wparam, lparam),
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
            // Take focus so Del/Enter accelerators target the treemap.
            let _ = SetFocus(hwnd);
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
                app.ctx_target = CtxTarget::Treemap;
                show_context_menu(app.main_hwnd, app);
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn lparam_xy(lparam: LPARAM) -> (i32, i32) {
    let x = (lparam.0 & 0xFFFF) as u16 as i16 as i32;
    let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
    (x, y)
}

// Keyboard navigation: move the selection to the nearest tile in the given
// direction (by tile centers, favoring movement along the pressed axis).
unsafe fn treemap_move_selection(app: &mut AppState, dx: i32, dy: i32) {
    if app.treemap_entries.is_empty() {
        return;
    }
    let cur = app.treemap_selected;
    if cur < 0 {
        app.treemap_selected = 0;
        let _ = InvalidateRect(app.treemap, None, false);
        return;
    }
    let center = |e: &TreemapEntry| {
        (
            (e.rect.left + e.rect.right) / 2,
            (e.rect.top + e.rect.bottom) / 2,
        )
    };
    let (ox, oy) = center(&app.treemap_entries[cur as usize]);
    let mut best: i32 = -1;
    let mut best_score = i64::MAX;
    for (i, e) in app.treemap_entries.iter().enumerate() {
        if i as i32 == cur {
            continue;
        }
        let (cx, cy) = center(e);
        let (vx, vy) = ((cx - ox) as i64, (cy - oy) as i64);
        let along = vx * dx as i64 + vy * dy as i64;
        if along <= 0 {
            continue; // wrong direction
        }
        let perp = (vx * dy as i64 - vy * dx as i64).abs();
        let score = along + 2 * perp;
        if score < best_score {
            best_score = score;
            best = i as i32;
        }
    }
    if best >= 0 {
        app.treemap_selected = best;
        let text = treemap_entry_status(&app.treemap_entries[best as usize]);
        set_status(app.status, &text);
        let _ = InvalidateRect(app.treemap, None, false);
    }
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

// Every tile is a light surface with black labels: this relative-luminance
// floor guarantees black text clears WCAG AAA (1.4.6, 7:1) — at 0.42 the ratio
// is ~9.4:1 — and black borders clear AA (1.4.11, 3:1) on every hue.
const TILE_LUM_FLOOR: f64 = 0.42;

// Reserved title-strip height on folder tiles.
const TREEMAP_HEADER_H: i32 = 16;

fn tile_color(hue_idx: usize, is_file: bool) -> u32 {
    let hue = PALETTE_HUES[hue_idx % PALETTE_HUES.len()];
    // Files read a touch paler/flatter than folders so the two kinds separate
    // at a glance; both are pastel enough to carry black text.
    let (sat, l) = if is_file { (0.30, 0.74) } else { (0.52, 0.62) };
    let base = hsl_to_colorref(hue, sat, l);
    // Hues differ wildly in luminance at equal HSL lightness (blue is dark,
    // yellow light); blend each toward white until it clears the floor so the
    // text-contrast guarantee holds for all of them.
    lighten_to_lum(base, TILE_LUM_FLOOR)
}

// Blend a COLORREF toward white until its relative luminance reaches `floor`.
// Binary search on the blend factor (luminance is monotonic in it).
fn lighten_to_lum(color: u32, floor: f64) -> u32 {
    if rel_luminance(color) >= floor {
        return color;
    }
    let (r, g, b) = (color & 0xFF, (color >> 8) & 0xFF, (color >> 16) & 0xFF);
    let mix = |c: u32, t: f64| (c as f64 + (255.0 - c as f64) * t).round().min(255.0) as u32;
    let build = |t: f64| (mix(b, t) << 16) | (mix(g, t) << 8) | mix(r, t);
    let (mut lo, mut hi) = (0.0f64, 1.0f64);
    for _ in 0..12 {
        let t = (lo + hi) / 2.0;
        if rel_luminance(build(t)) >= floor {
            hi = t;
        } else {
            lo = t;
        }
    }
    build(hi)
}

// WCAG relative luminance of a COLORREF (0x00BBGGRR).
fn rel_luminance(c: u32) -> f64 {
    let ch = |v: u32| {
        let s = v as f64 / 255.0;
        if s <= 0.03928 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    let r = ch(c & 0xFF);
    let g = ch((c >> 8) & 0xFF);
    let b = ch((c >> 16) & 0xFF);
    0.2126 * r + 0.7152 * g + 0.0722 * b
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

    // All fills are light (>= TILE_LUM_FLOOR), so a black border always clears
    // WCAG 1.4.11's 3:1 against the tile it outlines, and black label text
    // clears AAA's 7:1.
    let white_brush = CreateSolidBrush(COLORREF(0x00FF_FFFF));
    let black_brush = CreateSolidBrush(COLORREF(0x0000_0000));
    // Few distinct colors (hue × kind), many tiles — cache brushes.
    let mut brushes: HashMap<u32, windows::Win32::Graphics::Gdi::HBRUSH> = HashMap::new();
    for e in &app.treemap_entries {
        let color = tile_color(e.hue_idx, !e.file.is_null());
        let brush = *brushes
            .entry(color)
            .or_insert_with(|| CreateSolidBrush(COLORREF(color)));
        FillRect(mem, &e.rect, brush);
        FrameRect(mem, &e.rect, black_brush);
    }
    for (_, b) in brushes {
        let _ = DeleteObject(b);
    }

    // Labels: folder names in their reserved title strip, leaf names centered
    // in the tile body. Black on the light fills = AAA contrast.
    SetBkMode(mem, TRANSPARENT);
    SetTextColor(mem, COLORREF(0x0000_0000));
    for e in &app.treemap_entries {
        let (name, mut area, fmt) = if e.header_h > 0 {
            // Title strip along the top of a folder tile.
            let strip = RECT {
                left: e.rect.left + 4,
                top: e.rect.top,
                right: e.rect.right - 3,
                bottom: e.rect.top + e.header_h,
            };
            (
                (*e.folder).name.as_str(),
                strip,
                DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_END_ELLIPSIS,
            )
        } else if e.is_leaf
            && (e.rect.right - e.rect.left) >= 42
            && (e.rect.bottom - e.rect.top) >= 14
        {
            let body = RECT {
                left: e.rect.left + 4,
                top: e.rect.top,
                right: e.rect.right - 3,
                bottom: e.rect.bottom,
            };
            let name = if e.file.is_null() {
                (*e.folder).name.as_str()
            } else {
                (*e.file).name.as_str()
            };
            (
                name,
                body,
                DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_END_ELLIPSIS,
            )
        } else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        let mut label: Vec<u16> = name.encode_utf16().collect();
        DrawTextW(mem, &mut label, &mut area, fmt);
    }

    if app.treemap_selected >= 0 {
        if let Some(e) = app.treemap_entries.get(app.treemap_selected as usize) {
            // Two-tone selection ring (white outer + black inner): >= 3:1
            // against any tile color and both canvas backgrounds.
            FrameRect(mem, &e.rect, white_brush);
            let inner = RECT {
                left: e.rect.left + 1,
                top: e.rect.top + 1,
                right: (e.rect.right - 1).max(e.rect.left + 1),
                bottom: (e.rect.bottom - 1).max(e.rect.top + 1),
            };
            FrameRect(mem, &inner, black_brush);
        }
    }

    // Focus indicator: two-tone ring around the canvas edge while the treemap
    // owns keyboard focus (selection alone doesn't show where Enter/Del act).
    if GetFocus() == hwnd {
        FrameRect(mem, &rc, white_brush);
        let inner = RECT {
            left: rc.left + 1,
            top: rc.top + 1,
            right: (rc.right - 1).max(rc.left + 1),
            bottom: (rc.bottom - 1).max(rc.top + 1),
        };
        FrameRect(mem, &inner, black_brush);
    }
    let _ = DeleteObject(white_brush);
    let _ = DeleteObject(black_brush);

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
    for w in [hwnd, app.float_win, app.panel, app.treemap, app.status] {
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
    let _ = InvalidateRect(app.treemap, None, true);
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
    use super::{rel_luminance, tile_color, PALETTE_HUES};

    fn contrast(a: u32, b: u32) -> f64 {
        let (la, lb) = (rel_luminance(a), rel_luminance(b));
        let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
        (hi + 0.05) / (lo + 0.05)
    }

    // Every tile fill must carry black labels at WCAG AAA (7:1) and black
    // borders at AA (3:1). Guards the palette + luminance-floor invariant.
    #[test]
    fn every_tile_color_supports_aaa_black_text() {
        const BLACK: u32 = 0x0000_0000;
        for hue_idx in 0..PALETTE_HUES.len() {
            for is_file in [false, true] {
                let c = tile_color(hue_idx, is_file);
                let ratio = contrast(c, BLACK);
                assert!(
                    ratio >= 7.0,
                    "hue {hue_idx} file={is_file}: black-on-tile contrast {ratio:.2} < 7.0 (AAA)"
                );
            }
        }
    }
}
