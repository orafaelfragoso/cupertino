//! Which chords the compositor may rewrite for third-party clients, so that Super-based and Alt-based editing
//! chords work without breaking their Ctrl shortcuts (§10).
//!
//! The rule never rewrites Ctrl: a chord with Ctrl down passes unchanged. Only physical Super and Alt chords on keys
//! the compositor does not claim are rewritten, and only for clients that are not first-party. The rewrite changes
//! the modifiers the client is told about and, for the cursor chords, the key.
//!
//! The decision is pure. The table is from the design's split between menu key equivalents and
//! text bindings and the documented chords; the terminal copy and paste mapping and
//! the guards are approximate and configurable in the compositor.

use crate::modifiers::{ModifierProfile, RawMods};

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
const SYM_RETURN: u32 = 0xff0d;
const SYM_KP_ENTER: u32 = 0xff8d;
const SYM_C: u32 = 0x63;
const SYM_V: u32 = 0x76;
/// The upper-case forms, which a Shift or Caps Lock chord reports.
const SYM_UPPER_C: u32 = 0x43;
const SYM_UPPER_V: u32 = 0x56;

/// What kind of client has the keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientClass {
    /// A `cupertino-*` client: its toolkit reads the raw modifiers and uses the resolver itself.
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
    Key { evdev: u32, remove: RawMods, add: RawMods },
}

/// Whether a key's character counts as printable for the Super rewrite: a visible character. Space is not (Super+Space
/// is never a text chord) and neither is any control character.
pub fn is_printable(ch: Option<char>) -> bool {
    ch.is_some_and(|c| !c.is_control() && !c.is_whitespace())
}

