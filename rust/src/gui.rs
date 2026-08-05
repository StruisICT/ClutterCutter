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

mod about;
mod chrome;
mod gdi;
mod geometry;
mod listview;
mod palette;

use crate::analysis::{oldest_n_files, top_n_files};
use crate::format::{format_bytes, format_count, join_path};
use crate::mft::{is_ntfs_drive_root, MftScanner};
use crate::scanner::{wide, wstr_to_string, ProgressFn, Scanner};
use crate::temp::{self, TempFileEntry};
use crate::types::{FileEntry, FolderNode, ScanProgress};
use gdi::{
    card_round, draw_expand_box, draw_file_glyph, draw_folder_glyph, draw_text, fill_rect,
    fill_round, make_font, make_font_face,
};
use listview::{
    insert_column, insert_row_with_param, list_item_lparam, remove_side_rows, row_selected,
    selected_indices, selected_list_index, side_subitem_text,
};
use palette::{palette, ThemeMode};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use windows::core::{w, PCSTR, PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    BOOL, COLORREF, FILETIME, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SYSTEMTIME, WPARAM,
};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect, GetSysColor,
    GetSysColorBrush, GetWindowDC, InvalidateRect, MapWindowPoints, RedrawWindow, ReleaseDC,
    SelectObject, SetBkMode, SetTextColor, UpdateWindow, COLOR_BTNFACE, COLOR_HIGHLIGHT,
    COLOR_HIGHLIGHTTEXT, DT_CALCRECT, DT_CENTER, DT_END_ELLIPSIS, DT_HIDEPREFIX, DT_LEFT, DT_RIGHT,
    DT_SINGLELINE, DT_VCENTER, HBRUSH, HDC, HFONT, HGDIOBJ, PAINTSTRUCT, RDW_ALLCHILDREN,
    RDW_ERASE, RDW_FRAME, RDW_INVALIDATE, RDW_UPDATENOW, TRANSPARENT,
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
    ImageList_Create, InitCommonControlsEx, SetWindowTheme, CDDS_ITEMPREPAINT, CDDS_PREPAINT,
    CDDS_SUBITEM, CDRF_DODEFAULT, CDRF_NEWFONT, CDRF_NOTIFYITEMDRAW, CDRF_NOTIFYSUBITEMDRAW,
    CDRF_SKIPDEFAULT, DRAWITEMSTRUCT, HDF_SORTDOWN, HDF_SORTUP, HDITEMW, HDI_FORMAT,
    HDM_GETITEMCOUNT, HDM_GETITEMW, HDM_SETITEMW, ICC_BAR_CLASSES, ICC_LISTVIEW_CLASSES,
    ICC_STANDARD_CLASSES, ICC_TREEVIEW_CLASSES, ILC_COLOR32, INITCOMMONCONTROLSEX, LVIR_BOUNDS,
    LVM_DELETEALLITEMS, LVM_DELETECOLUMN, LVM_GETHEADER, LVM_GETITEMRECT, LVM_GETTOPINDEX,
    LVM_SCROLL, LVM_SETBKCOLOR, LVM_SETCOLUMNWIDTH, LVM_SETEXTENDEDLISTVIEWSTYLE, LVM_SETIMAGELIST,
    LVM_SETTEXTBKCOLOR, LVM_SETTEXTCOLOR, LVN_COLUMNCLICK, LVSIL_SMALL, LVS_EX_DOUBLEBUFFER,
    LVS_EX_FULLROWSELECT, LVS_NOCOLUMNHEADER, LVS_REPORT, LVS_SHOWSELALWAYS, NMCUSTOMDRAW, NMHDR,
    NMITEMACTIVATE, NMLISTVIEW, NMLVCUSTOMDRAW, NM_CLICK, NM_CUSTOMDRAW, NM_DBLCLK, NM_RCLICK,
    ODS_DISABLED, ODS_SELECTED, TVE_EXPAND, TVGN_CARET, TVGN_PARENT, TVGN_ROOT, TVIF_CHILDREN,
    TVIF_HANDLE, TVIF_PARAM, TVIF_TEXT, TVITEMW, TVI_ROOT, TVM_DELETEITEM, TVM_EXPAND,
    TVM_GETITEMW, TVM_GETNEXTITEM, TVM_INSERTITEMW, TVM_SELECTITEM, TVM_SETBKCOLOR, TVM_SETITEMW,
    TVM_SETTEXTCOLOR, TVN_ITEMEXPANDINGW, TVN_SELCHANGEDW, TVS_HASBUTTONS, TVS_HASLINES,
    TVS_LINESATROOT, TVS_SHOWSELALWAYS, TVS_TRACKSELECT,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, GetFocus};
use windows::Win32::UI::Shell::{
    DefSubclassProc, IsUserAnAdmin, SHFileOperationW, SetWindowSubclass, ShellExecuteW,
    FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FO_DELETE, SHFILEOPSTRUCTW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CheckMenuRadioItem, CreateAcceleratorTableW, CreateMenu, CreatePopupMenu,
    CreateWindowExW, DefWindowProcW, DestroyMenu, DispatchMessageW, DrawMenuBar, GetClientRect,
    GetCursorPos, GetMenuBarInfo, GetMenuItemInfoW, GetMessageW, GetSystemMetrics,
    GetWindowLongPtrW, GetWindowRect, GetWindowTextW, IsDialogMessageW, IsZoomed, KillTimer,
    LoadCursorW, LoadIconW, MessageBoxW, MoveWindow, PostMessageW, PostQuitMessage,
    RegisterClassExW, SendMessageW, SetForegroundWindow, SetMenu, SetParent, SetTimer,
    SetWindowLongPtrW, SetWindowPos, SetWindowTextW, ShowWindow, TrackPopupMenu,
    TranslateAcceleratorW, TranslateMessage, ACCEL, BS_OWNERDRAW, BS_PUSHBUTTON, CREATESTRUCTW,
    CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, FVIRTKEY, GWLP_USERDATA, HICON, HMENU, IDC_ARROW,
    IDC_SIZEWE, IDI_APPLICATION, IDYES, MB_ICONWARNING, MB_YESNO, MENUBARINFO, MENUITEMINFOW,
    MF_BYCOMMAND, MF_POPUP, MF_SEPARATOR, MF_STRING, MIIM_STRING, MSG, OBJID_MENU, SM_CXVSCROLL,
    SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SW_HIDE, SW_NORMAL,
    SW_SHOW, TPM_LEFTALIGN, TPM_RIGHTBUTTON, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_COMMAND,
    WM_CREATE, WM_DESTROY, WM_DRAWITEM, WM_ERASEBKGND, WM_HSCROLL, WM_LBUTTONDOWN, WM_MOUSEMOVE,
    WM_MOUSEWHEEL, WM_NCACTIVATE, WM_NCCREATE, WM_NCPAINT, WM_NOTIFY, WM_PAINT, WM_SETREDRAW,
    WM_SIZE, WM_TIMER, WM_VSCROLL, WNDCLASSEXW, WS_BORDER, WS_CHILD, WS_EX_CLIENTEDGE,
    WS_EX_CONTROLPARENT, WS_OVERLAPPEDWINDOW, WS_TABSTOP, WS_VISIBLE,
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
const ID_TOPBAR: u16 = 306;
const ID_SIDEBAR: u16 = 307;
const ID_CRUMB: u16 = 308;
const ID_STATUS: u16 = 400;

// Sum of the fixed main-list column widths (everything except the stretching
// Name column): 128 + 90 + 90 + 80 + 80 + 120. Keep in sync with insert_column.
const MAIN_FIXED_COLS_W: i32 = 128 + 90 + 90 + 80 + 80 + 120;

// Struis ICT redesign geometry.
const TOPBAR_H: i32 = 50; // branded strip: logo + title + theme pill
const SIDEBAR_W: i32 = 244; // left DRIVES column
const CRUMB_H: i32 = 30; // breadcrumb path bar above the table
const DRIVE_CARD_H: i32 = 66; // one drive card
const DRIVE_CARD_GAP: i32 = 8;

// Side panel geometry. The header is a single row: view-switch buttons, the
// view title, then the Detach/Recycle buttons.
const PANEL_W: i32 = 420;
const PANEL_HEADER_H: i32 = 40;
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
// Timer that advances the indeterminate "scanning" bars on drive rows.
const DRIVE_MARQUEE_TIMER: usize = 1;
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

// What the side panel shows. The tree + selected-folder list are always
// visible; these are the optional extra views.
#[derive(Copy, Clone, Default, PartialEq, Eq, Hash)]
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

// One visible row of the main list, in the flat tree-flattened order.
#[derive(Clone, Copy)]
struct ListRow {
    depth: i32,         // indentation level (0 = top of the current folder)
    is_folder: bool,    // folder vs file
    has_children: bool, // folder that can be expanded
    expanded: bool,     // currently expanded inline
    pct: f32,           // fraction 0..1 of this row's size vs its parent folder
}

// A built row: the ListRow model plus the strings needed to insert it.
struct BuiltRow {
    lparam: isize, // FolderNode ptr for folders, 0 for files
    file: isize,   // FileEntry ptr for file rows, 0 for folders
    owner: isize,  // owning FolderNode ptr for file rows (for path + ancestor math)
    name: String,
    subs: [String; 6],
    row: ListRow,
}

// Cross-thread mailbox for scan-all: each drive worker pushes (drive root, result).
type DriveInbox = Arc<Mutex<Vec<(String, Result<FolderNode, String>)>>>;

// A rendered side-panel row, cached so switching between the Top/Oldest views
// doesn't re-walk the whole tree. The raw pointers index the current scan tree
// and are only reused while `tree_version` is unchanged (bumped on any rescan or
// delete), so they never outlive the tree they point into.
#[derive(Clone)]
struct SideRow {
    name: String,
    size: String,
    time: String,
    path: String,
    folder: *const FolderNode,
    file: *const FileEntry,
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

    // Struis ICT redesign chrome: the branded top bar (logo + title + theme
    // pill), the left DRIVES sidebar of usage-bar cards, and the breadcrumb path
    // bar above the table. The folder tree above is kept but hidden — it still
    // holds navigation state that double-click / breadcrumb clicks drive.
    topbar: HWND,
    sidebar: HWND,
    crumb: HWND,
    font_title: HFONT,
    font_small: HFONT,
    // Segoe MDL2 Assets glyphs (folder / file / drive / sun / moon icons).
    font_icon: HFONT,
    // Index into `drives` of the drive whose scan is shown (-1 = all drives),
    // for highlighting the active sidebar card.
    active_drive: i32,
    // Clickable breadcrumb segments: (left x, right x, HTREEITEM) recorded on
    // paint, consumed by the crumb's click hit-test.
    crumb_segs: Vec<(i32, i32, isize)>,
    // Back/forward navigation history of visited tree items, and the current
    // position within it. `nav_lock` suppresses history recording while a
    // Back/Forward action is programmatically re-selecting an item.
    nav_hist: Vec<isize>,
    nav_pos: i32,
    nav_lock: bool,
    // Clickable hotspots of the themed About window: (rect, action) where action
    // 0=coffee, 1=github, 2=site, 3=OK. Recorded on paint, used by click.
    about_hit: Vec<(RECT, i32)>,

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
    // When the current scan started, for the elapsed-time stat in the status bar.
    scan_start: Option<std::time::Instant>,
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
    // Per-row model for the main list (name, column strings, indent depth, expand
    // state, % of parent), indexed by row. Custom drawing reads strings from here
    // rather than re-querying the listview (LVM_GETITEMTEXTW mid-paint corrupts).
    list_rows: Vec<BuiltRow>,
    // FolderNode pointers whose children are expanded inline in the main list.
    expanded: HashSet<isize>,
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
    // Each drive scan reports (drive root path, result) so a finished drive can
    // be matched back to its pre-inserted placeholder row.
    drive_inbox: DriveInbox,
    // Node pointers of drives whose scan is still in flight — their rows render
    // an animated "scanning" bar instead of real numbers.
    pending_drives: std::collections::HashSet<isize>,
    // Advancing counter that animates the indeterminate progress bars.
    marquee_phase: i32,
    // Bumped whenever the scan tree is rebuilt or mutated; keys the side-view
    // cache so a stale (wrong-tree) result is never reused.
    tree_version: u64,
    side_cache: std::collections::HashMap<SideView, (u64, Vec<SideRow>)>,
    // Main-list sort: column index (0=Name..6=Modified) and descending flag,
    // persisted to sort.cfg. Folders still sort before files; the column orders
    // within each group.
    sort_col: i32,
    sort_desc: bool,
    // Cached fill brushes for the two list custom-draw paths, rebuilt on theme
    // change instead of created/destroyed per cell on every repaint.
    brush_card: HBRUSH,
    brush_panel: HBRUSH,
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
    // Recycled individual files (FileEntry pointers), tombstoned the same way so
    // the tree/side views hide them without the pointers dangling.
    deleted_files: HashSet<isize>,
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
            hIcon: load_app_icon(),
            hCursor: LoadCursorW(None, IDC_ARROW).expect("cursor"),
            hbrBackground: GetSysColorBrush(COLOR_BTNFACE),
            lpszMenuName: PCWSTR::null(),
            lpszClassName: class_name,
            hIconSm: load_app_icon(),
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
            topbar: HWND::default(),
            sidebar: HWND::default(),
            crumb: HWND::default(),
            font_title: HFONT::default(),
            font_small: HFONT::default(),
            font_icon: HFONT::default(),
            active_drive: -1,
            crumb_segs: Vec::new(),
            nav_hist: Vec::new(),
            nav_pos: -1,
            nav_lock: false,
            about_hit: Vec::new(),
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
            scan_start: None,
            cancel: Arc::new(AtomicBool::new(false)),
            shared: Arc::new(Mutex::new(ScanState::default())),
            is_admin: IsUserAnAdmin().as_bool(),
            root_node: None,
            item_by_node: HashMap::new(),
            populated: HashSet::new(),
            selected_node: 0,
            list_rows: Vec::new(),
            expanded: HashSet::new(),
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
            pending_drives: std::collections::HashSet::new(),
            marquee_phase: 0,
            tree_version: 0,
            side_cache: std::collections::HashMap::new(),
            sort_col: load_sort().0,
            sort_desc: load_sort().1,
            brush_card: HBRUSH::default(),
            brush_panel: HBRUSH::default(),
            scan_all_first_err: None,
            drive_inbox: Arc::new(Mutex::new(Vec::new())),
            progress_pending: Arc::new(AtomicBool::new(false)),
            deleted_nodes: HashSet::new(),
            deleted_files: HashSet::new(),
            recycle_result: Arc::new(Mutex::new(None)),
        });
        let app_ptr = Box::into_raw(app);

        // Restore the last window size (persisted on close); fall back to a
        // roomy default the first time.
        let (init_w, init_h) = load_window_size();
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class_name,
            w!("ClutterCutter"),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            init_w,
            init_h,
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

        // Open the "Top largest files" side panel by default; it fills in when
        // the startup scan below finishes.
        apply_side_view(hwnd, &mut *app_ptr, SideView::TopFiles);

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

