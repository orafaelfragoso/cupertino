//! The server's side of the global shortcuts, on the process's connection to the server, which offers
//! `cupertino_global_shortcuts_v1` only to the connection it started the process with. The D-Bus side's [`Keys`]
//! become `bind` and `unbind`; the server's `reserved` groups, `pressed` and `released` go to the sessions through the
//! [`Handle`].

use std::sync::mpsc;

use cupertino_protocols::client::global_shortcuts::cupertino_global_shortcuts_v1::{
    self, CupertinoGlobalShortcutsV1, Modifier,
};
use cupertino_wayland_client::{Connection, Event, State as Wayland};
use wayland_client::{Dispatch, QueueHandle};

use crate::global_shortcuts::{Handle, Keys, Trigger};

/// User data of the binding object.
pub struct Tag;

/// What the object received, handed on by [`Wayland::emit`].
enum Wire {
    Reserved(Trigger),
    Done,
    Fired { key: u32, pressed: bool, time: u32 },
}

pub struct ShortcutKeys {
    object: CupertinoGlobalShortcutsV1,
    requests: mpsc::Receiver<Keys>,
    /// The reserved chords of the group being sent.
    group: Vec<Trigger>,
}

impl ShortcutKeys {
    /// Bind the global on `conn`; `None` when the server does not offer it to this process. [`Keys`] reach it through
    /// `requests`, each followed by a wake of the connection.
    pub fn bind(conn: &mut Connection, requests: mpsc::Receiver<Keys>) -> Option<ShortcutKeys> {
        let object = conn.bind_extension(1..=1, Tag)?;
        Some(ShortcutKeys {
            object,
            requests,
            group: Vec::new(),
        })
    }

    /// Follow what `events` (one wait of the connection) brought and send what the D-Bus side asked.
    pub fn update(&mut self, events: &[Event], handle: &Handle, bus: &zbus::blocking::Connection) {
        for event in events {
            let Event::Extension(event) = event else {
                continue;
            };
            match event.downcast_ref::<Wire>() {
                Some(Wire::Reserved(trigger)) => self.group.push(*trigger),
                Some(Wire::Done) => handle.reserved(bus, std::mem::take(&mut self.group)),
                Some(Wire::Fired { key, pressed, time }) => {
                    handle.fired(bus, *key, *pressed, *time)
                }
                None => {}
            }
        }
        while let Ok(request) = self.requests.try_recv() {
            match request {
                Keys::Bind { key, trigger } => self.object.bind(
                    key,
                    Modifier::from_bits_truncate(trigger.modifiers),
                    trigger.keysym,
                ),
                Keys::Unbind { key } => self.object.unbind(key),
            }
        }
    }
}

impl Dispatch<CupertinoGlobalShortcutsV1, Tag> for Wayland {
    fn event(
        state: &mut Self,
        _: &CupertinoGlobalShortcutsV1,
        event: cupertino_global_shortcuts_v1::Event,
        _: &Tag,
        _: &wayland_client::Connection,
        _: &QueueHandle<Self>,
    ) {
        let wire = match event {
            cupertino_global_shortcuts_v1::Event::Reserved { modifiers, keysym } => {
                Wire::Reserved(Trigger {
                    modifiers: modifiers.into(),
                    keysym,
                })
            }
            cupertino_global_shortcuts_v1::Event::Done => Wire::Done,
            cupertino_global_shortcuts_v1::Event::Pressed { id, time } => Wire::Fired {
                key: id,
                pressed: true,
                time,
            },
            cupertino_global_shortcuts_v1::Event::Released { id, time } => Wire::Fired {
                key: id,
                pressed: false,
                time,
            },
            _ => return,
        };
        state.emit(wire);
    }
}
