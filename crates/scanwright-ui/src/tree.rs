//! The element tree: fixed-capacity node and text arenas, reset on every
//! rebuild. [`El`] is a 2-byte handle into the tree currently being built —
//! no lifetimes, no allocation — so authoring code reads like damascene's:
//!
//! ```ignore
//! column([h1("Oven 1"), row([button("-").key("dec"), button("+").key("inc")])])
//! ```
//!
//! The price is one piece of ambient state: builders find the tree through
//! [`crate::current`], which is only set inside [`crate::Ui::rebuild`].

use scanwright_core::font::Font;

use crate::input::Key;

pub(crate) const NONE: u16 = u16::MAX;

/// How a node sizes itself along one axis (damascene's vocabulary).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Size {
    /// Intrinsic size of the contents (the default).
    #[default]
    Hug,
    /// Claim a share of the leftover space; weights are relative.
    Fill(u8),
    /// Exact size in px.
    Fixed(i16),
}

/// Cross-axis placement of a container's children.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    Start,
    Center,
    End,
    /// Stretch non-`Fixed` children to the container's cross extent.
    #[default]
    Stretch,
}

/// Main-axis distribution of a container's children.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Justify {
    #[default]
    Start,
    Center,
    End,
    SpaceBetween,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextAlign {
    #[default]
    Start,
    Center,
    End,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Kind {
    #[default]
    Column,
    Row,
    /// Children overlaid; each placed in the content box by `align`
    /// (horizontal) and `justify` (vertical).
    Stack,
    /// A column whose content may be taller than it: clipped, and shifted up
    /// by a per-key scroll offset that a vertical drag changes.
    Scroll,
    Text,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sides {
    pub left: i16,
    pub top: i16,
    pub right: i16,
    pub bottom: i16,
}

impl From<i16> for Sides {
    fn from(v: i16) -> Self {
        Sides { left: v, top: v, right: v, bottom: v }
    }
}

/// `(horizontal, vertical)`
impl From<(i16, i16)> for Sides {
    fn from((x, y): (i16, i16)) -> Self {
        Sides { left: x, top: y, right: x, bottom: y }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i16,
    pub y: i16,
    pub w: i16,
    pub h: i16,
}

impl Rect {
    pub fn contains(&self, x: i16, y: i16) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Node {
    pub kind: Kind,
    pub first_child: u16,
    pub last_child: u16,
    pub next: u16,
    pub width: Size,
    pub height: Size,
    pub pad: Sides,
    pub gap: i16,
    pub align: Align,
    pub justify: Justify,
    pub fill: Option<u16>,
    pub radius: u8,
    /// Outline drawn on the fill's edge (needs a fill).
    pub border_w: u8,
    pub border_color: u16,
    /// Darken the fill while this node's key is pressed.
    pub pressable: bool,
    /// Everything painted before this node is dimmed (a modal sheet's scrim).
    pub scrim: bool,
    pub key: Option<Key>,
    // Text nodes.
    pub text_off: u32,
    pub text_len: u16,
    pub font: Option<&'static Font>,
    pub text_color: u16,
    pub text_align: TextAlign,
    /// Extra px after every glyph (letter spacing).
    pub tracking: i8,
    // Scroll nodes: current offset (set from the state table before layout)
    // and the content height (set by layout).
    pub scroll: i16,
    pub extent: i16,
    // Layout results.
    pub intrinsic: (i16, i16),
    pub rect: Rect,
}

impl Node {
    pub const EMPTY: Node = Node {
        kind: Kind::Column,
        first_child: NONE,
        last_child: NONE,
        next: NONE,
        width: Size::Hug,
        height: Size::Hug,
        pad: Sides { left: 0, top: 0, right: 0, bottom: 0 },
        gap: 0,
        align: Align::Stretch,
        justify: Justify::Start,
        fill: None,
        radius: 0,
        border_w: 0,
        border_color: 0,
        pressable: false,
        scrim: false,
        key: None,
        text_off: 0,
        text_len: 0,
        font: None,
        text_color: 0,
        text_align: TextAlign::Start,
        tracking: 0,
        scroll: 0,
        extent: 0,
        intrinsic: (0, 0),
        rect: Rect { x: 0, y: 0, w: 0, h: 0 },
    };
}

impl Default for Node {
    fn default() -> Self {
        Node::EMPTY
    }
}

/// The arenas of the tree being built. Non-generic view over the storage
/// owned by [`crate::Ui`].
pub(crate) struct Tree<'a> {
    pub theme: crate::theme::Theme,
    pub nodes: &'a mut [Node],
    pub n_nodes: usize,
    pub text: &'a mut [u8],
    pub n_text: usize,
    /// Nodes / text bytes that did not fit.
    pub dropped_nodes: usize,
    pub dropped_text: usize,
}

impl Tree<'_> {
    pub fn alloc(&mut self, node: Node) -> El {
        if self.n_nodes == self.nodes.len() || self.n_nodes == usize::from(NONE) {
            self.dropped_nodes += 1;
            return El(NONE);
        }
        self.nodes[self.n_nodes] = node;
        self.n_nodes += 1;
        El(self.n_nodes as u16 - 1)
    }

    pub fn node(&mut self, el: El) -> Option<&mut Node> {
        self.nodes[..self.n_nodes].get_mut(usize::from(el.0))
    }

    pub fn append_child(&mut self, parent: El, child: El) {
        if child.0 == NONE || parent.0 == NONE {
            return;
        }
        let last = self.nodes[usize::from(parent.0)].last_child;
        if last == NONE {
            self.nodes[usize::from(parent.0)].first_child = child.0;
        } else {
            self.nodes[usize::from(last)].next = child.0;
        }
        self.nodes[usize::from(parent.0)].last_child = child.0;
    }

    pub fn text_of(&self, node: &Node) -> &str {
        let (a, b) = (node.text_off as usize, node.text_off as usize + usize::from(node.text_len));
        core::str::from_utf8(&self.text[a..b]).unwrap_or("")
    }
}

/// Writes formatted text into the tree's text arena, truncating (on a char
/// boundary) when it is full.
pub(crate) struct TextWriter<'t, 'a> {
    pub tree: &'t mut Tree<'a>,
}

impl core::fmt::Write for TextWriter<'_, '_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let t = &mut *self.tree;
        let room = t.text.len() - t.n_text;
        let mut take = s.len().min(room);
        while !s.is_char_boundary(take) {
            take -= 1;
        }
        t.text[t.n_text..t.n_text + take].copy_from_slice(&s.as_bytes()[..take]);
        t.n_text += take;
        t.dropped_text += s.len() - take;
        Ok(())
    }
}

