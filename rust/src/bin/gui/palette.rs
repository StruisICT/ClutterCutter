// Colour palette for the egui frontend, ported from the Win32 GUI's
// `palette()` (gui/palette.rs). Light mode is the warm-white theme; the two
// accents are brand blue (usage/size) and green (%-of-parent bars).

use eframe::egui::{self, Color32};

pub struct Pal {
    pub win_bg: Color32,
    pub panel_bg: Color32,
    pub card_bg: Color32,
    pub card_sel: Color32,
    pub text: Color32,
    pub subtext: Color32,
    pub hairline: Color32,
    pub track: Color32,
    pub blue: Color32,
    pub green: Color32,
}

pub fn palette(dark: bool) -> Pal {
    if dark {
        Pal {
            win_bg: Color32::from_rgb(0x1C, 0x1E, 0x22),
            panel_bg: Color32::from_rgb(0x22, 0x25, 0x2A),
            card_bg: Color32::from_rgb(0x2A, 0x2E, 0x34),
            card_sel: Color32::from_rgb(0x33, 0x39, 0x42),
            text: Color32::from_rgb(0xE6, 0xE9, 0xEC),
            subtext: Color32::from_rgb(0x9A, 0xA2, 0xAC),
            hairline: Color32::from_rgb(0x3A, 0x40, 0x48),
            track: Color32::from_rgb(0x34, 0x3A, 0x42),
            blue: Color32::from_rgb(0x4C, 0x8B, 0xFF),
            green: Color32::from_rgb(0x7E, 0xC8, 0x5F),
        }
    } else {
        // Warm-white light theme (matches the Win32 build).
        Pal {
            win_bg: Color32::from_rgb(0xF0, 0xEB, 0xE3),
            panel_bg: Color32::from_rgb(0xF6, 0xF1, 0xE8),
            card_bg: Color32::from_rgb(0xFB, 0xF8, 0xF2),
            card_sel: Color32::from_rgb(0xEE, 0xF3, 0xFC),
            text: Color32::from_rgb(0x26, 0x23, 0x20),
            subtext: Color32::from_rgb(0x8C, 0x83, 0x78),
            hairline: Color32::from_rgb(0xE8, 0xE1, 0xD6),
            track: Color32::from_rgb(0xED, 0xE6, 0xDB),
            blue: Color32::from_rgb(0x2D, 0x6B, 0xF0),
            green: Color32::from_rgb(0x70, 0xBB, 0x51),
        }
    }
}

/// Push the palette into egui's global visuals so standard widgets match.
pub fn apply(ctx: &egui::Context, dark: bool) {
    let p = palette(dark);
    let mut v = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    v.override_text_color = Some(p.text);
    v.panel_fill = p.win_bg;
    v.window_fill = p.panel_bg;
    v.extreme_bg_color = p.track;
    v.hyperlink_color = p.blue;
    v.selection.bg_fill = p.blue.linear_multiply(0.35);
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, p.hairline);
    ctx.set_visuals(v);
}
