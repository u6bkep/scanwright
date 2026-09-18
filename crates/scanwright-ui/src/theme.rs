//! Theme: the colours and baked fonts the stock vocabulary draws with.

use scanwright_core::{font, font::Font, hex};

#[derive(Clone, Copy)]
pub struct Theme {
    pub background: u16,
    pub surface: u16,
    pub bar: u16,
    pub track: u16,
    pub text: u16,
    pub muted: u16,
    pub accent: u16,
    pub on_accent: u16,
    pub success: u16,

    /// The type scale: four roles. `body`/`small` have a bold companion for
    /// emphasis within a role; `title` and `display` are bold only.
    pub body: &'static Font,
    pub body_bold: &'static Font,
    pub small: &'static Font,
    pub small_bold: &'static Font,
    pub title: &'static Font,
    pub display: &'static Font,

    pub card_radius: u8,
    pub card_padding: i16,
    pub button_radius: u8,
}

impl Theme {
    /// The oven controller's dark theme, on the fonts baked into core.
    pub const DARK: Theme = Theme {
        background: hex(0x151a1f),
        surface: hex(0x242c34),
        bar: hex(0x10151a),
        track: hex(0x3c4a54),
        text: hex(0xe8edf1),
        muted: hex(0x93a3ae),
        accent: hex(0xf4650f),
        on_accent: hex(0x0f1317),
        success: hex(0x6fcf7f),
        body: &font::BODY,
        body_bold: &font::BODY_BOLD,
        small: &font::CAPTION,
        small_bold: &font::CAPTION_BOLD,
        title: &font::TITLE,
        display: &font::DISPLAY,
        card_radius: 18,
        card_padding: 20,
        button_radius: 14,
    };
}

/// A pressed control's fill: the colour pulled 25 % towards black.
pub(crate) const fn pressed(c: u16) -> u16 {
    let (r, g, b) = (c >> 11, (c >> 5) & 0x3f, c & 0x1f);
    ((r * 3 / 4) << 11) | ((g * 3 / 4) << 5) | (b * 3 / 4)
}
