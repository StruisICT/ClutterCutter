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

/// The Home button rectangle on the left of the top bar.
pub(crate) fn home_button_rect(client: &RECT) -> RECT {
    let (bw, bh, x0) = (34, 32, 14);
    let ty = (client.bottom - bh) / 2;
    RECT {
        left: x0,
        top: ty,
        right: x0 + bw,
        bottom: ty + bh,
    }
}

/// The Delete-selected button, sitting just right of the Home button with a
/// small gap.
pub(crate) fn delete_button_rect(client: &RECT) -> RECT {
    let (bw, bh, gap, x0) = (34, 32, 6, 14);
    let l = x0 + bw + gap;
    let ty = (client.bottom - bh) / 2;
    RECT {
        left: l,
        top: ty,
        right: l + bw,
        bottom: ty + bh,
    }
}
