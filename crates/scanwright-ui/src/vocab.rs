//! The authoring vocabulary: free functions that build elements in the tree
//! being rebuilt. Names follow damascene's where the concept exists there.
//!
//! Stock widgets are built from the same public surface applications use.

use core::fmt::{Display, Write as _};

use crate::{
    current::with_tree,
    theme::Theme,
    tree::{Align, El, Justify, Kind, Node, Size, TextWriter},
};

fn container(kind: Kind, children: impl IntoIterator<Item = El>) -> El {
    // Children are built (by the caller / by this iterator) *between* tree
    // borrows, never inside one.
    let parent = with_tree(|t| t.alloc(Node { kind, ..Node::default() }));
    for child in children {
        with_tree(|t| t.append_child(parent, child));
    }
    parent
}

/// Children top to bottom.
pub fn column(children: impl IntoIterator<Item = El>) -> El {
    container(Kind::Column, children)
}

/// Children left to right.
pub fn row(children: impl IntoIterator<Item = El>) -> El {
    container(Kind::Row, children)
}

/// Children overlaid, placed by `.align` (horizontal) / `.justify` (vertical).
pub fn stack(children: impl IntoIterator<Item = El>) -> El {
    container(Kind::Stack, children)
}

/// Takes up leftover space along its parent's main axis.
pub fn spacer() -> El {
    container(Kind::Column, []).fill_size()
}

/// A column that scrolls: content taller than the element is clipped and a
/// vertical drag moves it. **Needs a `.key(..)`** — the offset lives in the
/// `Ui`'s per-key state and survives rebuilds. Size it with `fill_height()`
/// or a fixed height; its own intrinsic height is zero.
pub fn scroll(children: impl IntoIterator<Item = El>) -> El {
    container(Kind::Scroll, children)
}

fn text_with(theme: &Theme, font: &'static scanwright_core::font::Font, s: impl Display) -> El {
    with_tree(|t| {
        let off = t.n_text;
        let _ = write!(TextWriter { tree: t }, "{s}");
        let len = t.n_text - off;
        t.alloc(Node {
            kind: Kind::Text,
            text_off: off as u32,
            text_len: len as u16,
            font: Some(font),
            text_color: theme.text,
            ..Node::default()
        })
    })
}

/// Body text. Accepts anything `Display`: `text("Fan")`,
/// `text(format_args!("{temp:.1}°C"))`.
pub fn text(s: impl Display) -> El {
    let theme = crate::theme();
    text_with(&theme, theme.body, s)
}

/// Small, muted text: captions and field labels.
pub fn label(s: impl Display) -> El {
    let theme = crate::theme();
    text_with(&theme, theme.small, s).text_color(theme.muted)
}

/// Title text.
pub fn h1(s: impl Display) -> El {
    let theme = crate::theme();
    text_with(&theme, theme.title, s)
}

/// Large numeric readout.
pub fn display(s: impl Display) -> El {
    let theme = crate::theme();
    text_with(&theme, theme.display, s)
}

/// A rounded surface holding a column of children.
pub fn card(children: impl IntoIterator<Item = El>) -> El {
    let theme = crate::theme();
    column(children)
        .fill(theme.surface)
        .radius(theme.card_radius)
        .padding(theme.card_padding)
        .gap(8)
}

/// A pressable accent button. Give it a `.key(...)` to receive its clicks.
pub fn button(label: impl Display) -> El {
    let theme = crate::theme();
    let caption = text_with(&theme, theme.title, label).text_color(theme.on_accent);
    let el = row([caption])
        .fill(theme.accent)
        .radius(theme.button_radius)
        .padding((24, 0))
        .height(Size::Fixed(80))
        .align(Align::Center)
        .justify(Justify::Center);
    with_tree(|t| {
        if let Some(n) = t.node(el) {
            n.pressable = true;
        }
    });
    el
}

/// A horizontal progress bar, `fraction` in 0..=1 (as parts per 1000).
pub fn progress(permille: u16) -> El {
    let theme = crate::theme();
    const H: i16 = 16;
    let permille = permille.min(1000);
    // The bar is a rounded track holding a rounded, proportionally wide fill;
    // weights split the track's width.
    let done = row([]).fill(theme.accent).radius(H as u8 / 2).height(Size::Fixed(H));
    let done = if permille == 0 { done.width(Size::Fixed(0)) } else { done.width(Size::Fill(fill_weight(permille))) };
    let rest = spacer().width(Size::Fill(fill_weight(1000 - permille)));
    row([done, rest]).fill(theme.track).radius(H as u8 / 2).height(Size::Fixed(H))
}

/// Map 0..=1000 onto the 8-bit `Fill` weight without ever returning 0 for a
/// non-zero share.
fn fill_weight(permille: u16) -> u8 {
    ((u32::from(permille) * 255).div_ceil(1000)) as u8
}
