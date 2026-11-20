//! The calls of the bus interfaces as the model carries them out, do not disturb, and `notifications.conf`.
//!
//! `Notify` and `CloseNotification` are carried out one at a time, in the order they came: each is prepared on the
//! I/O pool, where its caller is named from `/proc`, its hints decoded and its desktop entry's icon looked up, and then
//! applied to the model. The other calls need nothing but the model, and are answered at once.

use std::sync::Arc;
use std::time::Duration;

use cupertino_defaults::notifications::NotificationSettings;
use cupertino_defaults::watch::ConfigWatcher;
use cupertino_kit::core::Command as Do;
use cupertino_kit::task;
use cupertino_peer::Caller;
use zbus::fdo::ConnectionCredentials;

use super::{Handed, Message, Model};
use crate::center::DndMode;
use crate::control::{self, Arrival, Call, DndChange};
use crate::focus::{self, SystemCalendar};
use crate::notification::NotifyInput;

/// How long naming a caller should take: a few small files of `/proc`, and a desktop entry for a notification.
const CALLER_DEADLINE: Duration = Duration::from_secs(1);
/// How long reading `notifications.conf` or starting to watch it should take: a small file on a local disk.
const SETTINGS_DEADLINE: Duration = Duration::from_secs(2);

/// A model action that waits for an activation token.
#[derive(Clone, Debug, PartialEq)]
pub enum Pending {
    Activate(u32),
    Invoke(u32, String),
}

impl Pending {
    pub fn id(&self) -> u32 {
        match self {
            Pending::Activate(id) | Pending::Invoke(id, _) => *id,
        }
    }
}

/// A call that changes the notifications, waiting to be prepared.
pub(super) enum Queued {
    Notify {
        credentials: Option<Arc<ConnectionCredentials>>,
        arrival: Arrival,
        reply: async_channel::Sender<u32>,
    },
    Close {
        credentials: Option<Arc<ConnectionCredentials>>,
        id: u32,
    },
}

/// A call that changes the notifications, its caller named.
#[derive(Clone, Debug)]
pub enum Prepared {
    /// Show `input`, which `caller` sent; its id goes to `reply`.
    Notify {
        caller: Arc<Caller>,
        input: Box<NotifyInput>,
        reply: async_channel::Sender<u32>,
    },
    /// Close notification `id`, if the caller may.
    Close { id: u32, allowed: bool },
}

impl Model {
    /// Carry out a call of the bus interfaces, or queue it.
    pub(super) fn call(&mut self, call: Call) -> Vec<Do<Message>> {
        match call {
            Call::Judge { credentials, reply } => vec![task::io(
                CALLER_DEADLINE,
                move || {
                    let caller = control::judge(credentials.as_deref());
                    // the call waits on the receiver; one that went away needs no answer
                    let _ = reply.try_send(Arc::new(caller));
                },
                |()| Message::Judged,
            )],
            Call::Notify {
                credentials,
                arrival,
                reply,
            } => {
                self.queue.push_back(Queued::Notify {
                    credentials,
                    arrival,
                    reply,
                });
                self.next_call()
            }
            Call::Close { credentials, id } => {
                self.queue.push_back(Queued::Close { credentials, id });
                self.next_call()
            }
            Call::Toggle => self.toggle(None),
            Call::Dnd(change) => {
                self.set_dnd(change);
                Vec::new()
            }
            Call::LockScreen { reply } => {
                let _ = reply.try_send(control::lock_rows(&self.center.borrow().lock_screen()));
                Vec::new()
            }
            Call::Applications { reply } => {
                let _ = reply.try_send(self.center.borrow().applications());
                Vec::new()
            }
        }
    }

    /// Prepare the next queued call, when none is being prepared.
    fn next_call(&mut self) -> Vec<Do<Message>> {
        if self.preparing {
            return Vec::new();
        }
        let Some(queued) = self.queue.pop_front() else {
            return Vec::new();
        };
        self.preparing = true;
        let command = match queued {
            Queued::Notify {
                credentials,
                arrival,
                reply,
            } => {
                let origin = self.origins.get(&arrival.replaces_id).cloned();
                task::io(
                    CALLER_DEADLINE,
                    move || {
                        control::prepare_notify(credentials.as_deref(), arrival, origin.as_deref())
                    },
                    move |(caller, input)| {
                        Message::Prepared(Prepared::Notify {
                            caller: Arc::new(caller),
                            input: Box::new(input),
                            reply,
                        })
                    },
                )
            }
            Queued::Close { credentials, id } => {
                let origin = self.origins.get(&id).cloned();
                task::io(
                    CALLER_DEADLINE,
                    move || control::may_close(credentials.as_deref(), origin.as_deref()),
                    move |allowed| Message::Prepared(Prepared::Close { id, allowed }),
                )
            }
        };
        vec![command]
    }

