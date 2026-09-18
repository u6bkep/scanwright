//! `scanwright-sim` — the target panel in a window on the host.
//!
//! The application is the same code that runs on the device: it owns a
//! [`DisplayList`](scanwright_core::list::DisplayList) and rebuilds it from
//! state. The viewer stands in for the hardware around it:
//!
//! * the **panel**: every frame is rasterized with the real
//!   [`Raster`](scanwright_core::raster::Raster), line by line, then rotated
//!   back to portrait for the window;
//! * the **touch controller**: mouse presses, drags and releases arrive as
//!   [`Touch`] in logical (UI) coordinates;
//! * the **clock**: [`App::tick`] runs at a fixed period for animation and
//!   polling;
//! * the **cost model**: every rebuild is priced against the panel's
//!   [`Scanout`] and the verdict goes in the title bar, so an author sees a
//!   screen that would not fit *while designing it*.
//!
//! Keys: `S` saves the frame as a PNG in the working directory, `Esc` quits.

use std::{
    num::NonZeroU32,
    rc::Rc,
    time::{Duration, Instant},
};

use scanwright_core::{
    cost::{self, CostModel},
    list::ListView,
    raster::Raster,
};
pub use scanwright_ui::input::Touch;
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::{ElementState, KeyEvent, MouseButton, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{Key, NamedKey},
    window::{Window, WindowId},
};

/// A panel and the scan-out timing the cost model prices against.
#[derive(Clone, Copy, Debug)]
pub struct Panel {
    /// Panel (scan-out) size; the UI is portrait, rotated 90° onto it.
    pub width: u16,
    pub height: u16,
    pub scanout: cost::Scanout,
    pub model: CostModel,
}

impl Panel {
    /// Waveshare RP2350-Touch-LCD-4.3B: 800x480 at 22 MHz pclk (820 clocks a
    /// line), RP2350 at 264 MHz, 16-line ring, 20 blanking lines.
    pub const WS_LCD43B: Panel = Panel {
        width: 800,
        height: 480,
        scanout: cost::Scanout {
            budget_cycles: cost::line_budget_cycles(264_000_000, 37_273),
            ring_lines: 16,
            vblank_lines: 20,
        },
        model: CostModel::CORTEX_M33,
    };

    /// Logical (portrait UI) size.
    pub const fn logical(&self) -> (u16, u16) {
        (self.height, self.width)
    }
}

/// What the viewer drives.
pub trait App {
    /// Touch input in logical coordinates. Return `true` if the screen must be
    /// rebuilt.
    fn touch(&mut self, touch: Touch) -> bool;

    /// Called every [`Viewer::tick`]. Return `true` if the screen must be rebuilt.
    fn tick(&mut self) -> bool {
        false
    }

    /// Rebuild the display list and hand back its view. Called only when
    /// something reported dirty, and once at start.
    fn build(&mut self) -> ListView<'_>;
}

/// The verdict on the last rebuild.
#[derive(Clone, Copy, Debug)]
pub struct Stats {
    pub report: cost::Report,
    pub items: usize,
    pub glyphs: usize,
    /// Host time spent in `App::build`.
    pub build: Duration,
    /// Host time spent rasterizing the frame.
    pub raster: Duration,
}

pub struct Viewer<A: App> {
    panel: Panel,
    title: String,
    /// `App::tick` period.
    pub tick: Duration,
    app: A,
    // Panel-space RGB565 frame.
    fb: Vec<u16>,
    raster: Raster,
    dirty: bool,
    pressed: bool,
    cursor: (f64, f64),
    last_stats: Option<Stats>,
    frame_no: u32,
    next_tick: Instant,
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
}

impl<A: App> Viewer<A> {
    pub fn new(title: &str, panel: Panel, app: A) -> Self {
        let n = usize::from(panel.width) * usize::from(panel.height);
        Viewer {
            panel,
            title: title.to_owned(),
            tick: Duration::from_millis(100),
            app,
            fb: vec![0; n],
            raster: Raster::new(),
            dirty: true,
            pressed: false,
            cursor: (0.0, 0.0),
            last_stats: None,
            frame_no: 0,
            next_tick: Instant::now(),
            window: None,
            surface: None,
        }
    }

