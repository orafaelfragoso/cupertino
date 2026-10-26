//! Selection model: click, toggle and range semantics, deferred collapse on mouse-up, keyboard
//! extension, rubber band, and index maintenance under model changes.
//!
//! The semantics are the behaviour of the design: plain click replaces,
//! the primary modifier toggles, shift selects the range from the anchor, shift+primary adds the
//! range; clicking an already selected item keeps the selection until the button is released
//! without dragging.

use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    /// Command on the design, `Ctrl` on Linux.
    pub primary: bool,
}

impl Modifiers {
    pub const NONE: Modifiers = Modifiers {
        shift: false,
        primary: false,
    };
    pub const SHIFT: Modifiers = Modifiers {
        shift: true,
        primary: false,
    };
    pub const PRIMARY: Modifiers = Modifiers {
        shift: false,
        primary: true,
    };
    pub const BOTH: Modifiers = Modifiers {
        shift: true,
        primary: true,
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseDown {
    /// Selection changed immediately.
    Changed,
    /// Item was already selected: selection kept so that a drag can start; call `mouse_up`.
    Deferred,
    /// Nothing changed.
    Unchanged,
}

#[derive(Clone, Debug, Default)]
pub struct Selection {
    selected: BTreeSet<usize>,
    anchor: Option<usize>,
    focus: Option<usize>,
    pending_collapse: Option<usize>,
    rubber_base: Option<BTreeSet<usize>>,
}

impl Selection {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn contains(&self, i: usize) -> bool {
        self.selected.contains(&i)
    }
    pub fn len(&self) -> usize {
        self.selected.len()
    }
    pub fn is_empty(&self) -> bool {
        self.selected.is_empty()
    }
    pub fn indices(&self) -> impl Iterator<Item = usize> + '_ {
        self.selected.iter().copied()
    }
    pub fn anchor(&self) -> Option<usize> {
        self.anchor
    }
    pub fn focus(&self) -> Option<usize> {
        self.focus
    }

    pub fn clear(&mut self) {
        self.selected.clear();
        self.anchor = None;
        self.focus = None;
        self.pending_collapse = None;
    }

    pub fn set_single(&mut self, i: usize) {
        self.selected.clear();
        self.selected.insert(i);
        self.anchor = Some(i);
        self.focus = Some(i);
        self.pending_collapse = None;
    }

    fn range(a: usize, b: usize) -> impl Iterator<Item = usize> {
        a.min(b)..=a.max(b)
    }

    /// Mouse press on item `i` of a list with `len` items.
    pub fn mouse_down(&mut self, i: usize, mods: Modifiers) -> MouseDown {
        self.pending_collapse = None;
        match (mods.shift, mods.primary) {
            (false, false) => {
                if self.selected.contains(&i) && self.selected.len() > 1 {
                    self.pending_collapse = Some(i);
                    self.focus = Some(i);
                    MouseDown::Deferred
                } else if self.selected.len() == 1 && self.selected.contains(&i) {
                    self.anchor = Some(i);
                    self.focus = Some(i);
                    MouseDown::Unchanged
                } else {
                    self.set_single(i);
                    MouseDown::Changed
                }
            }
            (false, true) => {
                if !self.selected.remove(&i) {
                    self.selected.insert(i);
                    self.anchor = Some(i);
                }
                self.focus = Some(i);
                MouseDown::Changed
            }
            (true, primary) => {
                let a = self.anchor.unwrap_or(i);
                if !primary {
                    self.selected.clear();
                }
                self.selected.extend(Self::range(a, i));
                self.anchor = Some(a);
                self.focus = Some(i);
                MouseDown::Changed
            }
        }
    }

    /// Mouse release; `dragged` tells whether a drag happened. Returns true if the selection changed.
    pub fn mouse_up(&mut self, dragged: bool) -> bool {
        match self.pending_collapse.take() {
            Some(i) if !dragged => {
                self.set_single(i);
                true
            }
            _ => false,
        }
    }

    /// Click on empty space: clears unless a modifier is held (so a missed toggle does not lose the selection).
    pub fn click_empty(&mut self, mods: Modifiers) -> bool {
        if mods.shift || mods.primary || self.selected.is_empty() {
            return false;
        }
        self.clear();
        true
    }

    /// Keyboard navigation to `to`; `extend` is shift. Without extension the selection becomes `{to}`.
    pub fn move_focus(&mut self, to: usize, extend: bool) {
        if extend {
            let a = self.anchor.unwrap_or(to);
            self.selected = Self::range(a, to).collect();
            self.anchor = Some(a);
        } else {
            self.selected.clear();
            self.selected.insert(to);
            self.anchor = Some(to);
        }
        self.focus = Some(to);
        self.pending_collapse = None;
    }

    /// Replace the whole state, for example with the indexes the same items have after the list was
    /// rebuilt. A collapse deferred to mouse-up is dropped.
    pub fn restore(
        &mut self,
        selected: impl IntoIterator<Item = usize>,
        anchor: Option<usize>,
        focus: Option<usize>,
    ) {
        self.selected = selected.into_iter().collect();
        self.anchor = anchor;
        self.focus = focus;
        self.pending_collapse = None;
    }

    pub fn select_all(&mut self, len: usize) {
        self.selected = (0..len).collect();
        self.anchor = if len > 0 { Some(0) } else { None };
        self.focus = if len > 0 { Some(len - 1) } else { None };
    }

    /// Start a rubber-band drag: remembers the selection to combine with.
    pub fn rubber_band_begin(&mut self, mods: Modifiers) {
        self.rubber_base = Some(if mods.shift || mods.primary {
            self.selected.clone()
        } else {
            BTreeSet::new()
        });
    }

    /// Update the rubber band with the indexes currently intersected. With a modifier the hit set
    /// toggles items of the base selection (symmetric difference), otherwise it replaces it.
    pub fn rubber_band_update(&mut self, hits: &[usize], mods: Modifiers) {
        let base = self.rubber_base.clone().unwrap_or_default();
        let hit: BTreeSet<usize> = hits.iter().copied().collect();
        self.selected = if mods.shift || mods.primary {
            base.symmetric_difference(&hit).copied().collect()
        } else {
            hit
        };
        if let Some(last) = hits.last() {
            self.focus = Some(*last);
            self.anchor.get_or_insert(*last);
        }
    }

    pub fn rubber_band_end(&mut self) {
        self.rubber_base = None;
    }

    /// Keep indexes valid after an item was inserted at `at`.
    pub fn on_insert(&mut self, at: usize) {
        let shift = |i: usize| if i >= at { i + 1 } else { i };
        self.selected = self.selected.iter().map(|i| shift(*i)).collect();
        self.anchor = self.anchor.map(shift);
        self.focus = self.focus.map(shift);
        self.pending_collapse = self.pending_collapse.map(shift);
    }

    /// Keep indexes valid after the item at `at` was removed (it leaves the selection).
    pub fn on_remove(&mut self, at: usize) {
        self.selected = self
            .selected
            .iter()
            .filter(|i| **i != at)
            .map(|i| if *i > at { i - 1 } else { *i })
            .collect();
        let fix = |o: Option<usize>| match o {
            Some(i) if i == at => None,
            Some(i) if i > at => Some(i - 1),
            o => o,
        };
        self.anchor = fix(self.anchor);
        self.focus = fix(self.focus);
        self.pending_collapse = fix(self.pending_collapse);
    }

    /// An item moved from `from` to `to` (re-sorted after a change): the selection follows the item.
    pub fn on_move(&mut self, from: usize, to: usize) {
        let was = self.selected.contains(&from);
        let (anchor, focus) = (self.anchor == Some(from), self.focus == Some(from));
        self.on_remove(from);
        self.on_insert(to);
        if was {
            self.selected.insert(to);
        }
        if anchor {
            self.anchor = Some(to);
        }
        if focus {
            self.focus = Some(to);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sel(s: &Selection) -> Vec<usize> {
        s.indices().collect()
    }

    #[test]
    fn click_replaces_and_toggle_adds() {
        let mut s = Selection::new();
        assert_eq!(s.mouse_down(3, Modifiers::NONE), MouseDown::Changed);
        s.mouse_down(5, Modifiers::PRIMARY);
        assert_eq!(sel(&s), [3, 5]);
        s.mouse_down(3, Modifiers::PRIMARY);
        assert_eq!(sel(&s), [5]);
        s.mouse_down(1, Modifiers::NONE);
        assert_eq!(sel(&s), [1]);
    }

    #[test]
    fn shift_selects_range_from_anchor() {
        let mut s = Selection::new();
        s.mouse_down(2, Modifiers::NONE);
        s.mouse_down(5, Modifiers::SHIFT);
        assert_eq!(sel(&s), [2, 3, 4, 5]);
        s.mouse_down(0, Modifiers::SHIFT);
        assert_eq!(sel(&s), [0, 1, 2]);
        s.mouse_down(8, Modifiers::BOTH);
        assert_eq!(sel(&s), [0, 1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn selected_item_click_defers_collapse_until_mouse_up() {
        let mut s = Selection::new();
        s.mouse_down(1, Modifiers::NONE);
        s.mouse_down(2, Modifiers::SHIFT);
        assert_eq!(s.mouse_down(2, Modifiers::NONE), MouseDown::Deferred);
        assert_eq!(sel(&s), [1, 2]);
        assert!(!s.mouse_up(true));
        assert_eq!(sel(&s), [1, 2]);
        s.mouse_down(2, Modifiers::NONE);
        assert!(s.mouse_up(false));
        assert_eq!(sel(&s), [2]);
    }

    #[test]
    fn empty_click_clears_only_without_modifiers() {
        let mut s = Selection::new();
        s.mouse_down(1, Modifiers::NONE);
        assert!(!s.click_empty(Modifiers::SHIFT));
        assert_eq!(s.len(), 1);
        assert!(s.click_empty(Modifiers::NONE));
        assert!(s.is_empty());
    }

    #[test]
    fn keyboard_extension_keeps_anchor() {
        let mut s = Selection::new();
        s.move_focus(4, false);
        s.move_focus(6, true);
        assert_eq!(sel(&s), [4, 5, 6]);
        s.move_focus(2, true);
        assert_eq!(sel(&s), [2, 3, 4]);
        s.move_focus(9, false);
        assert_eq!(sel(&s), [9]);
    }

    #[test]
    fn rubber_band_replaces_or_toggles() {
        let mut s = Selection::new();
        s.mouse_down(1, Modifiers::NONE);
        s.rubber_band_begin(Modifiers::NONE);
        s.rubber_band_update(&[3, 4], Modifiers::NONE);
        assert_eq!(sel(&s), [3, 4]);
        s.rubber_band_end();
        s.rubber_band_begin(Modifiers::PRIMARY);
        s.rubber_band_update(&[4, 5], Modifiers::PRIMARY);
        assert_eq!(sel(&s), [3, 5]);
        s.rubber_band_update(&[], Modifiers::PRIMARY);
        assert_eq!(sel(&s), [3, 4]);
    }

    #[test]
    fn indexes_follow_model_changes() {
        let mut s = Selection::new();
        s.mouse_down(2, Modifiers::NONE);
        s.mouse_down(4, Modifiers::PRIMARY);
        s.on_insert(0);
        assert_eq!(sel(&s), [3, 5]);
        s.on_remove(3);
        assert_eq!(sel(&s), [4]);
        s.on_move(4, 0);
        assert_eq!(sel(&s), [0]);
        assert_eq!(s.focus(), Some(0));
    }
}
