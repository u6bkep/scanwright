//! Bakes the glyph atlas: every (font, size, charset) below is rasterized on
//! the host, quantized to 4-bit coverage and stored **pre-rotated into panel
//! space** (UI rotated 90°: logical x runs down the panel, logical y runs
//! right-to-left), so the rasterizer reads each scanline's slice of a glyph as
//! one contiguous run of nibbles.
//!
//! Mask layout: row-major in panel space, `stride = (mask_w + 1) / 2` bytes per
//! row, two pixels per byte, low nibble first.

use std::{env, fmt::Write as _, fs, path::PathBuf};

struct Spec {
    name: &'static str,
    file: &'static str,
    px: f32,
    chars: &'static str,
}

const ASCII: &str = " !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~°";

const SPECS: &[Spec] = &[
    Spec { name: "REGULAR_18", file: "Roboto-Regular.ttf", px: 18.0, chars: ASCII },
    Spec { name: "REGULAR_21", file: "Roboto-Regular.ttf", px: 21.0, chars: ASCII },
    Spec { name: "BOLD_24", file: "Roboto-Bold.ttf", px: 24.0, chars: ASCII },
    Spec { name: "BOLD_72", file: "Roboto-Bold.ttf", px: 72.0, chars: " 0123456789.:-°C%" },
];

fn main() {
    let dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("fonts");
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=fonts");

    let mut blob: Vec<u8> = Vec::new();
    let mut src = String::new();
    for spec in SPECS {
        let data = fs::read(dir.join(spec.file)).unwrap();
        let font = fontdue::Font::from_bytes(data, fontdue::FontSettings::default()).unwrap();
        let lm = font.horizontal_line_metrics(spec.px).unwrap();
        writeln!(
            src,
            "pub static {}: Font = Font {{ px: {}, ascent: {}, descent: {}, glyphs: &[",
            spec.name,
            spec.px as u16,
            lm.ascent.round() as i16,
            (-lm.descent).round() as i16
        )
        .unwrap();
        let mut chars: Vec<char> = spec.chars.chars().collect();
        chars.sort_unstable();
        chars.dedup();
        for ch in chars {
            let (m, cov) = font.rasterize(ch, spec.px);
            assert!(m.width < 256 && m.height < 256);
            let (w, h) = (m.width, m.height);
            let offset = blob.len();
            // Panel-space mask: mask_w = h, mask_h = w.
            // mask(mx, my) = glyph(glx = my, gly = h - 1 - mx).
            let stride = h.div_ceil(2);
            for my in 0..w {
                let mut row = vec![0u8; stride];
                for mx in 0..h {
                    let c = cov[(h - 1 - mx) * w + my] as u32;
                    let a = ((c * 15 + 127) / 255) as u8;
                    row[mx / 2] |= a << (4 * (mx & 1));
                }
                blob.extend_from_slice(&row);
            }
            // `top`: logical rows from the baseline up to the bitmap's top row.
            writeln!(
                src,
                "    Glyph {{ ch: {:?}, advance_64: {}, left: {}, top: {}, w: {}, h: {}, offset: {} }},",
                ch,
                (m.advance_width * 64.0).round() as u16,
                m.xmin,
                m.ymin + h as i32,
                w,
                h,
                offset
            )
            .unwrap();
        }
        writeln!(src, "] }};").unwrap();
    }
    writeln!(src, "pub const ATLAS_LEN: usize = {};", blob.len()).unwrap();
    fs::write(out.join("atlas.bin"), &blob).unwrap();
    fs::write(out.join("atlas.rs"), src).unwrap();
}
