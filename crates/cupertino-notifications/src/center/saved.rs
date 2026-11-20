//! What of the model outlives a restart of the component (D21): the notifications of the list, which the component
//! writes to a file as they change and reads back as it starts again ([`crate::store`]).

use super::{Entry, NotificationCenter};
use crate::notification::Notification;

/// A notification of the list, as it is kept across a restart.
#[derive(Clone, Debug, PartialEq)]
pub struct Kept {
    pub notification: Notification,
    pub read: bool,
    /// `NotificationClosed` was sent for it: it left the screen, and its sender heard so.
    pub closed_signalled: bool,
    /// It arrived while the session was locked, and the lock screen lists it.
    pub lock_screen: bool,
}

impl NotificationCenter {
    /// The notifications of the list, in the model's order: every one but the transient ones, which never enter the
    /// list. A banner on screen is kept as the list has it.
    pub fn kept(&self) -> Vec<Kept> {
        self.entries
            .iter()
            .filter(|e| e.in_centre && !e.n.transient)
            .map(|e| Kept {
                notification: e.n.clone(),
                read: e.read,
                closed_signalled: e.closed_signalled,
                lock_screen: e.locked_arrival,
            })
            .collect()
    }

    /// The id the next notification takes, unless another one has it.
    pub fn next_id(&self) -> u32 {
        self.next_id
    }

    /// Put back the list's notifications `kept` by an earlier run, before this one took any: in the list, none on
    /// screen. A notification whose id the model has already is left out. New notifications take ids from
    /// `next_id` on, or after the highest kept one, so an id the earlier run gave out is not given again soon.
    pub fn restore(&mut self, kept: Vec<Kept>, next_id: u32) {
        for k in kept {
            let id = k.notification.id;
            if id == 0 || self.get(id).is_some() {
                continue;
            }
            self.next_id = self.next_id.max(id.saturating_add(1));
            self.entries.push(Entry {
                n: k.notification,
                in_centre: true,
                read: k.read,
                closed_signalled: k.closed_signalled,
                banner: None,
                locked_arrival: k.lock_screen,
            });
        }
        self.next_id = self.next_id.max(next_id).max(1);
        self.enforce_history_limit();
        self.touch();
    }
}
