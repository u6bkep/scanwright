//! The oven home page authored through the El vocabulary: rendered on the
//! host (-> `target/ui-oven-home.png`), priced by the cost model, and poked
//! with touches.

use scanwright_core::{
    cost::{self, CostModel},
    font,
    list::DisplayList,
    raster::Raster,
};
use scanwright_ui::prelude::*;

const W: usize = 800;
const H: usize = 480;

type List = DisplayList<256, 1024>;
type OvenUi = Ui<256, 2048, 32>;

/// WS-LCD43B scan-out (see scanwright-core's render test).
const SCANOUT: cost::Scanout = cost::Scanout {
    budget_cycles: cost::line_budget_cycles(264_000_000, 37_273),
    ring_lines: 16,
    vblank_lines: 20,
};

struct Oven {
    temp_tenths: u32,
    progress_permille: u16,
    heater_percent: u8,
    fan_on: bool,
    tab: usize,
}

impl Oven {
    fn build(&self) -> El {
        let theme = scanwright_ui::theme();
        let status = row([h1("OVEN 1"), spacer(), text("12:00:12").text_color(theme.muted)])
            .fill(theme.bar)
            .height(Size::Fixed(48))
            .padding((16, 0))
            .align(Align::Center);

        let chamber = card([
            label("CHAMBER"),
            display(format_args!("{}.{}°C", self.temp_tenths / 10, self.temp_tenths % 10)).center_text(),
            text("Setpoint 185.0°C    Ramp 2.0°C/min").text_color(theme.muted).center_text(),
        ]);

        let profile = card([
            h1("Cure cycle B"),
            text("Step 3 of 7 - soak at 185°C").text_color(theme.muted),
            progress(self.progress_permille),
            label("01:12:47 remaining"),
        ]);

        let tiles = row([
            card([label("HEATER"), h1(format_args!("{} %", self.heater_percent))]).fill_width(),
            card([
                label("FAN"),
                h1(if self.fan_on { "ON" } else { "OFF" }).text_color(if self.fan_on { theme.success } else { theme.muted }),
            ])
            .fill_width(),
        ])
        .gap(16);

        let tabs = row(["Home", "Profiles", "Log", "Settings"].into_iter().enumerate().map(|(i, name)| {
            let active = i == self.tab;
            column([
                row([]).fill(if active { theme.accent } else { theme.bar }).height(Size::Fixed(4)).width(Size::Fixed(80)),
                spacer(),
                label(name).text_color(if active { theme.accent } else { theme.muted }).center_text(),
                spacer(),
            ])
            .align(Align::Center)
            .fill_width()
            .key(("tab", i))
        }))
        .fill(theme.bar)
        .height(Size::Fixed(80));

        column([
            status,
            column([chamber, profile, tiles, button("START A PROFILE").key("start")]).padding(16).gap(16),
            spacer(),
            tabs,
        ])
    }
}

