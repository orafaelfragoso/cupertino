//! What a press on an item means besides selecting it, in the icon grid and the list: the second press on the same item
//! within the double-click interval opens it, and a press on the name of the item that was the only one selected starts
//! renaming it once the interval passes without another press ([`RenameTrigger`]). A drag, a key, a scroll or another
//! folder cancels the rename.

use cupertino_kit::core::EventCx;
use cupertino_kit::platform::Timestamp;

use super::{Event as WindowEvent, Message};
use crate::metrics::timing::DOUBLE_CLICK_INTERVAL;
use crate::rename::{RenameEvent, RenameTrigger};

fn seconds(at: Timestamp) -> f64 {
    at.0 as f64 / 1e9
}

/// The presses of a view, in its node state.
#[derive(Debug, Default)]
pub(super) struct Clicks {
    /// The item last pressed, and when.
    last: Option<(usize, f64)>,
    rename: RenameTrigger,
}

impl Clicks {
    /// A press on item `index` at `now`: `on_name` when it hit the item's name, `sole` when the item was the only one
    /// selected before it, `plain` without Shift or Primary held. Whether it is the second press of a double click,
    /// which opens the item. A press with Shift or Primary only changes the selection: it neither opens nor renames,
    /// and the press after it starts again.
    pub fn press(
        &mut self,
        index: usize,
        on_name: bool,
        sole: bool,
        plain: bool,
        now: Timestamp,
    ) -> bool {
        if !plain {
            self.press_empty();
            return false;
        }
        let at = seconds(now);
        let double = self
            .last
            .is_some_and(|(i, t)| i == index && at - t <= DOUBLE_CLICK_INTERVAL);
        if double {
            self.last = None;
            self.rename.cancel();
            return true;
        }
        self.last = Some((index, at));
        self.rename.click(index, on_name, sole, at);
        false
    }

    /// A press on no item: the next press on an item starts again.
    pub fn press_empty(&mut self) {
        self.last = None;
        self.rename.cancel();
    }

    /// A drag, a key or a scroll: no rename starts.
    pub fn cancel(&mut self) {
        self.rename.cancel();
    }

    /// Ask the view's node to wake once a pending rename is due.
    pub fn arm(&self, cx: &mut EventCx<'_, Message>) {
        if let Some(deadline) = self.rename.deadline() {
            // the trigger fires strictly after its interval
            cx.wake_at(Timestamp((deadline * 1e9) as u64 + 1));
        }
    }

    /// At a wake at `now`: start the rename that is due, in the window `window`.
    pub fn wake(&mut self, window: u64, now: Timestamp, cx: &mut EventCx<'_, Message>) {
        if let Some(RenameEvent::Begin(index)) = self.rename.poll(seconds(now)) {
            cx.emit(Message::Window(window, WindowEvent::RenameItem(index)));
        }
    }

    #[cfg(test)]
    pub fn is_pending(&self) -> bool {
        self.rename.is_pending()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: f64) -> Timestamp {
        Timestamp((seconds * 1e9) as u64)
    }

    #[test]
    fn a_second_press_on_the_same_item_in_time_is_a_double_click_and_renames_nothing() {
        let mut clicks = Clicks::default();
        assert!(!clicks.press(3, true, true, true, at(10.0)));
        assert!(clicks.is_pending(), "the only selected item's name");
        assert!(clicks.press(3, true, true, true, at(10.3)));
        assert!(!clicks.is_pending());
        assert!(
            !clicks.press(3, true, true, true, at(10.5)),
            "a third press starts again"
        );
        assert!(!clicks.press(4, true, false, true, at(12.0)));
        assert!(
            !clicks.press(3, false, false, true, at(12.1)),
            "another item"
        );
        assert!(!clicks.press(3, false, false, true, at(13.0)), "too late");
        assert!(
            !clicks.press(3, false, false, false, at(13.1)),
            "with Shift or Primary"
        );
    }

    #[test]
    fn a_rename_waits_for_the_interval_and_needs_the_only_selected_items_name() {
        let mut clicks = Clicks::default();
        clicks.press(1, false, true, true, at(1.0));
        assert!(!clicks.is_pending(), "the icon, not the name");
        clicks.press(2, true, false, true, at(3.0));
        assert!(!clicks.is_pending(), "not selected alone before");
        clicks.press(2, true, true, true, at(5.0));
        assert!(clicks.is_pending());
        clicks.cancel();
        assert!(!clicks.is_pending());
    }
}