/// The rewrite for `k` going to `t`, first matching row of the table §10.4.
pub fn decide(profile: &ModifierProfile, t: &ClientTarget, k: &KeyPress) -> Rewrite {
    // 1 and 2: only the design profile (Super is the primary modifier) and only third-party clients
    if profile.primary_physical() != Some(RawMods::SUPER) || t.opted_out || t.class == ClientClass::FirstParty {
        return Rewrite::Pass;
    }
    // 3 and 4: modifier keys, and every chord with Ctrl down (third-party Ctrl shortcuts, Ctrl+Super combinations)
    if k.is_modifier || k.raw.contains(RawMods::CTRL) {
        return Rewrite::Pass;
    }
    let (sup, alt, shift) = (k.raw.contains(RawMods::SUPER), k.raw.contains(RawMods::ALT), k.raw.contains(RawMods::SHIFT));
    // 5 and 6: terminals get copy and paste as Ctrl+Shift and nothing else
    if t.class == ClientClass::Terminal {
        let copy_or_paste = matches!(k.sym, SYM_C | SYM_V | SYM_UPPER_C | SYM_UPPER_V);
        return if sup && !alt && !shift && copy_or_paste {
            Rewrite::Mods { remove: RawMods::SUPER, add: RawMods::CTRL | RawMods::SHIFT }
        } else {
            Rewrite::Pass
        };
    }
    let text_input = t.text_input_active;
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
            return Rewrite::Key { evdev, remove: RawMods::SUPER, add };
        }
        // 8: the arrows and Backspace stay as they are without a text input
        if arrow || k.evdev == KEY_BACKSPACE {
            return Rewrite::Pass;
        }
        // 9: Super+letter, digit, punctuation or Return is the Ctrl chord of the same key; Shift and Alt are kept
        if k.printable || matches!(k.sym, SYM_RETURN | SYM_KP_ENTER) {
            return Rewrite::Mods { remove: RawMods::SUPER, add: RawMods::CTRL };
        }
        // 10: function keys, Tab, Escape, Space
        return Rewrite::Pass;
    }
    // 11: Alt word chords, with Alt and the level-3 bit it sets turned into Ctrl
    if alt && text_input && matches!(k.evdev, KEY_LEFT | KEY_RIGHT | KEY_BACKSPACE | KEY_DELETE) {
        return Rewrite::Mods { remove: RawMods::ALT, add: RawMods::CTRL };
    }
    // 12
    Rewrite::Pass
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY_C: u32 = 46;
    const KEY_F5: u32 = 63;
    const KEY_TAB: u32 = 15;
    const KEY_SPACE: u32 = 57;
    const SYM_TAB: u32 = 0xff09;
    const SYM_F5: u32 = 0xffc2;
    const SYM_ARROW: u32 = 0xff51;

    const SUP: RawMods = RawMods::SUPER;
    const ALT: RawMods = RawMods::ALT;
    const CTRL: RawMods = RawMods::CTRL;
    const SHIFT: RawMods = RawMods::SHIFT;

    fn target(class: ClientClass) -> ClientTarget {
        ClientTarget { class, text_input_active: false, opted_out: false }
    }

    fn letter(evdev: u32, sym: u32, raw: RawMods) -> KeyPress {
        KeyPress { evdev, sym, printable: true, is_modifier: false, raw }
    }

    fn named(evdev: u32, sym: u32, raw: RawMods) -> KeyPress {
        KeyPress { evdev, sym, printable: false, is_modifier: false, raw }
    }

    fn reference(t: &ClientTarget, k: &KeyPress) -> Rewrite {
        decide(&ModifierProfile::cupertino(), t, k)
    }

    fn to_ctrl(remove: RawMods) -> Rewrite {
        Rewrite::Mods { remove, add: CTRL }
    }

    #[test]
    fn super_chords_become_ctrl_chords_and_ctrl_is_never_touched() {
        let other = target(ClientClass::Other);
        assert_eq!(reference(&other, &letter(KEY_C, SYM_C, SUP)), to_ctrl(SUP));
        assert_eq!(reference(&other, &letter(KEY_C, SYM_C, CTRL)), Rewrite::Pass);
        assert_eq!(reference(&other, &letter(KEY_C, SYM_C, CTRL | SUP)), Rewrite::Pass);
        // Shift and Alt stay with the chord
        assert_eq!(reference(&other, &letter(KEY_C, SYM_C, SUP | SHIFT)), to_ctrl(SUP));
        assert_eq!(reference(&other, &letter(KEY_C, SYM_C, SUP | ALT)), to_ctrl(SUP));
        assert_eq!(reference(&other, &named(28, SYM_RETURN, SUP)), to_ctrl(SUP));
    }

    #[test]
    fn keys_that_are_not_text_chords_pass() {
        let other = target(ClientClass::Other);
        assert_eq!(reference(&other, &named(KEY_F5, SYM_F5, SUP)), Rewrite::Pass);
        assert_eq!(reference(&other, &named(KEY_TAB, SYM_TAB, SUP)), Rewrite::Pass);
        assert_eq!(reference(&other, &KeyPress { printable: is_printable(Some(' ')), ..named(KEY_SPACE, 0x20, SUP) }), Rewrite::Pass);
        assert_eq!(reference(&other, &KeyPress { is_modifier: true, ..named(125, 0xffeb, SUP) }), Rewrite::Pass);
        assert_eq!(reference(&other, &named(KEY_BACKSPACE, 0xff08, SUP)), Rewrite::Pass);
    }

    #[test]
    fn terminals_get_copy_and_paste_as_ctrl_shift_only() {
        let term = target(ClientClass::Terminal);
        let copy = Rewrite::Mods { remove: SUP, add: CTRL | SHIFT };
        assert_eq!(reference(&term, &letter(KEY_C, SYM_C, SUP)), copy);
        assert_eq!(reference(&term, &letter(47, SYM_V, SUP)), copy);
        assert_eq!(reference(&term, &letter(KEY_C, SYM_C, SUP | SHIFT)), Rewrite::Pass);
        assert_eq!(reference(&term, &letter(37, 0x6b, SUP)), Rewrite::Pass);
        assert_eq!(reference(&term, &letter(KEY_C, SYM_C, CTRL)), Rewrite::Pass);
        assert_eq!(reference(&ClientTarget { text_input_active: true, ..term }, &named(KEY_LEFT, SYM_ARROW, SUP)), Rewrite::Pass);
    }

    #[test]
    fn super_arrows_move_the_text_cursor_only_in_a_wayland_text_input() {
        let plain = target(ClientClass::Other);
        let text = ClientTarget { text_input_active: true, ..plain };
        let left = named(KEY_LEFT, SYM_ARROW, SUP);
        assert_eq!(reference(&plain, &left), Rewrite::Pass);
        assert_eq!(reference(&text, &left), Rewrite::Key { evdev: KEY_HOME, remove: SUP, add: RawMods::default() });
        assert_eq!(reference(&text, &named(KEY_RIGHT, SYM_ARROW, SUP | SHIFT)), Rewrite::Key { evdev: KEY_END, remove: SUP, add: RawMods::default() });
        assert_eq!(reference(&text, &named(KEY_UP, SYM_ARROW, SUP)), Rewrite::Key { evdev: KEY_HOME, remove: SUP, add: CTRL });
        assert_eq!(reference(&text, &named(KEY_DOWN, SYM_ARROW, SUP)), Rewrite::Key { evdev: KEY_END, remove: SUP, add: CTRL });
        assert_eq!(reference(&text, &named(KEY_LEFT, SYM_ARROW, SUP | ALT)), Rewrite::Pass);
    }

    #[test]
    fn alt_word_chords_become_ctrl_chords_in_a_wayland_text_input() {
        let plain = target(ClientClass::Other);
        let text = ClientTarget { text_input_active: true, ..plain };
        let alt_left = named(KEY_LEFT, SYM_ARROW, ALT);
        assert_eq!(reference(&plain, &alt_left), Rewrite::Pass);
        assert_eq!(reference(&text, &alt_left), to_ctrl(ALT));
        assert_eq!(reference(&text, &named(KEY_DELETE, 0xffff, ALT | SHIFT)), to_ctrl(ALT));
        assert_eq!(reference(&text, &named(KEY_UP, SYM_ARROW, ALT)), Rewrite::Pass);
        assert_eq!(reference(&text, &letter(KEY_C, SYM_C, ALT)), Rewrite::Pass);
    }

    #[test]
    fn first_party_clients_opt_outs_and_the_classic_profile_pass() {
        let copy = letter(KEY_C, SYM_C, SUP);
        assert_eq!(reference(&target(ClientClass::FirstParty), &copy), Rewrite::Pass);
        assert_eq!(reference(&ClientTarget { opted_out: true, ..target(ClientClass::Other) }, &copy), Rewrite::Pass);
        assert_eq!(decide(&ModifierProfile::classic_pc(), &target(ClientClass::Other), &copy), Rewrite::Pass);
    }

    #[test]
    fn a_printable_character_is_visible() {
        assert!([Some('c'), Some('1'), Some('ç'), Some(',')].into_iter().all(is_printable));
        assert!(![None, Some(' '), Some('\r'), Some('\u{1b}')].into_iter().any(is_printable));
    }
}
