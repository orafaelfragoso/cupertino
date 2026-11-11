//! The grab serials desktop-control actions come with. A popup of the action's window tree grabs with one while the
//! action is handled and a moment after it was answered as applied, for a popup the action's work shows a little later
//! (a folder's stack once it is listed); any other answer ends it. The compositor takes a serial once, for the menu
//! that opens from a window: it is then spent, and only that menu's own popups, which grow its grab, use it.

use std::time::{Duration, Instant};

use cupertino_ui_platform::WindowId;

/// How long an action's grab serial is used after the action was answered as applied. The compositor keeps it good
/// this long after it reads the answer, which is later than it is sent.
pub(crate) const ANSWERED_GRAB_GRACE: Duration = Duration::from_secs(1);

struct Grant {
    /// The window the action was for, and the window its popup chain hangs from.
    window: WindowId,
    root: WindowId,
    call: u64,
    serial: u32,
    /// `None` while the action is handled.
    until: Option<Instant>,
    spent: bool,
}

#[derive(Default)]
pub(crate) struct ActionGrabs {
    grants: Vec<Grant>,
}

impl ActionGrabs {
    /// The action `call` for `window`, whose popup chain hangs from `root`, came with `serial`.
    pub(crate) fn record(&mut self, window: WindowId, root: WindowId, call: u64, serial: u32) {
        self.grants.push(Grant {
            window,
            root,
            call,
            serial,
            until: None,
            spent: false,
        });
    }

    /// The action `call` for `window` was answered at `now`, as applied or not.
    pub(crate) fn answered(&mut self, window: WindowId, call: u64, applied: bool, now: Instant) {
        self.grants.retain_mut(|g| {
            if (g.window, g.call) == (window, call) {
                if !applied {
                    return false;
                }
                g.until = Some(now + ANSWERED_GRAB_GRACE);
            }
            g.until.is_none_or(|until| now < until)
        });
    }

    /// The serial a popup grabs with at `now` when it opens from a window of the tree hanging from `root`: from
    /// `root` itself (`submenu` false) only a serial not spent yet, from one of its popups a spent one too.
    pub(crate) fn usable(&self, root: WindowId, submenu: bool, now: Instant) -> Option<u32> {
        self.grants
            .iter()
            .find(|g| {
                g.root == root && (submenu || !g.spent) && g.until.is_none_or(|until| now < until)
            })
            .map(|g| g.serial)
    }

    /// A menu grabbed with `serial` from a window: the compositor will not take it again.
    pub(crate) fn spend(&mut self, serial: u32) {
        for g in self.grants.iter_mut().filter(|g| g.serial == serial) {
            g.spent = true;
        }
    }

    /// `window` closed: the serials of actions for it, or for a window that hangs from it, are gone.
    pub(crate) fn forget(&mut self, window: WindowId) {
        self.grants
            .retain(|g| g.window != window && g.root != window);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BAR: WindowId = WindowId(1);
    const MENU: WindowId = WindowId(2);
    const OTHER: WindowId = WindowId(3);

    #[test]
    fn a_serial_is_usable_while_its_action_is_handled_and_only_in_its_window_tree() {
        let now = Instant::now();
        let mut grabs = ActionGrabs::default();
        assert_eq!(grabs.usable(BAR, false, now), None);
        grabs.record(BAR, BAR, 7, 40);
        assert_eq!(grabs.usable(BAR, false, now), Some(40));
        assert_eq!(grabs.usable(OTHER, false, now), None);
        // long after, the action is still being handled: the compositor ends it, not the client
        let later = now + Duration::from_secs(4);
        assert_eq!(grabs.usable(BAR, false, later), Some(40));
    }

    #[test]
    fn an_applied_answer_leaves_a_moment_and_any_other_ends_it_at_once() {
        let now = Instant::now();
        let mut grabs = ActionGrabs::default();
        grabs.record(BAR, BAR, 1, 40);
        grabs.record(BAR, BAR, 2, 41);
        grabs.answered(BAR, 1, true, now);
        assert_eq!(grabs.usable(BAR, false, now), Some(40));
        let just_before = now + ANSWERED_GRAB_GRACE - Duration::from_millis(1);
        assert_eq!(grabs.usable(BAR, false, just_before), Some(40));
        let after = now + ANSWERED_GRAB_GRACE;
        assert_eq!(
            grabs.usable(BAR, false, after),
            Some(41),
            "the other action is still handled"
        );
        grabs.answered(BAR, 2, false, now);
        assert_eq!(grabs.usable(BAR, false, after), None);
        // the next answer drops the serial whose moment passed
        grabs.record(BAR, BAR, 3, 42);
        grabs.answered(BAR, 3, false, after);
        assert!(grabs.grants.is_empty());
    }

    #[test]
    fn a_spent_serial_serves_only_the_menu_s_own_popups() {
        let now = Instant::now();
        let mut grabs = ActionGrabs::default();
        grabs.record(BAR, BAR, 1, 40);
        grabs.spend(40);
        assert_eq!(
            grabs.usable(BAR, false, now),
            None,
            "a second menu of the bar"
        );
        assert_eq!(
            grabs.usable(BAR, true, now),
            Some(40),
            "a submenu of the menu"
        );
    }

    #[test]
    fn a_closed_window_takes_the_serials_of_its_actions_and_of_its_tree() {
        let now = Instant::now();
        let mut grabs = ActionGrabs::default();
        grabs.record(MENU, BAR, 1, 40);
        grabs.record(OTHER, OTHER, 2, 41);
        grabs.forget(MENU);
        assert_eq!(grabs.usable(BAR, true, now), None);
        grabs.record(MENU, BAR, 3, 42);
        grabs.forget(BAR);
        assert_eq!(grabs.usable(BAR, true, now), None);
        assert_eq!(grabs.usable(OTHER, false, now), Some(41));
    }
}
