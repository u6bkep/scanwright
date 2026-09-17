//! The display list: the UI in panel space, ready for the scanline rasterizer.
//!
//! Built on the soft side (core 0) through the logical-coordinate methods
//! below; read by [`crate::raster::Raster`] on the hard-real-time side. Fixed
//! capacity, no allocation. Everything is clipped and rotated here so the
//! rasterizer does neither.
//!
//! # Rotation
//!
//! The spike hard-wires the WS-LCD43B arrangement: a portrait UI on a
//! landscape panel, rotated 90° — logical `(lx, ly)` lands on panel
//! `(panel_w - 1 - ly, lx)`. The glyph atlas is baked for the same rotation.
//! [`DisplayList::to_panel`] is the one place that knows.

use crate::font::{ATLAS, Font};

pub const MAX_ITEMS: usize = 1280;
pub const MAX_LUTS: usize = 24;
/// List-local mask storage (rounded-rect corners).
pub const POOL_BYTES: usize = 2048;
const MAX_CORNER_RADII: usize = 4;

/// Item operations.
pub const OP_FILL: u8 = 0;
/// 4-bit mask resolved through a colour LUT (`color` = LUT index). Zero
/// coverage leaves the pixel alone.
pub const OP_MASK_LUT: u8 = 1;
/// 4-bit mask blended over whatever is already in the line (`color` = fg).
pub const OP_MASK_BLEND: u8 = 2;

/// Mask data lives in the static glyph atlas / in the list's own pool.
pub const POOL_ATLAS: u8 = 0;
pub const POOL_LOCAL: u8 = 1;

/// One primitive, in panel space: columns `[x0, x1)` on lines `[y0, y1)`.
#[derive(Clone, Copy)]
#[repr(C)]
pub struct Item {
    pub x0: u16,
    pub x1: u16,
    pub y0: u16,
    pub y1: u16,
    pub op: u8,
    pub pool: u8,
    /// Fill colour, mask foreground, or LUT index — per `op`.
    pub color: u16,
    /// Mask byte offset within its pool.
    pub mask_off: u32,
    /// Mask bytes per row.
    pub stride: u8,
    /// Mask column / row that `x0` / `y0` correspond to (non-zero when clipped).
    pub mx0: u8,
    pub my0: u8,
    _pad: u8,
}

const EMPTY: Item = Item {
    x0: 0,
    x1: 0,
    y0: 0,
    y1: 0,
    op: OP_FILL,
    pool: 0,
    color: 0,
    mask_off: 0,
    stride: 0,
    mx0: 0,
    my0: 0,
    _pad: 0,
};

pub struct DisplayList {
    pub(crate) panel_w: u16,
    pub(crate) panel_h: u16,
    pub(crate) n: u16,
    /// In z order (painter's: later items draw over earlier ones).
    pub(crate) items: [Item; MAX_ITEMS],
    /// Item indices sorted by `y0` — the order lines activate them in.
    pub(crate) order: [u16; MAX_ITEMS],
    pub(crate) luts: [[u16; 16]; MAX_LUTS],
    lut_keys: [(u16, u16); MAX_LUTS],
    n_luts: u8,
    pub(crate) pool: [u8; POOL_BYTES],
    pool_len: u16,
    /// (radius, pool offsets of the TL / TR / BL / BR corner masks)
    corners: [(u8, [u16; 4]); MAX_CORNER_RADII],
    n_corners: u8,
    /// Primitives dropped because a capacity ran out.
    dropped: u16,
}

impl DisplayList {
    pub const fn new() -> Self {
        Self {
            panel_w: 0,
            panel_h: 0,
            n: 0,
            items: [EMPTY; MAX_ITEMS],
            order: [0; MAX_ITEMS],
            luts: [[0; 16]; MAX_LUTS],
            lut_keys: [(0, 0); MAX_LUTS],
            n_luts: 0,
            pool: [0; POOL_BYTES],
            pool_len: 0,
            corners: [(0, [0; 4]); MAX_CORNER_RADII],
            n_corners: 0,
            dropped: 0,
        }
    }

    /// Start a new list for a `panel_w` x `panel_h` panel, with the whole
    /// screen filled with `background` (so every pixel of every line is
    /// written and the rasterizer never has to clear).
    pub fn begin(&mut self, panel_w: u16, panel_h: u16, background: u16) {
        self.panel_w = panel_w;
        self.panel_h = panel_h;
        self.n = 0;
        self.n_luts = 0;
        self.pool_len = 0;
        self.n_corners = 0;
        self.dropped = 0;
        self.push_fill(0, 0, i32::from(panel_w), i32::from(panel_h), background);
    }