// ---- Window-size persistence ----
//
// The last window size is saved to %APPDATA%\ClutterCutter\window.cfg on close
// and restored on open, so the app remembers how big the user made it.

fn window_cfg_path() -> Option<std::path::PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(
        std::path::Path::new(&appdata)
            .join("ClutterCutter")
            .join("window.cfg"),
    )
}

fn load_window_size() -> (i32, i32) {
    // Roomy first-run default.
    let default = (1700, 900);
    let Some(p) = window_cfg_path() else {
        return default;
    };
    let Ok(s) = std::fs::read_to_string(&p) else {
        return default;
    };
    let mut it = s.split_whitespace();
    let w = it.next().and_then(|x| x.parse().ok()).unwrap_or(default.0);
    let h = it.next().and_then(|x| x.parse().ok()).unwrap_or(default.1);
    // Guard against a corrupt/absurd file.
    (w.clamp(700, 20000), h.clamp(500, 20000))
}

fn save_window_size(w: i32, h: i32) {
    if w < 200 || h < 200 {
        return;
    }
    if let Some(p) = window_cfg_path() {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&p, format!("{w} {h}"));
    }
}

fn sort_cfg_path() -> Option<std::path::PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(
        std::path::Path::new(&appdata)
            .join("ClutterCutter")
            .join("sort.cfg"),
    )
}

// The main-list sort (column index + descending flag) persists across runs. The
// default — column 0 (Name), ascending — reproduces the historical order.
fn load_sort() -> (i32, bool) {
    let default = (0i32, false);
    let Some(p) = sort_cfg_path() else {
        return default;
    };
    let Ok(s) = std::fs::read_to_string(&p) else {
        return default;
    };
    let mut it = s.split_whitespace();
    let col = it.next().and_then(|x| x.parse().ok()).unwrap_or(default.0);
    let desc = it.next().map(|x| x == "1").unwrap_or(default.1);
    ((col).clamp(0, 6), desc)
}

fn save_sort(col: i32, desc: bool) {
    if let Some(p) = sort_cfg_path() {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&p, format!("{col} {}", desc as i32));
    }
}

// Loads the app's own icon embedded by app.rc (resource id 1) so the window's
// title bar / Alt-Tab icon matches the taskbar icon; falls back to the system
// application icon if the resource can't be loaded.
unsafe fn load_app_icon() -> HICON {
    // PCWSTR(1) is MAKEINTRESOURCE(1) — a resource ordinal, not a real pointer.
    #[allow(clippy::manual_dangling_ptr)]
    let icon_ordinal = PCWSTR(1 as *const u16);
    if let Ok(h) = GetModuleHandleW(None) {
        if let Ok(icon) = LoadIconW(HINSTANCE(h.0), icon_ordinal) {
            return icon;
        }
    }
    LoadIconW(None, IDI_APPLICATION).unwrap_or_default()
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
        WM_TIMER if wparam.0 == DRIVE_MARQUEE_TIMER => {
            // Advance the indeterminate bars on drives still being scanned.
            if app.scan_all_active && !app.pending_drives.is_empty() {
                app.marquee_phase = app.marquee_phase.wrapping_add(1);
                let _ = InvalidateRect(app.list, None, false);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            app.cancel.store(true, Ordering::SeqCst);
            if !app.brush_card.is_invalid() {
                let _ = DeleteObject(app.brush_card);
            }
            if !app.brush_panel.is_invalid() {
                let _ = DeleteObject(app.brush_panel);
            }
            // Persist the window size so next launch opens at the same size.
            // Skip when maximized so we save the restored size, not the maxed one.
            if !IsZoomed(hwnd).as_bool() {
                let mut wr = RECT::default();
                if GetWindowRect(hwnd, &mut wr).is_ok() {
                    save_window_size(wr.right - wr.left, wr.bottom - wr.top);
                }
            }
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
        fill_rect(hdc, &rc, 0x0020_2020);
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
            about::show_about(hwnd, app);
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
                app.active_drive = -1;
                let _ = InvalidateRect(app.sidebar, None, false);
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
                // Highlight this card as the active drive in the sidebar.
                app.active_drive = idx as i32;
                let _ = InvalidateRect(app.sidebar, None, false);
                start_scan(hwnd, app, drive.root, use_mft);
            }
        }
        _ => {}
    }
}

