//! Emit: walk the laid-out tree in paint order into a display list and a hit
//! list.
//!
//! The walk tracks the **known flat background** under each element — the
//! nearest ancestor's fill — and hands it to the list builder, which turns
//! text and rounded corners over a known colour into LUT masks (no blending on
//! the real-time side). Authors never think about it. Later children of a
//! `stack` sit over their siblings, so they blend.
//!
//! Two more things are resolved here rather than at raster time:
//!
//! * **Scrims.** Everything painted before a `.scrim()` node is emitted with
//!   its colours dimmed ([`theme::scrimmed`]), so the modal sheet's overlay
//!   costs nothing per line.
//! * **Clipping.** A `scroll` viewport, and a text node wider than its box,
//!   set the builder's clip rect; hit rects are clipped the same way, so a
//!   button scrolled out of view cannot be tapped.

use scanwright_core::list::ListBuilder;

use crate::{
    input::{Hit, Key},
    theme,
    tree::{Kind, NONE, Rect, TextAlign, Tree, text_size},
};

pub(crate) struct Emit<'a, 'l> {
    pub tree: &'a Tree<'a>,
    pub list: &'a mut ListBuilder<'l>,
    pub hits: &'a mut [Hit],
    pub n_hits: usize,
    pub dropped_hits: usize,
    pub pressed: Option<Key>,
    /// Dim colours: on until the first scrim node is reached.
    pub tint: bool,
    /// Current clip, logical coordinates.
    pub clip: Rect,
}

impl Emit<'_, '_> {
    fn color(&self, c: u16) -> u16 {
        if self.tint { theme::scrimmed(c) } else { c }
    }

    fn hit(&mut self, r: Rect, key: Option<Key>, scroll: bool) {
        let r = r.intersect(&self.clip);
        if r.is_empty() {
            return;
        }
        if self.n_hits < self.hits.len() {
            self.hits[self.n_hits] = Hit { rect: r, key, scroll };
            self.n_hits += 1;
        } else {
            self.dropped_hits += 1;
        }
    }

    fn set_clip(&mut self, r: Rect) {
        self.clip = r;
        self.list.set_clip(r.x.into(), r.y.into(), r.w.into(), r.h.into());
    }

    pub fn node(&mut self, i: u16, bg: Option<u16>) {
        if i == NONE {
            return;
        }
        let node = &self.tree.nodes[usize::from(i)];
        if node.scrim {
            // From here on things sit over the scrim: full colour.
            self.tint = false;
        }
        let r = node.rect;
        let mut bg = bg;
        if let Some(fill) = node.fill {
            let fill = if node.pressable && node.key.is_some() && node.key == self.pressed {
                theme::pressed(fill)
            } else {
                fill
            };
            let (fill, bg_c) = (self.color(fill), bg.map(|c| self.color(c)));
            if r.w > 0 && r.h > 0 {
                let (x, y, w, h, rad) = (r.x.into(), r.y.into(), r.w.into(), r.h.into(), node.radius.into());
                if node.border_w > 0 {
                    let bc = self.color(node.border_color);
                    self.list.rounded_rect_bordered(x, y, w, h, rad, fill, node.border_w.into(), bc, bg_c);
                } else {
                    self.list.rounded_rect(x, y, w, h, rad, fill, bg_c);
                }
            }
            // Track the *untinted* colour; it is tinted again where it is used.
            bg = Some(if node.pressable && node.key == self.pressed { theme::pressed(node.fill.unwrap_or(0)) } else { node.fill.unwrap_or(0) });
        }
        if node.key.is_some() {
            self.hit(r, node.key, node.kind == Kind::Scroll);
        }
        if node.kind == Kind::Text {
            self.text(i, r, bg);
            return;
        }
        let outer_clip = self.clip;
        if node.kind == Kind::Scroll {
            self.set_clip(r.intersect(&outer_clip));
        }
        let mut c = node.first_child;
        let mut first = true;
        while c != NONE {
            let child_bg = if node.kind == Kind::Stack && !first { None } else { bg };
            self.node(c, child_bg);
            first = false;
            c = self.tree.nodes[usize::from(c)].next;
        }
        if node.kind == Kind::Scroll {
            self.set_clip(outer_clip);
        }
    }

    fn text(&mut self, i: u16, r: Rect, bg: Option<u16>) {
        let node = &self.tree.nodes[usize::from(i)];
        let Some(font) = node.font else { return };
        let s = self.tree.text_of(node);
        let tracking = i32::from(node.tracking);
        let inner = Rect {
            x: r.x + node.pad.left,
            y: r.y + node.pad.top,
            w: r.w - node.pad.left - node.pad.right,
            h: r.h - node.pad.top - node.pad.bottom,
        };
        let (tw, th) = text_size(font, s, tracking);
        // Wider than its box: clip instead of spilling over the neighbour.
        let outer_clip = self.clip;
        let clipped = tw > inner.w || th > inner.h;
        if clipped {
            self.set_clip(inner.intersect(&outer_clip));
        }
        let color = self.color(node.text_color);
        let bg = bg.map(|c| self.color(c));
        let lh = font.line_height();
        let mut baseline = i32::from(inner.y) + (i32::from(inner.h) - i32::from(th)) / 2 + i32::from(font.ascent);
        for line in s.split('\n') {
            let lw = font.measure_tracked(line, tracking);
            let x = match node.text_align {
                TextAlign::Start => i32::from(inner.x),
                TextAlign::Center => i32::from(inner.x) + (i32::from(inner.w) - lw) / 2,
                TextAlign::End => i32::from(inner.x) + i32::from(inner.w) - lw,
            };
            self.list.text_tracked(font, x, baseline, line, color, bg, tracking);
            baseline += lh;
        }
        if clipped {
            self.set_clip(outer_clip);
        }
    }
}
