//! Cost model: predict the rasterizer's real-time cost of a list **without
//! hardware**, so a screen that cannot hold the panel's line rate fails on the
//! host — in the simulator, in CI, in an agent's edit loop — not on the bench.
//!
//! The rasterizer's work per line is a simple function of what crosses it:
//! pixels filled, mask pixels resolved through a LUT, mask pixels blended,
//! items visited, glyphs visited. [`CostModel`] prices each; [`analyze`] walks
//! the list, replays the frame against the scan-out ring, and reports the
//! worst line, the whole-frame load and whether any line would be late.

use crate::list::{ListView, OP_FILL, OP_MASK_BLEND, OP_MASK_LUT, OP_RUN_BLEND, OP_RUN_LUT};

/// What crosses one panel line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LineWork {
    pub fill_px: u32,
    pub lut_px: u32,
    pub blend_px: u32,
    /// Items crossing the line (text runs included).
    pub items: u32,
    /// Text runs crossing the line, whether or not a glyph of theirs does.
    pub runs: u32,
    /// Glyphs crossing the line.
    pub glyphs: u32,
    /// Items that become active on this line.
    pub activations: u32,
    /// Active records moved on this line: shifted up by a sorted insert, or
    /// down when an item below them retires.
    pub moves: u32,
    /// Records scanned by the retire pass (runs only on lines where an item ends).
    pub retire_scan: u32,
}

impl LineWork {
    pub fn add(&mut self, o: &LineWork) {
        self.fill_px += o.fill_px;
        self.lut_px += o.lut_px;
        self.blend_px += o.blend_px;
        self.items += o.items;
        self.runs += o.runs;
        self.glyphs += o.glyphs;
        self.activations += o.activations;
        self.moves += o.moves;
        self.retire_scan += o.retire_scan;
    }
}

/// Prices, in CPU cycles x 16 (so sub-cycle costs stay integral).
#[derive(Clone, Copy, Debug)]
pub struct CostModel {
    pub fill_px_x16: u32,
    pub lut_px_x16: u32,
    pub blend_px_x16: u32,
    /// Per non-run item crossing the line.
    pub item_x16: u32,
    /// Per text run crossing the line (with or without a glyph under it).
    pub run_x16: u32,
    /// Per glyph crossing the line.
    pub glyph_x16: u32,
    /// Per item becoming active (unpacking it into an active record).
    pub activate_x16: u32,
    /// Per active record moved (sorted insert / retire compaction).
    pub move_x16: u32,
    /// Per record scanned on a line where something retires.
    pub retire_scan_x16: u32,
    /// Fixed per-line overhead.
    pub line_x16: u32,
}

impl CostModel {
    /// Cortex-M33 (RP2350 @ 264 MHz), rasterizer and data in SRAM,
    /// `opt-level = 3`, scan-out DMA running. **Measured**, not estimated: a
    /// least-squares fit of [`crate::bench::run`] on the WS-LCD43B, 2026-09-17
    /// (12 scenes, residuals <= 2.5 %), rounded up ~3 % (activation: the higher
    /// of the two bench conditions). Re-run the bench when the inner loops
    /// change. The fit: line 154, fill 0.78/px, LUT 6.37/px, blend 15.5/px,
    /// item 40.0, run 24.1, glyph 31.9, activate 68-98, move 22.6, retire scan 22.9.
    pub const CORTEX_M33: CostModel = CostModel {
        fill_px_x16: 13,
        lut_px_x16: 105,
        blend_px_x16: 255,
        item_x16: 41 * 16,
        run_x16: 25 * 16,
        glyph_x16: 33 * 16,
        activate_x16: 100 * 16,
        move_x16: 24 * 16,
        retire_scan_x16: 24 * 16,
        line_x16: 158 * 16,
    };

    pub fn cycles(&self, w: &LineWork) -> u32 {
        (w.fill_px * self.fill_px_x16
            + w.lut_px * self.lut_px_x16
            + w.blend_px * self.blend_px_x16
            + (w.items - w.runs) * self.item_x16
            + w.runs * self.run_x16
            + w.glyphs * self.glyph_x16
            + w.activations * self.activate_x16
            + w.moves * self.move_x16
            + w.retire_scan * self.retire_scan_x16
            + self.line_x16)
            / 16
    }
}

