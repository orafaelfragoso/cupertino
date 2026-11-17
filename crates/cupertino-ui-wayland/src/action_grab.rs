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
