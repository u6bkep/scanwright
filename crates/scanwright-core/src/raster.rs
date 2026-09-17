//! The hard-real-time side: expand a [`DisplayList`] to pixels, one panel line
//! per call.
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

use crate::list::{DisplayList, Item, OP_FILL, OP_MASK_LUT};

/// Most items that may cross one line. Excess items are not drawn (counted in
/// [`Raster::overflows`]).
pub const MAX_ACTIVE: usize = 192;

pub struct Raster {
    /// Next entry of `list.order` to activate.
    next: u16,
    n_active: u16,
    /// Item indices crossing the current line, ascending (= z order).
    active: [u16; MAX_ACTIVE],
    overflows: u32,
    peak_active: u16,
}

impl Raster {
    pub const fn new() -> Self {
        Self {
            next: 0,
            n_active: 0,
            active: [0; MAX_ACTIVE],
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
    /// `list` must be sealed ([`DisplayList::finish`]) and not mutated while a
    /// frame is in progress.
    #[cfg_attr(target_os = "none", unsafe(link_section = ".data.ram_func"))]
    #[inline(never)]
    pub unsafe fn line(&mut self, list: &DisplayList, y: u16, out: *mut u16, draw: bool) {
        unsafe {
            let items: *const Item = list.items.as_ptr();
            let order: *const u16 = list.order.as_ptr();
            let active: *mut u16 = self.active.as_mut_ptr();
            let n_items = list.n;

            // Activate items starting on or before this line.
            let mut n = usize::from(self.n_active);
            while self.next < n_items {
                let idx = *order.add(usize::from(self.next));
                if (*items.add(usize::from(idx))).y0 > y {
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
                    k -= 1;
                }
                core::ptr::write_volatile(active.add(k), idx);
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
                i += 1;
                let it = &*items.add(usize::from(idx));
                if it.y1 <= y {
                    continue;
                }
                core::ptr::write_volatile(active.add(keep), idx);
                keep += 1;
                if !draw {
                    continue;
                }
                let x0 = usize::from(it.x0);
                let len = usize::from(it.x1) - x0;
                let dst = out.add(x0);
                if it.op == OP_FILL {
                    fill16(dst, len, it.color);
                } else {
                    let row = list
                        .pool_base(it.pool)
                        .add(it.mask_off as usize)
                        .add((usize::from(y - it.y0) + usize::from(it.my0)) * usize::from(it.stride));
                    let mx0 = usize::from(it.mx0);
                    if it.op == OP_MASK_LUT {
                        let lut = list.luts.as_ptr().add(usize::from(it.color)) as *const u16;
                        mask_row::<false>(dst, row, mx0, len, lut, 0);
                    } else {
                        mask_row::<true>(dst, row, mx0, len, core::ptr::null(), crate::spread(it.color));
                    }
                }
            }
            self.n_active = keep as u16;
        }
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