/// The work crossing line `y` — a host-side mirror of what
/// [`crate::raster::Raster::line`] does, bookkeeping included.
pub fn line_work(list: &ListView<'_>, y: u16) -> LineWork {
    let mut w = LineWork::default();
    // Retire pass: runs when something ended on the previous line; it scans
    // every record that was active and moves the ones above the first gap.
    let was_active = |it: &crate::list::Item| y > 0 && it.y0 < y && y - 1 < it.y1;
    if let Some(first_gone) = list.items.iter().position(|it| was_active(it) && it.y1 == y) {
        w.retire_scan = list.items.iter().filter(|it| was_active(it)).count() as u32;
        w.moves += list.items[first_gone..].iter().filter(|it| was_active(it) && it.y1 > y).count() as u32;
    }
    for (z, it) in list.items.iter().enumerate().filter(|(_, it)| it.y0 <= y && y < it.y1) {
        w.items += 1;
        if it.y0 == y {
            // Sorted insert: everything already active above it shifts up.
            // (Items starting on the same line arrive in z order.)
            w.activations += 1;
            w.moves += list.items[z + 1..].iter().filter(|o| o.y0 < y && y < o.y1).count() as u32;
        }
        let px = u32::from(it.x1 - it.x0);
        match it.op {
            OP_FILL => w.fill_px += px,
            OP_MASK_LUT => w.lut_px += px,
            OP_MASK_BLEND => w.blend_px += px,
            OP_RUN_LUT | OP_RUN_BLEND => {
                w.runs += 1;
                let refs = &list.glyphs[it.a as usize..it.a as usize + usize::from(it.n)];
                for r in refs {
                    if r.y0 <= y && y < r.y0 + u16::from(r.mask_h) {
                        w.glyphs += 1;
                        if it.op == OP_RUN_LUT {
                            w.lut_px += u32::from(r.mask_w);
                        } else {
                            w.blend_px += u32::from(r.mask_w);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    w
}

/// The scan-out the rasterizer feeds: a ring of line buffers drained at the
/// panel's line rate.
#[derive(Clone, Copy, Debug)]
pub struct Scanout {
    /// CPU cycles per panel line period.
    pub budget_cycles: u32,
    /// Ring depth in lines. The rasterizer may run `ring_lines - 2` ahead.
    pub ring_lines: u16,
    /// Blanking lines between frames — time to pre-fill the ring.
    pub vblank_lines: u16,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Report {
    /// The most expensive single line and its predicted cycles.
    pub worst_line: u16,
    pub worst_line_cycles: u32,
    /// Predicted cycles for the whole frame.
    pub frame_cycles: u64,
    /// Lines the rasterizer would deliver late (0 = the screen fits).
    pub late_lines: u16,
    /// Tightest margin between a line being ready and being needed, in line
    /// periods x 16 (negative = late).
    pub min_slack_x16: i32,
}

impl Report {
    pub fn fits(&self) -> bool {
        self.late_lines == 0
    }

    /// Whole-frame load as a percentage of one core.
    pub fn core_percent(&self, scanout: &Scanout, panel_h: u16) -> u32 {
        let frame = u64::from(scanout.budget_cycles) * u64::from(panel_h + scanout.vblank_lines);
        (self.frame_cycles * 100 / frame.max(1)) as u32
    }
}

/// Price every line of `list` and replay the frame against the scan-out: the
/// rasterizer starts at the top of blanking, may run at most `ring_lines - 2`
/// lines ahead, and line `y` is due `y` line periods after blanking ends.
pub fn analyze(list: &ListView<'_>, model: &CostModel, scanout: &Scanout) -> Report {
    let budget = i64::from(scanout.budget_cycles.max(1));
    let lead_max = i64::from(scanout.ring_lines.max(3)) - 2;
    let mut r = Report { min_slack_x16: i32::MAX, ..Report::default() };
    // Time in cycles; 0 = first active line is due.
    let mut t = -i64::from(scanout.vblank_lines) * budget;
    for y in 0..list.panel_h {
        let c = model.cycles(&line_work(list, y));
        if c > r.worst_line_cycles {
            r.worst_line_cycles = c;
            r.worst_line = y;
        }
        r.frame_cycles += u64::from(c);
        // The slot for line y frees once line y - lead_max has been scanned.
        let slot_free = (i64::from(y) - lead_max + 1) * budget;
        t = t.max(slot_free) + i64::from(c);
        let due = i64::from(y) * budget;
        let slack = ((due - t) * 16 / budget).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
        r.min_slack_x16 = r.min_slack_x16.min(slack);
        if t > due {
            r.late_lines += 1;
            // A late line is skipped, not waited for: the pump resumes ahead of the beam.
            t = due;
        }
    }
    r
}

/// Cycles available per line: `line_period_ns` of a core at `clk_hz`.
pub const fn line_budget_cycles(clk_hz: u32, line_period_ns: u32) -> u32 {
    ((clk_hz as u64 * line_period_ns as u64) / 1_000_000_000) as u32
}
