//! The viewer without a window: rebuild, rasterize, price. What CI runs.

use scanwright_core::{font, list::DisplayList};
use scanwright_sim::{App, Panel, Touch, Viewer};
use scanwright_ui::{demo::Demo, prelude::*};

struct DemoApp {
    ui: Box<Ui<256, 2048, 32>>,
    list: Box<DisplayList<256, 1280>>,
    demo: Demo,
}

impl App for DemoApp {
    fn touch(&mut self, t: Touch) -> bool {
        let r = self.ui.touch(t);
        if let Some(ev) = r.event {
            self.demo.on_event(ev);
        }
        r.redraw || r.event.is_some()
    }

    fn build(&mut self) -> scanwright_core::list::ListView<'_> {
        let p = Panel::WS_LCD43B;
        let report = self.ui.rebuild(&mut self.list, p.width, p.height, || self.demo.build());
        assert!(report.complete(), "{report:?}");
        self.list.view()
    }
}

#[test]
fn renders_and_prices_every_tab() {
    let app = DemoApp {
        ui: Box::new(Ui::new(Theme::DARK)),
        list: Box::new(DisplayList::new(font::BUILTIN)),
        demo: Demo::new(),
    };
    let mut v = Viewer::new("headless", Panel::WS_LCD43B, app);
    for tab in 0..4 {
        // Tap the tab in the bar (bottom 80 px, four equal columns).
        let x = 60 + 120 * tab as i16;
        v.app_mut().touch(Touch::Down(x, 760));
        v.app_mut().touch(Touch::Up);
        let s = v.render();
        println!("tab {tab}: {:?}", s.report);
        assert!(s.report.fits(), "tab {tab} predicted late");
        assert!(s.items > 10);
        let (w, h, rgb) = v.portrait_rgb();
        assert_eq!((w, h), (480, 800));
        assert!(rgb.iter().any(|&b| b != 0));
    }
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
    std::fs::create_dir_all(&dir).unwrap();
    v.save_png(dir.join("sim-headless.png")).unwrap();
}
