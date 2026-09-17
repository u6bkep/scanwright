//! Keys, touch input and events.
//!
//! Emitting a tree also emits a **hit list** — the rect and key of every keyed
//! element, in paint order — so touch handling needs neither the tree nor the
//! display list. Interaction state (what is pressed) lives in [`crate::Ui`]
//! and survives rebuilds because it is keyed, not positional.

use crate::tree::Rect;

/// Element identity: a name (hashed) and an optional index for repeated
/// elements. `"start".into()`, `("tab", 2).into()`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key {
    name: u32,
    index: u16,
}

impl Key {
    pub const fn new(name: &str) -> Self {
        Self::indexed(name, 0)
    }

    pub const fn indexed(name: &str, index: u16) -> Self {
        // FNV-1a.
        let bytes = name.as_bytes();
        let mut h: u32 = 0x811c_9dc5;
        let mut i = 0;
        while i < bytes.len() {
            h = (h ^ bytes[i] as u32).wrapping_mul(0x0100_0193);
            i += 1;
        }
        Key { name: h, index }
    }

    pub const fn index(&self) -> u16 {
        self.index
    }

    pub const fn is_named(&self, name: &str) -> bool {
        self.name == Key::new(name).name
    }
}

impl From<&str> for Key {
    fn from(name: &str) -> Self {
        Key::new(name)
    }
}

impl From<(&str, usize)> for Key {
    fn from((name, index): (&str, usize)) -> Self {
        Key::indexed(name, index as u16)
    }
}

/// Touch input in logical (UI) coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Touch {
    Down(i16, i16),
    Move(i16, i16),
    Up,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiEvent {
    /// A keyed element was pressed and released.
    Click(Key),
}

impl UiEvent {
    pub fn is_click(&self, key: impl Into<Key>) -> bool {
        matches!(self, UiEvent::Click(k) if *k == key.into())
    }

    /// For repeated elements keyed `(name, index)`: the clicked index.
    pub fn click_index(&self, name: &str) -> Option<usize> {
        match self {
            UiEvent::Click(k) if k.is_named(name) => Some(usize::from(k.index())),
            _ => None,
        }
    }
}

/// What [`crate::Ui::touch`] made of an input.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Response {
    pub event: Option<UiEvent>,
    /// Interaction state changed: rebuild to show it.
    pub redraw: bool,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct Hit {
    pub rect: Rect,
    pub key: Option<Key>,
}

/// Topmost hit under the point (the list is in paint order).
pub(crate) fn hit_test(hits: &[Hit], x: i16, y: i16) -> Option<Key> {
    hits.iter().rev().find(|h| h.rect.contains(x, y)).and_then(|h| h.key)
}
