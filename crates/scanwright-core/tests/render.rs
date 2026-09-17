//! Host render of the spike scenes: writes portrait PNGs to
//! `target/scene-*.png` (workspace root) for eyeballing and checks the basics.

use scanwright_core::{
    demo::{self, Scene},
    list::{DisplayList, OP_FILL},
    raster::Raster,
};

const W: usize = 800;
const H: usize = 480;

fn render(scene: Scene, tick: u32) -> (Box<DisplayList>, Vec<u16>, Raster) {
    let mut list = Box::new(DisplayList::new());
    demo::build(&mut list, W as u16, H as u16, scene, tick);
    let mut fb = vec![0xF81Fu16; W * H]; // magenta: any unwritten pixel shows
    let mut raster = Raster::new();
    raster.begin_frame();
    for y in 0..H {
        unsafe { raster.line(&list, y as u16, fb[y * W..].as_mut_ptr(), true) };
    }
    (list, fb, raster)
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

fn report(name: &str, list: &DisplayList, raster: &Raster) {
    let fills = list.items().iter().filter(|i| i.op == OP_FILL).count();
    // Work per line: filled pixels and mask pixels crossing it.
    let (mut max_fill, mut max_mask, mut sum_fill, mut sum_mask) = (0, 0, 0usize, 0usize);
    for y in 0..H as u16 {
        let (mut f, mut m) = (0usize, 0usize);
        for it in list.items().iter().filter(|i| i.y0 <= y && y < i.y1) {
            let n = usize::from(it.x1 - it.x0);
            if it.op == OP_FILL { f += n } else { m += n }
        }
        max_fill = max_fill.max(f);
        max_mask = max_mask.max(m);
        sum_fill += f;
        sum_mask += m;
    }
    println!(
        "{name}: {} items ({fills} fills, {} masks), {} dropped, peak active {}, list {} B; \
         per line: fill px avg {} max {max_fill}, mask px avg {} max {max_mask}",
        list.len(),
        list.len() - fills,
        list.dropped(),
        raster.peak_active(),
        std::mem::size_of::<DisplayList>(),
        sum_fill / H,
        sum_mask / H,
    );
}

#[test]
fn scenes_render_fully() {
    for scene in Scene::ALL {
        let (list, fb, raster) = render(scene, 123);
        let name = format!("scene-{}.png", scene.name());
        save(&name, &fb);
        report(&name, &list, &raster);
        assert_eq!(list.dropped(), 0);
        assert_eq!(raster.overflows(), 0);
        assert!(fb.iter().all(|&p| p != 0xF81F), "unwritten pixels");
    }
}

#[test]
fn skipped_lines_keep_the_active_list_in_step() {
    let (list, reference, _) = render(Scene::Home, 7);
    let mut raster = Raster::new();
    raster.begin_frame();
    let mut line = vec![0u16; W];
    for y in 0..H {
        let draw = y % 3 == 0;
        unsafe { raster.line(&list, y as u16, line.as_mut_ptr(), draw) };
        if draw {
            assert_eq!(&line[..], &reference[y * W..(y + 1) * W], "line {y}");
        }
    }
}