    /// Run until the window closes. Returns the app.
    pub fn run(mut self) -> A {
        let event_loop = EventLoop::new().expect("event loop");
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_tick));
        event_loop.run_app(&mut self).expect("event loop");
        self.app
    }

    /// Rebuild, rasterize and price one frame (no window needed): what the
    /// device would show and what it would cost.
    pub fn render(&mut self) -> Stats {
        let t0 = Instant::now();
        let view = self.app.build();
        let build = t0.elapsed();
        let t1 = Instant::now();
        let w = usize::from(self.panel.width);
        self.raster.begin_frame();
        for y in 0..self.panel.height {
            let out = self.fb[usize::from(y) * w..].as_mut_ptr();
            // Safety: `out` has `panel.width` pixels; the view was built for this panel.
            unsafe { self.raster.line(&view, y, out, true) };
        }
        let raster = t1.elapsed();
        let report = cost::analyze(&view, &self.panel.model, &self.panel.scanout);
        let stats = Stats { report, items: view.items.len(), glyphs: view.glyphs.len(), build, raster };
        self.last_stats = Some(stats);
        self.dirty = false;
        stats
    }

    pub fn stats(&self) -> Option<Stats> {
        self.last_stats
    }

    pub fn app(&self) -> &A {
        &self.app
    }

    /// The app, for driving it from a test; the next [`render`](Self::render)
    /// picks up whatever changed.
    pub fn app_mut(&mut self) -> &mut A {
        self.dirty = true;
        &mut self.app
    }

    /// The last frame as portrait RGB8.
    pub fn portrait_rgb(&self) -> (u32, u32, Vec<u8>) {
        let (lw, lh) = self.panel.logical();
        let (lw, lh) = (usize::from(lw), usize::from(lh));
        let pw = usize::from(self.panel.width);
        let mut rgb = vec![0u8; lw * lh * 3];
        for ly in 0..lh {
            for lx in 0..lw {
                let p = self.fb[lx * pw + (pw - 1 - ly)];
                let o = (ly * lw + lx) * 3;
                rgb[o] = ((p >> 11) as u8) << 3;
                rgb[o + 1] = ((p >> 5) as u8 & 0x3f) << 2;
                rgb[o + 2] = (p as u8 & 0x1f) << 3;
            }
        }
        (lw as u32, lh as u32, rgb)
    }

    pub fn save_png(&self, path: impl AsRef<std::path::Path>) -> std::io::Result<()> {
        let (w, h, rgb) = self.portrait_rgb();
        let file = std::fs::File::create(path)?;
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&rgb).map_err(std::io::Error::other)
    }

    fn title_for(&self, s: &Stats) -> String {
        let (_, lh) = self.panel.logical();
        let r = &s.report;
        let verdict = if r.fits() { "fits" } else { "LATE" };
        format!(
            "{} — {}: worst line {} = {} cyc ({} % of budget), {} % of a core, {} late, slack {:.1} lines; {} items + {} glyphs; build {:.1} ms, raster {:.1} ms",
            self.title,
            verdict,
            r.worst_line,
            r.worst_line_cycles,
            r.worst_line_cycles * 100 / self.panel.scanout.budget_cycles.max(1),
            r.core_percent(&self.panel.scanout, lh),
            r.late_lines,
            r.min_slack_x16 as f64 / 16.0,
            s.items,
            s.glyphs,
            s.build.as_secs_f64() * 1e3,
            s.raster.as_secs_f64() * 1e3,
        )
    }

    fn present(&mut self) {
        let Some(window) = self.window.clone() else { return };
        let Some(surface) = self.surface.as_mut() else { return };
        let size = window.inner_size();
        let (Some(sw), Some(sh)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
            return;
        };
        surface.resize(sw, sh).expect("resize");
        let mut buf = surface.buffer_mut().expect("buffer");
        let (lw, lh) = self.panel.logical();
        let (lw, lh) = (u32::from(lw), u32::from(lh));
        let pw = usize::from(self.panel.width);
        // Nearest-neighbour into whatever size the window is (HiDPI, resizes).
        for wy in 0..size.height {
            let ly = (wy * lh / size.height) as usize;
            for wx in 0..size.width {
                let lx = (wx * lw / size.width) as usize;
                let p = u32::from(self.fb[lx * pw + (pw - 1 - ly)]);
                let r = ((p >> 11) << 3) | ((p >> 13) & 7);
                let g = (((p >> 5) & 0x3f) << 2) | ((p >> 9) & 3);
                let b = ((p & 0x1f) << 3) | ((p >> 2) & 7);
                buf[(wy * size.width + wx) as usize] = (r << 16) | (g << 8) | b;
            }
        }
        buf.present().expect("present");
    }

    fn logical_from_window(&self, x: f64, y: f64) -> (i16, i16) {
        let Some(window) = &self.window else { return (0, 0) };
        let size = window.inner_size();
        let (lw, lh) = self.panel.logical();
        let lx = (x * f64::from(lw) / f64::from(size.width.max(1))).floor();
        let ly = (y * f64::from(lh) / f64::from(size.height.max(1))).floor();
        (lx.clamp(-1.0, f64::from(lw)) as i16, ly.clamp(-1.0, f64::from(lh)) as i16)
    }

    fn touched(&mut self, t: Touch) {
        if self.app.touch(t) {
            self.dirty = true;
        }
    }

    fn redraw_if_dirty(&mut self) {
        if self.dirty {
            let stats = self.render();
            if let Some(w) = &self.window {
                w.set_title(&self.title_for(&stats));
                w.request_redraw();
            }
        }
    }
}

