//! Keyboard shortcuts inhibit (`zwp_keyboard_shortcuts_inhibit_manager_v1`) state and notice text helpers.

use std::collections::HashSet;

use smithay::reexports::wayland_server::backend::ClientId;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::Resource;
use smithay::wayland::keyboard_shortcuts_inhibit::{
    KeyboardShortcutsInhibitState, KeyboardShortcutsInhibitor,
};

use crate::input::Chord;

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
    pub state: KeyboardShortcutsInhibitState,
    pub inhibitors: Vec<KeyboardShortcutsInhibitor>,
    /// Clients the person released, until input focuses one of their windows again: a client that maps a new window
    /// is still released.
    pub released: HashSet<ClientId>,
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
    pub fn arm(&mut self, surface: &WlSurface) -> bool {
        surface
            .client()
            .is_some_and(|c| self.released.remove(&c.id()))
    }
}

/// The sentence of the notice that names the release chord, and the whole text, for what `app` holds.
pub fn notice_text(
    chord: Option<&str>,
    app: &str,
    shortcuts: bool,
    pointer: bool,
) -> (String, String) {
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
