//! Which chords the compositor may rewrite for third-party clients, so that Super-based and Alt-based editing
//! chords work without breaking their Ctrl shortcuts (§10).
//!
//! Only physical Super, Ctrl and Alt chords on keys the compositor does not claim are rewritten, and only for clients
//! that are neither first-party nor terminals for Ctrl. Super is the command key: its chords reach the client as the
//! Ctrl chords it binds, and a Ctrl chord reaches it with Super instead, so Ctrl never acts as the command key. The rewrite changes
//! the modifiers the client is told about and, for the cursor chords, the key.
//!
//! The decision is pure. The table is from the design's split between menu key equivalents and
//! text bindings and the documented chords; the terminal mapping and
//! the guards are approximate and configurable in the compositor.

use cupertino_modifiers::{Physical, Profile, RawMods};

/// evdev codes of the keys the table names (`linux/input-event-codes.h`).
pub const KEY_HOME: u32 = 102;
pub const KEY_UP: u32 = 103;
pub const KEY_LEFT: u32 = 105;
pub const KEY_RIGHT: u32 = 106;
pub const KEY_END: u32 = 107;
pub const KEY_DOWN: u32 = 108;
pub const KEY_BACKSPACE: u32 = 14;
pub const KEY_DELETE: u32 = 111;

/// Latin keysyms of the keys the table names (`xkbcommon-keysyms.h`).
pub const SYM_RETURN: u32 = 0xff0d;
pub const SYM_KP_ENTER: u32 = 0xff8d;
/// The keys of a terminal's text size chords, which it binds with Ctrl alone: equals and plus (larger), minus (smaller)
/// and zero (the configured size).
const TERMINAL_SIZE_SYMS: [u32; 4] = [0x3d, 0x2b, 0x2d, 0x30];

/// Whether `sym` is a Latin letter, either case (a Shift or Caps Lock chord reports the upper-case one).
fn is_letter(sym: u32) -> bool {
    (0x41..=0x5a).contains(&sym) || (0x61..=0x7a).contains(&sym)
}

/// What kind of client has the keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientClass {
    /// A client of the desktop's toolkit: the toolkit reads the raw modifiers and uses the resolver itself.
    FirstParty,
    /// A terminal emulator: Ctrl chords are control characters there, so copy and paste take Ctrl+Shift.
    Terminal,
    /// Everything else, browsers and Electron applications included.
    Other,
}

/// The client that will receive the key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClientTarget {
    pub class: ClientClass,
    /// The client has an active `zwp_text_input_v3`.
    pub text_input_active: bool,
    /// The user listed the application in `rewrite_off`, or switched the rewrite off.
    pub opted_out: bool,
}

/// One key press that no compositor consumer claimed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyPress {
    /// evdev code of the physical key.
    pub evdev: u32,
    /// Latin keysym of the key (the layout-independent symbol shortcuts are matched with).
    pub sym: u32,
    /// The key types a visible character: see [`is_printable`].
    pub printable: bool,
    pub is_modifier: bool,
    /// Physical modifiers down; Alt includes the key that selects the third symbol level.
    pub raw: RawMods,
}

/// What to tell the client.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rewrite {
    /// Forward the key and the real modifiers.
    Pass,
    /// Forward the key with `remove` taken out of and `add` put into the modifiers.
    Mods { remove: RawMods, add: RawMods },
    /// Forward `evdev` instead of the key, with the modifiers changed the same way.
    Key {
        evdev: u32,
        remove: RawMods,
        add: RawMods,
    },
}

/// Whether a key's character counts as printable for the Super rewrite: a visible character. Space is not (Super+Space
/// is never a text chord) and neither is any control character.
pub fn is_printable(ch: Option<char>) -> bool {
    ch.is_some_and(|c| !c.is_control() && !c.is_whitespace())
}