unsafe fn on_notify(hwnd: HWND, app: &mut AppState, lparam: LPARAM) -> LRESULT {
    let hdr = &*(lparam.0 as *const NMHDR);
    // Owner-draw the main list's "% of parent" bar column.
    if hdr.hwndFrom == app.list && hdr.code == NM_CUSTOMDRAW {
        return custom_draw_main_list(app, lparam.0 as *const NMLVCUSTOMDRAW);
    }
    // Owner-draw the side panel's Size column as a brand-blue badge.
    if hdr.hwndFrom == app.side_list && hdr.code == NM_CUSTOMDRAW {
        return custom_draw_side_list(app, lparam.0 as *const NMLVCUSTOMDRAW);
    }
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
            // Single click on a folder's expand box toggles inline expansion.
            c if c == NM_CLICK => {
                let act = &*(lparam.0 as *const NMITEMACTIVATE);
                if act.iItem >= 0 {
                    let row = act.iItem as usize;
                    if let Some(lr) = app.list_rows.get(row).map(|b| b.row) {
                        if lr.is_folder && lr.has_children {
                            let gx = 4 + lr.depth * TREE_INDENT;
                            let px = act.ptAction.x;
                            if px >= gx && px < gx + TREE_GLYPH_W {
                                toggle_expand(app, row);
                            }
                        }
                    }
                }
            }
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
            // Click a column header to sort by it; click the same column again to
            // reverse. The choice persists across runs.
            c if c == LVN_COLUMNCLICK => {
                let nmlv = &*(lparam.0 as *const NMLISTVIEW);
                let col = nmlv.iSubItem;
                if (0..=6).contains(&col) {
                    if app.sort_col == col {
                        app.sort_desc = !app.sort_desc;
                    } else {
                        app.sort_col = col;
                        // Name reads best ascending; size/count/date default to
                        // descending (largest / most / newest first).
                        app.sort_desc = col != 0;
                    }
                    save_sort(app.sort_col, app.sort_desc);
                    update_sort_arrows(app);
                    if app.selected_node != 0 {
                        populate_list_folders(app, &*(app.selected_node as *const FolderNode));
                    }
                }
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

// Reliable per-row selection test: `nmcd.uItemState` is not dependable at the
// sub-item custom-draw stage, so query the list directly (LVIS_SELECTED = 2).
// Custom-drawn columns of the main list.
const NAME_COL: i32 = 0;
const PCT_COL: i32 = 1;
const MODIFIED_COL: i32 = 6;
// Tree layout: indent per depth level, and the width reserved for the expand box.
const TREE_INDENT: i32 = 16;
const TREE_GLYPH_W: i32 = 16;

// A small tree expand/collapse box (a bordered square with a "−" when expanded,
// "+" when collapsed) centred vertically at `cy`, left edge at `x`.
// Subclass applied to both custom-drawn listviews. It (1) recolors the main
// list's column-header text so it stays readable on the dark header background,
// and (2) forces a full client repaint after any scroll. Without (2), the
// listview blit-scrolls and only invalidates the newly-exposed strip, which
// leaves slivers of the previous frame behind — a stray expand-box/folder glyph
// in the header seam, or a duplicated card at the top of the side panel. A full
// invalidate is flicker-free here because both lists are double-buffered.
unsafe extern "system" fn list_header_subclass(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    refdata: usize,
) -> LRESULT {
    // Scrolling blit-scrolls the client and only invalidates the newly-exposed
    // strip, and hot-tracking (WM_MOUSEMOVE) repaints only the row under the
    // cursor — either way a sliver clipped at the very top of the list (the
    // expand-box/folder glyph in the header seam, or a duplicated first card in
    // the side panel) can survive one frame. Repaint the top *synchronously*
    // with RDW_UPDATENOW so the stale pixels are overwritten in the back buffer
    // before the frame is presented — a queued InvalidateRect repaints a cycle
    // later, which shows the sliver for a brief flash. RDW_ALLCHILDREN also
    // redraws the header on top of any glyph the scroll blitted into its band.
    if msg == WM_MOUSEWHEEL || msg == WM_VSCROLL || msg == WM_HSCROLL {
        let r = DefSubclassProc(hwnd, msg, wparam, lparam);
        let _ = RedrawWindow(
            hwnd,
            None,
            None,
            RDW_INVALIDATE | RDW_UPDATENOW | RDW_ALLCHILDREN,
        );
        return r;
    }
    if msg == WM_MOUSEMOVE {
        let r = DefSubclassProc(hwnd, msg, wparam, lparam);
        let mut cl = RECT::default();
        let _ = GetClientRect(hwnd, &mut cl);
        let strip = RECT {
            left: cl.left,
            top: cl.top,
            right: cl.right,
            bottom: (cl.top + 96).min(cl.bottom),
        };
        let _ = RedrawWindow(
            hwnd,
            Some(&strip),
            None,
            RDW_INVALIDATE | RDW_UPDATENOW | RDW_ALLCHILDREN,
        );
        return r;
    }
    if msg == WM_NOTIFY {
        let nmhdr = &*(lparam.0 as *const NMHDR);
        if nmhdr.code == NM_CUSTOMDRAW {
            let header = SendMessageW(hwnd, LVM_GETHEADER, WPARAM(0), LPARAM(0)).0;
            if header != 0 && nmhdr.hwndFrom.0 as isize == header {
                let app = &*(refdata as *const AppState);
                let nmcd = &*(lparam.0 as *const NMCUSTOMDRAW);
                let stage = nmcd.dwDrawStage.0;
                if stage == CDDS_PREPAINT.0 {
                    return LRESULT(CDRF_NOTIFYITEMDRAW as isize);
                }
                if stage == CDDS_ITEMPREPAINT.0 {
                    SetTextColor(nmcd.hdc, COLORREF(palette(app.is_dark).text));
                    return LRESULT(CDRF_NEWFONT as isize);
                }
            }
        }
    }
    DefSubclassProc(hwnd, msg, wparam, lparam)
}

// Owner-draws every column of the main list. Filling each cell's background also
// suppresses the themed hover-highlight that would otherwise show through. The
// Name column renders the tree indent + expand box + folder/file glyph + name.
unsafe fn custom_draw_main_list(app: &AppState, lv: *const NMLVCUSTOMDRAW) -> LRESULT {
    let lv = &*lv;
    let stage = lv.nmcd.dwDrawStage.0;
    if stage == CDDS_PREPAINT.0 {
        return LRESULT(CDRF_NOTIFYITEMDRAW as isize);
    }
    if stage == CDDS_ITEMPREPAINT.0 {
        return LRESULT(CDRF_NOTIFYSUBITEMDRAW as isize);
    }
    if stage != (CDDS_SUBITEM.0 | CDDS_ITEMPREPAINT.0) {
        return LRESULT(CDRF_DODEFAULT as isize);
    }
    let sub = lv.iSubItem;
    let row = lv.nmcd.dwItemSpec;
    let br = match app.list_rows.get(row) {
        Some(b) => b,
        None => return LRESULT(CDRF_SKIPDEFAULT as isize),
    };
    let hdc = lv.nmcd.hdc;
    let rc = lv.nmcd.rc;
    let selected = row_selected(app.list, row);
    let p = palette(app.is_dark);
    let (bg, fg) = if selected {
        (
            GetSysColor(COLOR_HIGHLIGHT),
            GetSysColor(COLOR_HIGHLIGHTTEXT),
        )
    } else {
        (p.card_bg, p.text)
    };
    // Fill the cell — this is what hides the theme's hot-track hover highlight.
    // Use the cached theme brush for the (very common) unselected fill; the
    // selected fill uses the shared system brush. Both avoid a per-cell
    // create/destroy on every repaint.
    let bgb = if selected {
        GetSysColorBrush(COLOR_HIGHLIGHT)
    } else {
        app.brush_card
    };
    FillRect(hdc, &rc, bgb);
    SetBkMode(hdc, TRANSPARENT);

    let lr = br.row;
    let cy = (rc.top + rc.bottom) / 2;

    if sub == NAME_COL {
        let glyph_x = rc.left + 4 + lr.depth * TREE_INDENT;
        if lr.is_folder && lr.has_children {
            let c = if selected { fg } else { p.subtext };
            draw_expand_box(hdc, glyph_x, cy, lr.expanded, c, bg);
        }
        let icx = glyph_x + TREE_GLYPH_W + 8;
        if lr.is_folder {
            draw_folder_glyph(hdc, icx, cy, if selected { fg } else { p.blue });
        } else {
            draw_file_glyph(hdc, icx, cy, if selected { fg } else { p.subtext }, bg);
        }
        let mut name: Vec<u16> = br.name.encode_utf16().collect();
        let mut nrc = RECT {
            left: icx + 14,
            top: rc.top,
            right: rc.right - 4,
            bottom: rc.bottom,
        };
        SetTextColor(hdc, COLORREF(fg));
        DrawTextW(
            hdc,
            &mut name,
            &mut nrc,
            DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_END_ELLIPSIS,
        );
        return LRESULT(CDRF_SKIPDEFAULT as isize);
    }

    if sub == PCT_COL {
        let bar_h = 8;
        let bar_left = rc.left + 6;
        let bar_top = rc.top + ((rc.bottom - rc.top) - bar_h) / 2;
        // Drives still being scanned show an indeterminate marquee instead of a
        // %-of-parent bar, so the user can see work is in progress.
        if app.pending_drives.contains(&br.lparam) {
            let bar_right = rc.right - 8;
            let track = RECT {
                left: bar_left,
                top: bar_top,
                right: bar_right,
                bottom: bar_top + bar_h,
            };
            fill_round(hdc, &track, 4, p.track);
            let track_w = bar_right - bar_left;
            if track_w > 20 {
                let seg_w = track_w / 3;
                let span = track_w + seg_w;
                let pos = (app.marquee_phase * 6).rem_euclid(span) - seg_w;
                let seg_left = (bar_left + pos).clamp(bar_left, bar_right);
                let seg_right = (bar_left + pos + seg_w).clamp(bar_left, bar_right);
                if seg_right > seg_left {
                    let seg = RECT {
                        left: seg_left,
                        top: bar_top,
                        right: seg_right,
                        bottom: bar_top + bar_h,
                    };
                    fill_round(hdc, &seg, 4, p.blue);
                }
            }
            return LRESULT(CDRF_SKIPDEFAULT as isize);
        }
        let pct = lr.pct.clamp(0.0, 1.0);
        let text_w = 52;
        let bar_right = rc.right - text_w - 4;
        if bar_right - bar_left > 16 {
            let track = RECT {
                left: bar_left,
                top: bar_top,
                right: bar_right,
                bottom: bar_top + bar_h,
            };
            fill_round(hdc, &track, 4, if selected { 0x00C8_C8C8 } else { p.track });
            let fill_w = ((bar_right - bar_left) as f32 * pct).round() as i32;
            if fill_w >= 4 {
                let fill = RECT {
                    left: bar_left,
                    top: bar_top,
                    right: bar_left + fill_w,
                    bottom: bar_top + bar_h,
                };
                fill_round(hdc, &fill, 4, p.green);
            }
        }
        let mut txt: Vec<u16> = format!("{:.1}%", pct * 100.0).encode_utf16().collect();
        let mut trc = RECT {
            left: rc.right - text_w - 2,
            top: rc.top,
            right: rc.right - 6,
            bottom: rc.bottom,
        };
        SetTextColor(hdc, COLORREF(fg));
        DrawTextW(
            hdc,
            &mut txt,
            &mut trc,
            DT_SINGLELINE | DT_VCENTER | DT_RIGHT,
        );
        return LRESULT(CDRF_SKIPDEFAULT as isize);
    }

    // A drive still being scanned has no real numbers yet: show "Scanning…" in
    // the Size column and leave the rest blank.
    if app.pending_drives.contains(&br.lparam) {
        if sub == 2 {
            let trc = RECT {
                left: rc.left + 4,
                top: rc.top,
                right: rc.right - 8,
                bottom: rc.bottom,
            };
            SetTextColor(hdc, COLORREF(if selected { fg } else { p.blue }));
            draw_text(
                hdc,
                "Scanning\u{2026}",
                &trc,
                DT_SINGLELINE | DT_VCENTER | DT_RIGHT,
            );
        }
        return LRESULT(CDRF_SKIPDEFAULT as isize);
    }

    // Detail columns: Size/Own size/Files/Folders are right-aligned; Modified is
    // left-aligned (matching the column headers). `subs[sub-1]` maps a column to
    // its stored string (subs[0] is the empty %-of-parent placeholder).
    let mut txt: Vec<u16> = br.subs[(sub - 1) as usize].encode_utf16().collect();
    SetTextColor(hdc, COLORREF(fg));
    let (fmt, trc) = if sub == MODIFIED_COL {
        (
            DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_END_ELLIPSIS,
            RECT {
                left: rc.left + 6,
                top: rc.top,
                right: rc.right - 4,
                bottom: rc.bottom,
            },
        )
    } else {
        (
            DT_SINGLELINE | DT_VCENTER | DT_RIGHT | DT_END_ELLIPSIS,
            RECT {
                left: rc.left + 4,
                top: rc.top,
                right: rc.right - 8,
                bottom: rc.bottom,
            },
        )
    };
    let mut trc = trc;
    // DrawTextW on an empty slice dereferences the Vec's dangling pointer and
    // faults — file rows have empty own-size/files/folders cells, so skip them.
    if !txt.is_empty() {
        DrawTextW(hdc, &mut txt, &mut trc, fmt);
    }
    LRESULT(CDRF_SKIPDEFAULT as isize)
}

// Reads a side-list sub-item's text into a UTF-16 vec.
// Owner-draws each side-panel row as a white rounded card: bold name + blue size
// on the first line, a muted path on the second — matching the Struis ICT mockup.
unsafe fn custom_draw_side_list(app: &AppState, lv: *const NMLVCUSTOMDRAW) -> LRESULT {
    let lv = &*lv;
    let stage = lv.nmcd.dwDrawStage.0;
    if stage == CDDS_PREPAINT.0 {
        return LRESULT(CDRF_NOTIFYITEMDRAW as isize);
    }
    // Draw the whole row at item-prepaint and skip default sub-item rendering.
    if stage != CDDS_ITEMPREPAINT.0 {
        return LRESULT(CDRF_DODEFAULT as isize);
    }

    let row = lv.nmcd.dwItemSpec;
    let hdc = lv.nmcd.hdc;
    let rc = lv.nmcd.rc;
    let selected = row_selected(app.side_list, row);
    let p = palette(app.is_dark);

    // The item rect spans the (wide) column layout; the card must fit the list's
    // *visible* width instead, so it doesn't scroll off to the right.
    let mut cl = RECT::default();
    let _ = GetClientRect(app.side_list, &mut cl);
    let rowbg = RECT {
        left: cl.left,
        top: rc.top,
        right: cl.right,
        bottom: rc.bottom,
    };
    FillRect(hdc, &rowbg, app.brush_panel);
    let card = RECT {
        left: cl.left + 6,
        top: rc.top + 3,
        right: cl.right - 6,
        bottom: rc.bottom - 3,
    };
    let border = if selected { p.blue } else { p.hairline };
    card_round(hdc, &card, 8, p.card_bg, border, 1);

    let name = side_subitem_text(app.side_list, row, 0);
    let size = side_subitem_text(app.side_list, row, 1);
    // The path lives in the last column, which differs per view.
    let path_col = if app.side_view == SideView::TempFiles {
        4
    } else {
        3
    };
    let path = side_subitem_text(app.side_list, row, path_col);

    SetBkMode(hdc, TRANSPARENT);
    let lx = card.left + 12;
    let top_b = card.top;
    let mid = (card.top + card.bottom) / 2;

    // Size (blue) on the right of the first line — measure it so the name clamps.
    let old = SelectObject(hdc, HGDIOBJ(app.font_small.0));
    let mut size_txt = size.clone();
    let mut scalc = RECT::default();
    DrawTextW(hdc, &mut size_txt, &mut scalc, DT_CALCRECT | DT_SINGLELINE);
    let size_w = scalc.right - scalc.left;
    SetTextColor(hdc, COLORREF(p.blue));
    let mut src = RECT {
        left: card.right - size_w - 12,
        top: top_b,
        right: card.right - 12,
        bottom: mid,
    };
    DrawTextW(
        hdc,
        &mut size_txt,
        &mut src,
        DT_SINGLELINE | DT_VCENTER | DT_RIGHT,
    );
    // Name (semibold, dark) on the first line.
    SetTextColor(hdc, COLORREF(p.text));
    let mut name_txt = name;
    let mut nrc = RECT {
        left: lx,
        top: top_b,
        right: card.right - size_w - 20,
        bottom: mid,
    };
    DrawTextW(
        hdc,
        &mut name_txt,
        &mut nrc,
        DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_END_ELLIPSIS,
    );
    // Path (regular, muted) on the second line.
    SelectObject(hdc, old);
    SetTextColor(hdc, COLORREF(p.subtext));
    let mut path_txt = path;
    let mut prc = RECT {
        left: lx,
        top: mid,
        right: card.right - 12,
        bottom: card.bottom,
    };
    DrawTextW(
        hdc,
        &mut path_txt,
        &mut prc,
        DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_END_ELLIPSIS,
    );
    LRESULT(CDRF_SKIPDEFAULT as isize)
}

// The theme palette (`Pal` + `palette` + `ThemeMode`) lives in `gui::palette`.
// The GDI drawing helpers live in `gui::gdi`.

// Filled rounded rectangle in `color` (no visible border).
// (Re)create the cached list fill brushes for the current theme. Called at
// startup and whenever the theme flips; the old brushes are freed first.
unsafe fn rebuild_theme_brushes(app: &mut AppState) {
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

// Owner-draws a flat, rounded push button matching the redesign. `primary` fills
// it with the accent blue (the Scan-all call-to-action); otherwise it is a
// card-style button with a hairline border. `parent_bg` clears the corners.
unsafe fn draw_flat_button(
    app: &AppState,
    dis: *const DRAWITEMSTRUCT,
    primary: bool,
    parent_bg: u32,
) {
    let dis = &*dis;
    let hdc = dis.hDC;
    let rc = dis.rcItem;
    let pressed = (dis.itemState.0 & ODS_SELECTED.0) != 0;
    let disabled = (dis.itemState.0 & ODS_DISABLED.0) != 0;
    let p = palette(app.is_dark);

    // Clear the item rect to the parent background so the rounded corners blend.
    let clr = CreateSolidBrush(COLORREF(parent_bg));
    FillRect(hdc, &rc, clr);
    let _ = DeleteObject(clr);

    let (fill, border, text) = if disabled {
        (parent_bg, p.hairline, p.subtext)
    } else if primary {
        let f = if pressed { 0x00C0_5A24 } else { p.blue };
        (f, f, 0x00FF_FFFF)
    } else if pressed {
        (p.track, p.blue, p.text)
    } else {
        (p.card_bg, p.hairline, p.text)
    };
    let radius = (rc.bottom - rc.top).min(rc.right - rc.left).clamp(6, 16) / 2 + 3;
    card_round(hdc, &rc, radius, fill, border, 1);

    // Centred label.
    let mut buf = [0u16; 64];
    let n = GetWindowTextW(dis.hwndItem, &mut buf) as usize;
    if n > 0 {
        SetBkMode(hdc, TRANSPARENT);
        SetTextColor(hdc, COLORREF(text));
        let old = SelectObject(hdc, HGDIOBJ(app.font_small.0));
        let mut r = rc;
        DrawTextW(
            hdc,
            &mut buf[..n],
            &mut r,
            DT_SINGLELINE | DT_VCENTER | DT_CENTER,
        );
        SelectObject(hdc, old);
    }
}

// Top-bar button/pill geometry lives in `gui::geometry`.

// The top-bar WNDPROC lives in `gui::chrome`.

// The DRIVES-sidebar WNDPROC lives in `gui::chrome`.

// The breadcrumb WNDPROC lives in `gui::chrome`.

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
        w!("Scan all drives"),
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_OWNERDRAW as u32),
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

    // No gridlines — the mockup uses clean rows with only subtle separators.
    let ext = (LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER) as isize;
    SendMessageW(
        app.list,
        LVM_SETEXTENDEDLISTVIEWSTYLE,
        WPARAM(0),
        LPARAM(ext),
    );
    // Recolor the column-header text so it stays readable in dark mode.
    let _ = SetWindowSubclass(
        app.list,
        Some(list_header_subclass),
        1,
        app as *mut AppState as usize,
    );

    // ---- Struis ICT redesign chrome: fonts + top bar + sidebar + breadcrumb ----
    app.font_title = make_font(-22, 700); // bold "ClutterCutter"
    app.font_small = make_font(-13, 600); // labels / subtitle
    app.font_icon = make_font_face(-15, 400, "Segoe MDL2 Assets"); // glyph icons
    let app_lp = app as *mut AppState as isize;

    let topbar_class = w!("ClutterCutterTop");
    RegisterClassExW(&WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(chrome::topbar_proc),
        hInstance: hinstance.into(),
        hCursor: LoadCursorW(None, IDC_ARROW).expect("cursor"),
        hbrBackground: HBRUSH::default(),
        lpszClassName: topbar_class,
        ..Default::default()
    });
    app.topbar = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        topbar_class,
        PCWSTR::null(),
        WS_CHILD | WS_VISIBLE,
        0,
        0,
        800,
        TOPBAR_H,
        hwnd,
        HMENU(ID_TOPBAR as isize as _),
        hinstance,
        None,
    )
    .expect("topbar");
    SetWindowLongPtrW(app.topbar, GWLP_USERDATA, app_lp);

    let sidebar_class = w!("ClutterCutterDrives");
    RegisterClassExW(&WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(chrome::sidebar_proc),
        hInstance: hinstance.into(),
        hCursor: LoadCursorW(None, IDC_ARROW).expect("cursor"),
        hbrBackground: HBRUSH::default(),
        lpszClassName: sidebar_class,
        ..Default::default()
    });
    app.sidebar = CreateWindowExW(
        WS_EX_CONTROLPARENT,
        sidebar_class,
        PCWSTR::null(),
        WS_CHILD | WS_VISIBLE,
        0,
        TOPBAR_H,
        SIDEBAR_W,
        400,
        hwnd,
        HMENU(ID_SIDEBAR as isize as _),
        hinstance,
        None,
    )
    .expect("sidebar");
    SetWindowLongPtrW(app.sidebar, GWLP_USERDATA, app_lp);

    let crumb_class = w!("ClutterCutterCrumb");
    RegisterClassExW(&WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(chrome::crumb_proc),
        hInstance: hinstance.into(),
        hCursor: LoadCursorW(None, IDC_ARROW).expect("cursor"),
        hbrBackground: HBRUSH::default(),
        lpszClassName: crumb_class,
        ..Default::default()
    });
    app.crumb = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        crumb_class,
        PCWSTR::null(),
        WS_CHILD | WS_VISIBLE,
        SIDEBAR_W,
        TOPBAR_H,
        400,
        CRUMB_H,
        hwnd,
        HMENU(ID_CRUMB as isize as _),
        hinstance,
        None,
    )
    .expect("crumb");
    SetWindowLongPtrW(app.crumb, GWLP_USERDATA, app_lp);

    // The horizontal drive-button bar and the folder tree are replaced by the
    // sidebar + breadcrumb, but kept alive (hidden): the tree still holds the
    // navigation state that double-click / breadcrumb clicks drive, and the
    // Scan-all button is reparented into the sidebar so its command still fires.
    let _ = ShowWindow(app.tree, SW_HIDE);
    let _ = ShowWindow(app.stop_btn, SW_HIDE);
    for b in &app.drive_buttons {
        let _ = ShowWindow(*b, SW_HIDE);
    }
    let _ = SetParent(app.scan_all_btn, app.sidebar);

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
        lpfnWndProc: Some(chrome::float_proc),
        hInstance: hinstance.into(),
        hIcon: load_app_icon(),
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
        lpfnWndProc: Some(chrome::splitter_proc),
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
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_OWNERDRAW as u32),
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
        WS_CHILD | WS_TABSTOP | WINDOW_STYLE(BS_OWNERDRAW as u32), // temp view only
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
        WS_CHILD
            | WS_TABSTOP
            | WINDOW_STYLE(LVS_REPORT)
            | WINDOW_STYLE(LVS_SHOWSELALWAYS)
            | WINDOW_STYLE(LVS_NOCOLUMNHEADER),
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
    // Same subclass as the main list: force a full repaint after scroll so no
    // sliver of the previous frame (a duplicated card) survives at the top.
    let _ = SetWindowSubclass(
        app.side_list,
        Some(list_header_subclass),
        1,
        app as *mut AppState as usize,
    );
    // A 1×46 image list forces ~46px rows so each row can hold a two-line card.
    let il = ImageList_Create(1, 46, ILC_COLOR32, 1, 1);
    if il.0 != 0 {
        SendMessageW(
            app.side_list,
            LVM_SETIMAGELIST,
            WPARAM(LVSIL_SMALL as usize),
            LPARAM(il.0),
        );
    }

    // Columns mirror the Struis ICT design: Name, a custom-drawn "% of parent"
    // bar, then the numeric/date detail columns. Keep MAIN_FIXED_COLS_W in sync
    // with the fixed widths below (everything except the stretching Name column).
    insert_column(app.list, 0, "NAME", 320, false);
    insert_column(app.list, 1, "% OF PARENT", 128, false);
    insert_column(app.list, 2, "SIZE", 90, true);
    insert_column(app.list, 3, "OWN SIZE", 90, true);
    insert_column(app.list, 4, "FILES", 80, true);
    insert_column(app.list, 5, "FOLDERS", 80, true);
    insert_column(app.list, 6, "MODIFIED", 120, false);
    update_sort_arrows(app); // reflect the persisted/default sort on the header

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
        lpfnWndProc: Some(chrome::status_proc),
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

