//! Keyboard shortcuts inhibit (`zwp_keyboard_shortcuts_inhibit_manager_v1`): an application such as a virtual machine,
//! a remote desktop client or a game takes the keys the system would act on.
//!
//! The compositor decides, not the client. An inhibitor is active only while its surface has the keyboard focus, its
//! client is an application (not a shell process), the session is not locked and the person has not released it. While
//! one is active the key router forwards every key but the reserved actions (`Action::is_reserved`), the rewrite for
//! third-party clients, the accent picker and the Super tap are off, and a notice names the application and the chord
//! that gives the shortcuts back (`release-shortcuts`).
//!
//! A released surface stays released until it gains the keyboard focus again by the person's own input (a click, the
//! switcher, a shortcut, a shell process's request), never by a request of the client (`xdg_activation`, the gtk shell,
//! a window that maps), so an application cannot take the shortcuts back by moving the focus itself.

use std::collections::HashMap;

use smithay::reexports::wayland_server::backend::ObjectId;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::Resource;
use smithay::wayland::keyboard_shortcuts_inhibit::{
    KeyboardShortcutsInhibitHandler, KeyboardShortcutsInhibitState, KeyboardShortcutsInhibitor,
};

use crate::input::{Action, Chord};
use crate::overlays::ShortcutNotice;
use crate::state::{ClientState, Cupertino};

/// The most characters of an application's name the notice shows.
const NOTICE_NAME_CHARS: usize = 48;

/// The name of an application as notice text: the name comes from the client, so it is shown without control
/// characters or the characters that turn the text around, and at most [`NOTICE_NAME_CHARS`] long.
pub fn notice_name(name: &str) -> String {
    let clean: Vec<char> = name
        .chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        .collect();
    let shown: String = if clean.len() > NOTICE_NAME_CHARS {
        clean[..NOTICE_NAME_CHARS - 1]
            .iter()
            .chain(&['…'])
            .collect()
    } else {
        clean.into_iter().collect()
    };
    match shown.trim() {
        "" => "An application".to_owned(),
        name => name.to_owned(),
    }
}

/// Seconds the notice stays when an inhibitor becomes active.
pub const NOTICE_SECONDS: f64 = 3.0;

pub struct ShortcutInhibit {
    pub(crate) state: KeyboardShortcutsInhibitState,
    inhibitors: Vec<KeyboardShortcutsInhibitor>,
    /// Surfaces whose inhibitor the person released, until input focuses them again.
    released: HashMap<ObjectId, WlSurface>,
}

impl ShortcutInhibit {
    pub fn new(state: KeyboardShortcutsInhibitState) -> Self {
        Self {
            state,
            inhibitors: Vec::new(),
            released: HashMap::new(),
        }
    }

    /// Whether an inhibitor is active now.
    pub fn holding(&self) -> bool {
        self.inhibitors.iter().any(|i| i.is_active())
    }
}

/// The chord as the person presses it, from the bindings.
pub fn chord_label(chord: &Chord) -> String {
    let mut label = String::new();
    for (on, name) in [
        (chord.mods.ctrl, "Ctrl"),
        (chord.mods.alt, "Alt"),
        (chord.mods.shift, "Shift"),
        (chord.mods.logo, "Super"),
    ] {
        if on {
            label.push_str(name);
            label.push('+');
        }
    }
    match cupertino_input::keysym::name_of(chord.sym) {
        Some(name) => label.push_str(&name),
        None => label.push_str(&format!("0x{:x}", chord.sym)),
    }
    label
}

impl Cupertino {
    /// Whether `surface` may hold the shortcuts: the toplevel of an application client, with the session unlocked. A
    /// layer surface, a popup or a lock surface never holds them, whoever owns it.
    fn may_hold_shortcuts(&self, surface: &WlSurface) -> bool {
        if self.session_lock.is_active() || self.window_id_for_surface(surface).is_none() {
            return false;
        }
        surface
            .client()
            .and_then(|c| {
                c.get_data::<ClientState>()
                    .map(|s| !s.internal && s.capabilities.is_empty())
            })
            .unwrap_or(false)
    }