    /// Seal the list: compute the activation order.
    pub fn finish(&mut self) {
        let n = usize::from(self.n);
        for (i, o) in self.order[..n].iter_mut().enumerate() {
            *o = i as u16;
        }
        let items = &self.items;
        self.order[..n].sort_unstable_by_key(|&i| items[usize::from(i)].y0);
    }

    pub fn len(&self) -> usize {
        usize::from(self.n)
    }

    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    pub fn dropped(&self) -> usize {
        usize::from(self.dropped)
    }

    pub fn items(&self) -> &[Item] {
        &self.items[..usize::from(self.n)]
    }

    /// Logical (portrait) size.
    pub fn logical_size(&self) -> (i32, i32) {
        (i32::from(self.panel_h), i32::from(self.panel_w))
    }

    // ------------------------------------------------------------------
    // Logical-space builders
    // ------------------------------------------------------------------

    /// Logical rect -> panel rect `(x0, y0, x1, y1)`.
    fn to_panel(&self, lx: i32, ly: i32, w: i32, h: i32) -> (i32, i32, i32, i32) {
        let pw = i32::from(self.panel_w);
        (pw - (ly + h), lx, pw - ly, lx + w)
    }

    pub fn rect(&mut self, lx: i32, ly: i32, w: i32, h: i32, color: u16) {
        let (x0, y0, x1, y1) = self.to_panel(lx, ly, w, h);
        self.push_fill(x0, y0, x1, y1, color);
    }

    /// Rounded rect. `bg` is the flat colour behind the corners if known
    /// (cheap LUT corners); `None` blends the corners over whatever is there.
    pub fn rounded_rect(&mut self, lx: i32, ly: i32, w: i32, h: i32, r: i32, color: u16, bg: Option<u16>) {
        let (x0, y0, x1, y1) = self.to_panel(lx, ly, w, h);
        let r = r.min(w / 2).min(h / 2).clamp(0, 64);
        if r == 0 {
            return self.push_fill(x0, y0, x1, y1, color);
        }
        self.push_fill(x0, y0 + r, x1, y1 - r, color);
        self.push_fill(x0 + r, y0, x1 - r, y0 + r, color);
        self.push_fill(x0 + r, y1 - r, x1 - r, y1, color);
        let Some(offs) = self.corner_masks(r as u8) else {
            self.dropped += 4;
            return;
        };
        let (op, c) = self.mask_op(color, bg);
        let stride = (r as u32).div_ceil(2) as u8;
        let at = [(x0, y0), (x1 - r, y0), (x0, y1 - r), (x1 - r, y1 - r)];
        for (k, (x, y)) in at.into_iter().enumerate() {
            self.push_mask(x, y, r, r, POOL_LOCAL, u32::from(offs[k]), stride, op, c);
        }
    }

    /// Draw `text` with its baseline at `baseline` (logical y), starting at
    /// logical `lx`. `bg` as for [`Self::rounded_rect`]. Returns the end pen x.
    pub fn text(&mut self, font: &Font, lx: i32, baseline: i32, text: &str, fg: u16, bg: Option<u16>) -> i32 {
        let (op, c) = self.mask_op(fg, bg);
        let mut pen_64 = lx << 6;
        for ch in text.chars() {
            let Some(g) = font.glyph(ch) else { continue };
            if g.w > 0 && g.h > 0 {
                let gx = (pen_64 >> 6) + g.left;
                let gy = baseline - g.top;
                let (x0, y0, _, _) = self.to_panel(gx, gy, g.w as i32, g.h as i32);
                // Pre-rotated mask: g.h wide, g.w tall.
                self.push_mask(
                    x0,
                    y0,
                    g.h as i32,
                    g.w as i32,
                    POOL_ATLAS,
                    g.offset as u32,
                    g.h.div_ceil(2) as u8,
                    op,
                    c,
                );
            }
            pen_64 += i32::from(g.advance_64);
        }
        (pen_64 + 32) >> 6
    }

    /// Text centred horizontally on logical `cx`.
    pub fn text_centered(&mut self, font: &Font, cx: i32, baseline: i32, text: &str, fg: u16, bg: Option<u16>) {
        let w = font.measure(text);
        self.text(font, cx - w / 2, baseline, text, fg, bg);
    }

    // ------------------------------------------------------------------
    // Panel-space pushes (clip here, never in the rasterizer)
    // ------------------------------------------------------------------

    fn push(&mut self, item: Item) {
        let n = usize::from(self.n);
        if n == MAX_ITEMS {
            self.dropped += 1;
            return;
        }
        self.items[n] = item;
        self.n += 1;
    }

