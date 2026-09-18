//! Borders, multiline + tracked text, clipped labels, scroll viewports with
//! drag, and a modal scrim — rendered (`target/ui-widgets-*.png`), priced,
//! and poked.

use scanwright_core::{
    cost::{self, CostModel},
    font,
    list::DisplayList,
    raster::Raster,
};
use scanwright_ui::prelude::*;

const W: usize = 800;
const H: usize = 480;
type List = DisplayList<512, 2048>;
type TestUi = Ui<512, 4096, 64>;

const SCANOUT: cost::Scanout = cost::Scanout {
    budget_cycles: cost::line_budget_cycles(264_000_000, 37_273),
    ring_lines: 16,
    vblank_lines: 20,
};

fn page(sheet: bool) -> El {
    let theme = scanwright_ui::theme();
    let head = column([
        label("SECTION HEAD").tracking(2).text_color(theme.muted),
        text("Two lines of copy,\nbroken by hand.").text_color(theme.text),
        text("A name far too long for the room it has been given here").fill_width(),
    ])
    .gap(6)
    .fill(theme.surface)
    .radius(12)
    .border(2, theme.accent)
    .padding(16);
    let rows = scroll((0..20).map(|i| {
        row([
            column([h1(format_args!("Profile {i}")), text("185°C, 7 steps").text_color(theme.muted)]).gap(4).fill_width(),
            button("RUN").key(("run", i)).height(Size::Fixed(56)),
        ])
        .fill(theme.surface)
        .radius(12)
        .padding(12)
        .gap(12)
        .align(Align::Center)
    }))
    .gap(8)
    .key("list")
    .fill_height();
    let body = column([head, rows]).padding(16).gap(16).fill_size();
    if !sheet {
        return body;
    }
    let sheet = column([
        h1("Stop the run?"),
        text("The oven keeps its set point\nuntil you start something else.").text_color(theme.muted),
        row([button("BACK").key("back").fill_width(), button("STOP").key("stop").fill_width()]).gap(8),
    ])
    .gap(12)
    .padding(16)
    .fill(theme.bar)
    .radius(21);
    stack([body, column([spacer(), sheet]).scrim().key("dismiss").fill_size()])
}

fn render(ui: &mut TestUi, list: &mut List, sheet: bool) -> (Vec<u16>, BuildReport) {
    let report = ui.rebuild(list, W as u16, H as u16, || page(sheet));
    let mut fb = vec![0xF81Fu16; W * H];
    let mut raster = Raster::new();
    raster.begin_frame();
    for y in 0..H {
        unsafe { raster.line(&list.view(), y as u16, fb[y * W..].as_mut_ptr(), true) };
    }
    assert_eq!(raster.overflows(), 0);
    (fb, report)
}

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

/// Portrait pixel (lx, ly) of a panel-space frame.
fn px(fb: &[u16], lx: usize, ly: usize) -> u16 {
    fb[lx * W + (W - 1 - ly)]
}

#[test]
fn widgets_render_fit_and_scroll() {
    let mut ui = Box::new(TestUi::new(Theme::DARK));
    let mut list = Box::new(List::new(font::BUILTIN));

    let (fb, report) = render(&mut ui, &mut list, false);
    save("ui-widgets.png", &fb);
    assert!(report.complete(), "{report:?}");
    let r = cost::analyze(&list.view(), &CostModel::CORTEX_M33, &SCANOUT);
    println!("widgets: {report:?}\n{r:?}");
    assert!(r.fits());
    assert!(fb.iter().all(|&p| p != 0xF81F), "unwritten pixels");

    // The clipped label never paints past its card's right padding edge.
    let theme = Theme::DARK;
    for ly in 16..200 {
        assert_eq!(px(&fb, 479, ly), theme.background, "row {ly} painted outside the card");
    }

    // A tap on the first row's button clicks it.
    let r1 = ui.touch(Touch::Down(420, 215));
    assert!(r1.redraw);
    let r2 = ui.touch(Touch::Up);
    assert!(r2.event.is_some_and(|e| e.click_index("run") == Some(0)), "{r2:?}");

    // A drag in the list scrolls it and cancels the press.
    assert_eq!(ui.scroll_offset("list"), 0);
    ui.touch(Touch::Down(240, 500));
    ui.touch(Touch::Move(240, 490)); // past the slop: now a drag
    let r = ui.touch(Touch::Move(240, 300));
    assert!(r.redraw);
    let r = ui.touch(Touch::Up);
    assert!(r.event.is_none(), "a drag must not click: {r:?}");
    let off = ui.scroll_offset("list");
    assert!(off > 150, "offset {off}");

    // After the rebuild the rows have moved up by the offset, and a button
    // now sitting above the viewport cannot be tapped.
    let (fb2, report) = render(&mut ui, &mut list, false);
    save("ui-widgets-scrolled.png", &fb2);
    assert!(report.complete());
    assert_eq!(ui.scroll_offset("list"), off, "clamp must not move a valid offset");
    let r = ui.touch(Touch::Down(420, 300));
    let r = ui.touch(Touch::Up).event.or(r.event);
    let idx = r.and_then(|e| e.click_index("run"));
    assert!(idx.is_some() && idx != Some(0), "expected a later row, got {idx:?}");

    // Dragging far past the end clamps.
    ui.set_scroll_offset("list", 30_000);
    let _ = render(&mut ui, &mut list, false);
    let max = ui.scroll_offset("list");
    assert!(max > off && max < 3000, "max {max}");
}

#[test]
fn scrim_dims_beneath_and_catches_taps() {
    let mut ui = Box::new(TestUi::new(Theme::DARK));
    let mut list = Box::new(List::new(font::BUILTIN));
    let (plain, _) = render(&mut ui, &mut list, false);
    let (dim, report) = render(&mut ui, &mut list, true);
    save("ui-widgets-sheet.png", &dim);
    assert!(report.complete(), "{report:?}");
    let r = cost::analyze(&list.view(), &CostModel::CORTEX_M33, &SCANOUT);
    println!("sheet: {r:?}");
    assert!(r.fits());

    // Under the scrim every pixel is darker than (or equal to) the plain frame.
    let lum = |p: u16| u32::from(p >> 11) * 2 + u32::from((p >> 5) & 0x3f) + u32::from(p & 0x1f) * 2;
    let (mut darker, mut same) = (0, 0);
    for ly in 0..500 {
        for lx in 0..480 {
            let (a, b) = (lum(px(&plain, lx, ly)), lum(px(&dim, lx, ly)));
            assert!(b <= a, "({lx}, {ly}) got brighter under the scrim");
            if b < a { darker += 1 } else { same += 1 }
        }
    }
    assert!(darker > 100_000, "darker {darker}, same {same}");

    // Taps outside the sheet dismiss; inside, the sheet's buttons win.
    ui.touch(Touch::Down(240, 200));
    assert!(ui.touch(Touch::Up).event.is_some_and(|e| e.is_click("dismiss")));
    ui.touch(Touch::Down(360, 760));
    assert!(ui.touch(Touch::Up).event.is_some_and(|e| e.is_click("stop")));
    // The list under the scrim does not scroll.
    ui.touch(Touch::Down(240, 400));
    ui.touch(Touch::Move(240, 200));
    ui.touch(Touch::Up);
    assert_eq!(ui.scroll_offset("list"), 0);
}
