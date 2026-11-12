//! The toolbar search field's width. At rest it is a round button [`window::TOOLBAR_SEARCH_FIELD`] wide. While it has
//! the keyboard or holds text it is open: [`window::SEARCH_OPEN`] wide, but never more than
//! [`window::SEARCH_MAX_SHARE`] of the toolbar, and never over the back and forward buttons, the [`window::TITLE_MIN`]
//! the title keeps or the controls after the title. The width changes over [`window::SEARCH_GROW_SECONDS`] with an ease-out, or at once with reduce motion.
//! Times are seconds on the window's clock.

use crate::metrics::window;

/// The resting width.
pub const REST: f64 = window::TOOLBAR_SEARCH_FIELD.0;

/// Whether the field is open: it has the keyboard or holds text.
pub fn is_open(focused: bool, text: &str) -> bool {
    focused || !text.is_empty()
}

/// The width of the toolbar's controls between the title and the open field, with the gap after each: the four views
/// are one pop-up while the field is open.
pub fn controls() -> f64 {
    [window::TOOLBAR_VIEW_MENU, window::TOOLBAR_ACTIONS]
        .iter()
        .map(|(w, _)| w + window::TOOLBAR_ITEM_GAP)
        .sum()
}

/// The open width in a toolbar `toolbar` points wide, whose items before the field are the back and forward buttons,
/// the title and the controls after it. It is never less than the resting width; a toolbar too narrow for that drops
/// nothing and leaves the field at rest.
pub fn open_width(toolbar: f64) -> f64 {
    let (back_forward, _) = window::TOOLBAR_BACK_FORWARD;
    let before = back_forward
        + window::TOOLBAR_ITEM_GAP
        + window::TITLE_MIN
        + window::TOOLBAR_ITEM_GAP
        + controls();
    let room = toolbar - 2.0 * window::TOOLBAR_INSET - before;
    window::SEARCH_OPEN
        .min(toolbar * window::SEARCH_MAX_SHARE)
        .min(room)
        .max(REST)
}

/// The field's width over time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SearchWidth {
    open: bool,
    /// The change asked for that the next [`tick`](Self::tick) starts.
    pending: bool,
    from: f64,
    to: f64,
    start: f64,
    now: f64,
}

impl Default for SearchWidth {
    fn default() -> Self {
        SearchWidth {
            open: false,
            pending: false,
            from: REST,
            to: REST,
            start: 0.0,
            now: 0.0,
        }
    }
}

