//! The hard-real-time side: expand a list ([`ListView`]) to pixels, one panel
//! line per call.
//!
//! Painter's algorithm over the items that cross the line. Lines must be
//! requested in order — the active set is maintained incrementally — but a line
//! can be *skipped* (`draw = false`) at almost no cost, which is how the caller
//! catches up after missing a deadline instead of dropping the frame.
//!
//! # Why the active set holds records, not indices
//!
//! Measured on a Cortex-M33 with `bench::run` (2026-09-17): with an active list
//! of item *indices*, every item cost ~68 cycles per line it crossed, a text
//! run 23 more, a glyph 36 more — on a wall of text that bookkeeping was 60 %
//! of the frame, more than the pixels. Each line re-derived the same things:
//! index -> item address, clip arithmetic, mask row = base + offset + dy *
//! stride, glyph lookup, and wrote the list back to compact it.
//!
//! So an item is unpacked **once**, when it becomes active, into an [`Active`]
//! record holding exactly what a line needs — destination, length, a resolved
//! LUT pointer / colour word, and a *running* mask-row pointer that advances by
//! one stride per line. Text runs are a tiny state machine over their glyph
//! refs (the builder guarantees a run's glyphs do not overlap in y). Records
//! are only compacted on lines where some item actually ends.
//!
//! [`Raster::line`] is RAM-resident on bare metal and calls nothing outside
//! this module: no `memcpy`/`memmove`, no panics, no bounds checks — **check
//! the disassembly for calls after touching it.** Record moves are word-wise
//! volatile copies on purpose (see `move_record`).

use crate::list::{GlyphRef, Item, ListView, OP_FILL, OP_MASK_BLEND, OP_MASK_LUT, OP_RUN_LUT, POOL_ATLAS};

/// Most items that may cross one line. Excess items are not drawn (counted in
/// [`Raster::overflows`]).
pub const MAX_ACTIVE: usize = 192;

/// Everything one line needs to know about an item that crosses it.
#[derive(Clone, Copy)]
#[repr(C)]
struct Active {
    /// Item index — the z-order sort key.
    z: u16,
    /// First line past the item.
    y1: u16,
    /// First pixel and pixel count (text runs: of the current glyph).
    x0: u16,
    len: u16,
    op: u8,
    /// Masks: first mask column (non-zero when clipped).
    mx0: u8,
    /// Mask bytes per row (text runs: of the current glyph).
    stride: u16,
    /// Fill: the colour doubled into a word. LUT ops: the LUT's address. Blend
    /// ops: the spread foreground.
    aux: usize,
    /// Masks / runs: this line's mask row.
    row: *const u8,
    /// Runs: rows left in the current glyph (0 = between glyphs).
    rows: u16,
    /// Runs: first line of the glyph at `cur` (`u16::MAX` when none is left).
    next_y: u16,
    /// Runs: next glyph ref, and one past the last.
    cur: u16,
    end: u16,
}

const IDLE: Active = Active {
    z: 0,
    y1: 0,
    x0: 0,
    len: 0,
    op: OP_FILL,
    mx0: 0,
    stride: 0,
    aux: 0,
    row: core::ptr::null(),
    rows: 0,
    next_y: u16::MAX,
    cur: 0,
    end: 0,
};

pub struct Raster {
    /// Next entry of `list.order` to activate.
    next: u16,
    n_active: u16,
    /// Earliest `y1` among the active records: nothing ends before this line.
    min_y1: u16,
    /// Records of the items crossing the current line, ascending z.
    active: [Active; MAX_ACTIVE],
    overflows: u32,
    peak_active: u16,
}

impl Raster {
    pub const fn new() -> Self {
        Self {
            next: 0,
            n_active: 0,
            min_y1: u16::MAX,
            active: [IDLE; MAX_ACTIVE],
            overflows: 0,
            peak_active: 0,
        }
    }

    /// Reset for line 0 (of the same or a new list).
    #[inline(always)]
    pub fn begin_frame(&mut self) {
        self.next = 0;
        self.n_active = 0;
        self.min_y1 = u16::MAX;
    }

    pub fn overflows(&self) -> u32 {
        self.overflows
    }

    /// Largest active-set size seen.
    pub fn peak_active(&self) -> u16 {
        self.peak_active
    }

