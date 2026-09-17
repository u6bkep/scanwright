//! The hard-real-time side: expand a list ([`ListView`]) to pixels, one panel
//! line per call.
//!
//! Painter's algorithm over the items that cross the line, kept in an active
//! list sorted by z (= item index). Lines must be requested in order — the
//! active list is maintained incrementally — but a line can be *skipped*
//! (`draw = false`) at almost no cost, which is how the caller catches up after
//! missing a deadline instead of dropping the frame.
//!
//! [`Raster::line`] is RAM-resident on bare metal and calls nothing outside
//! itself: no `memcpy`/`memset` (the loops below are written so LLVM's idiom
//! recognition cannot turn them into calls — check the disassembly after
//! touching them), no panics, no bounds checks.

use crate::{
    font::GlyphInfo,
    list::{GlyphRef, Item, ListView, OP_FILL, OP_MASK_LUT, OP_RUN_LUT, POOL_ATLAS},
};

/// Most items that may cross one line. Excess items are not drawn (counted in
/// [`Raster::overflows`]).
pub const MAX_ACTIVE: usize = 192;

pub struct Raster {
    /// Next entry of `list.order` to activate.
    next: u16,
    n_active: u16,
    /// Item indices crossing the current line, ascending (= z order).
    active: [u16; MAX_ACTIVE],
    /// Per active text run: its first glyph ref that has not ended yet.
    cursor: [u16; MAX_ACTIVE],
    overflows: u32,
    peak_active: u16,
}

impl Raster {
    pub const fn new() -> Self {
        Self {
            next: 0,
            n_active: 0,
            active: [0; MAX_ACTIVE],
            cursor: [0; MAX_ACTIVE],
            overflows: 0,
            peak_active: 0,
        }
    }

    /// Reset for line 0 (of the same or a new list).
    #[inline(always)]
    pub fn begin_frame(&mut self) {
        self.next = 0;
        self.n_active = 0;
    }

    pub fn overflows(&self) -> u32 {
        self.overflows
    }

    /// Largest active-list length seen.
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
            let items: *const Item = list.items.as_ptr();
            let order: *const u16 = list.order.as_ptr();
            let active: *mut u16 = self.active.as_mut_ptr();
            let cursor: *mut u16 = self.cursor.as_mut_ptr();
            let n_items = list.items.len() as u16;

            // Activate items starting on or before this line.
            let mut n = usize::from(self.n_active);
            while self.next < n_items {
                let idx = *order.add(usize::from(self.next));
                let it = &*items.add(usize::from(idx));
                if it.y0 > y {
                    break;
                }
                self.next += 1;
                if n == MAX_ACTIVE {
                    self.overflows = self.overflows.wrapping_add(1);
                    continue;
                }
                // Sorted insert. Volatile so the shift stays a loop (not memmove).
                let mut k = n;
                while k > 0 {
                    let prev = core::ptr::read_volatile(active.add(k - 1));
                    if prev < idx {
                        break;
                    }
                    core::ptr::write_volatile(active.add(k), prev);
                    core::ptr::write_volatile(cursor.add(k), core::ptr::read_volatile(cursor.add(k - 1)));
                    k -= 1;
                }
                core::ptr::write_volatile(active.add(k), idx);
                core::ptr::write_volatile(cursor.add(k), it.a as u16);
                n += 1;
            }
            if n as u16 > self.peak_active {
                self.peak_active = n as u16;
            }

            // Draw back to front, dropping finished items as we go.
            let mut keep = 0usize;
            let mut i = 0usize;
            while i < n {
                let idx = *active.add(i);
                let mut cur = *cursor.add(i);
                i += 1;
                let it = &*items.add(usize::from(idx));
                if it.y1 <= y {
                    continue;
                }
                if it.op >= OP_RUN_LUT {
                    cur = run_line(list, it, cur, y, out, draw);
                } else if draw {
                    let x0 = usize::from(it.x0);
                    let len = usize::from(it.x1) - x0;
                    let dst = out.add(x0);
                    if it.op == OP_FILL {
                        fill16(dst, len, it.color);
                    } else {
                        let base = if it.pool == POOL_ATLAS { list.atlas.as_ptr() } else { list.pool.as_ptr() };
                        let row = base
                            .add(it.a as usize)
                            .add((usize::from(y - it.y0) + usize::from(it.my0)) * usize::from(it.n));
                        let mx0 = usize::from(it.mx0);
                        if it.op == OP_MASK_LUT {
                            let lut = list.luts.as_ptr().add(usize::from(it.color)) as *const u16;
                            mask_row::<false>(dst, row, mx0, len, lut, 0);
                        } else {
                            mask_row::<true>(dst, row, mx0, len, core::ptr::null(), crate::spread(it.color));
                        }
                    }
                }
                core::ptr::write_volatile(active.add(keep), idx);
                core::ptr::write_volatile(cursor.add(keep), cur);
                keep += 1;
            }
            self.n_active = keep as u16;
        }
    }
}

/// One line of a text run: advance the cursor past glyphs that ended, then
/// draw every glyph that has started (they are sorted by first line). Returns
/// the new cursor.
#[inline(always)]
unsafe fn run_line(list: &ListView<'_>, it: &Item, mut cur: u16, y: u16, out: *mut u16, draw: bool) -> u16 {
    unsafe {
        let refs: *const GlyphRef = list.glyphs.as_ptr();
        let infos: *const GlyphInfo = list.glyph_info.as_ptr();
        let end = it.a as u16 + it.n;
        while cur < end {
            let r = &*refs.add(usize::from(cur));
            if r.y0 + u16::from((*infos.add(usize::from(r.glyph))).mask_h) > y {
                break;
            }
            cur += 1;
        }
        if !draw {
            return cur;
        }
        let lut = if it.op == OP_RUN_LUT {
            list.luts.as_ptr().add(usize::from(it.color)) as *const u16
        } else {
            core::ptr::null()
        };
        let fg = crate::spread(it.color);
        let mut k = cur;
        while k < end {
            let r = &*refs.add(usize::from(k));
            k += 1;
            if r.y0 > y {
                break;
            }
            let info = &*infos.add(usize::from(r.glyph));
            let dy = usize::from(y - r.y0);
            if dy >= usize::from(info.mask_h) {
                continue;
            }
            let w = usize::from(info.mask_w);
            let row = list.atlas.as_ptr().add(info.offset as usize + dy * w.div_ceil(2));
            let dst = out.add(usize::from(r.x0));
            if lut.is_null() {
                mask_row::<true>(dst, row, 0, w, lut, fg);
            } else {
                mask_row::<false>(dst, row, 0, w, lut, fg);
            }
        }
        cur
    }
}

impl Default for Raster {
    fn default() -> Self {
        Self::new()
    }
}

/// Fill `n` pixels. Word stores once aligned.
#[inline(always)]
unsafe fn fill16(mut p: *mut u16, mut n: usize, c: u16) {
    unsafe {
        if n == 0 {
            return;
        }
        if (p as usize) & 2 != 0 {
            p.write(c);
            p = p.add(1);
            n -= 1;
        }
        let w = u32::from(c) * 0x0001_0001;
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
