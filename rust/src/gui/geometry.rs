// Pure top-bar hit/paint geometry — button and pill rectangles computed from the
// bar's client rect. No Win32 calls, no AppState.

use windows::Win32::Foundation::RECT;

/// The theme-toggle pill rectangle, right-aligned within the top bar's client rect.
pub(crate) fn pill_rect(client: &RECT) -> RECT {
    let w = 92;
    let h = 28;
    let right = client.right - 16;
    let top = (client.bottom - h) / 2;
    RECT {
        left: right - w,
        top,
        right,
        bottom: top + h,
    }
}

/// The Home / Back / Forward navigation button rectangles on the left of the top bar.
pub(crate) fn nav_button_rects(client: &RECT) -> [RECT; 3] {
    let bw = 34;
    let bh = 32;
    let gap = 6;
    let x0 = 14;
    let ty = (client.bottom - bh) / 2;
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

/// The Delete-selected button, sitting just right of the three nav buttons with
/// a little extra separation.
pub(crate) fn delete_button_rect(client: &RECT) -> RECT {
    let (bw, bh, gap, x0) = (34, 32, 6, 14);
    let l = x0 + 3 * (bw + gap) + 16;
    let ty = (client.bottom - bh) / 2;
    RECT {
        left: l,
        top: ty,
        right: l + bw,
        bottom: ty + bh,
    }
}
