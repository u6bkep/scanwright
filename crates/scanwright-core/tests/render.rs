//! Host render of the spike scenes: writes portrait PNGs to
//! `target/scene-*.png` (workspace root) for eyeballing and checks the basics.

use scanwright_core::{
    cost::{self, CostModel},
    demo::{self, Scene},
    font,
    list::{DisplayList, ListUsage},
    raster::Raster,
};

type List = DisplayList<512, 1536>;

const W: usize = 800;
const H: usize = 480;

fn render(scene: Scene, tick: u32) -> (Box<List>, ListUsage, Vec<u16>, Raster) {
    let mut list = Box::new(List::new(font::BUILTIN));
    let usage = demo::build(&mut list, W as u16, H as u16, scene, tick);
    let mut fb = vec![0xF81Fu16; W * H]; // magenta: any unwritten pixel shows
    let mut raster = Raster::new();
    raster.begin_frame();
    for y in 0..H {
        unsafe { raster.line(&list.view(), y as u16, fb[y * W..].as_mut_ptr(), true) };
    }
    (list, usage, fb, raster)
}

/// Panel-space RGB565 -> portrait RGB8 PNG (undo the 90° rotation).
fn save(name: &str, fb: &[u16]) {
    let (lw, lh) = (H, W);
    let mut rgb = vec![0u8; lw * lh * 3];
    for ly in 0..lh {
        for lx in 0..lw {
            let p = fb[lx * W + (W - 1 - ly)];
            let o = (ly * lw + lx) * 3;
            rgb[o] = ((p >> 11) as u8) << 3;
            rgb[o + 1] = ((p >> 5) as u8 & 0x3f) << 2;
            rgb[o + 2] = (p as u8 & 0x1f) << 3;
        }
    }
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
    std::fs::create_dir_all(&dir).unwrap();
    let file = std::fs::File::create(dir.join(name)).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), lw as u32, lh as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&rgb).unwrap();
}

/// WS-LCD43B: RP2350 @ 264 MHz, 820 clocks per line at 22 MHz pclk, 32-line
/// ring, 20 blanking lines.
const SCANOUT: cost::Scanout = cost::Scanout {
    budget_cycles: cost::line_budget_cycles(264_000_000, 37_273),
    ring_lines: 32,
    vblank_lines: 20,
};

fn report(name: &str, list: &List, usage: &ListUsage, raster: &Raster) -> cost::Report {
    let r = cost::analyze(&list.view(), &CostModel::CORTEX_M33, &SCANOUT);
    println!(
        "{name}: {usage:?}, peak active {}, list {} B; predicted: worst line {} = {} cyc (budget {}), \
         {} % of a core, {} late lines, min slack {:.1} lines",
        raster.peak_active(),
        std::mem::size_of::<List>(),
        r.worst_line,
        r.worst_line_cycles,
        SCANOUT.budget_cycles,
        r.core_percent(&SCANOUT, H as u16),
        r.late_lines,
        r.min_slack_x16 as f64 / 16.0,
    );
    r
}

#[test]
fn scenes_render_fully() {
    for scene in Scene::ALL {
        let (list, usage, fb, raster) = render(scene, 123);
        let name = format!("scene-{}.png", scene.name());
        save(&name, &fb);
        let cost = report(&name, &list, &usage, &raster);
        assert!(cost.fits(), "{name}: predicted late lines");
        assert_eq!(list.dropped(), 0);
        assert_eq!(raster.overflows(), 0);
        assert!(fb.iter().all(|&p| p != 0xF81F), "unwritten pixels");
    }
}

#[test]
fn skipped_lines_keep_the_active_list_in_step() {
    let (list, _, reference, _) = render(Scene::Home, 7);
    let mut raster = Raster::new();
    raster.begin_frame();
    let mut line = vec![0u16; W];
    for y in 0..H {
        let draw = y % 3 == 0;
        unsafe { raster.line(&list.view(), y as u16, line.as_mut_ptr(), draw) };
        if draw {
            assert_eq!(&line[..], &reference[y * W..(y + 1) * W], "line {y}");
        }
    }
}

#[test]
fn cost_model_flags_a_screen_the_ring_cannot_absorb() {
    // The blend stress scene fits the real scan-out (measured on hardware:
    // worst line over budget, ring absorbs it). With a 4-line ring it cannot.
    let (list, _, _, _) = render(Scene::DenseBlend, 1);
    let tight = cost::Scanout { ring_lines: 4, ..SCANOUT };
    assert!(cost::analyze(&list.view(), &CostModel::CORTEX_M33, &SCANOUT).fits());
    assert!(!cost::analyze(&list.view(), &CostModel::CORTEX_M33, &tight).fits());
}

#[test]
fn bench_scenes_build_and_do_distinct_work() {
    let mut list = Box::new(List::new(font::BUILTIN));
    let mut line = vec![0u16; W];
    let ticks = std::cell::Cell::new(0u32);
    let mut samples = Vec::new();
    scanwright_core::bench::run(
        &mut list,
        &mut line,
        W as u16,
        H as u16,
        || {
            ticks.set(ticks.get() + 1);
            ticks.get()
        },
        |s| samples.push(s),
    );
    for s in &samples {
        println!("{:24} {:?}", s.name, s.work);
    }
    assert_eq!(samples.len(), 12);
    assert!(samples[9].work.moves < 1000 && samples[11].work.moves > 5000, "{:?}", samples[11].work);
    assert_eq!(samples[9].work.activations, samples[11].work.activations);
    assert!(samples[10].work.retire_scan < samples[9].work.retire_scan);
    assert_eq!(list.dropped(), 0);
    assert!(samples[0].work.items == H as u32 && samples[0].work.lut_px == 0);
    assert!(samples[4].work.lut_px > 0 && samples[4].work.blend_px == 0);
    assert!(samples[5].work.blend_px > 0 && samples[5].work.lut_px == 0);
    assert!(samples[7].work.runs > 20 * samples[7].work.glyphs / 10, "sparse runs should be mostly idle");
}

fn fnv(fb: &[u16]) -> u64 {
    fb.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &p| (h ^ u64::from(p)).wrapping_mul(0x0100_0000_01b3))
}

/// Pixel-exact regression guard for rasterizer work. The hashes change only
/// when scene content or the blend maths changes — update them deliberately.
#[test]
fn golden_framebuffers() {
    let got: Vec<(Scene, u64)> = Scene::ALL.iter().map(|&s| (s, fnv(&render(s, 123).2))).collect();
    println!("{got:#x?}");
    let want = [GOLDEN_HOME, GOLDEN_DENSE_LUT, GOLDEN_DENSE_BLEND];
    for ((scene, h), w) in got.iter().zip(want) {
        assert_eq!(*h, w, "{scene:?}");
    }
}
const GOLDEN_HOME: u64 = 0x2be9_36bc_963a_7ac1;
const GOLDEN_DENSE_LUT: u64 = 0x606f_d48f_0a43_3966;
// (A LUT is the blend, tabulated: same pixels.)
const GOLDEN_DENSE_BLEND: u64 = 0x606f_d48f_0a43_3966;