impl SearchWidth {
    /// The field should be open, or at rest; the change starts at the next tick.
    pub fn want(&mut self, open: bool) {
        if open != self.open {
            self.open = open;
            self.pending = true;
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The clock reads `now` in a toolbar `toolbar` wide: a change asked for starts from the width shown, animated
    /// unless `animate` is false. A toolbar that changes size moves an open field's width at once.
    pub fn tick(&mut self, now: f64, toolbar: f64, animate: bool) {
        let target = if self.open { open_width(toolbar) } else { REST };
        if self.pending {
            self.from = if animate { self.width_at(now) } else { target };
            self.to = target;
            self.start = now;
            self.pending = false;
        } else if target != self.to {
            let shown = self.width_at(now);
            self.to = target;
            if !animate || self.done_at(now) {
                self.from = target;
            } else {
                self.from = shown;
                self.start = now;
            }
        }
        self.now = now;
    }

    /// The toolbar is now `toolbar` wide: an open field follows it at once. A change waiting for its tick waits on.
    pub fn follow_toolbar(&mut self, toolbar: f64) {
        if !self.pending {
            self.tick(self.now, toolbar, false);
        }
    }

    /// The width at the last tick.
    pub fn width(&self) -> f64 {
        self.width_at(self.now)
    }

    /// A change is waiting for its first tick or still running at the last one.
    pub fn animating(&self) -> bool {
        self.pending || !self.done_at(self.now)
    }

    fn done_at(&self, now: f64) -> bool {
        self.from == self.to || now - self.start >= window::SEARCH_GROW_SECONDS
    }

    fn width_at(&self, now: f64) -> f64 {
        let t = ((now - self.start) / window::SEARCH_GROW_SECONDS).clamp(0.0, 1.0);
        let eased = 1.0 - (1.0 - t).powi(3);
        self.from + (self.to - self.from) * eased
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIDE: f64 = 800.0;
    const OPEN: f64 = window::SEARCH_OPEN;

    #[test]
    fn the_field_is_open_with_the_keyboard_or_text() {
        assert!(!is_open(false, ""), "resting");
        assert!(is_open(true, ""), "focused and empty");
        assert!(is_open(false, "report"), "unfocused with text");
        assert!(is_open(true, "report"), "focused with text");
    }

    #[test]
    fn the_open_width_is_its_own_within_half_the_toolbar_and_the_title() {
        assert_eq!(open_width(WIDE), OPEN);
        // 500 pt: the group, the title's room, the controls and the gaps leave 500 - 16 - 321.5
        assert_eq!(open_width(500.0), 162.5);
        // 360 pt: they leave 22.5, less than the rest: the field stays at rest
        assert_eq!(open_width(360.0), REST);
        for toolbar in [200.0, 300.0, 400.0, 600.0, 1000.0, 2000.0] {
            let w = open_width(toolbar);
            assert!((REST..=OPEN).contains(&w), "{toolbar}: {w}");
            assert!(
                w <= toolbar * window::SEARCH_MAX_SHARE || w == REST,
                "{toolbar}: {w}"
            );
            let room = toolbar - 2.0 * window::TOOLBAR_INSET - 73.0 - 8.0 - 60.0 - 8.0 - 172.5;
            assert!(w <= room.max(REST), "{toolbar}: {w} passes the title");
        }
    }

    #[test]
    fn opening_eases_out_over_a_fifth_of_a_second() {
        let mut width = SearchWidth::default();
        width.tick(1.0, WIDE, true);
        assert_eq!(width.width(), REST);
        assert!(!width.animating());
        width.want(true);
        assert!(width.animating(), "waits for its first tick");
        width.tick(2.0, WIDE, true);
        assert_eq!(width.width(), REST);
        width.tick(2.1, WIDE, true);
        let half = width.width();
        // an ease-out has covered more than half the way at half the time
        assert!(half > REST + (OPEN - REST) / 2.0 && half < OPEN, "{half}");
        assert!(width.animating());
        width.tick(2.2, WIDE, true);
        assert_eq!(width.width(), OPEN);
        assert!(!width.animating());
    }

    #[test]
    fn closing_mid_flight_starts_from_the_width_shown() {
        let mut width = SearchWidth::default();
        width.want(true);
        width.tick(0.0, WIDE, true);
        width.tick(0.05, WIDE, true);
        let shown = width.width();
        width.want(false);
        width.tick(0.05, WIDE, true);
        assert_eq!(width.width(), shown);
        width.tick(0.25, WIDE, true);
        assert_eq!(width.width(), REST);
    }

    #[test]
    fn with_reduce_motion_the_width_changes_at_once() {
        let mut width = SearchWidth::default();
        width.want(true);
        width.tick(3.0, WIDE, false);
        assert_eq!(width.width(), OPEN);
        assert!(!width.animating());
        width.want(false);
        width.tick(3.0, WIDE, false);
        assert_eq!(width.width(), REST);
    }

    #[test]
    fn an_open_field_follows_the_toolbar_at_once() {
        let mut width = SearchWidth::default();
        width.want(true);
        width.tick(0.0, WIDE, true);
        width.tick(1.0, WIDE, true);
        width.tick(1.5, 500.0, true);
        assert_eq!(width.width(), open_width(500.0));
        assert!(width.width() < OPEN);
        assert!(!width.animating());
    }
}
