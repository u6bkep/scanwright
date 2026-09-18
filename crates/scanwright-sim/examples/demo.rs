//! The four-tab oven demo in a window: `cargo run --release -p scanwright-sim --example demo`.

use scanwright_core::{font, list::DisplayList};
use scanwright_sim::{App, Panel, Touch, Viewer};
use scanwright_ui::{demo::Demo, prelude::*};

struct DemoApp {
    ui: Box<Ui<256, 2048, 32>>,
    list: Box<DisplayList<256, 1280>>,
    demo: Demo,
    panel: Panel,
}

impl App for DemoApp {
    fn touch(&mut self, t: Touch) -> bool {
        let r = self.ui.touch(t);
        if let Some(ev) = r.event {
            self.demo.on_event(ev);
        }
        r.redraw || r.event.is_some()
    }

    fn tick(&mut self) -> bool {
        self.demo.tick = self.demo.tick.wrapping_add(1);
        true
    }

    fn build(&mut self) -> scanwright_core::list::ListView<'_> {
        let report = self.ui.rebuild(&mut self.list, self.panel.width, self.panel.height, || self.demo.build());
        if !report.complete() {
            eprintln!("capacity exceeded: {report:?}");
        }
        self.list.view()
    }
}

fn main() {
    let panel = Panel::WS_LCD43B;
    let app = DemoApp {
        ui: Box::new(Ui::new(Theme::DARK)),
        list: Box::new(DisplayList::new(font::BUILTIN)),
        demo: Demo::new(),
        panel,
    };
    Viewer::new("scanwright demo", panel, app).run();
}
