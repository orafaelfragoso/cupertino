//! The tip of a status item: it shows once the pointer has rested on the item for [`ControllerConfig::tooltip_delay`]
//! with no menu open, and goes when the pointer leaves the item or the bar, a button is pressed, the wheel turns, a key
//! is pressed or a menu opens; it comes again only once the pointer has left the item and come back. Which items have
//! a tip, and its text, is the view's to say ([`crate::dbus::sni::StatusItem::tooltip_text`]).

use super::*;

/// How long the pointer rests on a status item before its tip shows, in seconds: the toolkit's tooltip delay, so the
/// bar's tips come as the applications' do.
pub const STATUS_TOOLTIP_DELAY: f64 = 0.6;

/// The status item under the pointer, for its tip.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Hover {
    /// The item, and when the pointer came onto it.
    slot: Option<(SlotKey, f64)>,
    /// The tip shows.
    shown: bool,
    /// A press, a scroll or a key put the tip away until the pointer leaves the item.
    dismissed: bool,
}

impl MenuController {
    /// The status item whose tip shows: the pointer rested on it long enough and nothing put the tip away since.
    pub fn tooltip(&self) -> Option<&SlotKey> {
        let hover = &self.hover;
        let shows = hover.shown && !hover.dismissed && !self.is_open();
        hover.slot.as_ref().map(|(slot, _)| slot).filter(|_| shows)
    }

    /// The pointer left the bar's window (onto the desktop or a window below it): a status item's tip goes.
    pub fn pointer_leave(&mut self, now: f64) -> Outcome {
        self.at(now);
        let mut out = Outcome::default();
        self.hover_off(&mut out);
        self.synced(out)
    }

    /// The tip itself was reached by the pointer or dismissed: it goes until the pointer leaves the item.
    pub fn dismiss_tooltip(&mut self) -> Outcome {
        let mut out = Outcome::default();
        self.hover_dismissed(&mut out);
        self.synced(out)
    }

    /// When the tip of the item under the pointer is due, while it waits to show.
    pub(super) fn tooltip_deadline(&self) -> Option<f64> {
        let hover = &self.hover;
        let waits = !hover.shown && !hover.dismissed && !self.is_open();
        hover
            .slot
            .as_ref()
            .filter(|_| waits)
            .map(|(_, since)| since + self.cfg.tooltip_delay)
    }

    /// The pointer moved to `pos` at `now`: onto a status item its tip waits for the pointer to rest; elsewhere the tip
    /// goes.
    pub(super) fn hover_moved(&mut self, pos: Point, now: f64, out: &mut Outcome) {
        let on = match self.layout.hit(pos) {
            Some(slot) if matches!(slot.key, SlotKey::Status(_)) => Some(slot.key.clone()),
            _ => None,
        };
        let same = self.hover.slot.as_ref().map(|(slot, _)| slot) == on.as_ref();
        if same {
            return;
        }
        self.hover_off(out);
        self.hover.slot = on.map(|slot| (slot, now));
    }

    /// A press, a scroll or a key: the tip goes, and stays away while the pointer stays on the item.
    pub(super) fn hover_dismissed(&mut self, out: &mut Outcome) {
        if self.tooltip().is_some() {
            out.effects.push(Effect::Redraw);
        }
        self.hover.shown = false;
        self.hover.dismissed = self.hover.slot.is_some();
    }

    /// The timers ran to `now`: the tip that was due shows.
    pub(super) fn hover_ticked(&mut self, now: f64, out: &mut Outcome) {
        if self.tooltip_deadline().is_some_and(|due| due <= now) {
            self.hover.shown = true;
            out.effects.push(Effect::Redraw);
        }
    }

    pub(super) fn hover_off(&mut self, out: &mut Outcome) {
        if self.tooltip().is_some() {
            out.effects.push(Effect::Redraw);
        }
        self.hover = Hover::default();
    }
}
