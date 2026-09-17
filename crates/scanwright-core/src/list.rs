//! The display list: the UI in panel space, ready for the scanline rasterizer.
//!
//! Three types, one per role:
//!
//! * [`DisplayList`] — the storage. Fixed capacity chosen by the application
//!   (`ITEMS` primitives, `GLYPHS` text glyphs); lives in a `static`.
//! * [`ListBuilder`] — the soft side's write handle ([`DisplayList::begin`]).
//!   Logical coordinates in, panel-space primitives out: rotation and clipping
//!   happen here so the rasterizer does neither.
//! * [`ListView`] — the hard side's read handle ([`DisplayList::view`]).
//!
//! # Rotation
//!
//! Hard-wired for now to a portrait UI on a landscape panel, rotated 90° —
//! logical `(lx, ly)` lands on panel `(panel_w - 1 - ly, lx)` — matching the
//! baked glyph atlas. [`ListBuilder::to_panel`] is the one place that knows.
//!
//! # Text
//!
//! A text run is **one** item plus a 6-byte [`GlyphRef`] per glyph (sorted by
//! first panel line); the rasterizer walks the run with a cursor. Glyphs that
//! are partly clipped fall back to individual mask items.

use crate::font::{Font, FontSet, GlyphInfo};

pub const MAX_LUTS: usize = 24;
/// List-local mask storage (rounded-rect corners).
pub const POOL_BYTES: usize = 2048;
const MAX_CORNER_RADII: usize = 4;

/// Solid fill of `color`.
pub const OP_FILL: u8 = 0;
/// 4-bit mask resolved through colour LUT number `color`. Zero coverage leaves
/// the pixel alone.
pub const OP_MASK_LUT: u8 = 1;
/// 4-bit mask blended in `color` over whatever is already in the line.
pub const OP_MASK_BLEND: u8 = 2;
/// Text run: `n` [`GlyphRef`]s starting at `a`, through colour LUT `color`.
pub const OP_RUN_LUT: u8 = 3;
/// Text run blended in `color`.
pub const OP_RUN_BLEND: u8 = 4;

/// `Item::pool` for mask ops: the font atlas / the list's own pool.
pub const POOL_ATLAS: u8 = 0;
pub const POOL_LOCAL: u8 = 1;

/// One primitive, in panel space: columns `[x0, x1)` on lines `[y0, y1)`.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct Item {
    pub x0: u16,
    pub x1: u16,
    pub y0: u16,
    pub y1: u16,
    pub op: u8,
    pub pool: u8,
    /// Fill colour, mask/run foreground, or LUT index — per `op`.
    pub color: u16,
    /// Mask ops: byte offset of the mask in its pool. Run ops: first glyph ref.
    pub a: u32,
    /// Mask ops: bytes per mask row. Run ops: number of glyph refs.
    pub n: u16,
    /// Mask ops: the mask column / row that `x0` / `y0` correspond to
    /// (non-zero when clipped).
    pub mx0: u8,
    pub my0: u8,
}

impl Item {
    pub const EMPTY: Item = Item {
        x0: 0,
        x1: 0,
        y0: 0,
        y1: 0,
        op: OP_FILL,
        pool: 0,
        color: 0,
        a: 0,
        n: 0,
        mx0: 0,
        my0: 0,
    };
}

/// One glyph of a text run: unclipped, top-left at panel `(x0, y0)`, size and
/// mask from the font set's [`GlyphInfo`] table.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct GlyphRef {
    pub x0: u16,
    pub y0: u16,
    pub glyph: u16,
}

/// Capacity-independent part of a list.
pub struct ListHead {
    fonts: FontSet,
    panel_w: u16,
    panel_h: u16,
    n_items: u16,
    n_glyphs: u16,
    luts: [[u16; 16]; MAX_LUTS],
    lut_keys: [(u16, u16); MAX_LUTS],
    n_luts: u8,
    pool: [u8; POOL_BYTES],
    pool_len: u16,
    /// (radius, pool offsets of the TL / TR / BL / BR corner masks)
    corners: [(u8, [u16; 4]); MAX_CORNER_RADII],
    n_corners: u8,
    /// Primitives dropped because a capacity ran out.
    dropped: u16,
}

