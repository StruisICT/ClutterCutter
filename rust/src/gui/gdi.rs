// Low-level GDI drawing helpers shared by every custom-drawn window. All are
// pure (no AppState) — they take an HDC + geometry + COLORREF colors.

use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, POINT, RECT};
use windows::Win32::Graphics::Gdi::*;

/// Fill a rectangle with a solid color — the create/fill/delete triad in one call.
pub(crate) unsafe fn fill_rect(hdc: HDC, rc: &RECT, color: u32) {
    let br = CreateSolidBrush(COLORREF(color));
    FillRect(hdc, rc, br);
    let _ = DeleteObject(br);
}

/// Draw a single string into `rc` with the given DrawTextW flags. Guards the
/// empty-string case: DrawTextW on an empty slice dereferences the Vec's
/// dangling pointer and faults, so every call routes through here.
pub(crate) unsafe fn draw_text(hdc: HDC, s: &str, rc: &RECT, flags: DRAW_TEXT_FORMAT) {
    if s.is_empty() {
        return;
    }
    let mut v: Vec<u16> = s.encode_utf16().collect();
    let mut r = *rc;
    DrawTextW(hdc, &mut v, &mut r, flags);
}

/// Filled rounded rectangle in `color` (no visible border).
pub(crate) unsafe fn fill_round(hdc: HDC, rc: &RECT, radius: i32, color: u32) {
    let br = CreateSolidBrush(COLORREF(color));
    let pen = CreatePen(PS_SOLID, 1, COLORREF(color));
    let ob = SelectObject(hdc, HGDIOBJ(br.0));
    let op = SelectObject(hdc, HGDIOBJ(pen.0));
    let _ = RoundRect(hdc, rc.left, rc.top, rc.right, rc.bottom, radius, radius);
    SelectObject(hdc, ob);
    SelectObject(hdc, op);
    let _ = DeleteObject(br);
    let _ = DeleteObject(pen);
}

/// Rounded rectangle filled with `fill` and stroked with a `border_w`px `border`.
pub(crate) unsafe fn card_round(
    hdc: HDC,
    rc: &RECT,
    radius: i32,
    fill: u32,
    border: u32,
    border_w: i32,
) {
    let br = CreateSolidBrush(COLORREF(fill));
    let pen = CreatePen(PS_SOLID, border_w, COLORREF(border));
    let ob = SelectObject(hdc, HGDIOBJ(br.0));
    let op = SelectObject(hdc, HGDIOBJ(pen.0));
    let _ = RoundRect(hdc, rc.left, rc.top, rc.right, rc.bottom, radius, radius);
    SelectObject(hdc, ob);
    SelectObject(hdc, op);
    let _ = DeleteObject(br);
    let _ = DeleteObject(pen);
}

/// A small bordered box with a "−" (expanded) or "+" (collapsed) glyph — the
/// tree expand toggle drawn in the Name column.
pub(crate) unsafe fn draw_expand_box(
    hdc: HDC,
    x: i32,
    cy: i32,
    expanded: bool,
    color: u32,
    bg: u32,
) {
    let s = 11;
    let t = cy - s / 2;
    let box_rc = RECT {
        left: x,
        top: t,
        right: x + s,
        bottom: t + s,
    };
    card_round(hdc, &box_rc, 3, bg, color, 1);
    let br = CreateSolidBrush(COLORREF(color));
    let midy = t + s / 2;
    let hbar = RECT {
        left: x + 3,
        top: midy,
        right: x + s - 3,
        bottom: midy + 1,
    };
    FillRect(hdc, &hbar, br);
    if !expanded {
        let midx = x + s / 2;
        let vbar = RECT {
            left: midx,
            top: t + 3,
            right: midx + 1,
            bottom: t + s - 3,
        };
        FillRect(hdc, &vbar, br);
    }
    let _ = DeleteObject(br);
}

/// A flat filled folder glyph (tab + body) centred at (cx, cy). Reads clearly as
/// a folder at row size, unlike the ambiguous MDL2 outline glyph.
pub(crate) unsafe fn draw_folder_glyph(hdc: HDC, cx: i32, cy: i32, color: u32) {
    let w = 16;
    let h = 12;
    let l = cx - w / 2;
    let t = cy - h / 2;
    let br = CreateSolidBrush(COLORREF(color));
    let pen = CreatePen(PS_SOLID, 1, COLORREF(color));
    let ob = SelectObject(hdc, HGDIOBJ(br.0));
    let op = SelectObject(hdc, HGDIOBJ(pen.0));
    let _ = RoundRect(hdc, l, t, l + 9, t + 5, 2, 2); // tab
    let _ = RoundRect(hdc, l, t + 3, l + w, t + h, 3, 3); // body
    SelectObject(hdc, ob);
    SelectObject(hdc, op);
    let _ = DeleteObject(br);
    let _ = DeleteObject(pen);
}

/// A flat page glyph with a folded top-right corner, outlined in `color` and
/// filled with `fill` (the cell background), centred at (cx, cy).
pub(crate) unsafe fn draw_file_glyph(hdc: HDC, cx: i32, cy: i32, color: u32, fill: u32) {
    let w = 12;
    let h = 15;
    let fold = 5;
    let l = cx - w / 2;
    let t = cy - h / 2;
    let r = l + w;
    let b = t + h;
    let pen = CreatePen(PS_SOLID, 1, COLORREF(color));
    let brush = CreateSolidBrush(COLORREF(fill));
    let op = SelectObject(hdc, HGDIOBJ(pen.0));
    let ob = SelectObject(hdc, HGDIOBJ(brush.0));
    let body = [
        POINT { x: l, y: t },
        POINT { x: r - fold, y: t },
        POINT { x: r, y: t + fold },
        POINT { x: r, y: b },
        POINT { x: l, y: b },
    ];
    let _ = Polygon(hdc, &body);
    // The folded corner.
    let fold_pts = [
        POINT { x: r - fold, y: t },
        POINT {
            x: r - fold,
            y: t + fold,
        },
        POINT { x: r, y: t + fold },
    ];
    let _ = Polyline(hdc, &fold_pts);
    SelectObject(hdc, op);
    SelectObject(hdc, ob);
    let _ = DeleteObject(pen);
    let _ = DeleteObject(brush);
}

/// A Segoe UI font at the given height/weight (ClearType). Segoe UI stands in
/// for the brand's Raleway, which isn't installed on Windows.
pub(crate) unsafe fn make_font(height: i32, weight: i32) -> HFONT {
    make_font_face(height, weight, "Segoe UI")
}

/// A font of a specific face (e.g. "Segoe MDL2 Assets" for glyph icons).
pub(crate) unsafe fn make_font_face(height: i32, weight: i32, face: &str) -> HFONT {
    let face: Vec<u16> = format!("{face}\0").encode_utf16().collect();
    CreateFontW(
        height,
        0,
        0,
        0,
        weight,
        0,
        0,
        0,
        0,
        0,
        0,
        5,
        0,
        PCWSTR(face.as_ptr()),
    )
}
