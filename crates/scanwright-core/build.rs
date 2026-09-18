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
    /// Primary face, then fallbacks for characters it lacks (symbols).
    files: &'static [&'static str],
    px: f32,
    chars: &'static str,
}

const ASCII: &str = " !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~°";
/// Punctuation and UI symbols the text faces carry beyond ASCII. Roboto has
/// the first few; the arrows, check, pencil, backspace and shift come from
/// DejaVu Sans at the same size.
const SYMBOLS: &str = "±—·…‹›✓✎▲▼⌫⇧●";
const TEXT: &[&str] = &[ASCII, SYMBOLS];
const NUMERIC: &[&str] = &[" 0123456789.:-—°CF%"];

const REGULAR: &[&str] = &["Roboto-Regular.ttf", "DejaVuSans.ttf"];
const BOLD: &[&str] = &["Roboto-Bold.ttf", "DejaVuSans-Bold.ttf"];

/// The type scale: four roles, six faces (ruling 2026-09-17, see
/// docs/DESIGN.md). Sizes are physical pixels on a 480 px wide portrait panel.
const SPECS: &[Spec] = &[
    Spec { name: "CAPTION", files: REGULAR, px: 18.0, chars: "TEXT" },
    Spec { name: "CAPTION_BOLD", files: BOLD, px: 18.0, chars: "TEXT" },
    Spec { name: "BODY", files: REGULAR, px: 22.0, chars: "TEXT" },
    Spec { name: "BODY_BOLD", files: BOLD, px: 22.0, chars: "TEXT" },
    Spec { name: "TITLE", files: BOLD, px: 27.0, chars: "TEXT" },
    Spec { name: "DISPLAY", files: BOLD, px: 81.0, chars: "NUMERIC" },
];

fn charset(name: &str) -> Vec<char> {
    let sets = match name {
        "TEXT" => TEXT,
        "NUMERIC" => NUMERIC,
        _ => unreachable!(),
    };
    let mut chars: Vec<char> = sets.iter().flat_map(|s| s.chars()).collect();
    chars.sort_unstable();
    chars.dedup();
    chars
}

fn main() {
    let dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("fonts");
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=fonts");

    let mut blob: Vec<u8> = Vec::new();
    let mut src = String::new();
    // Real-time glyph table (mask size + atlas offset), indexed globally
    // across all fonts; display-list text runs refer to glyphs by this index.
    let mut infos = String::new();
    let mut n_glyphs = 0usize;
    let mut sizes = String::new();
    for spec in SPECS {
        let faces: Vec<fontdue::Font> = spec
            .files
            .iter()
            .map(|f| {
                let data = fs::read(dir.join(f)).unwrap();
                fontdue::Font::from_bytes(data, fontdue::FontSettings::default()).unwrap()
            })
            .collect();
        // Line metrics come from the primary face; fallbacks only lend glyphs.
        let lm = faces[0].horizontal_line_metrics(spec.px).unwrap();
        let start = blob.len();
        writeln!(
            src,
            "pub static {}: Font = Font {{ px: {}, ascent: {}, descent: {}, glyphs: &[",
            spec.name,
            spec.px as u16,
            lm.ascent.round() as i16,
            (-lm.descent).round() as i16
        )
        .unwrap();
        for ch in charset(spec.chars) {
            let font = faces
                .iter()
                .find(|f| f.lookup_glyph_index(ch) != 0)
                .unwrap_or_else(|| panic!("{}: no face has {ch:?}", spec.name));
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
                "    Glyph {{ ch: {:?}, advance_64: {}, left: {}, top: {}, w: {}, h: {}, index: {} }},",
                ch,
                (m.advance_width * 64.0).round() as u16,
                m.xmin,
                m.ymin + h as i32,
                w,
                h,
                n_glyphs
            )
            .unwrap();
            writeln!(infos, "    GlyphInfo {{ mask_w: {h}, mask_h: {w}, offset: {offset} }},").unwrap();
            n_glyphs += 1;
        }
        writeln!(src, "] }};").unwrap();
        writeln!(sizes, "//! * `{}`: {} px, {} glyphs, {} B of atlas", spec.name, spec.px, charset(spec.chars).len(), blob.len() - start).unwrap();
    }
    fs::write(out.join("atlas-sizes.txt"), &sizes).unwrap();
    writeln!(src, "pub const ATLAS_LEN: usize = {};", blob.len()).unwrap();
    writeln!(src, "pub const GLYPH_COUNT: usize = {n_glyphs};").unwrap();
    writeln!(src, "const GLYPH_INFO_INIT: [GlyphInfo; GLYPH_COUNT] = [\n{infos}];").unwrap();
    fs::write(out.join("atlas.bin"), &blob).unwrap();
    fs::write(out.join("atlas.rs"), src).unwrap();
}
