//! Wire format for a sealed display list: what a device sends to a host so
//! the host can rasterize *exactly* what the panel shows (remote screenshots,
//! cost-model verification against measured line times).
//!
//! Layout, little-endian, no padding:
//!
//! ```text
//! "SWL1"  u32 seq  u16 panel_w  u16 panel_h  u16 n_items  u16 n_glyphs
//! u8 n_luts  u8 _  u16 pool_len  u32 atlas_len  u32 atlas_hash
//! items   n_items x 20 B  (x0 x1 y0 y1 op pool color a n mx0 my0)
//! order   n_items x u16
//! glyphs  n_glyphs x 12 B (x0 y0 mask_w mask_h _ offset)
//! luts    n_luts x 32 B
//! pool    pool_len B
//! ```
//!
//! The atlas is not carried: both sides link the same baked fonts, and the
//! header's `atlas_len`/`atlas_hash` let the decoder refuse a mismatch. `seq`
//! is the sender's publish counter so a capture fetched in several pieces can
//! be checked for tearing.

use crate::list::{GlyphRef, Item, ListView};

pub const MAGIC: &[u8; 4] = b"SWL1";
pub const HEADER_LEN: usize = 4 + 4 + 2 + 2 + 2 + 2 + 1 + 1 + 2 + 4 + 4;
pub const ITEM_LEN: usize = 20;
pub const GLYPH_LEN: usize = 12;
pub const LUT_LEN: usize = 32;

/// FNV-1a over the atlas bytes: the identity of the baked fonts.
pub fn atlas_hash(atlas: &[u8]) -> u32 {
    atlas.iter().fold(0x811c_9dc5u32, |h, &b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193))
}

pub fn encoded_len(view: &ListView<'_>) -> usize {
    HEADER_LEN
        + view.items.len() * ITEM_LEN
        + view.order.len() * 2
        + view.glyphs.len() * GLYPH_LEN
        + view.luts.len() * LUT_LEN
        + view.pool.len()
}

/// Serialize `view` into `out`. Returns the bytes written, or `Err(needed)`
/// when `out` is too small.
pub fn encode(view: &ListView<'_>, seq: u32, out: &mut [u8]) -> Result<usize, usize> {
    let need = encoded_len(view);
    if out.len() < need {
        return Err(need);
    }
    let mut w = Writer { buf: out, at: 0 };
    w.bytes(MAGIC);
    w.u32(seq);
    w.u16(view.panel_w);
    w.u16(view.panel_h);
    w.u16(view.items.len() as u16);
    w.u16(view.glyphs.len() as u16);
    w.u8(view.luts.len() as u8);
    w.u8(0);
    w.u16(view.pool.len() as u16);
    w.u32(view.atlas.len() as u32);
    w.u32(atlas_hash(view.atlas));
    for it in view.items {
        w.u16(it.x0);
        w.u16(it.x1);
        w.u16(it.y0);
        w.u16(it.y1);
        w.u8(it.op);
        w.u8(it.pool);
        w.u16(it.color);
        w.u32(it.a);
        w.u16(it.n);
        w.u8(it.mx0);
        w.u8(it.my0);
    }
    for &o in view.order {
        w.u16(o);
    }
    for g in view.glyphs {
        w.u16(g.x0);
        w.u16(g.y0);
        w.u8(g.mask_w);
        w.u8(g.mask_h);
        w.u16(0);
        w.u32(g.offset);
    }
    for lut in view.luts {
        for &c in lut {
            w.u16(c);
        }
    }
    w.bytes(view.pool);
    Ok(w.at)
}

struct Writer<'a> {
    buf: &'a mut [u8],
    at: usize,
}

impl Writer<'_> {
    fn bytes(&mut self, b: &[u8]) {
        self.buf[self.at..self.at + b.len()].copy_from_slice(b);
        self.at += b.len();
    }
    fn u8(&mut self, v: u8) {
        self.bytes(&[v]);
    }
    fn u16(&mut self, v: u16) {
        self.bytes(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.bytes(&v.to_le_bytes());
    }
}

