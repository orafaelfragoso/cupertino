//! What a press on an item means besides selecting it, in the icon grid and the list: the second press on the same item
//! within the double-click interval opens it, and a press on the name of the item that was the only one selected starts
//! renaming it once the interval passes without another press ([`RenameTrigger`]). A drag, a key, a scroll or another
//! folder cancels the rename.

use std::rc::Rc;

use cupertino_ui_core::platform::Timestamp;
use cupertino_ui_core::EventCx;

use crate::metrics::timing::DOUBLE_CLICK_INTERVAL;
use crate::rename::{RenameEvent, RenameTrigger};
use crate::ViewEvent;

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
    pub fn arm<M>(&self, cx: &mut EventCx<'_, M>) {
        if let Some(deadline) = self.rename.deadline() {
            // the trigger fires strictly after its interval
            cx.wake_at(Timestamp((deadline * 1e9) as u64 + 1));
        }
    }

    /// At a wake at `now`: start the rename that is due, reported through `on_event`. While the press is `held` it
    /// waits: the press may still become a drag, and the release [arms](Self::arm) it again.
    pub fn wake<M>(
        &mut self,
        on_event: &Rc<dyn Fn(ViewEvent) -> Option<M>>,
        now: Timestamp,
        held: bool,
        cx: &mut EventCx<'_, M>,
    ) {
        if held {
            return;
        }
        if let Some(RenameEvent::Begin(index)) = self.rename.poll(seconds(now)) {
            if let Some(message) = on_event(ViewEvent::Rename(index)) {
                cx.emit(message);
            }
        }
    }
}
