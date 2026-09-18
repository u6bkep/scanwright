//! `scanwright-ui` — the declarative layer over `scanwright-core`.
//!
//! ```text
//! app state -> build (El tree, fixed arena) -> layout -> emit -> display list + hit list
//! ```
//!
//! Everything here runs on the soft side, only when state changes. Memory is
//! fixed: the application picks the capacities as const parameters of [`Ui`];
//! every rebuild reports what it used ([`BuildReport`]) and overflow drops
//! elements loudly instead of corrupting anything.
//!
//! ```ignore
//! use scanwright_ui::prelude::*;
//!
//! static mut UI: Ui<256, 2048, 32> = Ui::new(Theme::DARK);
//!
//! let report = ui.rebuild(&mut list, 800, 480, || {
//!     column([
//!         h1("Oven 1"),
//!         card([label("CHAMBER"), display(format_args!("{temp:.1}°C")).center_text()]),
//!         button("START A PROFILE").key("start"),
//!     ])
//!     .padding(16)
//!     .gap(16)
//! });
//!
//! if let Some(ev) = ui.touch(Touch::Down(x, y)).event { ... ev.is_click("start") ... }
//! ```

#![cfg_attr(not(feature = "std"), no_std)]

mod current;
pub mod demo;
mod emit;
pub mod input;
mod layout;
pub mod theme;
pub mod tree;
pub mod vocab;

use scanwright_core::list::{DisplayList, ListUsage};

use crate::{
    input::{Hit, Key, Response, Touch, UiEvent},
    theme::Theme,
    tree::{El, Kind, Node, Rect, Tree},
};

/// Scroll containers a `Ui` remembers offsets for (by key).
pub const MAX_SCROLLS: usize = 8;

/// Movement before a press becomes a drag.
const DRAG_SLOP: i16 = 8;

#[derive(Clone, Copy, Default)]
struct ScrollState {
    key: Option<Key>,
    offset: i16,
    max: i16,
}

#[derive(Clone, Copy)]
struct Drag {
    key: Key,
    origin_y: i16,
    last_y: i16,
    scrolling: bool,
}

pub mod prelude {
    pub use scanwright_core::{hex, rgb};

    pub use crate::{
        BuildReport, Ui,
        input::{Key, Response, Touch, UiEvent},
        theme::Theme,
        tree::{Align, El, Justify, Sides, Size, TextAlign},
        vocab::*,
    };
}

/// The theme of the tree being built (for custom widgets).
pub fn theme() -> Theme {
    current::with_tree(|t| t.theme)
}

/// What one rebuild used of every capacity. Anything `dropped_*` is an
/// undersized arena: the UI is incomplete and the application should say so
/// loudly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BuildReport {
    pub nodes: usize,
    pub text_bytes: usize,
    pub hits: usize,
    pub dropped_nodes: usize,
    pub dropped_text_bytes: usize,
    pub dropped_hits: usize,
    pub list: ListUsage,
}

impl BuildReport {
    pub fn complete(&self) -> bool {
        self.dropped_nodes == 0 && self.dropped_text_bytes == 0 && self.dropped_hits == 0 && self.list.dropped == 0
    }
}

/// UI state with fixed capacities: `NODES` elements and `TEXT` bytes of text
/// per tree, `HITS` keyed (touchable) elements.
pub struct Ui<const NODES: usize, const TEXT: usize, const HITS: usize> {
    theme: Theme,
    nodes: [Node; NODES],
    text: [u8; TEXT],
    hits: [Hit; HITS],
    n_hits: usize,
    pressed: Option<Key>,
    scrolls: [ScrollState; MAX_SCROLLS],
    drag: Option<Drag>,
}

impl<const NODES: usize, const TEXT: usize, const HITS: usize> Ui<NODES, TEXT, HITS> {
    pub const fn new(theme: Theme) -> Self {
        Self {
            theme,
            nodes: [Node::EMPTY; NODES],
            text: [0; TEXT],
            hits: [Hit { rect: Rect { x: 0, y: 0, w: 0, h: 0 }, key: None, scroll: false }; HITS],
            n_hits: 0,
            pressed: None,
            scrolls: [ScrollState { key: None, offset: 0, max: 0 }; MAX_SCROLLS],
            drag: None,
        }
    }

    fn scroll_slot(&mut self, key: Key) -> Option<&mut ScrollState> {
        if let Some(i) = self.scrolls.iter().position(|s| s.key == Some(key)) {
            return Some(&mut self.scrolls[i]);
        }
        let i = self.scrolls.iter().position(|s| s.key.is_none())?;
        self.scrolls[i] = ScrollState { key: Some(key), offset: 0, max: 0 };
        Some(&mut self.scrolls[i])
    }

    /// Current scroll offset of the keyed `scroll(..)` container.
    pub fn scroll_offset(&self, key: impl Into<Key>) -> i16 {
        let key = key.into();
        self.scrolls.iter().find(|s| s.key == Some(key)).map_or(0, |s| s.offset)
    }