    fn push_fill(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, color: u16) {
        let (cx0, cy0) = (x0.max(0), y0.max(0));
        let (cx1, cy1) = (x1.min(i32::from(self.panel_w)), y1.min(i32::from(self.panel_h)));
        if cx0 >= cx1 || cy0 >= cy1 {
            return;
        }
        self.push(Item {
            x0: cx0 as u16,
            x1: cx1 as u16,
            y0: cy0 as u16,
            y1: cy1 as u16,
            op: OP_FILL,
            color,
            ..EMPTY
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn push_mask(&mut self, x: i32, y: i32, mw: i32, mh: i32, pool: u8, mask_off: u32, stride: u8, op: u8, color: u16) {
        let (cx0, cy0) = (x.max(0), y.max(0));
        let (cx1, cy1) = ((x + mw).min(i32::from(self.panel_w)), (y + mh).min(i32::from(self.panel_h)));
        if cx0 >= cx1 || cy0 >= cy1 {
            return;
        }
        self.push(Item {
            x0: cx0 as u16,
            x1: cx1 as u16,
            y0: cy0 as u16,
            y1: cy1 as u16,
            op,
            pool,
            color,
            mask_off,
            stride,
            mx0: (cx0 - x) as u8,
            my0: (cy0 - y) as u8,
            _pad: 0,
        });
    }

    /// Pick the mask op for `fg` over `bg`: a LUT when the background is known
    /// and a LUT slot is available, else a true blend.
    fn mask_op(&mut self, fg: u16, bg: Option<u16>) -> (u8, u16) {
        let Some(bg) = bg else { return (OP_MASK_BLEND, fg) };
        let n = usize::from(self.n_luts);
        if let Some(i) = self.lut_keys[..n].iter().position(|&k| k == (fg, bg)) {
            return (OP_MASK_LUT, i as u16);
        }
        if n == MAX_LUTS {
            return (OP_MASK_BLEND, fg);
        }
        let (f, b) = (crate::spread(fg), crate::spread(bg));
        for a in 0..16 {
            self.luts[n][a] = crate::blend_spread(b, f, a as u32);
        }
        self.lut_keys[n] = (fg, bg);
        self.n_luts += 1;
        (OP_MASK_LUT, n as u16)
    }

    /// Pool offsets of the four corner masks for radius `r` (TL, TR, BL, BR in
    /// panel space), generating them on first use.
    fn corner_masks(&mut self, r: u8) -> Option<[u16; 4]> {
        let n = usize::from(self.n_corners);
        if let Some(c) = self.corners[..n].iter().find(|c| c.0 == r) {
            return Some(c.1);
        }
        let ru = usize::from(r);
        let stride = ru.div_ceil(2);
        let bytes = stride * ru;
        let base = usize::from(self.pool_len);
        if n == MAX_CORNER_RADII || base + 4 * bytes > POOL_BYTES {
            return None;
        }
        self.pool[base..base + 4 * bytes].fill(0);
        // Coverage of the disc centred (r, r), 4x4 supersampled, in 1/8 px.
        let r8 = 8 * i32::from(r);
        for j in 0..ru {
            for i in 0..ru {
                let mut count = 0u32;
                for t in 0..4 {
                    for s in 0..4 {
                        let dx = 8 * i as i32 + 2 * s + 1 - r8;
                        let dy = 8 * j as i32 + 2 * t + 1 - r8;
                        count += u32::from(dx * dx + dy * dy <= r8 * r8);
                    }
                }
                let a = ((count * 15 + 8) / 16) as u8;
                // TL as computed; the others are its mirrors.
                for (k, (mi, mj)) in [(i, j), (ru - 1 - i, j), (i, ru - 1 - j), (ru - 1 - i, ru - 1 - j)]
                    .into_iter()
                    .enumerate()
                {
                    self.pool[base + k * bytes + mj * stride + mi / 2] |= a << (4 * (mi & 1));
                }
            }
        }
        let offs = core::array::from_fn(|k| (base + k * bytes) as u16);
        self.corners[n] = (r, offs);
        self.n_corners += 1;
        self.pool_len = (base + 4 * bytes) as u16;
        Some(offs)
    }

    /// Base address of mask pool `pool`.
    #[inline(always)]
    pub(crate) fn pool_base(&self, pool: u8) -> *const u8 {
        if pool == POOL_ATLAS {
            ATLAS.as_ptr()
        } else {
            self.pool.as_ptr()
        }
    }
}

impl Default for DisplayList {
    fn default() -> Self {
        Self::new()
    }
}