// The status-strip WNDPROC lives in `gui::chrome`.

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

    // Row 0: branded top bar spanning the full width.
    let _ = MoveWindow(app.topbar, 0, 0, rc.right, TOPBAR_H, true);

    // Row 1: left DRIVES sidebar, then the content area (breadcrumb + table +
    // optional side panel).
    let top = TOPBAR_H;
    let body_h = (rc.bottom - top - STATUS_H).max(0);
    let _ = MoveWindow(app.sidebar, 0, top, SIDEBAR_W, body_h, true);
    // The Scan-all button sits at the bottom of the sidebar (its child now).
    let sab_h = 34;
    let sab_margin = 12;
    let _ = MoveWindow(
        app.scan_all_btn,
        12,
        body_h - sab_h - sab_margin,
        SIDEBAR_W - 24,
        sab_h,
        true,
    );

    let content_x = SIDEBAR_W;
    let content_w = (rc.right - content_x).max(0);
    // Breadcrumb spans the content area above the table.
    let _ = MoveWindow(app.crumb, content_x, top, content_w, CRUMB_H, true);

    let table_top = top + CRUMB_H;
    let table_h = (body_h - CRUMB_H).max(0);
    // The side panel takes `panel_frac` of the content width, with a draggable
    // splitter between it and the table. Because it's a fraction, the panel grows
    // with the window and the user can drag the split.
    let panel_here = app.side_view != SideView::None && !app.detached;
    let split_w = if panel_here { SPLIT_W } else { 0 };
    let avail = content_w;
    let panel_w = if panel_here {
        let raw = (avail as f64 * app.panel_frac).round() as i32;
        // Keep both panes usable.
        raw.clamp(180, (avail - split_w - 180).max(180))
    } else {
        0
    };
    let list_w = (content_w - panel_w - split_w).max(0);
    let _ = MoveWindow(app.list, content_x, table_top, list_w, table_h, true);
    if panel_here {
        let _ = MoveWindow(
            app.splitter,
            content_x + list_w,
            table_top,
            split_w,
            table_h,
            true,
        );
        let _ = ShowWindow(app.splitter, SW_SHOW);
        let _ = MoveWindow(
            app.panel,
            content_x + list_w + split_w,
            table_top,
            panel_w,
            table_h,
            true,
        );
    } else {
        let _ = ShowWindow(app.splitter, SW_HIDE);
    }
    // Stretch the Name column so the folder list's columns always fill the
    // list width — otherwise widening the window (which slides the flush-right
    // panel over) leaves a growing block of empty list to the right of the
    // last column. All columns except Name are fixed (MAIN_FIXED_COLS_W).
    let vscroll = GetSystemMetrics(SM_CXVSCROLL);
    let name_w = (list_w - MAIN_FIXED_COLS_W - vscroll - 4).max(120);
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
    app.scan_start = Some(std::time::Instant::now());
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
        // Share one progress callback between the MFT attempt and the walker
        // fallback (ProgressFn is a Box and can't be cloned directly).
        let shared: Arc<ProgressFn> = Arc::new(progress);
        let p_mft: ProgressFn = {
            let s = shared.clone();
            Box::new(move |sp: &ScanProgress| (s)(sp))
        };
        match MftScanner::new()
            .with_cancel(cancel.clone())
            .with_progress(p_mft)
            .with_track_files(true)
            .scan(path)
        {
            Ok(node) => Ok(node),
            // The MFT fast path can fail on a volume that is NTFS yet has an
            // unexpected raw layout (dynamic disk, odd geometry, transient lock,
            // a removable NTFS stick that won't open raw). Rather than drop the
            // whole drive from the results, fall back to the ordinary walker —
            // unless the user cancelled, in which case honor that.
            Err(_) if !cancel.load(Ordering::Relaxed) => {
                let p_walk: ProgressFn = {
                    let s = shared.clone();
                    Box::new(move |sp: &ScanProgress| (s)(sp))
                };
                Scanner::new()
                    .with_cancel(cancel)
                    .with_progress(p_walk)
                    .with_track_files(true)
                    .scan(path)
                    .map_err(|e| e.to_string())
            }
            Err(e) => Err(e),
        }
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

    // Create the synthetic root now with a placeholder child per drive, so every
    // drive shows in the list immediately (each with an animated "scanning" bar)
    // and gets filled in place as its scan finishes. Capacity is exactly the
    // drive count, and finished drives replace their slot in place, so the Vec
    // never reallocates — keeping the raw child pointers the tree items hold valid.
    let mut root = FolderNode {
        full_path: String::new(), // synthetic — shell actions no-op on it
        name: "All drives".to_string(),
        ..Default::default()
    };
    root.children = Vec::with_capacity(n);
    for (path, _) in &targets {
        root.children.push(FolderNode {
            full_path: path.clone(),
            name: path.clone(),
            ..Default::default()
        });
    }
    app.root_node = Some(Box::new(root));
    let root_ptr = app.root_node.as_deref().unwrap() as *const FolderNode;
    let hti = insert_tree_item(app.tree, 0, &*root_ptr, false);
    set_tree_item_has_children(app.tree, hti);
    app.item_by_node.insert(root_ptr as isize, hti);
    app.populated.insert(hti); // drives are added by hand, not lazily
    app.selected_node = root_ptr as isize;
    SendMessageW(
        app.tree,
        TVM_SELECTITEM,
        WPARAM(TVGN_CARET as usize),
        LPARAM(hti),
    );

    // Insert a tree item for each placeholder drive and mark it pending.
    app.pending_drives.clear();
    let drive_ptrs: Vec<isize> = {
        let root = app.root_node.as_deref().unwrap();
        root.children
            .iter()
            .map(|c| c as *const FolderNode as isize)
            .collect()
    };
    for &dp in &drive_ptrs {
        let child = &*(dp as *const FolderNode);
        let dhti = insert_tree_item(app.tree, hti, child, true);
        app.item_by_node.insert(dp, dhti);
        app.pending_drives.insert(dp);
    }
    SendMessageW(
        app.tree,
        TVM_EXPAND,
        WPARAM(TVE_EXPAND.0 as usize),
        LPARAM(hti),
    );
    populate_list_folders(app, &*root_ptr);

    app.scan_all_active = true;
    app.drives_expected = n;
    app.drives_done = 0;
    app.scan_all_first_err = None;
    app.marquee_phase = 0;
    let inbox = Arc::new(Mutex::new(Vec::new()));
    app.drive_inbox = inbox.clone();
    // Animate the pending bars until every drive is in.
    SetTimer(hwnd, DRIVE_MARQUEE_TIMER, 60, None);

    let send_hwnd = SendHwnd(hwnd.0 as isize);
    for (path, use_mft) in targets {
        let inbox = inbox.clone();
        let cancel = app.cancel.clone();
        let progress = make_progress(send_hwnd, app.shared.clone(), app.progress_pending.clone());
        std::thread::spawn(move || {
            let res =
                scan_one(&path, use_mft, cancel, progress).map_err(|e| format!("{path}: {e}"));
            if let Ok(mut q) = inbox.lock() {
                q.push((path, res));
            }
            unsafe {
                let _ = PostMessageW(send_hwnd.to_hwnd(), WM_APP_DRIVE_DONE, WPARAM(0), LPARAM(0));
            }
        });
    }
}

