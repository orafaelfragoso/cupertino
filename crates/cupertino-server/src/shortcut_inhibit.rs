//! Keyboard shortcuts inhibit (`zwp_keyboard_shortcuts_inhibit_manager_v1`): an application such as a virtual machine,
//! a remote desktop client or a game takes the keys the system would act on.
//!
//! The compositor decides, not the client. An inhibitor is active only while its surface has the keyboard focus, its
//! client is an application (not a shell process), the session is not locked, the authentication prompt is not shown
//! and the person has not released it. While one is active the key router forwards every key but the reserved actions
//! (`Action::is_reserved`), the rewrite for third-party clients, the accent picker and the Super tap are off, and a
//! notice names the application and the chord that gives the shortcuts back (`release-shortcuts`).
//!
//! A client that does not answer its pings holds nothing until it answers again.
//!
//! The release chord also gives back the pointer (`crate::pointer_constraints`). It releases the client, not only the
//! window, so a new window of the client holds nothing either. A released client stays released until one of its
//! windows gains the keyboard focus again by the person's own input (a click, the switcher, a shortcut, a shell
//! process's request) or the person presses a button on it while it has the focus, never by a request of the client
//! (`xdg_activation`, the gtk shell, a window that maps), so an application cannot take the shortcuts or the pointer
//! back by moving the focus itself.

use std::collections::HashSet;

use smithay::backend::renderer::utils::with_renderer_surface_state;
use smithay::desktop::layer_map_for_output;
use smithay::reexports::wayland_server::backend::{ClientId, ObjectId};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::Resource;
use smithay::wayland::keyboard_shortcuts_inhibit::{
    KeyboardShortcutsInhibitHandler, KeyboardShortcutsInhibitState, KeyboardShortcutsInhibitor,
};
use smithay::wayland::shell::wlr_layer::Layer;

use crate::authorization::is_prompt;
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
    /// Clients the person released, until input focuses one of their windows again: a client that maps a new window
    /// is still released.
    released: HashSet<ClientId>,
}

impl ShortcutInhibit {
    pub fn new(state: KeyboardShortcutsInhibitState) -> Self {
        Self {
            state,
            inhibitors: Vec::new(),
            released: HashSet::new(),
        }
    }

    /// Whether an inhibitor is active now.
    pub fn holding(&self) -> bool {
        self.inhibitors.iter().any(|i| i.is_active())
    }

    /// Whether the person released the client of `surface`, and input has not focused one of its windows since.
    pub fn is_released(&self, surface: &WlSurface) -> bool {
        surface
            .client()
            .is_some_and(|c| self.released.contains(&c.id()))
    }

    /// Arm the client of `surface` again. Returns whether it was released.
    fn arm(&mut self, surface: &WlSurface) -> bool {
        surface
            .client()
            .is_some_and(|c| self.released.remove(&c.id()))
    }
}

