//! Build-time baked fonts (see `build.rs`): 4-bit coverage masks, pre-rotated
//! into panel space.
//!
//! The baked type scale — four roles, six faces, sizes in physical px of a
//! 480 px wide portrait panel (ruling 2026-09-17, docs/DESIGN.md):
#![doc = include_str!(concat!(env!("OUT_DIR"), "/atlas-sizes.txt"))]
//!
//! `CAPTION`, `BODY` and `TITLE` carry ASCII plus `°±—·…‹›✓✎▲▼⌫⇧`; `DISPLAY`
//! is numeric (`0-9 . : - — ° C F %`).
//!
//! Two views of the same glyphs:
//!
//! * [`Font`] / [`Glyph`] — metrics for the soft side (measuring, laying out,
//!   emitting). May live in flash.
//! * [`FontSet`] — what the rasterizer reads on the real-time path: the mask
//!   atlas and a compact [`GlyphInfo`] table. In SRAM on bare metal (feature
//!   `atlas-in-ram`).
//!
//! Today both are baked inside this crate; the split-out bake crate will let
//! the application declare them, which is why lists carry a [`FontSet`]
//! rather than the rasterizer reaching for statics.

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
    /// Index into the [`FontSet`]'s glyph table.
    pub index: u16,
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
        self.measure_tracked(text, 0)
    }

    /// Width of `text` with `tracking` extra px after every glyph.
    pub fn measure_tracked(&self, text: &str, tracking: i32) -> i32 {
        let adv: i32 = text
            .chars()
            .filter_map(|c| self.glyph(c))
            .map(|g| i32::from(g.advance_64) + (tracking << 6))
            .sum();
        (adv + 32) >> 6
    }

    /// Distance between baselines.
    pub fn line_height(&self) -> i32 {
        i32::from(self.ascent) + i32::from(self.descent)
    }
}

/// Real-time glyph record: panel-space mask size and where it is in the atlas.
/// Rows are `(mask_w + 1) / 2` bytes, two pixels per byte, low nibble first.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct GlyphInfo {
    pub mask_w: u8,
    pub mask_h: u8,
    pub offset: u32,
}

/// Everything the rasterizer needs to draw text.
#[derive(Clone, Copy)]
pub struct FontSet {
    pub atlas: &'static [u8],
    pub glyphs: &'static [GlyphInfo],
}

include!(concat!(env!("OUT_DIR"), "/atlas.rs"));

#[cfg_attr(
    all(target_os = "none", feature = "atlas-in-ram"),
    unsafe(link_section = ".data.scanwright_atlas")
)]
pub static ATLAS: [u8; ATLAS_LEN] = *include_bytes!(concat!(env!("OUT_DIR"), "/atlas.bin"));

#[cfg_attr(
    all(target_os = "none", feature = "atlas-in-ram"),
    unsafe(link_section = ".data.scanwright_glyphs")
)]
pub static GLYPH_INFO: [GlyphInfo; GLYPH_COUNT] = GLYPH_INFO_INIT;

/// The fonts baked into this crate.
pub const BUILTIN: FontSet = FontSet {
    atlas: &ATLAS,
    glyphs: &GLYPH_INFO,
};