// One or more drives finished: drain the inbox, fill each drive's placeholder
// row in place, and refresh the visible views. Runs on the UI thread.
unsafe fn on_drive_done(app: &mut AppState) {
    if !app.scan_all_active {
        return;
    }
    let drained: Vec<(String, Result<FolderNode, String>)> = {
        let mut q = app.drive_inbox.lock().unwrap();
        std::mem::take(&mut *q)
    };
    for (path, res) in drained {
        app.drives_done += 1;
        match res {
            Ok(node) => fill_drive(app, &path, node),
            Err(e) => {
                stop_drive_pending(app, &path);
                if app.scan_all_first_err.is_none() {
                    app.scan_all_first_err = Some(e);
                }
            }
        }
    }

    // Recompute the root totals from the (small) set of drive children.
    if let Some(root) = app.root_node.as_deref_mut() {
        let (mut size, mut fc, mut folc) = (0i64, 0i64, 0i64);
        for c in &root.children {
            size += c.size;
            fc += c.file_count;
            folc += c.folder_count + 1;
        }
        root.size = size;
        root.file_count = fc;
        root.folder_count = folc;
    }

    // Repaint the drive rows (filled numbers replace the animated bars).
    if let Some(root_ptr) = app.root_node.as_deref().map(|r| r as *const FolderNode) {
        if app.selected_node == root_ptr as isize {
            populate_list_folders(app, &*root_ptr);
        }
    }

    if app.pending_drives.is_empty() {
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

// Replace a drive's placeholder FolderNode with its scan result, in place (the
// slot address — and thus the tree item's pointer — is preserved).
unsafe fn fill_drive(app: &mut AppState, path: &str, node: FolderNode) {
    let root = match app.root_node.as_deref_mut() {
        Some(r) => r,
        None => return,
    };
    if let Some(slot) = root.children.iter_mut().find(|c| c.full_path == path) {
        let ptr = slot as *const FolderNode as isize;
        *slot = node;
        app.pending_drives.remove(&ptr);
    }
}

// A drive failed: stop animating its row (it will show as an empty/0-byte entry
// and the status line notes the skip).
fn stop_drive_pending(app: &mut AppState, path: &str) {
    if let Some(root) = app.root_node.as_deref() {
        if let Some(slot) = root.children.iter().find(|c| c.full_path == path) {
            let ptr = slot as *const FolderNode as isize;
            app.pending_drives.remove(&ptr);
        }
    }
}

// Final housekeeping once every drive thread has reported.
unsafe fn finish_scan_all(app: &mut AppState) {
    app.scan_all_active = false;
    app.scanning = false;
    app.pending_drives.clear();
    app.tree_version = app.tree_version.wrapping_add(1); // fresh tree — drop stale side caches
    let _ = KillTimer(app.main_hwnd, DRIVE_MARQUEE_TIMER);
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
    let elapsed = app
        .scan_start
        .map(|t| format!("  ·  {:.1}s", t.elapsed().as_secs_f64()))
        .unwrap_or_default();
    let mut left = "All drives scanned".to_string();
    if let Some(err) = &app.scan_all_first_err {
        left.push_str(&format!("  [some skipped: {err}]"));
    }
    let summary = format!(
        "{}\tSize: {}  ·  Files: {}  ·  Folders: {}{}",
        left,
        format_bytes(root.size),
        format_count(root.file_count),
        format_count(root.folder_count),
        elapsed,
    );
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

    // Two-part status: a left-aligned message and a right-aligned stats block
    // (split on a tab in status_proc). Matches the branded status strip design.
    let elapsed = app
        .scan_start
        .map(|t| format!("  ·  {:.1}s", t.elapsed().as_secs_f64()))
        .unwrap_or_default();
    let summary = format!(
        "{} scanned\tSize: {}  ·  Files: {}  ·  Folders: {}{}",
        node.name,
        format_bytes(node.size),
        format_count(node.file_count),
        format_count(node.folder_count),
        elapsed,
    );

    app.root_node = Some(Box::new(node));
    app.tree_version = app.tree_version.wrapping_add(1); // new tree — invalidate side caches
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
    // Record this location in the navigation history, unless we got here via a
    // Back/Forward button (which re-selects an item already in the history).
    if !app.nav_lock && app.nav_hist.get(app.nav_pos.max(0) as usize).copied() != Some(hti) {
        let keep = (app.nav_pos + 1).max(0) as usize;
        app.nav_hist.truncate(keep);
        app.nav_hist.push(hti);
        app.nav_pos = app.nav_hist.len() as i32 - 1;
    }
    // Refresh the breadcrumb + top-bar nav button states.
    let _ = InvalidateRect(app.crumb, None, false);
    let _ = InvalidateRect(app.topbar, None, false);
}

// Selects a tree item without recording it as a new history entry.
unsafe fn nav_select(app: &mut AppState, hti: isize) {
    app.nav_lock = true;
    SendMessageW(
        app.tree,
        TVM_SELECTITEM,
        WPARAM(TVGN_CARET as usize),
        LPARAM(hti),
    );
    app.nav_lock = false;
    let _ = InvalidateRect(app.topbar, None, false);
}

unsafe fn nav_back(app: &mut AppState) {
    if app.nav_pos > 0 {
        app.nav_pos -= 1;
        let hti = app.nav_hist[app.nav_pos as usize];
        nav_select(app, hti);
    }
}

unsafe fn nav_forward(app: &mut AppState) {
    if app.nav_pos >= 0 && (app.nav_pos as usize) < app.nav_hist.len().saturating_sub(1) {
        app.nav_pos += 1;
        let hti = app.nav_hist[app.nav_pos as usize];
        nav_select(app, hti);
    }
}

// HTREEITEM of the current selection's parent, or 0 at the root.
unsafe fn nav_parent_hti(app: &AppState) -> isize {
    let caret = SendMessageW(
        app.tree,
        TVM_GETNEXTITEM,
        WPARAM(TVGN_CARET as usize),
        LPARAM(0),
    )
    .0 as isize;
    if caret == 0 {
        return 0;
    }
    SendMessageW(
        app.tree,
        TVM_GETNEXTITEM,
        WPARAM(TVGN_PARENT as usize),
        LPARAM(caret),
    )
    .0 as isize
}

// Jumps straight to the top-level view (the "All drives" root), recording a new
// history entry like a click.
unsafe fn nav_up(app: &mut AppState) {
    let root = SendMessageW(
        app.tree,
        TVM_GETNEXTITEM,
        WPARAM(TVGN_ROOT as usize),
        LPARAM(0),
    )
    .0 as isize;
    let caret = SendMessageW(
        app.tree,
        TVM_GETNEXTITEM,
        WPARAM(TVGN_CARET as usize),
        LPARAM(0),
    )
    .0 as isize;
    if root != 0 && root != caret {
        SendMessageW(
            app.tree,
            TVM_SELECTITEM,
            WPARAM(TVGN_CARET as usize),
            LPARAM(root),
        );
    }
}

// Recursively flattens `node`'s subfolders (then files) into rows, descending
// into any folder whose pointer is in `expanded`. Subfolders first, files last,
// each group sorted case-insensitively; tombstoned folders are skipped.
// Numeric sort key for a folder under the given column (Name is handled
// separately as a string). Columns: 1/2=Size, 3=Own size, 4=Files, 5=Folders,
// 6=Modified.
fn folder_sort_key(n: &FolderNode, col: i32) -> i64 {
    match col {
        3 => n.own_size,
        4 => n.file_count,
        5 => n.folder_count,
        6 => n.last_modified_ft,
        _ => n.size,
    }
}

fn file_sort_key(f: &FileEntry, col: i32) -> i64 {
    match col {
        6 => f.last_modified_ft,
        4 | 5 => 0, // files have no child/folder counts
        _ => f.size,
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn build_list_rows(
    expanded: &HashSet<isize>,
    deleted: &HashSet<isize>,
    deleted_files: &HashSet<isize>,
    node: &FolderNode,
    depth: i32,
    sort_col: i32,
    sort_desc: bool,
    out: &mut Vec<BuiltRow>,
) {
    let total = node.size.max(1) as f32;
    let mut folders: Vec<&FolderNode> = node
        .children
        .iter()
        .filter(|c| !deleted.contains(&(*c as *const _ as isize)))
        .collect();
    folders.sort_by(|a, b| {
        let ord = if sort_col == 0 {
            a.name.to_lowercase().cmp(&b.name.to_lowercase())
        } else {
            folder_sort_key(a, sort_col)
                .cmp(&folder_sort_key(b, sort_col))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        };
        if sort_desc {
            ord.reverse()
        } else {
            ord
        }
    });
    for k in &folders {
        let kp = *k as *const _ as isize;
        let has_children = !k.children.is_empty() || !k.files.is_empty();
        let is_expanded = expanded.contains(&kp);
        out.push(BuiltRow {
            lparam: kp,
            file: 0,
            owner: 0,
            name: k.name.clone(),
            subs: [
                String::new(),
                format_bytes(k.size),
                format_bytes(k.own_size),
                format_count(k.file_count),
                format_count(k.folder_count),
                format_filetime(k.last_modified_ft),
            ],
            row: ListRow {
                depth,
                is_folder: true,
                has_children,
                expanded: is_expanded,
                pct: k.size as f32 / total,
            },
        });
        if is_expanded && has_children {
            build_list_rows(
                expanded,
                deleted,
                deleted_files,
                k,
                depth + 1,
                sort_col,
                sort_desc,
                out,
            );
        }
    }
    let mut files: Vec<&FileEntry> = node.files.iter().collect();
    files.sort_by(|a, b| {
        let ord = if sort_col == 0 {
            a.name.to_lowercase().cmp(&b.name.to_lowercase())
        } else {
            file_sort_key(a, sort_col)
                .cmp(&file_sort_key(b, sort_col))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        };
        if sort_desc {
            ord.reverse()
        } else {
            ord
        }
    });
    let owner_ptr = node as *const FolderNode as isize;
    for f in &files {
        let fp = *f as *const FileEntry as isize;
        if deleted_files.contains(&fp) {
            continue;
        }
        out.push(BuiltRow {
            lparam: 0,
            file: fp,
            owner: owner_ptr,
            name: f.name.clone(),
            subs: [
                String::new(),
                format_bytes(f.size),
                String::new(),
                String::new(),
                String::new(),
                format_filetime(f.last_modified_ft),
            ],
            row: ListRow {
                depth,
                is_folder: false,
                has_children: false,
                expanded: false,
                pct: f.size as f32 / total,
            },
        });
    }
}

// Flips the expand state of the folder at `row` and rebuilds the list from the
// current top-level folder, keeping the toggled row on screen.
unsafe fn toggle_expand(app: &mut AppState, row: usize) {
    let nodep = list_item_lparam(app.list, row as i32);
    if nodep == 0 || app.selected_node == 0 {
        return;
    }
    if !app.expanded.remove(&nodep) {
        app.expanded.insert(nodep);
    }
    // Remember the current scroll position. Expanding/collapsing only changes
    // rows *below* the clicked one, so the rows above (and the top row) keep
    // their position — restoring the same top row leaves the view steady instead
    // of resetting to the top and jumping back down via LVM_ENSUREVISIBLE.
    let top_before = SendMessageW(app.list, LVM_GETTOPINDEX, WPARAM(0), LPARAM(0)).0;
    let cur = &*(app.selected_node as *const FolderNode);
    populate_list_folders(app, cur);
    if top_before > 0 {
        // Row height from item 0's bounding rect; rebuilding leaves the list
        // scrolled to the top, so scroll down by top_before rows to restore it.
        let mut ir = RECT {
            left: LVIR_BOUNDS as i32,
            ..Default::default()
        };
        SendMessageW(
            app.list,
            LVM_GETITEMRECT,
            WPARAM(0),
            LPARAM(&mut ir as *mut RECT as isize),
        );
        let row_h = (ir.bottom - ir.top) as isize;
        if row_h > 0 {
            SendMessageW(app.list, LVM_SCROLL, WPARAM(0), LPARAM(top_before * row_h));
        }
    }
}

unsafe fn populate_list_folders(app: &mut AppState, node: &FolderNode) {
    let mut built: Vec<BuiltRow> = Vec::new();
    build_list_rows(
        &app.expanded,
        &app.deleted_nodes,
        &app.deleted_files,
        node,
        0,
        app.sort_col,
        app.sort_desc,
        &mut built,
    );

    // Suspend redraw while we clear and refill: otherwise the listview repaints
    // on every single insert, which is slow (and flickers) for large folders.
    SendMessageW(app.list, WM_SETREDRAW, WPARAM(0), LPARAM(0));
    SendMessageW(app.list, LVM_DELETEALLITEMS, WPARAM(0), LPARAM(0));

    // The "% of parent" cell (subitem 1) is custom-drawn from list_rows; its
    // stored text stays empty. lParam = the FolderNode pointer for folders (0 for
    // files), so double-click drills into it and the context menu can act on it.
    app.list_rows.clear();
    for (i, b) in built.into_iter().enumerate() {
        insert_row_with_param(app.list, i as i32, &b.name, &b.subs, b.lparam);
        app.list_rows.push(b);
    }
    SendMessageW(app.list, WM_SETREDRAW, WPARAM(1), LPARAM(0));
    let _ = RedrawWindow(
        app.list,
        None,
        None,
        RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN,
    );
}

unsafe fn populate_side_top_files(app: &mut AppState) {
    populate_side_view(app, SideView::TopFiles, |root| {
        top_n_files(root, TOP_N_FILES)
    });
}

unsafe fn populate_side_oldest_files(app: &mut AppState) {
    populate_side_view(app, SideView::OldestFiles, |root| {
        oldest_n_files(root, TOP_N_FILES)
    });
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
    app.scan_start = Some(std::time::Instant::now());
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

// Delete-button action: recycle the current main-list selection (folders and/or
// files) after showing exactly how many folders, files and bytes it will free.
unsafe fn delete_selected(hwnd: HWND, app: &mut AppState) {
    let indices = selected_indices(app.list);
    if indices.is_empty() {
        return;
    }
    let mut paths: Vec<String> = Vec::new();
    let mut folder_ptrs: Vec<isize> = Vec::new();
    let mut file_targets: Vec<(isize, isize, i64)> = Vec::new(); // (file, owner, size)
    let (mut n_folders, mut n_files, mut bytes) = (0i64, 0i64, 0i64);

    for &i in &indices {
        let br = match app.list_rows.get(i as usize) {
            Some(b) => b,
            None => continue,
        };
        if br.lparam != 0 {
            let node = &*(br.lparam as *const FolderNode);
            // Never recycle a whole drive root ("C:\") or the synthetic root.
            if node.full_path.len() <= 3 {
                continue;
            }
            paths.push(node.full_path.clone());
            folder_ptrs.push(br.lparam);
            n_folders += node.folder_count + 1;
            n_files += node.file_count;
            bytes += node.size;
        } else if br.file != 0 && br.owner != 0 {
            let f = &*(br.file as *const FileEntry);
            let owner = &*(br.owner as *const FolderNode);
            paths.push(join_path(&owner.full_path, &f.name));
            file_targets.push((br.file, br.owner, f.size));
            n_files += 1;
            bytes += f.size;
        }
    }
    if paths.is_empty() {
        return;
    }

    let prompt = format!(
        "Move the selected item{} to the Recycle Bin?\n\n\
         \u{2022} {} folder{}\n\
         \u{2022} {} file{}\n\
         \u{2022} {} freed",
        if paths.len() == 1 { "" } else { "s" },
        format_count(n_folders),
        if n_folders == 1 { "" } else { "s" },
        format_count(n_files),
        if n_files == 1 { "" } else { "s" },
        format_bytes(bytes),
    );
    if !confirm_delete(hwnd, &prompt) {
        return;
    }

    recycle_in_background(hwnd, app, paths);
    for fp in folder_ptrs {
        delete_folder_node(app, fp as *const FolderNode);
    }
    for (fp, owner, size) in file_targets {
        app.deleted_files.insert(fp);
        adjust_ancestors(app, owner as *const FolderNode, size, 1, 0, true);
    }
    app.tree_version = app.tree_version.wrapping_add(1);
    if app.selected_node != 0 {
        populate_list_folders(app, &*(app.selected_node as *const FolderNode));
    }
    match app.side_view {
        SideView::TopFiles => populate_side_top_files(app),
        SideView::OldestFiles => populate_side_oldest_files(app),
        _ => {}
    }
}

// Yes/No confirmation for a recycle. Returns true if the user chose Yes.
unsafe fn confirm_delete(hwnd: HWND, msg: &str) -> bool {
    let title = wide("Delete");
    let body = wide(msg);
    MessageBoxW(
        hwnd,
        PCWSTR(body.as_ptr()),
        PCWSTR(title.as_ptr()),
        MB_YESNO | MB_ICONWARNING,
    ) == IDYES
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
// Repaints the views after an in-place deletion, using the tree's current
// selection. No disk access.
unsafe fn refresh_after_delete(app: &mut AppState) {
    // The tree's sizes/tombstones changed, so any cached side-view result is
    // stale — force a recompute on the next populate.
    app.tree_version = app.tree_version.wrapping_add(1);
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

// Populate a side view (Top/Oldest), reusing a cached result when the tree
// hasn't changed since it was last computed — so toggling views doesn't re-walk
// the (potentially multi-million-file) tree each time.
unsafe fn populate_side_view<F>(app: &mut AppState, view: SideView, query: F)
where
    F: for<'a> FnOnce(&'a FolderNode) -> Vec<crate::analysis::FileHit<'a>>,
{
    let rows = side_rows_for(app, view, query);
    fill_side_list(app, &rows);
}

// Returns the rendered rows for a view, from cache if still valid, else by
// walking the tree once and caching the result under the current tree_version.
unsafe fn side_rows_for<F>(app: &mut AppState, view: SideView, query: F) -> Vec<SideRow>
where
    F: for<'a> FnOnce(&'a FolderNode) -> Vec<crate::analysis::FileHit<'a>>,
{
    if let Some((ver, rows)) = app.side_cache.get(&view) {
        if *ver == app.tree_version {
            return rows.clone();
        }
    }
    let root_ptr = match app.root_node.as_deref() {
        Some(r) => r as *const FolderNode,
        None => return Vec::new(),
    };
    let root: &FolderNode = &*root_ptr;
    let hits = query(root);
    let mut rows = Vec::with_capacity(hits.len());
    for h in hits.iter() {
        // Skip files under a folder that's been recycled in place, or the file
        // itself if it was individually recycled.
        if app.deleted_nodes.contains(&(h.folder as *const _ as isize))
            || app.deleted_files.contains(&(h.file as *const _ as isize))
        {
            continue;
        }
        rows.push(SideRow {
            name: h.file.name.clone(),
            size: format_bytes(h.file.size),
            time: format_filetime(h.file.last_modified_ft),
            path: join_path(&h.folder.full_path, &h.file.name),
            folder: h.folder as *const _,
            file: h.file as *const _,
        });
    }
    app.side_cache
        .insert(view, (app.tree_version, rows.clone()));
    rows
}

// Fills the side list from already-rendered rows (no tree access).
unsafe fn fill_side_list(app: &mut AppState, rows: &[SideRow]) {
    SendMessageW(app.side_list, LVM_DELETEALLITEMS, WPARAM(0), LPARAM(0));
    app.side_hits.clear();
    for (i, r) in rows.iter().enumerate() {
        // lParam is the index into side_hits so context actions can recover the
        // (folder, file) pair.
        app.side_hits.push((r.folder, r.file));
        insert_row_with_param(
            app.side_list,
            i as i32,
            &r.name,
            &[r.size.clone(), r.time.clone(), r.path.clone()],
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
    }
    if view != SideView::None {
        fit_side_columns(app.side_list);
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
    // Single-row header: Detach/Recycle-all are vertically centred at the right.
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
    fit_side_columns(app.side_list);
    // Buttons moved — repaint the header so the title re-clamps.
    let _ = InvalidateRect(panel, None, false);
}

// The side-panel rows are owner-drawn cards, so the columns are only data
// storage. Collapse them into column 0 spanning the visible width — that keeps
// the full row clickable while removing the horizontal scrollbar.
unsafe fn fit_side_columns(list: HWND) {
    let mut cl = RECT::default();
    let _ = GetClientRect(list, &mut cl);
    let w = (cl.right - cl.left - 2).max(0);
    SendMessageW(list, LVM_SETCOLUMNWIDTH, WPARAM(0), LPARAM(w as isize));
    for c in 1..6 {
        SendMessageW(list, LVM_SETCOLUMNWIDTH, WPARAM(c), LPARAM(0));
    }
}

// The three view-switch icon buttons at the left of the panel header row (Top
// largest / Oldest / Safe-to-delete temp).
fn panel_view_buttons() -> [RECT; 3] {
    let bw = 38;
    let bh = 28;
    let gap = 5;
    let x0 = 10;
    let ty = (PANEL_HEADER_H - bh) / 2;
    let mk = |i: i32| {
        let l = x0 + i * (bw + gap);
        RECT {
            left: l,
            top: ty,
            right: l + bw,
            bottom: ty + bh,
        }
    };
    [mk(0), mk(1), mk(2)]
}

// The SideView each toolbar button selects, and its Segoe MDL2 glyph.
const PANEL_VIEW_BUTTONS: [(SideView, &str); 3] = [
    (SideView::TopFiles, "\u{E8A5}"),    // Document
    (SideView::OldestFiles, "\u{E81C}"), // History
    (SideView::TempFiles, "\u{E74D}"),   // Delete
];

unsafe fn paint_panel_header(app: &AppState, panel: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(panel, &mut ps);
    let mut rc = RECT::default();
    let _ = GetClientRect(panel, &mut rc);
    let header = RECT {
        bottom: PANEL_HEADER_H,
        ..rc
    };
    // Light panel header: panel-bg fill, a view-switch toolbar row, the view
    // title below it, and a bottom hairline.
    let p = palette(app.is_dark);
    let brush = CreateSolidBrush(COLORREF(p.panel_bg));
    FillRect(hdc, &header, brush);
    let _ = DeleteObject(brush);
    let accent = RECT {
        top: PANEL_HEADER_H - 1,
        bottom: PANEL_HEADER_H,
        ..header
    };
    let accent_brush = CreateSolidBrush(COLORREF(p.hairline));
    FillRect(hdc, &accent, accent_brush);
    let _ = DeleteObject(accent_brush);

    SetBkMode(hdc, TRANSPARENT);

    // Toolbar buttons; the active view is outlined and coloured in blue.
    let btns = panel_view_buttons();
    let old = SelectObject(hdc, HGDIOBJ(app.font_icon.0));
    for (i, br) in btns.iter().enumerate() {
        let (view, glyph) = PANEL_VIEW_BUTTONS[i];
        let active = app.side_view == view;
        let (border, bw, icon) = if active {
            (p.blue, 2, p.blue)
        } else {
            (p.hairline, 1, p.subtext)
        };
        card_round(hdc, br, 6, p.card_bg, border, bw);
        SetTextColor(hdc, COLORREF(icon));
        let mut g: Vec<u16> = glyph.encode_utf16().collect();
        let mut grc = *br;
        DrawTextW(
            hdc,
            &mut g,
            &mut grc,
            DT_SINGLELINE | DT_VCENTER | DT_CENTER,
        );
    }

    // View title, between the view buttons and the Detach/Recycle buttons.
    SelectObject(hdc, HGDIOBJ(app.font_small.0));
    SetTextColor(hdc, COLORREF(p.text));
    let title_left = btns[2].right + 14;
    let title_right = (header_buttons_left_x(app, rc.right) - PANEL_BTN_GAP).max(title_left);
    let mut text_rc = RECT {
        left: title_left,
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
    SelectObject(hdc, old);
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

// The splitter + detached-panel-frame WNDPROCs live in `gui::chrome`.

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
    crate::format::format_ymdhm(
        local.wYear,
        local.wMonth,
        local.wDay,
        local.wHour,
        local.wMinute,
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
    // The side list is a card list on the panel background.
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
                                // non-client area and don't pick up theme changes from a client-area
                                // invalidate alone. The caption needs the extra nudge below.
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

// DWM caches the title bar, so flipping DWMWA_USE_IMMERSIVE_DARK_MODE with only
// an SWP_FRAMECHANGED doesn't reliably recomposite the caption (it intermittently
// keeps the old theme). A 1px size wobble forces DWM to repaint it. Skipped when
// maximized, where a resize would un-maximize the window.
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

// One wrapper for every ShellExecuteW call in the app. `params`/`dir` are
// optional; the wide buffers are kept alive until the call returns.
unsafe fn shell_exec(verb: &str, file: &str, params: Option<&str>, dir: Option<&str>) {
    let verb_w = wide(verb);
    let file_w = wide(file);
    let params_w = params.map(wide);
    let dir_w = dir.map(wide);
    let as_pcwstr =
        |v: &Option<Vec<u16>>| v.as_ref().map_or(PCWSTR::null(), |b| PCWSTR(b.as_ptr()));
    let _ = ShellExecuteW(
        HWND::default(),
        PCWSTR(verb_w.as_ptr()),
        PCWSTR(file_w.as_ptr()),
        as_pcwstr(&params_w),
        as_pcwstr(&dir_w),
        SW_NORMAL,
    );
}

fn relaunch_elevated() {
    if let Ok(exe) = std::env::current_exe() {
        unsafe { shell_exec("runas", &exe.to_string_lossy(), None, None) };
        std::process::exit(0);
    }
}

// ---- Shell actions ----

fn open_in_explorer(path: &str) {
    unsafe { shell_exec("open", path, None, None) };
}

// Opens a command prompt with `path` as its working directory (passed as
// lpDirectory, never as a command argument — so a crafted folder name can't
// inject a command).
fn open_cmd_at(path: &str) {
    unsafe { shell_exec("open", "cmd.exe", None, Some(path)) };
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

// Shows a little up/down triangle on the currently-sorted column header (and
// clears it from the others), via the standard header control's HDF_SORT* bits.
unsafe fn update_sort_arrows(app: &AppState) {
    let header = SendMessageW(app.list, LVM_GETHEADER, WPARAM(0), LPARAM(0)).0;
    if header == 0 {
        return;
    }
    let hwnd = HWND(header as _);
    let count = SendMessageW(hwnd, HDM_GETITEMCOUNT, WPARAM(0), LPARAM(0)).0 as i32;
    for i in 0..count {
        let mut item = HDITEMW {
            mask: HDI_FORMAT,
            ..Default::default()
        };
        SendMessageW(
            hwnd,
            HDM_GETITEMW,
            WPARAM(i as usize),
            LPARAM(&mut item as *mut _ as isize),
        );
        item.fmt.0 &= !(HDF_SORTUP.0 | HDF_SORTDOWN.0);
        if i == app.sort_col {
            item.fmt.0 |= if app.sort_desc {
                HDF_SORTDOWN.0
            } else {
                HDF_SORTUP.0
            };
        }
        item.mask = HDI_FORMAT;
        SendMessageW(
            hwnd,
            HDM_SETITEMW,
            WPARAM(i as usize),
            LPARAM(&mut item as *mut _ as isize),
        );
    }
}

unsafe fn set_status(status: HWND, text: &str) {
    let w: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let _ = SetWindowTextW(status, PCWSTR(w.as_ptr()));
    let _ = InvalidateRect(status, None, true);
}

// ---- Formatting ----
// The pure formatting helpers (`join_path`, `format_bytes`, `format_count`,
// `format_ymdhm`) live in `crate::format` so they can be unit-tested without a
// window. Imported at the top of this module.

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
