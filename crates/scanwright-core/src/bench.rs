//! On-target micro-benchmark: measure the rasterizer's unit costs instead of
//! guessing them.
//!
//! [`run`] builds a handful of synthetic scenes, each dominated by one kind of
//! work, rasterizes whole frames of them with the real [`Raster::line`], and
//! reports the cycles spent next to the exact work done ([`LineWork`] summed
//! over the frame). The application supplies the cycle counter (DWT CYCCNT on
//! a Cortex-M) and the sink for the results; a least-squares fit of cycles
//! against work on the host gives [`crate::cost::CostModel`]'s prices — and
//! shows where the time goes.
//!
//! Run it with interrupts masked and nothing else on the bus for clean
//! numbers, then again under the real scan-out for contention.

use crate::{
    cost::{LineWork, line_work},
    font::{DISPLAY, Font, CAPTION, BODY},
    hex,
    list::DisplayList,
    raster::Raster,
};

#[derive(Clone, Copy, Debug)]
pub struct Sample {
    pub name: &'static str,
    /// Work in one frame of the scene.
    pub work: LineWork,
    pub lines: u32,
    /// Fastest of the measured frames.
    pub cycles: u32,
}

const BG: u16 = hex(0x151a1f);
const FG: u16 = hex(0xe8edf1);
const ALT: u16 = hex(0x242c34);

/// Rows of `text` from `top` down to the bottom of the (portrait) screen.
fn text_wall(b: &mut crate::list::ListBuilder<'_>, font: &Font, text: &str, bg: Option<u16>) {
    let (_, h) = b.logical_size();
    let pitch = font.line_height() + 4;
    let mut y = pitch;
    while y < h - 4 {
        b.text(font, 8, y, text, FG, bg);
        y += pitch;
    }
}

fn tall_fills(b: &mut crate::list::ListBuilder<'_>) {
    let (w, _) = b.logical_size();
    for k in 0..16 {
        b.rect(0, 400 + k * 20, w, 8, ALT);
    }
}

fn staggered_fills(b: &mut crate::list::ListBuilder<'_>) {
    // Logical x is the panel line: each of these covers 6 lines, a new one
    // starts every 2 lines, in a band of the screen the tall fills don't cover.
    for k in 0..200 {
        b.rect(8 + k * 2, 16 + (k % 8) * 40, 6, 24, FG);
    }
}

type Scene<'a> = (&'static str, &'a dyn Fn(&mut crate::list::ListBuilder<'_>));

/// Measure every scene. `line` must hold `panel_w` pixels; `now` returns a
/// free-running cycle count (wrapping is fine).
pub fn run<const I: usize, const G: usize>(
    list: &mut DisplayList<I, G>,
    line: &mut [u16],
    panel_w: u16,
    panel_h: u16,
    now: impl Fn() -> u32,
    mut report: impl FnMut(Sample),
) {
    assert!(line.len() >= usize::from(panel_w));
    const WIDE: &str = "The quick brown fox jumps over the lazy dog 0123";
    const SPARSE: &str = "i   i   i   i   i   i   i   i   i   i   i   i   i";
    let scenes: [Scene<'_>; 12] = [
        ("background only", &|_| {}),
        ("+1 full-screen fill", &|b| {
            let (w, h) = b.logical_size();
            b.rect(0, 0, w, h, ALT);
        }),
        ("+24 thin fills", &|b| {
            // Full-height stripes 8 px wide in panel x: 24 extra items on every line.
            let (w, _) = b.logical_size();
            for k in 0..24 {
                b.rect(0, 16 + k * 30, w, 8, ALT);
            }
        }),
        ("+24 wide fills", &|b| {
            let (w, _) = b.logical_size();
            for k in 0..24 {
                b.rect(0, 16 + k * 30, w, 24, ALT);
            }
        }),
        ("text wall 22px LUT", &|b| text_wall(b, &BODY, WIDE, Some(BG))),
        ("text wall 22px blend", &|b| text_wall(b, &BODY, WIDE, None)),
        ("text wall 18px LUT", &|b| text_wall(b, &CAPTION, WIDE, Some(BG))),
        // Runs that are active on every line but rarely have a glyph under it.
        ("sparse runs 22px LUT", &|b| text_wall(b, &BODY, SPARSE, Some(BG))),
        ("big digits 81px LUT", &|b| text_wall(b, &DISPLAY, "0123456", Some(BG))),
        // Bookkeeping: 16 tall fills, plus 200 short fills staggered down the
        // panel so something starts and something ends on most lines. With
        // the short ones on top (higher z) inserts append and retiring moves
        // nothing; with them underneath every start and end moves 16 records.
        ("staggered fills on top", &|b| {
            tall_fills(b);
            staggered_fills(b);
        }),
        // ...and without the tall ones: same starts, a shorter retire scan.
        ("staggered fills alone", &|b| staggered_fills(b)),
        ("staggered fills beneath", &|b| {
            staggered_fills(b);
            tall_fills(b);
        }),
    ];

    for (name, scene) in scenes {
        let mut b = list.begin(panel_w, panel_h, BG);
        scene(&mut b);
        b.finish();
        let view = list.view();

        let mut work = LineWork::default();
        for y in 0..panel_h {
            work.add(&line_work(&view, y));
        }

        let mut best = u32::MAX;
        for _ in 0..3 {
            let mut raster = Raster::new();
            raster.begin_frame();
            let t0 = now();
            for y in 0..panel_h {
                // Safety: `line` holds a full panel line; lines are in order.
                unsafe { raster.line(&view, y, line.as_mut_ptr(), true) };
            }
            best = best.min(now().wrapping_sub(t0));
        }
        report(Sample { name, work, lines: u32::from(panel_h), cycles: best });
    }
}
