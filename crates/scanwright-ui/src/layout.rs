//! Layout: a measure pass (bottom-up intrinsic sizes) and an arrange pass
//! (top-down rects). Flex-like, integer px, single-line text.
//!
//! * `Hug` = intrinsic size, `Fixed(n)` = exactly n, `Fill(w)` = a weighted
//!   share of what the siblings leave over.
//! * Cross axis: `Align::Stretch` (default) gives non-`Fixed` children the
//!   container's cross extent; otherwise they keep their intrinsic size.
//! * `Justify` only matters when no child fills the main axis.

use crate::tree::{Align, Justify, Kind, NONE, Node, Rect, Size, Tree};

pub(crate) fn layout(tree: &mut Tree<'_>, root: u16, screen: Rect) {
    if root == NONE {
        return;
    }
    measure(tree, root);
    arrange(tree, root, screen);
}

fn children(nodes: &[Node], parent: u16) -> impl Iterator<Item = u16> + '_ {
    let mut next = nodes[usize::from(parent)].first_child;
    core::iter::from_fn(move || {
        (next != NONE).then(|| {
            let c = next;
            next = nodes[usize::from(c)].next;
            c
        })
    })
}

fn measure(tree: &mut Tree<'_>, i: u16) -> (i16, i16) {
    let node = tree.nodes[usize::from(i)];
    let (mut w, mut h) = (0i16, 0i16);
    if node.kind == Kind::Text {
        if let Some(font) = node.font {
            w = font.measure(tree.text_of(&node)) as i16;
            h = font.line_height() as i16;
        }
    } else {
        let mut count = 0i16;
        let mut c = node.first_child;
        while c != NONE {
            let (cw, ch) = measure(tree, c);
            match node.kind {
                Kind::Row => (w, h) = (w + cw, h.max(ch)),
                Kind::Column => (w, h) = (w.max(cw), h + ch),
                _ => (w, h) = (w.max(cw), h.max(ch)),
            }
            count += 1;
            c = tree.nodes[usize::from(c)].next;
        }
        let gaps = node.gap * (count - 1).max(0);
        match node.kind {
            Kind::Row => w += gaps,
            Kind::Column => h += gaps,
            _ => {}
        }
    }
    w += node.pad.left + node.pad.right;
    h += node.pad.top + node.pad.bottom;
    if let Size::Fixed(v) = node.width {
        w = v;
    }
    if let Size::Fixed(v) = node.height {
        h = v;
    }
    tree.nodes[usize::from(i)].intrinsic = (w, h);
    (w, h)
}

fn arrange(tree: &mut Tree<'_>, i: u16, rect: Rect) {
    tree.nodes[usize::from(i)].rect = rect;
    let node = tree.nodes[usize::from(i)];
    let content = Rect {
        x: rect.x + node.pad.left,
        y: rect.y + node.pad.top,
        w: (rect.w - node.pad.left - node.pad.right).max(0),
        h: (rect.h - node.pad.top - node.pad.bottom).max(0),
    };
    match node.kind {
        Kind::Text => {}
        Kind::Stack => {
            let mut c = node.first_child;
            while c != NONE {
                let ch = tree.nodes[usize::from(c)];
                let (w, x) = place(ch.width, ch.intrinsic.0, content.x, content.w, node.align);
                let (h, y) = place(ch.height, ch.intrinsic.1, content.y, content.h, justify_as_align(node.justify));
                arrange(tree, c, Rect { x, y, w, h });
                c = ch.next;
            }
        }
        Kind::Row | Kind::Column => {
            let row = node.kind == Kind::Row;
            let main_of = |n: &Node| if row { (n.width, n.intrinsic.0) } else { (n.height, n.intrinsic.1) };
            let (main_start, main_len) = if row { (content.x, content.w) } else { (content.y, content.h) };

            let (mut fixed, mut weights, mut count) = (0i32, 0i32, 0i32);
            for c in children(tree.nodes, i) {
                match main_of(&tree.nodes[usize::from(c)]) {
                    (Size::Fill(w), _) => weights += i32::from(w),
                    (_, len) => fixed += i32::from(len),
                }
                count += 1;
            }
            fixed += i32::from(node.gap) * (count - 1).max(0);
            let spare = (i32::from(main_len) - fixed).max(0);

            let (mut cursor, mut gap) = (i32::from(main_start), i32::from(node.gap));
            if weights == 0 {
                match node.justify {
                    Justify::Start => {}
                    Justify::Center => cursor += spare / 2,
                    Justify::End => cursor += spare,
                    Justify::SpaceBetween if count > 1 => gap += spare / (count - 1),
                    Justify::SpaceBetween => {}
                }
            }
            let (mut spare_left, mut weights_left) = (spare, weights);
            let mut c = node.first_child;
            while c != NONE {
                let ch = tree.nodes[usize::from(c)];
                let len = match main_of(&ch) {
                    (Size::Fill(w), _) => {
                        // Hand out the remainder exactly: no rounding gap at the end.
                        let share = if weights_left > 0 { spare_left * i32::from(w) / weights_left } else { 0 };
                        spare_left -= share;
                        weights_left -= i32::from(w);
                        share
                    }
                    (_, len) => i32::from(len),
                };
                let child = if row {
                    let (h, y) = place(ch.height, ch.intrinsic.1, content.y, content.h, node.align);
                    Rect { x: cursor as i16, y, w: len as i16, h }
                } else {
                    let (w, x) = place(ch.width, ch.intrinsic.0, content.x, content.w, node.align);
                    Rect { x, y: cursor as i16, w, h: len as i16 }
                };
                arrange(tree, c, child);
                cursor += len + gap;
                c = ch.next;
            }
        }
    }
}

fn justify_as_align(j: Justify) -> Align {
    match j {
        Justify::Start | Justify::SpaceBetween => Align::Start,
        Justify::Center => Align::Center,
        Justify::End => Align::End,
    }
}

/// Size and position of a child along an axis it does not flow on.
fn place(size: Size, intrinsic: i16, start: i16, extent: i16, align: Align) -> (i16, i16) {
    let len = match (size, align) {
        (Size::Fixed(_), _) => intrinsic,
        (Size::Fill(_), _) | (_, Align::Stretch) => extent,
        _ => intrinsic,
    };
    let pos = match align {
        Align::Start | Align::Stretch => start,
        Align::Center => start + (extent - len) / 2,
        Align::End => start + extent - len,
    };
    (len, pos)
}