/// The sentence of the notice that names the release chord, and the whole text, for what `app` holds.
fn notice_text(chord: Option<&str>, app: &str, shortcuts: bool, pointer: bool) -> (String, String) {
    // "shortcuts and pointer" makes the sentence too wide for the narrowest output
    let (what, held) = match (shortcuts, pointer) {
        (true, true) => ("input", "holding your shortcuts and pointer"),
        (false, true) => ("pointer", "holding it"),
        _ => ("shortcuts", "receiving them"),
    };
    // the way out comes first: it must not be pushed out of sight by a long name
    match chord {
        Some(chord) => {
            let clause = format!("Press {chord} to get your {what} back.");
            let text = format!("{clause} {app} is {held}.");
            (clause, text)
        }
        None => {
            let clause = match (shortcuts, pointer) {
                (true, true) => format!("{app} is {held}."),
                (false, true) => format!("{app} is holding your pointer."),
                _ => format!("{app} is receiving your shortcuts."),
            };
            (clause.clone(), clause)
        }
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
    /// Whether `surface` may hold the shortcuts: the toplevel of an application client that answers its pings, with the
    /// session unlocked and the authentication prompt not shown. A layer surface, a popup or a lock surface never holds
    /// them, whoever owns it.
    pub(crate) fn may_hold_shortcuts(&self, surface: &WlSurface) -> bool {
        if self.session_lock.is_active()
            || self.prompt_shown()
            || self
                .window_id_for_surface(surface)
                .is_none_or(|id| self.window_is_unresponsive(id))
        {
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

    /// Whether the authentication prompt is shown: a layer surface with a buffer on the overlay layer of an output, of a
    /// client with the Prompt role.
    pub fn prompt_shown(&self) -> bool {
        self.outputs.iter().any(|o| {
            layer_map_for_output(&o.output)
                .layers_on(Layer::Overlay)
                .any(|l| {
                    is_prompt(l.wl_surface())
                        && with_renderer_surface_state(l.wl_surface(), |s| s.buffer().is_some())
                            .unwrap_or(false)
                })
        })
    }

    /// Make every inhibitor and pointer constraint active or not with the current keyboard focus: a layer surface of
    /// the authentication prompt mapped or went, so they end at once while it is shown and are taken again after.
    pub(crate) fn refresh_holds(&mut self) {
        let focus = self.seat.get_keyboard().and_then(|k| k.current_focus());
        self.refresh_shortcut_inhibitors(focus.as_ref());
        self.refresh_pointer_constraints(focus.as_ref());
    }

    /// Make every inhibitor active or not by the rules above, with `focus` the surface that has the keyboard.
    pub fn refresh_shortcut_inhibitors(&mut self, focus: Option<&WlSurface>) {
        let now = self.now();
        let inhibitors = self.shortcut_inhibit.inhibitors.clone();
        for inhibitor in inhibitors {
            let surface = inhibitor.wl_surface();
            let want = focus == Some(surface)
                && self.may_hold_shortcuts(surface)
                && !self.shortcut_inhibit.is_released(surface);
            if want && !inhibitor.is_active() {
                inhibitor.activate();
                self.show_hold_notice(surface);
                self.request_redraw();
            } else if !want && inhibitor.is_active() {
                inhibitor.inactivate();
                self.settle_hold_notice();
                self.request_redraw();
            }
        }
        let backend = self.display_handle.backend_handle();
        self.shortcut_inhibit
            .released
            .retain(|client| backend.get_client_data(client.clone()).is_ok());
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

    /// What the window of the toplevel `root` holds now: the shortcuts, the pointer.
    fn held_by(&self, root: &ObjectId) -> (bool, bool) {
        let shortcuts = self
            .shortcut_inhibit
            .inhibitors
            .iter()
            .any(|i| i.is_active() && i.wl_surface().id() == *root);
        let pointer = self.pointer_held()
            && self
                .pointer
                .current_focus()
                .and_then(|surface| self.root_window_surface(&surface))
                .is_some_and(|surface| surface.id() == *root);
        (shortcuts, pointer)
    }

    /// Show the notice for the window of the toplevel `root`, which has just taken the shortcuts or the pointer.
    pub(crate) fn show_hold_notice(&mut self, root: &WlSurface) {
        let (shortcuts, pointer) = self.held_by(&root.id());
        let Some(output) = self
            .window_id_for_surface(root)
            .and_then(|id| self.wm.window(id))
            .map(|w| w.output)
        else {
            return;
        };
        let chord = self
            .key_router
            .bindings
            .entries()
            .iter()
            .find(|(_, a)| *a == Action::ReleaseShortcuts)
            .map(|(c, _)| chord_label(c));
        let app = notice_name(&self.application_name(root));
        let (clause, text) = notice_text(chord.as_deref(), &app, shortcuts, pointer);
        self.overlays.shortcut_notice = Some(ShortcutNotice {
            short: chord.unwrap_or_else(|| clause.clone()),
            clause,
            text,
            output,
            owner: Some(root.id()),
            until: self.now() + NOTICE_SECONDS,
        });
    }

    /// The notice goes once the window it is for holds neither the shortcuts nor the pointer, however that came about.
    pub(crate) fn settle_hold_notice(&mut self) {
        let Some(owner) = self
            .overlays
            .shortcut_notice
            .as_ref()
            .and_then(|n| n.owner.clone())
        else {
            return;
        };
        if self.held_by(&owner) == (false, false) {
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
                self.shortcut_inhibit.arm(surface);
            }
        }
        self.refresh_shortcut_inhibitors(focused);
        self.refresh_pointer_constraints(focused);
    }

    /// A press of the person's on the window that already has the keyboard focus arms it again: it moves no focus
    /// that would.
    pub fn arm_by_press(&mut self) {
        let focus = self.seat.get_keyboard().and_then(|k| k.current_focus());
        let pressed = self
            .pointer
            .current_focus()
            .and_then(|surface| self.root_window_surface(&surface));
        let Some(surface) = focus.filter(|f| pressed.as_ref() == Some(f)) else {
            return;
        };
        if self.shortcut_inhibit.arm(&surface) {
            self.refresh_shortcut_inhibitors(Some(&surface));
            self.refresh_pointer_constraints(Some(&surface));
        }
    }

    /// `release-shortcuts`: give the shortcuts and the pointer of the focused surface back to the system. Returns
    /// whether an inhibitor or a pointer constraint was active.
    pub fn release_shortcuts(&mut self) -> bool {
        let focus = self.seat.get_keyboard().and_then(|k| k.current_focus());
        let Some(surface) =
            focus.filter(|_| self.shortcut_inhibit.holding() || self.pointer_held())
        else {
            return false;
        };
        if let Some(client) = surface.client() {
            self.shortcut_inhibit.released.insert(client.id());
        }
        self.forget_pointer_notice();
        self.refresh_shortcut_inhibitors(Some(&surface));
        self.refresh_pointer_constraints(Some(&surface));
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
        self.settle_hold_notice();
        self.request_redraw();
    }
}

smithay::delegate_keyboard_shortcuts_inhibit!(Cupertino);
