//! A four-tab oven-controller demo, authored in the public vocabulary. Used
//! by the host tests (one PNG + cost report per tab) and by the hardware spike,
//! so what is measured on the bench is what is priced on the host.

use crate::prelude::*;

pub const TABS: [&str; 4] = ["Home", "Profiles", "Log", "Settings"];
const PROFILES: [(&str, &str); 4] = [
    ("Cure cycle A", "120°C, 45 min soak"),
    ("Cure cycle B", "185°C, 7 steps, 2 h 10 min"),
    ("Post-cure", "80°C, 8 h"),
    ("Dry-out", "60°C, 30 min"),
];

/// Stand-in for the oven's state.
pub struct Demo {
    pub tick: u32,
    pub tab: usize,
    /// Index into [`PROFILES`] of the running profile.
    pub running: Option<usize>,
    pub setpoint: i32,
}

impl Demo {
    pub const fn new() -> Self {
        Demo { tick: 0, tab: 0, running: Some(1), setpoint: 185 }
    }

    pub fn on_event(&mut self, ev: UiEvent) {
        if let Some(i) = ev.click_index("tab") {
            self.tab = i;
        } else if let Some(i) = ev.click_index("run") {
            self.running = Some(i);
            self.tab = 0;
        } else if ev.is_click("start") {
            self.running = match self.running {
                Some(_) => None,
                None => Some(1),
            };
        } else if ev.is_click("sp-") {
            self.setpoint -= 5;
        } else if ev.is_click("sp+") {
            self.setpoint += 5;
        }
    }

    pub fn build(&self) -> El {
        let theme = crate::theme();
        let t = self.tick;
        let status = row([
            h1("OVEN 1"),
            spacer(),
            text(format_args!("12:{:02}:{:02}", (t / 600) % 60, (t / 10) % 60)).text_color(theme.muted),
        ])
        .fill(theme.bar)
        .height(Size::Fixed(48))
        .padding((16, 0))
        .align(Align::Center);

        let page = match self.tab {
            0 => self.home(),
            1 => self.profiles(),
            2 => self.log(),
            _ => self.settings(),
        };

        let tabs = row(TABS.into_iter().enumerate().map(|(i, name)| {
            let active = i == self.tab;
            column([
                row([])
                    .fill(if active { theme.accent } else { theme.bar })
                    .height(Size::Fixed(4))
                    .width(Size::Fixed(80)),
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

        column([status, page.fill_height(), tabs])
    }

    fn home(&self) -> El {
        let theme = crate::theme();
        let t = self.tick;
        let tenths = 1800 + (t % 100);
        let chamber = card([
            label("CHAMBER"),
            display(format_args!("{}.{}°C", tenths / 10, tenths % 10)).center_text(),
            text(format_args!("Setpoint {}.0°C    Ramp 2.0°C/min", self.setpoint))
                .text_color(theme.muted)
                .center_text(),
        ]);
        let profile = match self.running {
            Some(i) => card([
                h1(PROFILES[i].0),
                text("Step 3 of 7 - soak").text_color(theme.muted),
                progress(((t % 200) * 5) as u16),
                label(format_args!("01:{:02}:{:02} remaining", 12 - (t / 600) % 12, 59 - (t / 10) % 60)),
            ]),
            None => card([h1("Idle"), text("No profile running").text_color(theme.muted)]),
        };
        let tiles = row([
            card([
                label("HEATER"),
                h1(format_args!("{} %", if self.running.is_some() { 60 + (t / 5) % 30 } else { 0 })),
            ])
            .fill_width(),
            card([label("FAN"), h1("ON").text_color(theme.success)]).fill_width(),
        ])
        .gap(16);
        let action = if self.running.is_some() { "STOP PROFILE" } else { "START A PROFILE" };
        column([chamber, profile, tiles, button(action).key("start")]).padding(16).gap(16)
    }

    fn profiles(&self) -> El {
        let theme = crate::theme();
        column(PROFILES.into_iter().enumerate().map(|(i, (name, what))| {
            row([
                column([h1(name), text(what).text_color(theme.muted)]).gap(6).fill_width(),
                button("RUN").key(("run", i)).height(Size::Fixed(64)),
            ])
            .fill(theme.surface)
            .radius(theme.card_radius)
            .padding(theme.card_padding)
            .gap(12)
            .align(Align::Center)
        }))
        .padding(16)
        .gap(16)
    }

    /// A wall of body text: the rasterizer's text-heavy case.
    fn log(&self) -> El {
        const LINES: [&str; 4] = [
            "heater 72 %, chamber 183.4°C, sp 185",
            "profile step 3/7 soak: 00:41:12 remaining",
            "watlow: read pv ok (12 ms), write sp ok",
            "net: http GET /api/status 200 in 3 ms",
        ];
        column((0..23).map(|i| text(format_args!("{:02}:{:02} {}", 11 + i / 10, (i * 7) % 60, LINES[i % LINES.len()]))))
            .padding((8, 12))
            .gap(2)
    }

    fn settings(&self) -> El {
        let theme = crate::theme();
        let field = |name: &'static str, value: &'static str| {
            row([text(name).text_color(theme.muted), spacer(), text(value)])
        };
        column([
            card([
                label("SETPOINT"),
                row([
                    button("-").key("sp-"),
                    display(format_args!("{}", self.setpoint)).center_text().fill_width(),
                    button("+").key("sp+"),
                ])
                .gap(12)
                .align(Align::Center),
            ]),
            card([
                field("Units", "°C"),
                field("Controller", "Watlow EZ-ZONE, addr 1"),
                field("Network", "USB-Ethernet"),
                field("Firmware", "display-beam spike"),
            ]),
        ])
        .padding(16)
        .gap(16)
    }
}

impl Default for Demo {
    fn default() -> Self {
        Self::new()
    }
}
