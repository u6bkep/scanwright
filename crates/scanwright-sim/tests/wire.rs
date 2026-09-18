//! Encode a built list, decode it as a capture, rasterize both: identical.

use scanwright_core::{demo, font, list::DisplayList, raster::Raster, wire};
use scanwright_sim::{Capture, Panel};

#[test]
fn capture_round_trips_pixel_exact() {
    let p = Panel::WS_LCD43B;
    let mut list = Box::new(DisplayList::<512, 1536>::new(font::BUILTIN));
    demo::build(&mut list, p.width, p.height, demo::Scene::Home, 5);
    let view = list.view();
    let mut buf = vec![0u8; wire::encoded_len(&view)];
    let n = wire::encode(&view, 42, &mut buf).unwrap();
    assert_eq!(n, buf.len());
    assert_eq!(wire::encode(&view, 42, &mut buf[..10]), Err(n));

    let cap = Capture::decode(&buf, font::BUILTIN).unwrap();
    assert_eq!(cap.header.seq, 42);
    assert_eq!(cap.header.total_len(), n);
    let got = cap.rasterize(font::BUILTIN);

    let (w, h) = (usize::from(p.width), usize::from(p.height));
    let mut want = vec![0u16; w * h];
    let mut r = Raster::new();
    r.begin_frame();
    for y in 0..h {
        unsafe { r.line(&view, y as u16, want[y * w..].as_mut_ptr(), true) };
    }
    assert!(got == want, "capture rasterizes differently");
    assert!(cap.report(font::BUILTIN, &p).fits());

    // Wrong fonts are refused.
    let other = font::FontSet { atlas: &font::ATLAS[1..], glyphs: font::BUILTIN.glyphs };
    assert!(Capture::decode(&buf, other).is_err());
}
