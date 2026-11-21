//! `cupertino_global_shortcuts_v1`: the chords of the shortcuts the person allowed applications through the desktop
//! portal. The portal process, the only connection with [`ShellRole::GlobalShortcuts`], binds each chord here; the key
//! router ([`crate::input::KeyRouter::global`]) takes a bound chord where none of the server's own bindings does, before
//! the focused client sees it, and its press and release reach the portal as `pressed` and `released`. The portal
//! needs the chords the server keeps for itself to refuse a shortcut on one of them, so they are sent at bind and again
//! whenever the bindings change.

use cupertino_protocols::server::global_shortcuts::cupertino_global_shortcuts_v1::{
    self, CupertinoGlobalShortcutsV1,
};
use smithay::reexports::wayland_server::backend::{ClientId, GlobalId};
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
};

use crate::authorization::{authorized, ShellRole};
use crate::input::{normalize_sym, Chord, GlobalKey};
use crate::state::Cupertino;

pub use cupertino_server_input::global_shortcuts::{
    is_modifier_key, modifier_bits, modifiers_of, reserved,
};

/// Version of the global.
const VERSION: u32 = 1;

/// The global and the objects bound to it.
pub struct GlobalShortcuts {
    _global: GlobalId,
    /// Each bound object with the number its shortcuts are known by in the key router.
    bound: Vec<(CupertinoGlobalShortcutsV1, u32)>,
    next_owner: u32,
}

impl GlobalShortcuts {
    pub fn new(dh: &DisplayHandle) -> Self {
        Self {
            _global: dh.create_global::<Cupertino, CupertinoGlobalShortcutsV1, ()>(VERSION, ()),
            bound: Vec::new(),
            next_owner: 0,
        }
    }
}

fn send_reserved(resource: &CupertinoGlobalShortcutsV1, chords: &[Chord]) {
    for chord in chords {
        resource.reserved(modifier_bits(chord.mods), chord.sym);
    }
    resource.done();
}

impl Cupertino {
    /// The chord of the portal's shortcut `key` went down or its key up: tell the object that bound it, if it is still
    /// there.
    pub fn global_shortcut(&mut self, key: GlobalKey, pressed: bool) {
        let time = (self.now() * 1000.0) as u32;
        let Some((resource, _)) = self
            .global_shortcuts
            .bound
            .iter()
            .find(|(_, owner)| *owner == key.owner)
        else {
            return;
        };
        if pressed {
            resource.pressed(key.id, time);
        } else {
            resource.released(key.id, time);
        }
    }

    /// Send the chords the server keeps to every bound object again, after its bindings changed.
    pub fn refresh_global_shortcut_reserved(&mut self) {
        let chords = reserved(&self.key_router.bindings);
        for (resource, _) in &self.global_shortcuts.bound {
            send_reserved(resource, &chords);
        }
    }
}

impl GlobalDispatch<CupertinoGlobalShortcutsV1, ()> for Cupertino {
    fn bind(
        state: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<CupertinoGlobalShortcutsV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        let owner = state.global_shortcuts.next_owner;
        state.global_shortcuts.next_owner = owner.wrapping_add(1);
        let resource = init.init(resource, owner);
        send_reserved(&resource, &reserved(&state.key_router.bindings));
        state.global_shortcuts.bound.push((resource, owner));
    }

    fn can_view(client: Client, _: &()) -> bool {
        authorized(&client, ShellRole::GlobalShortcuts)
    }
}

impl Dispatch<CupertinoGlobalShortcutsV1, u32> for Cupertino {
    fn request(
        state: &mut Self,
        client: &Client,
        _: &CupertinoGlobalShortcutsV1,
        request: cupertino_global_shortcuts_v1::Request,
        owner: &u32,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        // the role is checked again on every request: a process that lost it binds nothing more
        let allowed = authorized(client, ShellRole::GlobalShortcuts);
        let global = &mut state.key_router.global;
        match request {
            cupertino_global_shortcuts_v1::Request::Bind {
                id,
                modifiers,
                keysym,
            } => {
                let key = GlobalKey { owner: *owner, id };
                global.retain(|_, k| *k != key);
                if allowed {
                    let chord = Chord {
                        mods: modifiers_of(modifiers.into()),
                        sym: normalize_sym(keysym),
                    };
                    global.insert(chord, key);
                }
            }
            cupertino_global_shortcuts_v1::Request::Unbind { id } => {
                let key = GlobalKey { owner: *owner, id };
                global.retain(|_, k| *k != key);
            }
            cupertino_global_shortcuts_v1::Request::Destroy => {}
            _ => {}
        }
    }

    fn destroyed(
        state: &mut Self,
        _: ClientId,
        resource: &CupertinoGlobalShortcutsV1,
        owner: &u32,
    ) {
        state.key_router.global.retain(|_, k| k.owner != *owner);
        state
            .global_shortcuts
            .bound
            .retain(|(r, _)| r.id() != resource.id());
    }
}