    /// Produce panel line `y` into `out` (`list.panel_w` RGB565 pixels), or
    /// just advance past it when `draw` is false.
    ///
    /// # Safety
    ///
    /// `out` must be valid for `panel_w` pixel writes when `draw` is set;
    /// lines must be requested in increasing order after [`Self::begin_frame`];
    /// `list` must come from a sealed list ([`crate::list::ListBuilder::finish`])
    /// that is not rebuilt while a frame is in progress.
    #[cfg_attr(target_os = "none", unsafe(link_section = ".data.ram_func"))]
    #[inline(never)]
    pub unsafe fn line(&mut self, list: &ListView<'_>, y: u16, out: *mut u16, draw: bool) {
        unsafe {
            let active: *mut Active = self.active.as_mut_ptr();
            let mut n = usize::from(self.n_active);

            // Retire finished items — only on lines where one actually ends.
            if y >= self.min_y1 {
                let (mut keep, mut min_y1) = (0usize, u16::MAX);
                let mut i = 0usize;
                while i < n {
                    let y1 = (*active.add(i)).y1;
                    if y1 > y {
                        if y1 < min_y1 {
                            min_y1 = y1;
                        }
                        if keep != i {
                            move_record(active.add(keep), active.add(i));
                        }
                        keep += 1;
                    }
                    i += 1;
                }
                n = keep;
                self.min_y1 = min_y1;
            }

            // Activate items starting on or before this line.
            let items: *const Item = list.items.as_ptr();
            let order: *const u16 = list.order.as_ptr();
            let n_items = list.items.len() as u16;
            while self.next < n_items {
                let idx = *order.add(usize::from(self.next));
                let it = &*items.add(usize::from(idx));
                if it.y0 > y {
                    break;
                }
                self.next += 1;
                if it.y1 <= y {
                    continue;
                }
                if n == MAX_ACTIVE {
                    self.overflows = self.overflows.wrapping_add(1);
                    continue;
                }
                // Sorted insert by z, unpacking straight into the slot.
                let mut k = n;
                while k > 0 && (*active.add(k - 1)).z > idx {
                    move_record(active.add(k), active.add(k - 1));
                    k -= 1;
                }
                let rec = unpack(list, it, idx, y);
                *active.add(k) = rec;
                n += 1;
                if rec.y1 < self.min_y1 {
                    self.min_y1 = rec.y1;
                }
            }
            self.n_active = n as u16;
            if n as u16 > self.peak_active {
                self.peak_active = n as u16;
            }

            // Draw back to front.
            let glyphs: *const GlyphRef = list.glyphs.as_ptr();
            let atlas: *const u8 = list.atlas.as_ptr();
            let mut rec = active;
            let end = active.add(n);
            while rec < end {
                let r = &mut *rec;
                rec = rec.add(1);
                if r.op == OP_FILL {
                    if draw {
                        fill16(out.add(usize::from(r.x0)), usize::from(r.len), r.aux as u32);
                    }
                    continue;
                }
                if r.op >= OP_RUN_LUT && r.rows == 0 {
                    // Between glyphs: start the next one when the beam reaches it.
                    if y < r.next_y {
                        continue;
                    }
                    let g = &*glyphs.add(usize::from(r.cur));
                    let dy = usize::from(y - g.y0);
                    r.x0 = g.x0;
                    r.len = u16::from(g.mask_w);
                    r.stride = (u16::from(g.mask_w) + 1) >> 1;
                    r.row = atlas.add(g.offset as usize + dy * usize::from(r.stride));
                    r.rows = u16::from(g.mask_h) - dy as u16;
                    r.cur += 1;
                    r.next_y = if r.cur < r.end { (*glyphs.add(usize::from(r.cur))).y0 } else { u16::MAX };
                }
                if draw {
                    let dst = out.add(usize::from(r.x0));
                    if r.op == OP_MASK_LUT || r.op == OP_RUN_LUT {
                        mask_row_lut(dst, r.row, usize::from(r.mx0), usize::from(r.len), r.aux as *const u16);
                    } else {
                        mask_row_blend(dst, r.row, usize::from(r.mx0), usize::from(r.len), r.aux as u32);
                    }
                }
                r.row = r.row.add(usize::from(r.stride));
                r.rows = r.rows.wrapping_sub(1);
            }
        }
    }
}

/// Copy one active record as single-word volatile moves.
///
/// Measured 2026-09-17 on the RP2350 (scan-out DMA running): a plain struct
/// copy compiles to multi-word `ldm`/`stm` and, although ~35 % cheaper on
/// average, produced rare 25-35 us stalls on random lines; field-wise or
/// word-wise volatile copies are cycle-for-cycle deterministic. Mechanism not
/// understood (bus fabric behaviour of multi-beat transfers is the suspect).
/// Determinism wins: the ring absorbs cost, not surprises.
#[inline(always)]
unsafe fn move_record(dst: *mut Active, src: *const Active) {
    const WORDS: usize = core::mem::size_of::<Active>().div_ceil(4);
    const { assert!(core::mem::size_of::<Active>().is_multiple_of(4) && core::mem::align_of::<Active>() >= 4) };
    unsafe {
        let (d, s) = (dst as *mut u32, src as *const u32);
        let mut i = 0;
        while i < WORDS {
            core::ptr::write_volatile(d.add(i), core::ptr::read_volatile(s.add(i)));
            i += 1;
        }
    }
}

