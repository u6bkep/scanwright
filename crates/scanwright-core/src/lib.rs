//! **Spike** — display-list scanline rasterizer for racing the beam.
//!
//! The target is an RGB (DPI) panel with no frame memory, driven from a
//! microcontroller that cannot afford a framebuffer's memory *or* the bus
//! bandwidth to scan one out. Instead of storing pixels, store the UI in its
//! most compressed form — a flat display list — and expand it to pixels one
//! scanline at a time, in step with the panel:
//!
//! ```text
//! core 0 (soft):  app state -> build DisplayList (panel space) -> publish at vsync
//! core 1 (hard):  for each panel line: Raster::line(list, y, line buffer) -> DMA -> PIO
//! ```
//!
//! * [`list::DisplayList`] is fixed-capacity, allocation-free, and already in
//!   **panel space** (rotation applied by the builder), so the rasterizer never
//!   transforms or clips anything.
//! * Two primitives only: solid fills and 4-bit coverage masks (glyphs,
//!   rounded-rect corners). A mask over a known flat background is resolved
//!   through a 16-entry colour LUT built on core 0 — no per-pixel blend math
//!   and no read-back on the real-time side.
//! * [`raster::Raster::line`] is the hot path: painter's algorithm over the
//!   items crossing the line. On bare metal it is placed in RAM
//!   (`.data.ram_func`) so it never waits on the flash XIP bus.
//!
//! The question the spike answers: does the worst-case line fit the panel's
//! line period (37 µs at 22 MHz pclk on the WS-LCD43B) on a 264 MHz Cortex-M33?

#![no_std]

pub mod demo;
pub mod font;
pub mod list;
pub mod raster;

/// RGB565 from 8-bit components.
pub const fn rgb(r: u8, g: u8, b: u8) -> u16 {
    ((r as u16 >> 3) << 11) | ((g as u16 >> 2) << 5) | (b as u16 >> 3)
}

/// RGB565 from a `0xRRGGBB` literal.
pub const fn hex(c: u32) -> u16 {
    rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

const BLEND_MASK: u32 = 0x07E0_F81F;

/// Spread an RGB565 pixel so each channel has ≥ 5 bits of headroom.
#[inline(always)]
pub(crate) const fn spread(c: u16) -> u32 {
    let c = c as u32;
    (c | (c << 16)) & BLEND_MASK
}

/// Blend spread `fg` over spread `bg` with 4-bit coverage `a` (0..=15).
#[inline(always)]
pub(crate) const fn blend_spread(bg: u32, fg: u32, a: u32) -> u16 {
    // 0..=15 -> 0..=32 (15 maps to fully opaque).
    let a = a * 2 + (a >> 3) + (a == 15) as u32;
    let v = ((bg * (32 - a) + fg * a) >> 5) & BLEND_MASK;
    (v | (v >> 16)) as u16
}