/// The rewrite for `k` going to `t`, first matching row of the table §10.4.
pub fn decide(profile: &Profile, t: &ClientTarget, k: &KeyPress) -> Rewrite {
    // 1 and 2: only the design profile (Super is the primary modifier) and only third-party clients
    if profile.primary() != Physical::Super || t.opted_out || t.class == ClientClass::FirstParty {
        return Rewrite::Pass;
    }
    // 3: modifier keys, and Ctrl+Super combinations
    if k.is_modifier || k.raw.contains(RawMods::CTRL | RawMods::SUPER) {
        return Rewrite::Pass;
    }
    let (sup, alt) = (k.raw.contains(RawMods::SUPER), k.raw.contains(RawMods::ALT));
    // 5 and 6: a terminal's Ctrl chords on letters are control characters, so its own commands (copy, paste, new
    // window, new tab, find) are the Ctrl+Shift ones: Super+letter, with Shift or without, is the Ctrl+Shift chord of
    // the letter, and Super on a text-size key is the Ctrl chord of that key. Everything else, Ctrl chords included,
    // reaches the terminal as it is.
    if t.class == ClientClass::Terminal {
        let add = match k.sym {
            _ if !sup || alt => return Rewrite::Pass,
            sym if is_letter(sym) => RawMods::CTRL | RawMods::SHIFT,
            sym if TERMINAL_SIZE_SYMS.contains(&sym) => RawMods::CTRL,
            _ => return Rewrite::Pass,
        };
        return Rewrite::Mods {
            remove: RawMods::SUPER,
            add,
        };
    }
    let text_input = t.text_input_active;
    // 4: Ctrl is not the command key here. In a text input its line-editing chords move and delete as their keys do
    // (Shift kept, so it selects); every other Ctrl chord on a printable key or Return reaches the client with Super,
    // which carries no shortcut there, so the Ctrl key cannot copy, paste or close as the command key does
    if k.raw.contains(RawMods::CTRL) {
        if text_input && !alt {
            if let Some(evdev) = line_editing_key(k.sym) {
                return Rewrite::Key {
                    evdev,
                    remove: RawMods::CTRL,
                    add: RawMods::default(),
                };
            }
        }
        if k.printable || matches!(k.sym, SYM_RETURN | SYM_KP_ENTER) {
            return Rewrite::Mods {
                remove: RawMods::CTRL,
                add: RawMods::SUPER,
            };
        }
        return Rewrite::Pass;
    }
    if sup {
        let arrow = matches!(k.evdev, KEY_LEFT | KEY_RIGHT | KEY_UP | KEY_DOWN);
        // 7: line and document ends, as the key the toolkits bind (Home and End, with Ctrl for the document)
        if arrow && !alt && text_input {
            let (evdev, add) = match k.evdev {
                KEY_LEFT => (KEY_HOME, RawMods::default()),
                KEY_RIGHT => (KEY_END, RawMods::default()),
                KEY_UP => (KEY_HOME, RawMods::CTRL),
                _ => (KEY_END, RawMods::CTRL),
            };
            return Rewrite::Key {
                evdev,
                remove: RawMods::SUPER,
                add,
            };
        }
        // 8: the arrows and Backspace stay as they are without a text input
        if arrow || k.evdev == KEY_BACKSPACE {
            return Rewrite::Pass;
        }
        // 9: Super+letter, digit, punctuation or Return is the Ctrl chord of the same key; Shift and Alt are kept
        if k.printable || matches!(k.sym, SYM_RETURN | SYM_KP_ENTER) {
            return Rewrite::Mods {
                remove: RawMods::SUPER,
                add: RawMods::CTRL,
            };
        }
        // 10: function keys, Tab, Escape, Space
        return Rewrite::Pass;
    }
    // 11: Alt word chords, with Alt and the level-3 bit it sets turned into Ctrl
    if alt && text_input && matches!(k.evdev, KEY_LEFT | KEY_RIGHT | KEY_BACKSPACE | KEY_DELETE) {
        return Rewrite::Mods {
            remove: RawMods::ALT,
            add: RawMods::CTRL,
        };
    }
    // 12
    Rewrite::Pass
}

/// The key a Ctrl line-editing chord on the key of Latin keysym `sym` stands for in a text input: the line's start and
/// end, a character back and forward, a line up and down, and the character after and before the caret deleted.
fn line_editing_key(sym: u32) -> Option<u32> {
    let letter = if (0x41..=0x5a).contains(&sym) {
        sym + 0x20
    } else {
        sym
    };
    Some(match char::from_u32(letter)? {
        'a' => KEY_HOME,
        'e' => KEY_END,
        'b' => KEY_LEFT,
        'f' => KEY_RIGHT,
        'p' => KEY_UP,
        'n' => KEY_DOWN,
        'd' => KEY_DELETE,
        'h' => KEY_BACKSPACE,
        _ => return None,
    })
}

/// The chord the person presses so that a client of class `class` receives `received`, outside a text input: the
/// inverse of the Super and Ctrl rows of [`decide`]. For a terminal, Ctrl+Shift+letter comes from Super+letter and a
/// Ctrl text size chord from the Super one (5 and 6); for any other third-party client, a Ctrl chord without Super on a printable key or Return comes from the Super
/// chord of the same key with Shift and Alt kept (9), and a Super chord without Ctrl from the Ctrl one (4). Every other
/// chord, every chord of a first-party client and every chord
/// under a profile whose primary modifier is not Super is pressed as it is received.
pub fn pressed_for(profile: &Profile, class: ClientClass, received: &KeyPress) -> KeyPress {
    let raw = received.raw;
    if profile.primary() != Physical::Super
        || received.is_modifier
        || raw.contains(RawMods::CTRL | RawMods::SUPER)
    {
        return *received;
    }
    let swap = |from: RawMods, to: RawMods| RawMods((raw.0 & !from.0) | to.0);
    let chord_key = received.printable || matches!(received.sym, SYM_RETURN | SYM_KP_ENTER);
    let pressed = match class {
        ClientClass::FirstParty => None,
        ClientClass::Terminal
            if raw == RawMods::CTRL | RawMods::SHIFT && is_letter(received.sym) =>
        {
            Some(RawMods::SUPER)
        }
        ClientClass::Terminal => (raw.contains(RawMods::CTRL)
            && !raw.contains(RawMods::ALT)
            && TERMINAL_SIZE_SYMS.contains(&received.sym))
        .then(|| swap(RawMods::CTRL, RawMods::SUPER)),
        ClientClass::Other if !chord_key => None,
        ClientClass::Other if raw.contains(RawMods::CTRL) => {
            Some(swap(RawMods::CTRL, RawMods::SUPER))
        }
        ClientClass::Other => raw
            .contains(RawMods::SUPER)
            .then(|| swap(RawMods::SUPER, RawMods::CTRL)),
    };
    match pressed {
        Some(raw) => KeyPress { raw, ..*received },
        None => *received,
    }
}