    /// Apply a prepared call, and prepare the next one.
    pub(super) fn prepared(&mut self, prepared: Prepared) -> Vec<Do<Message>> {
        self.preparing = false;
        let now = self.now();
        let center = self.center.get_mut();
        match prepared {
            Prepared::Notify {
                caller,
                input,
                reply,
            } => {
                let id = center.notify(*input, now);
                // the notifications that are gone take their senders with them
                self.origins.retain(|id, _| center.get(*id).is_some());
                if control::sandboxed(&caller).is_some() {
                    self.origins.insert(id, caller);
                } else {
                    self.origins.remove(&id);
                }
                let _ = reply.try_send(id);
            }
            // a notification the caller may not change stays, as if it did not exist
            Prepared::Close { id, allowed } => {
                if allowed {
                    center.close(id, now);
                }
            }
        }
        self.next_call()
    }

    /// Change do not disturb; a duration ends at the time the session's calendar gives it.
    pub(super) fn set_dnd(&mut self, change: DndChange) {
        let now = self.now();
        let mode = match change {
            DndChange::Off => DndMode::Off,
            DndChange::On => DndMode::On,
            DndChange::For(lifetime) => {
                let Ok(unix) = control::unix_now() else {
                    eprintln!("cupertino-notifications: the clock is before 1970; do not disturb is unchanged");
                    return;
                };
                let ends = focus::end(lifetime, unix, &SystemCalendar);
                DndMode::Until(now + (ends - unix) as f64)
            }
        };
        self.center().set_dnd(mode, now);
    }

    /// `notifications.conf` as it is now; the watch it came with follows its changes from now on.
    pub(super) fn policy(
        &mut self,
        settings: NotificationSettings,
        watch: Option<Handed<ConfigWatcher>>,
    ) -> Vec<Do<Message>> {
        self.center().set_policy(settings);
        let Some(watch) = watch.and_then(|watch| watch.take()) else {
            return Vec::new();
        };
        let changed = self.policy.0.clone();
        vec![Do::Perform(Box::pin(task::follow_settings(
            watch,
            move |_| {
                // the model holds the receiver for as long as it lives
                let _ = changed.try_send(());
            },
        )))]
    }

    /// Read `notifications.conf` again; a line not understood keeps its value.
    pub(super) fn reload_policy(&self) -> Do<Message> {
        let last = self.center.borrow().policy().clone();
        task::io(
            SETTINGS_DEADLINE,
            move || load_policy(Some(&last)),
            |settings| Message::Policy(settings, None),
        )
    }
}

/// Read `notifications.conf` and start watching it. Without a watch its changes apply at the next start.
pub(super) fn first_policy() -> Do<Message> {
    task::io(
        SETTINGS_DEADLINE,
        || {
            let settings = load_policy(None);
            let watch = NotificationSettings::path().and_then(|path| {
                ConfigWatcher::new(vec![path.clone()])
                    .map_err(|e| {
                        eprintln!(
                            "cupertino-notifications: cannot watch {}: {e}; changes apply at the next start",
                            path.display()
                        );
                    })
                    .ok()
            });
            (settings, watch)
        },
        |(settings, watch)| Message::Policy(settings, watch.map(Handed::new)),
    )
}

/// `notifications.conf` as it is now: its lines that are not understood are reported, and keep the value of `last`.
/// A file that cannot be read keeps `last` (the defaults at start). For the I/O pool.
fn load_policy(last: Option<&NotificationSettings>) -> NotificationSettings {
    let kept = || last.cloned().unwrap_or_default();
    let Some(path) = NotificationSettings::path() else {
        return kept();
    };
    match NotificationSettings::load(&path, last) {
        Ok((settings, problems)) => {
            for p in problems {
                eprintln!(
                    "cupertino-notifications: {} line {}: {}",
                    path.display(),
                    p.line,
                    p.message
                );
            }
            settings
        }
        Err(e) => {
            eprintln!(
                "cupertino-notifications: cannot read {}: {e}",
                path.display()
            );
            kept()
        }
    }
}