/// The fixed-size part of a capture, readable without the rest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub seq: u32,
    pub panel_w: u16,
    pub panel_h: u16,
    pub n_items: u16,
    pub n_glyphs: u16,
    pub n_luts: u8,
    pub pool_len: u16,
    pub atlas_len: u32,
    pub atlas_hash: u32,
}

impl Header {
    pub fn parse(b: &[u8]) -> Option<Header> {
        if b.len() < HEADER_LEN || &b[..4] != MAGIC {
            return None;
        }
        let r = Reader { buf: b, at: 4 };
        let mut r = r;
        Some(Header {
            seq: r.u32(),
            panel_w: r.u16(),
            panel_h: r.u16(),
            n_items: r.u16(),
            n_glyphs: r.u16(),
            n_luts: {
                let n = r.u8();
                r.u8();
                n
            },
            pool_len: r.u16(),
            atlas_len: r.u32(),
            atlas_hash: r.u32(),
        })
    }

    /// Total encoded length of the capture this header begins.
    pub fn total_len(&self) -> usize {
        HEADER_LEN
            + usize::from(self.n_items) * (ITEM_LEN + 2)
            + usize::from(self.n_glyphs) * GLYPH_LEN
            + usize::from(self.n_luts) * LUT_LEN
            + usize::from(self.pool_len)
    }
}

/// Decode into caller-provided storage (no allocation). Each slice must hold
/// at least the header's count. Returns the header and the filled prefixes.
#[allow(clippy::type_complexity)]
pub fn decode_into<'a>(
    b: &[u8],
    items: &'a mut [Item],
    order: &'a mut [u16],
    glyphs: &'a mut [GlyphRef],
    luts: &'a mut [[u16; 16]],
    pool: &'a mut [u8],
) -> Option<(Header, &'a [Item], &'a [u16], &'a [GlyphRef], &'a [[u16; 16]], &'a [u8])> {
    let h = Header::parse(b)?;
    if b.len() < h.total_len()
        || items.len() < usize::from(h.n_items)
        || order.len() < usize::from(h.n_items)
        || glyphs.len() < usize::from(h.n_glyphs)
        || luts.len() < usize::from(h.n_luts)
        || pool.len() < usize::from(h.pool_len)
    {
        return None;
    }
    let mut r = Reader { buf: b, at: HEADER_LEN };
    for it in items[..usize::from(h.n_items)].iter_mut() {
        *it = Item {
            x0: r.u16(),
            x1: r.u16(),
            y0: r.u16(),
            y1: r.u16(),
            op: r.u8(),
            pool: r.u8(),
            color: r.u16(),
            a: r.u32(),
            n: r.u16(),
            mx0: r.u8(),
            my0: r.u8(),
        };
    }
    for o in order[..usize::from(h.n_items)].iter_mut() {
        *o = r.u16();
    }
    for g in glyphs[..usize::from(h.n_glyphs)].iter_mut() {
        let (x0, y0, mw, mh) = (r.u16(), r.u16(), r.u8(), r.u8());
        r.u16();
        *g = GlyphRef::new(x0, y0, mw, mh, r.u32());
    }
    for lut in luts[..usize::from(h.n_luts)].iter_mut() {
        for c in lut.iter_mut() {
            *c = r.u16();
        }
    }
    let pl = usize::from(h.pool_len);
    pool[..pl].copy_from_slice(&b[r.at..r.at + pl]);
    Some((
        h,
        &items[..usize::from(h.n_items)],
        &order[..usize::from(h.n_items)],
        &glyphs[..usize::from(h.n_glyphs)],
        &luts[..usize::from(h.n_luts)],
        &pool[..pl],
    ))
}

struct Reader<'a> {
    buf: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn u8(&mut self) -> u8 {
        let v = self.buf[self.at];
        self.at += 1;
        v
    }
    fn u16(&mut self) -> u16 {
        let v = u16::from_le_bytes([self.buf[self.at], self.buf[self.at + 1]]);
        self.at += 2;
        v
    }
    fn u32(&mut self) -> u32 {
        let v = u32::from_le_bytes(self.buf[self.at..self.at + 4].try_into().unwrap());
        self.at += 4;
        v
    }
}
