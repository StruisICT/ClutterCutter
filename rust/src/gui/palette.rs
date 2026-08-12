// Theme palette: the light/dark color model shared by every custom-drawn part
// of the GUI. Colors are stored as Win32 COLORREF values (0x00BBGGRR).

/// Which theme the user has chosen. `Auto` follows the system setting.
#[derive(Copy, Clone, Default, PartialEq)]
pub(crate) enum ThemeMode {
    #[default]
    Auto,
    Light,
    Dark,
}

/// The resolved set of colors for one theme, returned by [`palette`].
#[derive(Clone, Copy)]
pub(crate) struct Pal {
    pub win_bg: u32,   // top bar / sidebar / status strip background
    pub card_bg: u32,  // cards, table, panel header
    pub panel_bg: u32, // side-panel body
    pub text: u32,     // primary text
    pub subtext: u32,  // secondary / muted text
    pub hairline: u32, // borders and separators
    pub track: u32,    // unfilled bar track
    pub blue: u32,     // #2D6BF0 accent (drive bars, sizes, active card border)
    pub green: u32,    // #70BB51 accent (table %-of-parent bars)
}

/// The light or dark palette. Colors are COLORREF (0x00BBGGRR); the trailing
/// comments give the intuitive #RRGGBB.
pub(crate) fn palette(is_dark: bool) -> Pal {
    if is_dark {
        Pal {
            win_bg: 0x0026_2626,
            card_bg: 0x002E_2E2E,
            panel_bg: 0x0022_2222,
            text: 0x00EC_ECEC,
            subtext: 0x00A0_A0A0,
            hairline: 0x003C_3C3C,
            track: 0x0040_4040,
            blue: 0x00F5_824C,  // #4C82F5
            green: 0x005C_C87C, // #7CC85C
        }
    } else {
        // Warm-white light theme: off-whites with a slight red/yellow cast
        // instead of pure/cool white, which several users found harshly bright.
        Pal {
            win_bg: 0x00E3_EBF0,   // #F0EBE3 warm light gray (chrome)
            card_bg: 0x00F2_F8FB,  // #FBF8F2 warm white (cards / table)
            panel_bg: 0x00E8_F1F6, // #F6F1E8 warm off-white (panel body)
            text: 0x0020_2326,     // #262320 warm near-black
            subtext: 0x0078_838C,  // #8C8378 warm gray
            hairline: 0x00D6_E1E8, // #E8E1D6 warm hairline
            track: 0x00DB_E6ED,    // #EDE6DB warm bar track
            blue: 0x00F0_6B2D,     // #2D6BF0
            green: 0x0051_BB70,    // #70BB51
        }
    }
}