/// Unpack `it` (item number `idx`), which becomes active on line `y`.
#[inline(always)]
unsafe fn unpack(list: &ListView<'_>, it: &Item, idx: u16, y: u16) -> Active {
    unsafe {
        let mut r = Active {
            z: idx,
            y1: it.y1,
            x0: it.x0,
            len: it.x1 - it.x0,
            op: it.op,
            ..IDLE
        };
        let lut_or_fg = |lut: bool| {
            if lut {
                list.luts.as_ptr().add(usize::from(it.color)) as usize
            } else {
                crate::spread(it.color) as usize
            }
        };
        if it.op == OP_FILL {
            r.aux = usize::from(it.color) * 0x0001_0001;
        } else if it.op == OP_MASK_LUT || it.op == OP_MASK_BLEND {
            let base = if it.pool == POOL_ATLAS { list.atlas.as_ptr() } else { list.pool.as_ptr() };
            r.mx0 = it.mx0;
            r.stride = it.n;
            r.row = base.add(it.a as usize + (usize::from(y - it.y0) + usize::from(it.my0)) * usize::from(it.n));
            r.aux = lut_or_fg(it.op == OP_MASK_LUT);
        } else {
            r.cur = it.a as u16;
            r.end = it.a as u16 + it.n;
            r.next_y = (*list.glyphs.as_ptr().add(it.a as usize)).y0;
            r.aux = lut_or_fg(it.op == OP_RUN_LUT);
        }
        r
    }
}

impl Default for Raster {
    fn default() -> Self {
        Self::new()
    }
}

/// Fill `n` pixels. Word stores once aligned.
#[inline(always)]
unsafe fn fill16(mut p: *mut u16, mut n: usize, w: u32) {
    unsafe {
        if n == 0 {
            return;
        }
        let c = w as u16;
        if (p as usize) & 2 != 0 {
            p.write(c);
            p = p.add(1);
            n -= 1;
        }
        let mut q = p as *mut u32;
        let mut words = n >> 1;
        while words >= 8 {
            q.write(w);
            q.add(1).write(w);
            q.add(2).write(w);
            q.add(3).write(w);
            q.add(4).write(w);
            q.add(5).write(w);
            q.add(6).write(w);
            q.add(7).write(w);
            q = q.add(8);
            words -= 8;
        }
        while words > 0 {
            q.write(w);
            q = q.add(1);
            words -= 1;
        }
        if n & 1 != 0 {
            (q as *mut u16).write(c);
        }
    }
}

#[inline(always)]
unsafe fn put<const BLEND: bool>(p: *mut u16, a: u32, lut: *const u16, fg: u32) {
    unsafe {
        if a == 0 {
            return;
        }
        if BLEND {
            p.write(crate::blend_spread(crate::spread(p.read()), fg, a));
        } else {
            p.write(*lut.add(a as usize));
        }
    }
}

/// One mask row through a colour LUT.
#[inline(always)]
unsafe fn mask_row_lut(out: *mut u16, row: *const u8, mx0: usize, n: usize, lut: *const u16) {
    unsafe { mask_row::<false>(out, row, mx0, n, lut, 0) }
}

/// One mask row blended in spread colour `fg` over the line.
///
/// Deliberately *not* inlined into `line` (measured 2026-09-17, Cortex-M33):
/// the blend body is big enough that, inlined next to the run/cursor logic,
/// it spills to the stack (-5 % frame time out of line on a blend-heavy
/// screen). The fill and LUT loops are the opposite: they are tiny, and a
/// call costs ~20 cycles per item per line (+10 % on a fill-heavy screen).
#[cfg_attr(target_os = "none", unsafe(link_section = ".data.ram_func"))]
#[inline(never)]
unsafe fn mask_row_blend(out: *mut u16, row: *const u8, mx0: usize, n: usize, fg: u32) {
    unsafe { mask_row::<true>(out, row, mx0, n, core::ptr::null(), fg) }
}

/// One mask row: `n` pixels starting at mask column `mx0` (two 4-bit
/// coverage values per byte, low nibble first).
#[inline(always)]
unsafe fn mask_row<const BLEND: bool>(mut out: *mut u16, row: *const u8, mx0: usize, mut n: usize, lut: *const u16, fg: u32) {
    unsafe {
        let mut src = row.add(mx0 >> 1);
        if mx0 & 1 != 0 && n > 0 {
            put::<BLEND>(out, u32::from(*src >> 4), lut, fg);
            src = src.add(1);
            out = out.add(1);
            n -= 1;
        }
        while n >= 2 {
            let b = u32::from(*src);
            // Roughly half of a glyph box is empty: skip both pixels at once.
            if b != 0 {
                put::<BLEND>(out, b & 15, lut, fg);
                put::<BLEND>(out.add(1), b >> 4, lut, fg);
            }
            src = src.add(1);
            out = out.add(2);
            n -= 2;
        }
        if n != 0 {
            put::<BLEND>(out, u32::from(*src & 15), lut, fg);
        }
    }
}
