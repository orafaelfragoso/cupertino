//! A window's tabs without a toolkit: their order, which one is shown, where a new one goes and which one is shown after
//! one closes, the room each takes in the tab bar's track and where a tab dragged along the track lands.
//!
//! A new tab opens after the one shown and is shown. Closing the tab shown shows the one after it, else the one before
//! it. The tabs share the track's width equally. A tab dragged along the track takes the place its middle is over.

/// A tab's identity, which stays while tabs open, close and move.
pub type TabId = u64;

/// The tabs of a window, in order, and the one shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabStrip {
    order: Vec<TabId>,
    active: usize,
    next: TabId,
}

impl Default for TabStrip {
    fn default() -> Self {
        TabStrip::new()
    }
}

impl TabStrip {
    /// One tab, shown.
    pub fn new() -> Self {
        TabStrip {
            order: vec![0],
            active: 0,
            next: 1,
        }
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    /// Never: a window keeps at least one tab.
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// The tabs, in order.
    pub fn order(&self) -> &[TabId] {
        &self.order
    }

    /// The place of the tab shown.
    pub fn active(&self) -> usize {
        self.active
    }

    /// The tab shown.
    pub fn active_id(&self) -> TabId {
        self.order[self.active]
    }

    /// The place of tab `id`.
    pub fn position(&self, id: TabId) -> Option<usize> {
        self.order.iter().position(|&t| t == id)
    }

    /// Open a tab after the one shown and show it; its id.
    pub fn open(&mut self) -> TabId {
        let id = self.next;
        self.next += 1;
        self.active += 1;
        self.order.insert(self.active, id);
        id
    }

    /// Close tab `id`; the tab shown afterwards, `None` when it was the last tab (the window closes instead) or is not
    /// a tab of the window.
    pub fn close(&mut self, id: TabId) -> Option<TabId> {
        let at = self.position(id)?;
        if self.order.len() == 1 {
            return None;
        }
        self.order.remove(at);
        if at < self.active || self.active == self.order.len() {
            self.active -= 1;
        }
        Some(self.active_id())
    }

    /// Show tab `id`; whether it is a tab of the window.
    pub fn select(&mut self, id: TabId) -> bool {
        match self.position(id) {
            Some(at) => {
                self.active = at;
                true
            }
            None => false,
        }
    }

    /// Show the next tab (`by` 1) or the previous one (-1), going round.
    pub fn cycle(&mut self, by: isize) -> TabId {
        let n = self.order.len() as isize;
        self.active = (self.active as isize + by).rem_euclid(n) as usize;
        self.active_id()
    }

    /// Move tab `id` to place `to`; the tab shown stays shown.
    pub fn move_to(&mut self, id: TabId, to: usize) {
        let Some(from) = self.position(id) else {
            return;
        };
        let shown = self.active_id();
        let tab = self.order.remove(from);
        let to = to.min(self.order.len());
        self.order.insert(to, tab);
        self.active = self.position(shown).expect("the shown tab stays");
    }
}

/// The leading edge and width of each of `count` tabs sharing a track `width` wide.
pub fn tab_frames(count: usize, width: f64) -> Vec<(f64, f64)> {
    if count == 0 {
        return Vec::new();
    }
    let each = width / count as f64;
    (0..count).map(|i| (i as f64 * each, each)).collect()
}

/// The place a tab dragged so its middle is `x` along a track `width` wide holding `count` tabs takes.
pub fn drop_place(x: f64, count: usize, width: f64) -> usize {
    if count == 0 || width <= 0.0 {
        return 0;
    }
    let each = width / count as f64;
    ((x / each).floor().max(0.0) as usize).min(count - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_tab_opens_after_the_shown_one_and_is_shown() {
        let mut tabs = TabStrip::new();
        let b = tabs.open();
        assert_eq!((tabs.order(), tabs.active_id()), (&[0, b][..], b));
        tabs.select(0);
        let c = tabs.open();
        assert_eq!(tabs.order(), [0, c, b]);
        assert_eq!(tabs.active(), 1);
    }

    #[test]
    fn closing_shows_the_next_tab_else_the_previous_and_the_last_tab_closes_the_window() {
        let mut tabs = TabStrip::new();
        let b = tabs.open();
        let c = tabs.open();
        tabs.select(b);
        assert_eq!(tabs.close(b), Some(c));
        assert_eq!(tabs.close(c), Some(0));
        assert_eq!(tabs.close(0), None);
        assert_eq!(tabs.len(), 1);
        // closing a tab before the shown one keeps the shown one
        let d = tabs.open();
        assert_eq!(tabs.close(0), Some(d));
        assert_eq!(tabs.close(42), None);
    }

    #[test]
    fn tabs_move_and_cycle_and_the_shown_tab_stays_shown() {
        let mut tabs = TabStrip::new();
        let b = tabs.open();
        let c = tabs.open();
        tabs.move_to(c, 0);
        assert_eq!(tabs.order(), [c, 0, b]);
        assert_eq!(tabs.active_id(), c);
        assert_eq!(tabs.cycle(-1), b);
        assert_eq!(tabs.cycle(1), c);
        tabs.move_to(c, 99);
        assert_eq!(tabs.order(), [0, b, c]);
    }

    #[test]
    fn the_track_is_shared_and_a_dragged_tab_lands_under_its_middle() {
        assert_eq!(tab_frames(2, 600.0), [(0.0, 300.0), (300.0, 300.0)]);
        assert_eq!(drop_place(-20.0, 3, 600.0), 0);
        assert_eq!(drop_place(250.0, 3, 600.0), 1);
        assert_eq!(drop_place(900.0, 3, 600.0), 2);
    }
}
