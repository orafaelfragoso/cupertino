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
