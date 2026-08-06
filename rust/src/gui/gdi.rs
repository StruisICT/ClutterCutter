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

/// Draw the theme-toggle pill — a rounded track with a rounded sliding knob —
/// with smooth, anti-aliased edges. GDI's RoundRect has no anti-aliasing, so the
/// corners stair-step; instead we render the shapes into an offscreen bitmap at
/// 4x and shrink them back with HALFTONE, which gives clean, polished edges.
/// Glyphs are drawn by the caller at 1x afterwards (text stays crisp that way).
#[allow(clippy::too_many_arguments)]
pub(crate) unsafe fn draw_pill_aa(
    hdc: HDC,
    pill: &RECT,
    pill_r: i32,
    track_fill: u32,
    border: u32,
    border_w: i32,
    knob: &RECT,
    knob_r: i32,
    knob_col: u32,
    bar_bg: u32,
) {
    const S: i32 = 4; // supersample factor
    let pw = pill.right - pill.left;
    let ph = pill.bottom - pill.top;
    if pw <= 0 || ph <= 0 {
        return;
    }
    let mem = CreateCompatibleDC(hdc);
    let bmp = CreateCompatibleBitmap(hdc, pw * S, ph * S);
    let obmp = SelectObject(mem, HGDIOBJ(bmp.0));
    // Seed with the bar background so the pill's outer corners blend into the bar.
    let full = RECT {
        left: 0,
        top: 0,
        right: pw * S,
        bottom: ph * S,
    };
    fill_rect(mem, &full, bar_bg);
    // Track, then the knob on top (its corners blend into the track fill).
    card_round(
        mem,
        &full,
        pill_r * S,
        track_fill,
        border,
        (border_w * S).max(1),
    );
    let k = RECT {
        left: (knob.left - pill.left) * S,
        top: (knob.top - pill.top) * S,
        right: (knob.right - pill.left) * S,
        bottom: (knob.bottom - pill.top) * S,
    };
    fill_round(mem, &k, knob_r * S, knob_col);
    // Shrink back over the pill with smoothing.
    SetStretchBltMode(hdc, HALFTONE);
    let _ = SetBrushOrgEx(hdc, 0, 0, None);
    let _ = StretchBlt(
        hdc,
        pill.left,
        pill.top,
        pw,
        ph,
        mem,
        0,
        0,
        pw * S,
        ph * S,
        SRCCOPY,
    );
    SelectObject(mem, obmp);
    let _ = DeleteObject(bmp);
    let _ = DeleteDC(mem);
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