/// Handle to an element of the tree being built. Cheap to copy; only
/// meaningful inside the [`crate::Ui::rebuild`] that created it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct El(pub(crate) u16);

impl El {
    fn with(self, f: impl FnOnce(&mut Node)) -> Self {
        crate::current::with_tree(|t| {
            if let Some(n) = t.node(self) {
                f(n)
            }
        });
        self
    }

    // --- Identity ---------------------------------------------------------

    /// Stable identity: makes the element a touch target and names it in
    /// events. `"start"` or `("tab", index)`.
    pub fn key(self, key: impl Into<Key>) -> Self {
        let key = key.into();
        self.with(|n| n.key = Some(key))
    }

    // --- Sizing -----------------------------------------------------------

    pub fn width(self, w: Size) -> Self {
        self.with(|n| n.width = w)
    }
    pub fn height(self, h: Size) -> Self {
        self.with(|n| n.height = h)
    }
    pub fn hug(self) -> Self {
        self.with(|n| (n.width, n.height) = (Size::Hug, Size::Hug))
    }
    pub fn fill_size(self) -> Self {
        self.with(|n| (n.width, n.height) = (Size::Fill(1), Size::Fill(1)))
    }
    pub fn fill_width(self) -> Self {
        self.with(|n| n.width = Size::Fill(1))
    }
    pub fn fill_height(self) -> Self {
        self.with(|n| n.height = Size::Fill(1))
    }

    // --- Box --------------------------------------------------------------

    pub fn padding(self, p: impl Into<Sides>) -> Self {
        let p = p.into();
        self.with(|n| n.pad = p)
    }
    pub fn px(self, v: i16) -> Self {
        self.with(|n| (n.pad.left, n.pad.right) = (v, v))
    }
    pub fn py(self, v: i16) -> Self {
        self.with(|n| (n.pad.top, n.pad.bottom) = (v, v))
    }
    pub fn gap(self, g: i16) -> Self {
        self.with(|n| n.gap = g)
    }
    pub fn align(self, a: Align) -> Self {
        self.with(|n| n.align = a)
    }
    pub fn justify(self, j: Justify) -> Self {
        self.with(|n| n.justify = j)
    }

    // --- Paint ------------------------------------------------------------

    /// Background colour (RGB565; see [`scanwright_core::hex`]).
    pub fn fill(self, color: u16) -> Self {
        self.with(|n| n.fill = Some(color))
    }
    pub fn radius(self, r: u8) -> Self {
        self.with(|n| n.radius = r)
    }
    /// A `width` px outline in `color` along the fill's edge. Needs a fill.
    pub fn border(self, width: u8, color: u16) -> Self {
        self.with(|n| (n.border_w, n.border_color) = (width, color))
    }
    /// Darken the fill while this element's key is pressed (what the stock
    /// `button` does); for custom tappable widgets.
    pub fn pressable(self) -> Self {
        self.with(|n| n.pressable = true)
    }
    /// Dim everything painted before this element: the scrim under a modal
    /// sheet. Give it a key to catch the taps outside the sheet. The dimming
    /// is resolved at emit time — it costs nothing on the real-time side.
    pub fn scrim(self) -> Self {
        self.with(|n| n.scrim = true)
    }

    // --- Text -------------------------------------------------------------

    pub fn font(self, font: &'static Font) -> Self {
        self.with(|n| n.font = Some(font))
    }
    pub fn text_color(self, color: u16) -> Self {
        self.with(|n| n.text_color = color)
    }
    pub fn text_align(self, a: TextAlign) -> Self {
        self.with(|n| n.text_align = a)
    }
    pub fn center_text(self) -> Self {
        self.text_align(TextAlign::Center)
    }
    /// Letter spacing: extra px after every glyph.
    pub fn tracking(self, px: i8) -> Self {
        self.with(|n| n.tracking = px)
    }
}

impl Rect {
    pub fn intersect(&self, o: &Rect) -> Rect {
        let x0 = self.x.max(o.x);
        let y0 = self.y.max(o.y);
        let x1 = (self.x + self.w).min(o.x + o.w);
        let y1 = (self.y + self.h).min(o.y + o.h);
        Rect { x: x0, y: y0, w: (x1 - x0).max(0), h: (y1 - y0).max(0) }
    }

    pub fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }
}

/// Text measurement shared by layout and emit: `\n` separates lines.
pub(crate) fn text_size(font: &Font, s: &str, tracking: i32) -> (i16, i16) {
    let mut w = 0i32;
    let mut lines = 0i32;
    for line in s.split('\n') {
        w = w.max(font.measure_tracked(line, tracking));
        lines += 1;
    }
    (w as i16, (lines * font.line_height()) as i16)
}