pub struct DisplayList<const ITEMS: usize, const GLYPHS: usize> {
    head: ListHead,
    /// In z order (painter's: later items draw over earlier ones).
    items: [Item; ITEMS],
    /// Item indices sorted by `y0` — the order lines activate them in.
    order: [u16; ITEMS],
    glyphs: [GlyphRef; GLYPHS],
}

impl<const ITEMS: usize, const GLYPHS: usize> DisplayList<ITEMS, GLYPHS> {
    pub const fn new(fonts: FontSet) -> Self {
        assert!(ITEMS <= u16::MAX as usize && GLYPHS <= u16::MAX as usize);
        Self {
            head: ListHead {
                fonts,
                panel_w: 0,
                panel_h: 0,
                n_items: 0,
                n_glyphs: 0,
                luts: [[0; 16]; MAX_LUTS],
                lut_keys: [(0, 0); MAX_LUTS],
                n_luts: 0,
                pool: [0; POOL_BYTES],
                pool_len: 0,
                corners: [(0, [0; 4]); MAX_CORNER_RADII],
                n_corners: 0,
                dropped: 0,
            },
            items: [Item::EMPTY; ITEMS],
            order: [0; ITEMS],
            glyphs: [GlyphRef { x0: 0, y0: 0, glyph: 0 }; GLYPHS],
        }
    }

    /// Start a new list for a `panel_w` x `panel_h` panel, with the whole
    /// screen filled with `background` (so every pixel of every line is
    /// written and the rasterizer never has to clear).
    pub fn begin(&mut self, panel_w: u16, panel_h: u16, background: u16) -> ListBuilder<'_> {
        let h = &mut self.head;
        h.panel_w = panel_w;
        h.panel_h = panel_h;
        h.n_items = 0;
        h.n_glyphs = 0;
        h.n_luts = 0;
        h.pool_len = 0;
        h.n_corners = 0;
        h.dropped = 0;
        let mut b = ListBuilder {
            head: h,
            items: &mut self.items,
            order: &mut self.order,
            glyphs: &mut self.glyphs,
        };
        b.push_fill(0, 0, i32::from(panel_w), i32::from(panel_h), background);
        b
    }

    /// The sealed list, as the rasterizer sees it.
    pub fn view(&self) -> ListView<'_> {
        let h = &self.head;
        ListView {
            panel_w: h.panel_w,
            panel_h: h.panel_h,
            items: &self.items[..usize::from(h.n_items)],
            order: &self.order[..usize::from(h.n_items)],
            glyphs: &self.glyphs[..usize::from(h.n_glyphs)],
            luts: &h.luts,
            pool: &h.pool,
            atlas: h.fonts.atlas,
            glyph_info: h.fonts.glyphs,
        }
    }

    /// Primitives dropped by the last build because a capacity ran out.
    pub fn dropped(&self) -> usize {
        usize::from(self.head.dropped)
    }
}

/// What the last build used of each capacity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ListUsage {
    pub items: usize,
    pub glyphs: usize,
    pub luts: usize,
    pub pool_bytes: usize,
    pub dropped: usize,
}

/// Read handle: everything [`crate::raster::Raster`] touches.
#[derive(Clone, Copy)]
pub struct ListView<'a> {
    pub panel_w: u16,
    pub panel_h: u16,
    pub items: &'a [Item],
    pub order: &'a [u16],
    pub glyphs: &'a [GlyphRef],
    pub luts: &'a [[u16; 16]],
    pub pool: &'a [u8],
    pub atlas: &'a [u8],
    pub glyph_info: &'a [GlyphInfo],
}

/// Write handle for one build of a list. Only [`Self::finish`] seals the
/// list for the rasterizer.
pub struct ListBuilder<'a> {
    head: &'a mut ListHead,
    items: &'a mut [Item],
    order: &'a mut [u16],
    glyphs: &'a mut [GlyphRef],
}

