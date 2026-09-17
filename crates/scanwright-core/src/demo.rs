//! Spike content: hand-built display lists approximating the oven UI, in
//! physical portrait pixels (480 x 800 on the WS-LCD43B).

use core::fmt::Write as _;

use crate::{
    font::{BOLD_24, BOLD_72, REGULAR_18, REGULAR_21},
    hex,
    list::{DisplayList, ListBuilder, ListUsage},
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scene {
    /// The oven home page: cards, a big readout, a button, a tab bar.
    Home,
    /// Stress: the whole screen covered in body text over a known flat
    /// background (LUT masks).
    DenseText,
    /// The same, blended over whatever is in the line (general masks).
    DenseBlend,
}

impl Scene {
    pub const ALL: [Scene; 3] = [Scene::Home, Scene::DenseText, Scene::DenseBlend];

    pub const fn name(self) -> &'static str {
        match self {
            Scene::Home => "home",
            Scene::DenseText => "dense-lut",
            Scene::DenseBlend => "dense-blend",
        }
    }
}

const BG: u16 = hex(0x151a1f);
const BAR: u16 = hex(0x10151a);
const CARD: u16 = hex(0x242c34);
const TRACK: u16 = hex(0x3c4a54);
const TEXT: u16 = hex(0xe8edf1);
const MUTED: u16 = hex(0x93a3ae);
const ACCENT: u16 = hex(0xf4650f);
const GREEN: u16 = hex(0x6fcf7f);
const DARK: u16 = hex(0x0f1317);

/// Tiny stack string for formatted numbers.
struct Buf {
    b: [u8; 48],
    n: usize,
}

impl Buf {
    fn new() -> Self {
        Self { b: [0; 48], n: 0 }
    }
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.b[..self.n]).unwrap_or("")
    }
}

impl core::fmt::Write for Buf {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let bytes = s.as_bytes();
        let end = (self.n + bytes.len()).min(self.b.len());
        self.b[self.n..end].copy_from_slice(&bytes[..end - self.n]);
        self.n = end;
        Ok(())
    }
}

macro_rules! fmt {
    ($($arg:tt)*) => {{
        let mut b = Buf::new();
        let _ = write!(b, $($arg)*);
        b
    }};
}

/// Build `scene` into `list`. `tick` animates the dynamic parts.
pub fn build<const I: usize, const G: usize>(
    list: &mut DisplayList<I, G>,
    panel_w: u16,
    panel_h: u16,
    scene: Scene,
    tick: u32,
) -> ListUsage {
    let mut b = list.begin(panel_w, panel_h, BG);
    match scene {
        Scene::Home => home(&mut b, tick),
        Scene::DenseText => dense(&mut b, tick, Some(BG)),
        Scene::DenseBlend => dense(&mut b, tick, None),
    }
    b.finish()
}

fn home(l: &mut ListBuilder<'_>, tick: u32) {
    let (w, h) = l.logical_size();

    // Status bar.
    l.rect(0, 0, w, 48, BAR);
    l.text(&BOLD_24, 16, 33, "OVEN 1", TEXT, Some(BAR));
    let clock = fmt!("{:02}:{:02}:{:02}", 12, (tick / 600) % 60, (tick / 10) % 60);
    let cw = REGULAR_21.measure(clock.as_str());
    l.text(&REGULAR_21, w - 16 - cw, 32, clock.as_str(), MUTED, Some(BAR));

    // Chamber temperature.
    l.rounded_rect(16, 64, w - 32, 230, 18, CARD, Some(BG));
    l.text(&REGULAR_18, 36, 98, "CHAMBER", MUTED, Some(CARD));
    let tenths = 1800 + (tick % 100);
    let temp = fmt!("{}.{}°C", tenths / 10, tenths % 10);
    l.text_centered(&BOLD_72, w / 2, 196, temp.as_str(), TEXT, Some(CARD));
    l.text_centered(&REGULAR_21, w / 2, 262, "Setpoint 185.0°C    Ramp 2.0°C/min", MUTED, Some(CARD));

    // Profile progress.
    l.rounded_rect(16, 310, w - 32, 150, 18, CARD, Some(BG));
    l.text(&BOLD_24, 36, 350, "Cure cycle B", TEXT, Some(CARD));
    l.text(&REGULAR_21, 36, 384, "Step 3 of 7 - soak at 185°C", MUTED, Some(CARD));
    let track_w = w - 72;
    l.rounded_rect(36, 404, track_w, 16, 8, TRACK, Some(CARD));
    let done = 16 + (track_w - 16) * (tick % 200) as i32 / 200;
    l.rounded_rect(36, 404, done, 16, 8, ACCENT, None);
    let rem = fmt!("{:02}:{:02}:{:02} remaining", 1, 12 - (tick / 600) % 12, 59 - (tick / 10) % 60);
    l.text(&REGULAR_18, 36, 446, rem.as_str(), MUTED, Some(CARD));

    // Two half cards.
    let half = (w - 48) / 2;
    l.rounded_rect(16, 476, half, 120, 18, CARD, Some(BG));
    l.text(&REGULAR_18, 36, 510, "HEATER", MUTED, Some(CARD));
    let duty = fmt!("{} %", 60 + (tick / 5) % 30);
    l.text(&BOLD_24, 36, 560, duty.as_str(), TEXT, Some(CARD));
    l.rounded_rect(32 + half, 476, half, 120, 18, CARD, Some(BG));
    l.text(&REGULAR_18, 52 + half, 510, "FAN", MUTED, Some(CARD));
    l.text(&BOLD_24, 52 + half, 560, "ON", GREEN, Some(CARD));

    // Primary button.
    l.rounded_rect(16, 612, w - 32, 80, 14, ACCENT, Some(BG));
    l.text_centered(&BOLD_24, w / 2, 661, "START A PROFILE", DARK, Some(ACCENT));

    // Tab bar.
    l.rect(0, h - 80, w, 80, BAR);
    let tabs = ["Home", "Profiles", "Log", "Settings"];
    let tw = w / tabs.len() as i32;
    for (i, name) in tabs.iter().enumerate() {
        let cx = tw * i as i32 + tw / 2;
        let active = i == 0;
        l.text_centered(&REGULAR_18, cx, h - 34, name, if active { ACCENT } else { MUTED }, Some(BAR));
        if active {
            l.rect(cx - 40, h - 80, 80, 4, ACCENT);
        }
    }
}

fn dense(l: &mut ListBuilder<'_>, tick: u32, bg: Option<u16>) {
    let (w, h) = l.logical_size();
    l.rect(0, 0, w, 48, BAR);
    let title = fmt!("DENSE TEXT  frame {}", tick);
    l.text(&BOLD_24, 16, 33, title.as_str(), TEXT, Some(BAR));
    const LINES: [&str; 4] = [
        "The quick brown fox jumps over the lazy dog 0123",
        "Pack my box with five dozen liquor jugs; 456789",
        "Sphinx of black quartz, judge my vow! (185.0°C)",
        "How vexingly quick daft zebras jump: 12:34:56 %",
    ];
    let mut y = 76;
    let mut i = 0usize;
    while y < h - 8 {
        l.text(&REGULAR_21, 8, y, LINES[i % LINES.len()], TEXT, bg);
        y += 25;
        i += 1;
    }
}
