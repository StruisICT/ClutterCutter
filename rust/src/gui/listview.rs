// Thin wrappers over the SysListView32 LVM_* messages. All take an HWND (never
// AppState) so they're a clean, self-contained layer over the control.

use windows::core::PWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Controls::{
    LVCFMT_LEFT, LVCFMT_RIGHT, LVCF_FMT, LVCF_TEXT, LVCF_WIDTH, LVCOLUMNW, LVIF_PARAM, LVIF_TEXT,
    LVITEMW, LVM_DELETEITEM, LVM_GETITEMSTATE, LVM_GETITEMTEXTW, LVM_GETITEMW, LVM_GETNEXTITEM,
    LVM_INSERTCOLUMNW, LVM_INSERTITEMW, LVM_SETITEMTEXTW, LVNI_SELECTED,
};
use windows::Win32::UI::WindowsAndMessaging::SendMessageW;

/// Is the list row currently selected? (LVIS_SELECTED == 2.)
pub(crate) unsafe fn row_selected(list: HWND, row: usize) -> bool {
    SendMessageW(list, LVM_GETITEMSTATE, WPARAM(row), LPARAM(2)).0 & 2 != 0
}

/// Reads a side-list sub-item's text into a UTF-16 vec.
pub(crate) unsafe fn side_subitem_text(list: HWND, row: usize, sub: i32) -> Vec<u16> {
    let mut buf = [0u16; 320];
    let mut it = LVITEMW {
        iSubItem: sub,
        pszText: PWSTR(buf.as_mut_ptr()),
        cchTextMax: buf.len() as i32,
        ..Default::default()
    };
    let len = SendMessageW(
        list,
        LVM_GETITEMTEXTW,
        WPARAM(row),
        LPARAM(&mut it as *mut _ as isize),
    )
    .0 as usize;
    buf[..len.min(buf.len())].to_vec()
}

/// Deletes the given (ascending) row indices, bottom-up so the remaining indices
/// stay valid.
pub(crate) unsafe fn remove_side_rows(list: HWND, indices: &[i32]) {
    for &i in indices.iter().rev() {
        SendMessageW(list, LVM_DELETEITEM, WPARAM(i as usize), LPARAM(0));
    }
}

/// All selected row indices, in ascending order.
pub(crate) unsafe fn selected_indices(list: HWND) -> Vec<i32> {
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

/// The first selected row index, or -1 if none.
pub(crate) unsafe fn selected_list_index(list: HWND) -> i32 {
    let r = SendMessageW(
        list,
        LVM_GETNEXTITEM,
        WPARAM((-1isize) as usize),
        LPARAM(LVNI_SELECTED as isize),
    );
    r.0 as i32
}

/// The lParam stored on a list item (a node pointer for our lists).
pub(crate) unsafe fn list_item_lparam(list: HWND, idx: i32) -> isize {
    let mut item = LVITEMW {
        mask: LVIF_PARAM,
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

/// Inserts a column with a title, width, and left/right alignment.
pub(crate) unsafe fn insert_column(
    list: HWND,
    idx: i32,
    title: &str,
    width: i32,
    right_align: bool,
) {
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

/// Inserts a row (item + sub-items) with an lParam.
pub(crate) unsafe fn insert_row_with_param(
    list: HWND,
    idx: i32,
    name: &str,
    subitems: &[String],
    lparam: isize,
) {
    let mut name_w: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let item = LVITEMW {
        mask: LVIF_TEXT | LVIF_PARAM,
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