impl<A: App> ApplicationHandler for Viewer<A> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let (lw, lh) = self.panel.logical();
        let attrs = Window::default_attributes()
            .with_title(&self.title)
            .with_inner_size(LogicalSize::new(lw, lh))
            .with_resizable(true);
        let window = Rc::new(event_loop.create_window(attrs).expect("window"));
        let context = softbuffer::Context::new(window.clone()).expect("softbuffer context");
        let surface = softbuffer::Surface::new(&context, window.clone()).expect("softbuffer surface");
        self.window = Some(window);
        self.surface = Some(surface);
        self.dirty = true;
        self.redraw_if_dirty();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => self.present(),
            WindowEvent::Resized(_) => {
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                if self.pressed {
                    let (x, y) = self.logical_from_window(position.x, position.y);
                    self.touched(Touch::Move(x, y));
                }
                self.cursor = (position.x, position.y);
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => match state {
                ElementState::Pressed => {
                    self.pressed = true;
                    let (x, y) = self.logical_from_window(self.cursor.0, self.cursor.1);
                    self.touched(Touch::Down(x, y));
                }
                ElementState::Released => {
                    if self.pressed {
                        self.pressed = false;
                        self.touched(Touch::Up);
                    }
                }
            },
            WindowEvent::KeyboardInput {
                event: KeyEvent { logical_key, state: ElementState::Pressed, .. },
                ..
            } => match logical_key {
                Key::Named(NamedKey::Escape) => event_loop.exit(),
                Key::Character(c) if c.eq_ignore_ascii_case("s") => {
                    self.frame_no += 1;
                    let name = format!("scanwright-{:03}.png", self.frame_no);
                    match self.save_png(&name) {
                        Ok(()) => eprintln!("saved {name}"),
                        Err(e) => eprintln!("save {name}: {e}"),
                    }
                }
                _ => {}
            },
            _ => {}
        }
        self.redraw_if_dirty();
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        if now >= self.next_tick {
            if self.app.tick() {
                self.dirty = true;
            }
            self.next_tick = now + self.tick;
            self.redraw_if_dirty();
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_tick));
    }
}