    /// Make every inhibitor active or not by the rules above, with `focus` the surface that has the keyboard.
    pub fn refresh_shortcut_inhibitors(&mut self, focus: Option<&WlSurface>) {
        let now = self.now();
        let inhibitors = self.shortcut_inhibit.inhibitors.clone();
        for inhibitor in inhibitors {
            let surface = inhibitor.wl_surface();
            let want = focus == Some(surface)
                && self.may_hold_shortcuts(surface)
                && !self.shortcut_inhibit.released.contains_key(&surface.id());
            if want && !inhibitor.is_active() {
                inhibitor.activate();
                let chord = self
                    .key_router
                    .bindings
                    .entries()
                    .iter()
                    .find(|(_, a)| *a == Action::ReleaseShortcuts)
                    .map(|(c, _)| chord_label(c));
                let app = notice_name(&self.application_name(surface));
                if let Some(output) = self
                    .window_id_for_surface(surface)
                    .and_then(|id| self.wm.window(id))
                    .map(|w| w.output)
                {
                    self.overlays.shortcut_notice = Some(ShortcutNotice {
                        // the way out comes first: it must not be pushed out of sight by a long name
                        text: match chord {
                            Some(chord) => format!(
                                "Press {chord} to get your shortcuts back. {app} is receiving them."
                            ),
                            None => format!("{app} is receiving your shortcuts."),
                        },
                        output,
                        owner: Some(surface.id()),
                        until: now + NOTICE_SECONDS,
                    });
                }
                self.request_redraw();
            } else if !want && inhibitor.is_active() {
                inhibitor.inactivate();
                // only the notice of this inhibitor: another one may have been activated in this same pass
                if self
                    .overlays
                    .shortcut_notice
                    .as_ref()
                    .is_some_and(|n| n.owner.as_ref() == Some(&surface.id()))
                {
                    self.overlays.shortcut_notice = None;
                }
                self.request_redraw();
            }
        }
        self.shortcut_inhibit
            .released
            .retain(|_, surface| surface.is_alive());
        if self
            .overlays
            .shortcut_notice
            .as_ref()
            .is_some_and(|n| n.until <= now)
        {
            self.overlays.shortcut_notice = None;
            self.request_redraw();
        }
    }

    /// The name of the application that owns `surface`, for the notice.
    fn application_name(&self, surface: &WlSurface) -> String {
        self.window_id_for_surface(surface)
            .and_then(|id| self.wm.window(id))
            .and_then(|w| self.wm.app(w.app))
            .map(|a| a.identity.clone())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "An application".to_owned())
    }

    /// The keyboard focus moved to `focused`. A surface the person released is armed again only by focus the person's
    /// own input gave it.
    pub fn shortcut_inhibit_focus(&mut self, focused: Option<&WlSurface>) {
        if self.input_focus {
            if let Some(surface) = focused {
                self.shortcut_inhibit.released.remove(&surface.id());
            }
        }
        self.refresh_shortcut_inhibitors(focused);
    }

    /// `release-shortcuts`: give the shortcuts of the focused surface back to the system. Returns whether an
    /// inhibitor was active.
    pub fn release_shortcuts(&mut self) -> bool {
        let focus = self.seat.get_keyboard().and_then(|k| k.current_focus());
        let Some(surface) = focus.filter(|_| self.shortcut_inhibit.holding()) else {
            return false;
        };
        self.shortcut_inhibit
            .released
            .insert(surface.id(), surface.clone());
        self.refresh_shortcut_inhibitors(Some(&surface));
        true
    }

    /// Run `f` as the person's own input: the keyboard focus it gives clears a release.
    pub fn as_input<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        let before = std::mem::replace(&mut self.input_focus, true);
        let result = f(self);
        self.input_focus = before;
        result
    }

    /// Give `window` the keyboard focus on the person's own input.
    pub fn focus_window_by_input(&mut self, window: cupertino_wm::WindowId) {
        self.as_input(|s| s.command(cupertino_wm::Command::FocusWindow { window }));
    }
}

impl KeyboardShortcutsInhibitHandler for Cupertino {
    fn keyboard_shortcuts_inhibit_state(&mut self) -> &mut KeyboardShortcutsInhibitState {
        &mut self.shortcut_inhibit.state
    }

    fn new_inhibitor(&mut self, inhibitor: KeyboardShortcutsInhibitor) {
        self.shortcut_inhibit.inhibitors.push(inhibitor);
        let focus = self.seat.get_keyboard().and_then(|k| k.current_focus());
        self.refresh_shortcut_inhibitors(focus.as_ref());
    }

    fn inhibitor_destroyed(&mut self, inhibitor: KeyboardShortcutsInhibitor) {
        self.shortcut_inhibit.inhibitors.retain(|i| *i != inhibitor);
        if !self.shortcut_inhibit.holding() {
            self.overlays.shortcut_notice = None;
        }
        self.request_redraw();
    }
}

smithay::delegate_keyboard_shortcuts_inhibit!(Cupertino);