fn render<const I: usize, const G: usize>(list: &DisplayList<I, G>) -> Vec<u16> {
    let mut fb = vec![0xF81Fu16; W * H];
    let mut raster = Raster::new();
    raster.begin_frame();
    for y in 0..H {
        unsafe { raster.line(&list.view(), y as u16, fb[y * W..].as_mut_ptr(), true) };
    }
    assert_eq!(raster.overflows(), 0);
    fb
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

fn oven() -> Oven {
    Oven { temp_tenths: 1823, progress_permille: 630, heater_percent: 84, fan_on: true, tab: 0 }
}

#[test]
fn oven_home_page() {
    let app = oven();
    let mut ui = Box::new(OvenUi::new(Theme::DARK));
    let mut list = Box::new(List::new(font::BUILTIN));
    let report = ui.rebuild(&mut list, W as u16, H as u16, || app.build());
    println!("{report:?}");
    println!("Ui {} B, list {} B", std::mem::size_of::<OvenUi>(), std::mem::size_of::<List>());
    assert!(report.complete());

    let fb = render(&list);
    assert!(fb.iter().all(|&p| p != 0xF81F), "unwritten pixels");
    save("ui-oven-home.png", &fb);

    let cost = cost::analyze(&list.view(), &CostModel::CORTEX_M33, &SCANOUT);
    println!(
        "predicted: worst line {} = {} cyc (budget {}), {} % of a core, {} late lines",
        cost.worst_line,
        cost.worst_line_cycles,
        SCANOUT.budget_cycles,
        cost.core_percent(&SCANOUT, H as u16),
        cost.late_lines
    );
    assert!(cost.fits());
}

#[test]
fn touch_clicks_and_press_feedback() {
    let app = oven();
    let mut ui = Box::new(OvenUi::new(Theme::DARK));
    let mut list = Box::new(List::new(font::BUILTIN));
    ui.rebuild(&mut list, W as u16, H as u16, || app.build());
    let idle = render(&list);

    // Press the button: state changes, the rebuild shows it darker.
    let r = ui.touch(Touch::Down(240, 590));
    assert_eq!(r, Response { event: None, redraw: true });
    ui.rebuild(&mut list, W as u16, H as u16, || app.build());
    assert_ne!(render(&list), idle, "pressed button should look different");
    let r = ui.touch(Touch::Up);
    assert!(r.event.unwrap().is_click("start"));
    assert!(r.redraw);

    // Tabs are indexed keys.
    ui.touch(Touch::Down(300, 770));
    assert_eq!(ui.touch(Touch::Up).event.unwrap().click_index("tab"), Some(2));

    // Sliding off cancels; dead space does nothing.
    ui.touch(Touch::Down(240, 590));
    ui.touch(Touch::Move(240, 100));
    assert_eq!(ui.touch(Touch::Up).event, None);
    assert_eq!(ui.touch(Touch::Down(5, 705)), Response::default());
}

#[test]
fn undersized_capacities_are_reported_not_fatal() {
    let app = oven();

    // Too few nodes: elements are built children-first, so it is the *outer*
    // containers that fail to allocate — the screen is lost, loudly.
    let mut ui = Box::new(Ui::<12, 2048, 32>::new(Theme::DARK));
    let mut list = Box::new(List::new(font::BUILTIN));
    let report = ui.rebuild(&mut list, W as u16, H as u16, || app.build());
    assert!(!report.complete() && report.dropped_nodes > 0);
    render(&list);

    // Too little text: strings truncate.
    let mut ui = Box::new(Ui::<256, 64, 32>::new(Theme::DARK));
    let report = ui.rebuild(&mut list, W as u16, H as u16, || app.build());
    assert!(!report.complete() && report.dropped_text_bytes > 0 && report.dropped_nodes == 0);
    render(&list);

    // Too few hit slots: later touch targets are dead.
    let mut ui = Box::new(Ui::<256, 2048, 2>::new(Theme::DARK));
    let report = ui.rebuild(&mut list, W as u16, H as u16, || app.build());
    assert!(!report.complete() && report.dropped_hits == 3);

    // Too small a display list: primitives are dropped.
    let mut ui = Box::new(OvenUi::new(Theme::DARK));
    let mut small = Box::new(DisplayList::<16, 32>::new(font::BUILTIN));
    let report = ui.rebuild(&mut small, W as u16, H as u16, || app.build());
    assert!(!report.complete() && report.list.dropped > 0);
    render(&small);
}

#[test]
#[should_panic(expected = "inside Ui::rebuild")]
fn building_outside_rebuild_is_refused() {
    let _ = text("stray");
}

#[test]
fn demo_tabs_render_and_fit() {
    use scanwright_ui::demo::{Demo, TABS};
    let mut ui = Box::new(OvenUi::new(Theme::DARK));
    let mut list = Box::new(DisplayList::<256, 1280>::new(font::BUILTIN));
    let mut app = Demo::new();
    app.tick = 123;
    for (tab, name) in TABS.iter().enumerate() {
        app.tab = tab;
        let report = ui.rebuild(&mut list, W as u16, H as u16, || app.build());
        assert!(report.complete(), "{name}: {report:?}");
        let fb = render(&list);
        assert!(fb.iter().all(|&p| p != 0xF81F));
        save(&format!("demo-{}.png", name.to_lowercase()), &fb);
        let cost = cost::analyze(&list.view(), &CostModel::CORTEX_M33, &SCANOUT);
        println!(
            "demo[{name}]: {} nodes, {} text B, {} hits, {} items + {} glyphs; predicted worst line {} cyc, {} % of a core, {} late",
            report.nodes,
            report.text_bytes,
            report.hits,
            report.list.items,
            report.list.glyphs,
            cost.worst_line_cycles,
            cost.core_percent(&SCANOUT, H as u16),
            cost.late_lines
        );
        assert!(cost.fits(), "{name}");
    }

    // The flow the bench exercises: Profiles -> RUN #2 -> lands on Home, running.
    app.tab = 1;
    ui.rebuild(&mut list, W as u16, H as u16, || app.build());
    ui.touch(Touch::Down(420, 330));
    let ev = ui.touch(Touch::Up).event.expect("RUN button under the finger");
    app.on_event(ev);
    assert_eq!((app.tab, app.running), (0, Some(2)));
}
