//! Build-time baked fonts (see `build.rs`): 4-bit coverage masks, pre-rotated
//! into panel space.

/// One baked glyph. `w`/`h` are the bitmap's **logical** (unrotated) size; the
/// stored mask is `h` wide and `w` tall.
#[derive(Clone, Copy, Debug)]
pub struct Glyph {
    pub ch: char,
    /// Advance in 1/64 logical px.
    pub advance_64: u16,
    /// Logical px from the pen position to the bitmap's left column.
    pub left: i32,
    /// Logical px from the baseline up to the bitmap's top row.
    pub top: i32,
    pub w: usize,
    pub h: usize,
    /// Byte offset of the mask in [`ATLAS`].
    pub offset: usize,
}

pub struct Font {
    pub px: u16,
    pub ascent: i16,
    pub descent: i16,
    /// Sorted by `ch`.
    pub glyphs: &'static [Glyph],
}

impl Font {
    pub fn glyph(&self, ch: char) -> Option<&'static Glyph> {
        self.glyphs
            .binary_search_by_key(&ch, |g| g.ch)
            .ok()
            .map(|i| &self.glyphs[i])
    }

    /// Width of `text` in logical px (no kerning).
    pub fn measure(&self, text: &str) -> i32 {
        let adv: u32 = text
            .chars()
            .filter_map(|c| self.glyph(c))
            .map(|g| u32::from(g.advance_64))
            .sum();
        ((adv + 32) / 64) as i32
    }
}

include!(concat!(env!("OUT_DIR"), "/atlas.rs"));

/// The glyph masks. In SRAM on bare metal (feature `atlas-in-ram`): the
/// rasterizer reads this on the real-time path.
#[cfg_attr(
    all(target_os = "none", feature = "atlas-in-ram"),
    unsafe(link_section = ".data.scanwright_atlas")
)]
pub static ATLAS: [u8; ATLAS_LEN] = *include_bytes!(concat!(env!("OUT_DIR"), "/atlas.bin"));
