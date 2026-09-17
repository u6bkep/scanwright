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
    tree::{El, Node, Rect, Tree},
};

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
}

impl<const NODES: usize, const TEXT: usize, const HITS: usize> Ui<NODES, TEXT, HITS> {
    pub const fn new(theme: Theme) -> Self {
        Self {
            theme,
            nodes: [Node::EMPTY; NODES],
            text: [0; TEXT],
            hits: [Hit { rect: Rect { x: 0, y: 0, w: 0, h: 0 }, key: None }; HITS],
            n_hits: 0,
            pressed: None,
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

        let mut builder = list.begin(panel_w, panel_h, self.theme.background);
        let (w, h) = builder.logical_size();
        layout::layout(&mut tree, root.0, Rect { x: 0, y: 0, w: w as i16, h: h as i16 });

        let mut emit = emit::Emit {
            tree: &tree,
            list: &mut builder,
            hits: &mut self.hits,
            n_hits: 0,
            dropped_hits: 0,
            pressed: self.pressed,
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
    pub fn touch(&mut self, touch: Touch) -> Response {
        let hits = &self.hits[..self.n_hits];
        let before = self.pressed;
        let mut event = None;
        match touch {
            Touch::Down(x, y) => self.pressed = input::hit_test(hits, x, y),
            Touch::Move(x, y) => {
                // Sliding off the pressed element cancels the press.
                if self.pressed.is_some() && input::hit_test(hits, x, y) != self.pressed {
                    self.pressed = None;
                }
            }
            Touch::Up => {
                event = self.pressed.take().map(UiEvent::Click);
            }
        }
        Response { event, redraw: before != self.pressed }
    }
}