impl ListBuilder<'_> {
    /// Seal the list: compute the activation order.
    pub fn finish(self) -> ListUsage {
        let n = usize::from(self.head.n_items);
        for (i, o) in self.order[..n].iter_mut().enumerate() {
            *o = i as u16;
        }
        let items = &*self.items;
        self.order[..n].sort_unstable_by_key(|&i| items[usize::from(i)].y0);
        ListUsage {
            items: n,
            glyphs: usize::from(self.head.n_glyphs),
            luts: usize::from(self.head.n_luts),
            pool_bytes: usize::from(self.head.pool_len),
            dropped: usize::from(self.head.dropped),
        }
    }

    /// Logical (portrait) size.
    pub fn logical_size(&self) -> (i32, i32) {
        (i32::from(self.head.panel_h), i32::from(self.head.panel_w))
    }

    // ------------------------------------------------------------------
    // Logical-space builders
    // ------------------------------------------------------------------

    /// Logical rect -> panel rect `(x0, y0, x1, y1)`.
    fn to_panel(&self, lx: i32, ly: i32, w: i32, h: i32) -> (i32, i32, i32, i32) {
        let pw = i32::from(self.head.panel_w);
        (pw - (ly + h), lx, pw - ly, lx + w)
    }

    pub fn rect(&mut self, lx: i32, ly: i32, w: i32, h: i32, color: u16) {
        let (x0, y0, x1, y1) = self.to_panel(lx, ly, w, h);
        self.push_fill(x0, y0, x1, y1, color);
    }

    /// Rounded rect. `bg` is the flat colour behind the corners if known
    /// (cheap LUT corners); `None` blends the corners over whatever is there.
    #[allow(clippy::too_many_arguments)]
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
            self.head.dropped += 4;
            return;
        };
        let (lut, c) = self.mask_color(color, bg);
        let op = if lut { OP_MASK_LUT } else { OP_MASK_BLEND };
        let stride = (r as u16).div_ceil(2);
        let at = [(x0, y0), (x1 - r, y0), (x0, y1 - r), (x1 - r, y1 - r)];
        for (k, (x, y)) in at.into_iter().enumerate() {
            self.push_mask(x, y, r, r, POOL_LOCAL, u32::from(offs[k]), stride, op, c);
        }
    }

    /// Draw `text` with its baseline at `baseline` (logical y), starting at
    /// logical `lx`. `bg` as for [`Self::rounded_rect`]. Returns the end pen x.
    pub fn text(&mut self, font: &Font, lx: i32, baseline: i32, text: &str, fg: u16, bg: Option<u16>) -> i32 {
        let (lut, c) = self.mask_color(fg, bg);
        let (pw, ph) = (i32::from(self.head.panel_w), i32::from(self.head.panel_h));
        let start = usize::from(self.head.n_glyphs);
        // Bounding box of the run's (unclipped) glyphs, panel space.
        let (mut bx0, mut by0, mut bx1, mut by1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        let mut pen_64 = lx << 6;
        for ch in text.chars() {
            let Some(g) = font.glyph(ch) else { continue };
            let advance = i32::from(g.advance_64);
            if g.w == 0 || g.h == 0 {
                pen_64 += advance;
                continue;
            }
            let gx = (pen_64 >> 6) + g.left;
            let gy = baseline - g.top;
            pen_64 += advance;
            // Pre-rotated mask: g.h wide, g.w tall.
            let (x0, y0, x1, y1) = self.to_panel(gx, gy, g.w as i32, g.h as i32);
            if x1 <= 0 || y1 <= 0 || x0 >= pw || y0 >= ph {
                continue;
            }
            let inside = x0 >= 0 && y0 >= 0 && x1 <= pw && y1 <= ph;
            let n = usize::from(self.head.n_glyphs);
            if inside && n < self.glyphs.len() {
                // Keep the run sorted by first line (insertion; nearly sorted).
                let r = GlyphRef { x0: x0 as u16, y0: y0 as u16, glyph: g.index };
                let mut k = n;
                while k > start && self.glyphs[k - 1].y0 > r.y0 {
                    self.glyphs[k] = self.glyphs[k - 1];
                    k -= 1;
                }
                self.glyphs[k] = r;
                self.head.n_glyphs += 1;
                bx0 = bx0.min(x0);
                by0 = by0.min(y0);
                bx1 = bx1.max(x1);
                by1 = by1.max(y1);
            } else if inside {
                self.head.dropped += 1;
            } else {
                // Partly off-panel: an individually clipped mask item.
                let info = self.head.fonts.glyphs[usize::from(g.index)];
                let stride = u16::from(info.mask_w).div_ceil(2);
                let op = if lut { OP_MASK_LUT } else { OP_MASK_BLEND };
                self.push_mask(x0, y0, g.h as i32, g.w as i32, POOL_ATLAS, info.offset, stride, op, c);
            }
        }
        let count = usize::from(self.head.n_glyphs) - start;
        if count > 0 {
            self.push(Item {
                x0: bx0 as u16,
                x1: bx1 as u16,
                y0: by0 as u16,
                y1: by1 as u16,
                op: if lut { OP_RUN_LUT } else { OP_RUN_BLEND },
                color: c,
                a: start as u32,
                n: count as u16,
                ..Item::EMPTY
            });
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
        let n = usize::from(self.head.n_items);
        if n == self.items.len() {
            self.head.dropped += 1;
            return;
        }
        self.items[n] = item;
        self.head.n_items += 1;
    }

    fn push_fill(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, color: u16) {
        let (cx0, cy0) = (x0.max(0), y0.max(0));
        let (cx1, cy1) = (x1.min(i32::from(self.head.panel_w)), y1.min(i32::from(self.head.panel_h)));
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
            ..Item::EMPTY
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn push_mask(&mut self, x: i32, y: i32, mw: i32, mh: i32, pool: u8, mask_off: u32, stride: u16, op: u8, color: u16) {
        let (cx0, cy0) = (x.max(0), y.max(0));
        let (cx1, cy1) = ((x + mw).min(i32::from(self.head.panel_w)), (y + mh).min(i32::from(self.head.panel_h)));
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
            a: mask_off,
            n: stride,
            mx0: (cx0 - x) as u8,
            my0: (cy0 - y) as u8,
        });
    }

    /// Resolve `fg` over `bg` for a mask: `(true, lut index)` when the
    /// background is known and a LUT slot is available, else `(false, fg)`.
    fn mask_color(&mut self, fg: u16, bg: Option<u16>) -> (bool, u16) {
        let Some(bg) = bg else { return (false, fg) };
        let h = &mut *self.head;
        let n = usize::from(h.n_luts);
        if let Some(i) = h.lut_keys[..n].iter().position(|&k| k == (fg, bg)) {
            return (true, i as u16);
        }
        if n == MAX_LUTS {
            return (false, fg);
        }
        let (f, b) = (crate::spread(fg), crate::spread(bg));
        for a in 0..16 {
            h.luts[n][a] = crate::blend_spread(b, f, a as u32);
        }
        h.lut_keys[n] = (fg, bg);
        h.n_luts += 1;
        (true, n as u16)
    }

    /// Pool offsets of the four corner masks for radius `r` (TL, TR, BL, BR in
    /// panel space), generating them on first use.
    fn corner_masks(&mut self, r: u8) -> Option<[u16; 4]> {
        let h = &mut *self.head;
        let n = usize::from(h.n_corners);
        if let Some(c) = h.corners[..n].iter().find(|c| c.0 == r) {
            return Some(c.1);
        }
        let ru = usize::from(r);
        let stride = ru.div_ceil(2);
        let bytes = stride * ru;
        let base = usize::from(h.pool_len);
        if n == MAX_CORNER_RADII || base + 4 * bytes > POOL_BYTES {
            return None;
        }
        h.pool[base..base + 4 * bytes].fill(0);
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
                    h.pool[base + k * bytes + mj * stride + mi / 2] |= a << (4 * (mi & 1));
                }
            }
        }
        let offs = core::array::from_fn(|k| (base + k * bytes) as u16);
        h.corners[n] = (r, offs);
        h.n_corners += 1;
        h.pool_len = (base + 4 * bytes) as u16;
        Some(offs)
    }
}