    /// Scroll a keyed container to `offset` px (clamped on the next rebuild).
    pub fn set_scroll_offset(&mut self, key: impl Into<Key>, offset: i16) {
        if let Some(s) = self.scroll_slot(key.into()) {
            s.offset = offset.max(0);
        }
    }

    /// Build the tree with `build`, lay it out over the whole screen and emit
    /// it into `list` (sealed on return, ready to publish).
    pub fn rebuild<const I: usize, const G: usize>(
        &mut self,
        list: &mut DisplayList<I, G>,
        panel_w: u16,
        panel_h: u16,
        build: impl FnOnce() -> El,
    ) -> BuildReport {
        let mut tree = Tree {
            theme: self.theme,
            nodes: &mut self.nodes,
            n_nodes: 0,
            text: &mut self.text,
            n_text: 0,
            dropped_nodes: 0,
            dropped_text: 0,
        };
        let root = current::scope(&mut tree, build);

        // Scroll offsets in, so layout can place the content.
        let has_scrim = tree.nodes[..tree.n_nodes].iter().any(|n| n.scrim);
        for n in tree.nodes[..tree.n_nodes].iter_mut().filter(|n| n.kind == Kind::Scroll) {
            n.scroll = match n.key {
                Some(k) => self.scrolls.iter().find(|s| s.key == Some(k)).map_or(0, |s| s.offset),
                None => 0,
            };
        }

        // Under a scrim the screen's own background is dimmed too.
        let background = if has_scrim { theme::scrimmed(self.theme.background) } else { self.theme.background };
        let mut builder = list.begin(panel_w, panel_h, background);
        let (w, h) = builder.logical_size();
        let screen = Rect { x: 0, y: 0, w: w as i16, h: h as i16 };
        layout::layout(&mut tree, root.0, screen);

        // Content heights out: clamp the offsets for next time (and now, if a
        // shorter list left an offset past the end).
        let mut relayout = false;
        for n in tree.nodes[..tree.n_nodes].iter_mut().filter(|n| n.kind == Kind::Scroll) {
            let max = (n.extent - n.rect.h).max(0);
            let Some(k) = n.key else { continue };
            let clamped = n.scroll.clamp(0, max);
            if let Some(s) = self.scrolls.iter_mut().find(|s| s.key == Some(k)) {
                s.max = max;
                s.offset = clamped;
            } else if let Some(s) = self.scrolls.iter_mut().find(|s| s.key.is_none()) {
                *s = ScrollState { key: Some(k), offset: clamped, max };
            }
            if clamped != n.scroll {
                n.scroll = clamped;
                relayout = true;
            }
        }
        if relayout {
            layout::layout(&mut tree, root.0, screen);
        }

        let mut emit = emit::Emit {
            tree: &tree,
            list: &mut builder,
            hits: &mut self.hits,
            n_hits: 0,
            dropped_hits: 0,
            pressed: self.pressed,
            tint: has_scrim,
            clip: screen,
        };
        emit.node(root.0, Some(self.theme.background));
        let (n_hits, dropped_hits) = (emit.n_hits, emit.dropped_hits);
        self.n_hits = n_hits;

        BuildReport {
            nodes: tree.n_nodes,
            text_bytes: tree.n_text,
            hits: n_hits,
            dropped_nodes: tree.dropped_nodes,
            dropped_text_bytes: tree.dropped_text,
            dropped_hits,
            list: builder.finish(),
        }
    }

    /// Feed a touch (logical coordinates) against the last emitted tree.
    ///
    /// A press on a tappable element highlights it and clicks on release. A
    /// press inside a `scroll(..)` that then moves more than a few px becomes
    /// a drag: the press is cancelled and the offset follows the finger.
    pub fn touch(&mut self, touch: Touch) -> Response {
        let n_hits = self.n_hits;
        let before = self.pressed;
        let mut event = None;
        let mut scrolled = false;
        match touch {
            Touch::Down(x, y) => {
                self.pressed = input::hit_test(&self.hits[..n_hits], x, y);
                self.drag = input::scroll_test(&self.hits[..n_hits], x, y)
                    .map(|key| Drag { key, origin_y: y, last_y: y, scrolling: false });
            }
            Touch::Move(x, y) => {
                if let Some(mut d) = self.drag {
                    if !d.scrolling && (y - d.origin_y).abs() > DRAG_SLOP {
                        d.scrolling = true;
                        self.pressed = None;
                    }
                    if d.scrolling {
                        let dy = y - d.last_y;
                        if let Some(s) = self.scrolls.iter_mut().find(|s| s.key == Some(d.key)) {
                            let next = (s.offset - dy).clamp(0, s.max);
                            scrolled = next != s.offset;
                            s.offset = next;
                        }
                    }
                    d.last_y = y;
                    self.drag = Some(d);
                }
                // Sliding off the pressed element cancels the press.
                if self.pressed.is_some() && input::hit_test(&self.hits[..n_hits], x, y) != self.pressed {
                    self.pressed = None;
                }
            }
            Touch::Up => {
                event = self.pressed.take().map(UiEvent::Click);
                self.drag = None;
            }
        }
        Response { event, redraw: before != self.pressed || scrolled }
    }
}
