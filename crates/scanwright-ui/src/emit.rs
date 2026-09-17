//! Emit: walk the laid-out tree in paint order into a display list and a hit
//! list.
//!
//! The walk tracks the **known flat background** under each element — the
//! nearest ancestor's fill — and hands it to the list builder, which turns
//! text and rounded corners over a known colour into LUT masks (no blending on
//! the real-time side). Authors never think about it. Later children of a
//! `stack` sit over their siblings, so they blend.

use scanwright_core::list::ListBuilder;

use crate::{
    input::{Hit, Key},
    theme,
    tree::{Kind, NONE, Rect, TextAlign, Tree},
};

pub(crate) struct Emit<'a, 'l> {
    pub tree: &'a Tree<'a>,
    pub list: &'a mut ListBuilder<'l>,
    pub hits: &'a mut [Hit],
    pub n_hits: usize,
    pub dropped_hits: usize,
    pub pressed: Option<Key>,
}

impl Emit<'_, '_> {
    pub fn node(&mut self, i: u16, bg: Option<u16>) {
        if i == NONE {
            return;
        }
        let node = &self.tree.nodes[usize::from(i)];
        let r = node.rect;
        let mut bg = bg;
        if let Some(fill) = node.fill {
            let fill = if node.pressable && node.key.is_some() && node.key == self.pressed {
                theme::pressed(fill)
            } else {
                fill
            };
            if r.w > 0 && r.h > 0 {
                self.list.rounded_rect(r.x.into(), r.y.into(), r.w.into(), r.h.into(), node.radius.into(), fill, bg);
            }
            bg = Some(fill);
        }
        if node.key.is_some() {
            if self.n_hits < self.hits.len() {
                self.hits[self.n_hits] = Hit { rect: r, key: node.key };
                self.n_hits += 1;
            } else {
                self.dropped_hits += 1;
            }
        }
        if node.kind == Kind::Text {
            self.text(i, r, bg);
            return;
        }
        let mut c = node.first_child;
        let mut first = true;
        while c != NONE {
            let child_bg = if node.kind == Kind::Stack && !first { None } else { bg };
            self.node(c, child_bg);
            first = false;
            c = self.tree.nodes[usize::from(c)].next;
        }
    }

    fn text(&mut self, i: u16, r: Rect, bg: Option<u16>) {
        let node = &self.tree.nodes[usize::from(i)];
        let Some(font) = node.font else { return };
        let s = self.tree.text_of(node);
        let inner_x = i32::from(r.x + node.pad.left);
        let inner_w = i32::from(r.w - node.pad.left - node.pad.right);
        let inner_y = i32::from(r.y + node.pad.top);
        let inner_h = i32::from(r.h - node.pad.top - node.pad.bottom);
        let x = match node.text_align {
            TextAlign::Start => inner_x,
            TextAlign::Center => inner_x + (inner_w - font.measure(s)) / 2,
            TextAlign::End => inner_x + inner_w - font.measure(s),
        };
        let baseline = inner_y + (inner_h - font.line_height()) / 2 + i32::from(font.ascent);
        self.list.text(font, x, baseline, s, node.text_color, bg);
    }
}