/// A display list received from a device ([`scanwright_core::wire`]), owned.
/// Rasterize it with the host's copy of the same baked fonts to see exactly
/// what the panel shows.
pub struct Capture {
    pub header: scanwright_core::wire::Header,
    items: Vec<scanwright_core::list::Item>,
    order: Vec<u16>,
    glyphs: Vec<scanwright_core::list::GlyphRef>,
    luts: Vec<[u16; 16]>,
    pool: Vec<u8>,
}

impl Capture {
    /// Decode a complete wire blob. Refuses a list baked against other fonts.
    pub fn decode(bytes: &[u8], fonts: scanwright_core::font::FontSet) -> Result<Capture, String> {
        use scanwright_core::wire;
        let h = wire::Header::parse(bytes).ok_or("not a scanwright list (bad magic / short header)")?;
        if bytes.len() < h.total_len() {
            return Err(format!("truncated: {} of {} bytes", bytes.len(), h.total_len()));
        }
        if h.atlas_len as usize != fonts.atlas.len() || h.atlas_hash != wire::atlas_hash(fonts.atlas) {
            return Err("the device's fonts differ from this build's".into());
        }
        let mut items = vec![scanwright_core::list::Item::EMPTY; usize::from(h.n_items)];
        let mut order = vec![0u16; usize::from(h.n_items)];
        let mut glyphs = vec![scanwright_core::list::GlyphRef::new(0, 0, 0, 0, 0); usize::from(h.n_glyphs)];
        let mut luts = vec![[0u16; 16]; usize::from(h.n_luts)];
        let mut pool = vec![0u8; usize::from(h.pool_len)];
        wire::decode_into(bytes, &mut items, &mut order, &mut glyphs, &mut luts, &mut pool).ok_or("decode failed")?;
        Ok(Capture { header: h, items, order, glyphs, luts, pool })
    }

    pub fn view(&self, fonts: scanwright_core::font::FontSet) -> ListView<'_> {
        ListView {
            panel_w: self.header.panel_w,
            panel_h: self.header.panel_h,
            items: &self.items,
            order: &self.order,
            glyphs: &self.glyphs,
            luts: &self.luts,
            pool: &self.pool,
            atlas: fonts.atlas,
        }
    }

    /// Rasterize the whole frame (panel space, RGB565).
    pub fn rasterize(&self, fonts: scanwright_core::font::FontSet) -> Vec<u16> {
        let view = self.view(fonts);
        let (w, h) = (usize::from(view.panel_w), usize::from(view.panel_h));
        let mut fb = vec![0u16; w * h];
        let mut raster = Raster::new();
        raster.begin_frame();
        for y in 0..h {
            // Safety: each line has `panel_w` pixels; the view is self-consistent.
            unsafe { raster.line(&view, y as u16, fb[y * w..].as_mut_ptr(), true) };
        }
        fb
    }

    /// Rasterize and save as a portrait PNG (the 90° rotation undone).
    pub fn save_png(&self, fonts: scanwright_core::font::FontSet, path: impl AsRef<std::path::Path>) -> std::io::Result<()> {
        let fb = self.rasterize(fonts);
        let (pw, ph) = (usize::from(self.header.panel_w), usize::from(self.header.panel_h));
        let (lw, lh) = (ph, pw);
        let mut rgb = vec![0u8; lw * lh * 3];
        for ly in 0..lh {
            for lx in 0..lw {
                let p = fb[lx * pw + (pw - 1 - ly)];
                let o = (ly * lw + lx) * 3;
                rgb[o] = ((p >> 11) as u8) << 3;
                rgb[o + 1] = ((p >> 5) as u8 & 0x3f) << 2;
                rgb[o + 2] = (p as u8 & 0x1f) << 3;
            }
        }
        let file = std::fs::File::create(path)?;
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), lw as u32, lh as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&rgb).map_err(std::io::Error::other)
    }

    /// Price the captured list against a panel.
    pub fn report(&self, fonts: scanwright_core::font::FontSet, panel: &Panel) -> cost::Report {
        cost::analyze(&self.view(fonts), &panel.model, &panel.scanout)
    }
}
