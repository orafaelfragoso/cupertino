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

#[cfg(test)]
mod tests {
    use super::*;

    const KEY_C: u32 = 46;
    const KEY_N: u32 = 49;
    const KEY_T: u32 = 20;
    const KEY_W: u32 = 17;
    const KEY_MINUS: u32 = 12;
    const KEY_EQUAL: u32 = 13;
    const KEY_LEFTBRACE: u32 = 26;
    const KEY_BACKSLASH: u32 = 43;
    const SYM_C: u32 = 0x63;
    const SYM_V: u32 = 0x76;
    const SYM_N: u32 = 0x6e;
    const SYM_UPPER_N: u32 = 0x4e;
    const SYM_T: u32 = 0x74;
    const SYM_W: u32 = 0x77;
    const SYM_MINUS: u32 = 0x2d;
    const SYM_EQUAL: u32 = 0x3d;
    const SYM_BRACKETLEFT: u32 = 0x5b;
    const SYM_BACKSLASH: u32 = 0x5c;
    /// The upper-case form, which a Shift or Caps Lock chord reports.
    const SYM_UPPER_C: u32 = 0x43;
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
        ClientTarget {
            class,
            text_input_active: false,
            opted_out: false,
        }
    }

    fn letter(evdev: u32, sym: u32, raw: RawMods) -> KeyPress {
        KeyPress {
            evdev,
            sym,
            printable: true,
            is_modifier: false,
            raw,
        }
    }

    fn named(evdev: u32, sym: u32, raw: RawMods) -> KeyPress {
        KeyPress {
            evdev,
            sym,
            printable: false,
            is_modifier: false,
            raw,
        }
    }

    fn reference(t: &ClientTarget, k: &KeyPress) -> Rewrite {
        decide(&Profile::cupertino(), t, k)
    }

    fn to_ctrl(remove: RawMods) -> Rewrite {
        Rewrite::Mods { remove, add: CTRL }
    }

    #[test]
    fn super_chords_become_ctrl_chords_and_ctrl_chords_super_ones() {
        let other = target(ClientClass::Other);
        assert_eq!(reference(&other, &letter(KEY_C, SYM_C, SUP)), to_ctrl(SUP));
        assert_eq!(
            reference(&other, &letter(KEY_C, SYM_C, CTRL)),
            Rewrite::Mods {
                remove: CTRL,
                add: SUP
            },
            "Ctrl is not the command key"
        );
        assert_eq!(
            reference(&other, &letter(KEY_C, SYM_C, CTRL | SUP)),
            Rewrite::Pass
        );
        // Shift and Alt stay with the chord
        assert_eq!(
            reference(&other, &letter(KEY_C, SYM_C, SUP | SHIFT)),
            to_ctrl(SUP)
        );
        assert_eq!(
            reference(&other, &letter(KEY_C, SYM_C, SUP | ALT)),
            to_ctrl(SUP)
        );
        assert_eq!(reference(&other, &named(28, SYM_RETURN, SUP)), to_ctrl(SUP));
    }

    #[test]
    fn keys_that_are_not_text_chords_pass() {
        let other = target(ClientClass::Other);
        assert_eq!(
            reference(&other, &named(KEY_F5, SYM_F5, SUP)),
            Rewrite::Pass
        );
        assert_eq!(
            reference(&other, &named(KEY_TAB, SYM_TAB, SUP)),
            Rewrite::Pass
        );
        assert_eq!(
            reference(
                &other,
                &KeyPress {
                    printable: is_printable(Some(' ')),
                    ..named(KEY_SPACE, 0x20, SUP)
                }
            ),
            Rewrite::Pass
        );
        assert_eq!(
            reference(
                &other,
                &KeyPress {
                    is_modifier: true,
                    ..named(125, 0xffeb, SUP)
                }
            ),
            Rewrite::Pass
        );
        assert_eq!(
            reference(&other, &named(KEY_BACKSPACE, 0xff08, SUP)),
            Rewrite::Pass
        );
    }

    #[test]
    fn a_terminal_gets_super_letter_chords_as_its_ctrl_shift_commands() {
        let term = target(ClientClass::Terminal);
        let command = Rewrite::Mods {
            remove: SUP,
            add: CTRL | SHIFT,
        };
        // copy, paste, new window, new tab, and a letter no terminal binds: Shift held or not, the Ctrl+Shift chord
        for (evdev, sym) in [
            (KEY_C, SYM_C),
            (47, SYM_V),
            (KEY_N, SYM_N),
            (KEY_T, SYM_T),
            (KEY_W, SYM_W),
            (37, 0x6b),
        ] {
            assert_eq!(
                reference(&term, &letter(evdev, sym, SUP)),
                command,
                "{sym:#x}"
            );
        }
        assert_eq!(
            reference(&term, &letter(KEY_C, SYM_UPPER_C, SUP | SHIFT)),
            command
        );
        assert_eq!(
            reference(&term, &letter(KEY_N, SYM_UPPER_N, SUP | SHIFT)),
            command
        );
        // the text size keys take Ctrl alone, as terminals bind them
        let size = Rewrite::Mods {
            remove: SUP,
            add: CTRL,
        };
        assert_eq!(reference(&term, &letter(KEY_EQUAL, SYM_EQUAL, SUP)), size);
        assert_eq!(reference(&term, &letter(KEY_MINUS, SYM_MINUS, SUP)), size);
        assert_eq!(reference(&term, &letter(11, 0x30, SUP)), size);
        // punctuation whose Ctrl chord is a control character (Escape, quit) and other digits stay Super chords
        assert_eq!(
            reference(&term, &letter(KEY_LEFTBRACE, SYM_BRACKETLEFT, SUP)),
            Rewrite::Pass
        );
        assert_eq!(
            reference(&term, &letter(KEY_BACKSLASH, SYM_BACKSLASH, SUP)),
            Rewrite::Pass
        );
        assert_eq!(reference(&term, &letter(2, 0x31, SUP)), Rewrite::Pass);
        // Super with Alt, and the terminal's own Ctrl chords (control characters), are left alone
        assert_eq!(
            reference(&term, &letter(KEY_N, SYM_N, SUP | ALT)),
            Rewrite::Pass
        );
        assert_eq!(reference(&term, &letter(KEY_C, SYM_C, CTRL)), Rewrite::Pass);
        assert_eq!(
            reference(&term, &letter(KEY_C, SYM_C, CTRL | SHIFT)),
            Rewrite::Pass
        );
        assert_eq!(
            reference(
                &ClientTarget {
                    text_input_active: true,
                    ..term
                },
                &named(KEY_LEFT, SYM_ARROW, SUP)
            ),
            Rewrite::Pass
        );
    }

    #[test]
    fn super_arrows_move_the_text_cursor_only_in_a_wayland_text_input() {
        let plain = target(ClientClass::Other);
        let text = ClientTarget {
            text_input_active: true,
            ..plain
        };
        let left = named(KEY_LEFT, SYM_ARROW, SUP);
        assert_eq!(reference(&plain, &left), Rewrite::Pass);
        assert_eq!(
            reference(&text, &left),
            Rewrite::Key {
                evdev: KEY_HOME,
                remove: SUP,
                add: RawMods::default()
            }
        );
        assert_eq!(
            reference(&text, &named(KEY_RIGHT, SYM_ARROW, SUP | SHIFT)),
            Rewrite::Key {
                evdev: KEY_END,
                remove: SUP,
                add: RawMods::default()
            }
        );
        assert_eq!(
            reference(&text, &named(KEY_UP, SYM_ARROW, SUP)),
            Rewrite::Key {
                evdev: KEY_HOME,
                remove: SUP,
                add: CTRL
            }
        );
        assert_eq!(
            reference(&text, &named(KEY_DOWN, SYM_ARROW, SUP)),
            Rewrite::Key {
                evdev: KEY_END,
                remove: SUP,
                add: CTRL
            }
        );
        assert_eq!(
            reference(&text, &named(KEY_LEFT, SYM_ARROW, SUP | ALT)),
            Rewrite::Pass
        );
    }

    #[test]
    fn alt_word_chords_become_ctrl_chords_in_a_wayland_text_input() {
        let plain = target(ClientClass::Other);
        let text = ClientTarget {
            text_input_active: true,
            ..plain
        };
        let alt_left = named(KEY_LEFT, SYM_ARROW, ALT);
        assert_eq!(reference(&plain, &alt_left), Rewrite::Pass);
        assert_eq!(reference(&text, &alt_left), to_ctrl(ALT));
        assert_eq!(
            reference(&text, &named(KEY_DELETE, 0xffff, ALT | SHIFT)),
            to_ctrl(ALT)
        );
        assert_eq!(
            reference(&text, &named(KEY_UP, SYM_ARROW, ALT)),
            Rewrite::Pass
        );
        assert_eq!(reference(&text, &letter(KEY_C, SYM_C, ALT)), Rewrite::Pass);
    }

    #[test]
    fn first_party_clients_opt_outs_and_the_classic_profile_pass() {
        let copy = letter(KEY_C, SYM_C, SUP);
        assert_eq!(
            reference(&target(ClientClass::FirstParty), &copy),
            Rewrite::Pass
        );
        assert_eq!(
            reference(
                &ClientTarget {
                    opted_out: true,
                    ..target(ClientClass::Other)
                },
                &copy
            ),
            Rewrite::Pass
        );
        assert_eq!(
            decide(&Profile::classic_pc(), &target(ClientClass::Other), &copy),
            Rewrite::Pass
        );
    }

    #[test]
    fn a_printable_character_is_visible() {
        assert!([Some('c'), Some('1'), Some('ç'), Some(',')]
            .into_iter()
            .all(is_printable));
        assert!(![None, Some(' '), Some('\r'), Some('\u{1b}')]
            .into_iter()
            .any(is_printable));
    }

    /// What the client is told for `k` under `r`, outside a text input.
    fn received(k: &KeyPress, r: Rewrite) -> KeyPress {
        match r {
            Rewrite::Pass => *k,
            Rewrite::Mods { remove, add } => KeyPress {
                raw: RawMods((k.raw.0 & !remove.0) | add.0),
                ..*k
            },
            Rewrite::Key { .. } => unreachable!("only a text input changes the key"),
        }
    }

    /// Letters, a digit, punctuation, Return, keys no row rewrites and a modifier key, with every set of Shift, Ctrl,
    /// Alt and Super.
    fn chords() -> Vec<KeyPress> {
        let keys = [
            letter(KEY_C, SYM_C, RawMods::default()),
            letter(47, SYM_V, RawMods::default()),
            letter(16, 0x71, RawMods::default()),
            letter(KEY_C, SYM_UPPER_C, RawMods::default()),
            letter(2, 0x31, RawMods::default()),
            letter(51, 0x2c, RawMods::default()),
            letter(KEY_EQUAL, SYM_EQUAL, RawMods::default()),
            named(28, SYM_RETURN, RawMods::default()),
            named(96, SYM_KP_ENTER, RawMods::default()),
            named(KEY_F5, SYM_F5, RawMods::default()),
            named(KEY_TAB, SYM_TAB, RawMods::default()),
            named(KEY_SPACE, 0x20, RawMods::default()),
            named(1, 0xff1b, RawMods::default()),
            named(KEY_DELETE, 0xffff, RawMods::default()),
            named(KEY_BACKSPACE, 0xff08, RawMods::default()),
            named(KEY_LEFT, SYM_ARROW, RawMods::default()),
            KeyPress {
                is_modifier: true,
                ..named(29, 0xffe3, RawMods::default())
            },
        ];
        keys.into_iter()
            .flat_map(|k| {
                (0..16).map(move |raw| KeyPress {
                    raw: RawMods(raw),
                    ..k
                })
            })
            .collect()
    }

    #[test]
    fn pressed_for_inverts_decide_for_every_class_and_profile() {
        for profile in [Profile::cupertino(), Profile::classic_pc()] {
            for class in [
                ClientClass::FirstParty,
                ClientClass::Terminal,
                ClientClass::Other,
            ] {
                let t = target(class);
                let all = chords();
                let rewritten: Vec<KeyPress> = all
                    .iter()
                    .filter_map(|p| {
                        let r = decide(&profile, &t, p);
                        (r != Rewrite::Pass).then(|| received(p, r))
                    })
                    .collect();
                for p in &all {
                    let r = decide(&profile, &t, p);
                    let got = received(p, r);
                    let back = pressed_for(&profile, class, &got);
                    assert_eq!(
                        received(&back, decide(&profile, &t, &back)),
                        got,
                        "{} {class:?}: {back:?} must give {got:?}",
                        profile.name
                    );
                    if r != Rewrite::Pass {
                        // a terminal gets Super+letter and Super+Shift+letter as one chord, pressed without Shift
                        let pressed = if class == ClientClass::Terminal && is_letter(p.sym) {
                            KeyPress {
                                raw: RawMods(p.raw.0 & !SHIFT.0),
                                ..*p
                            }
                        } else {
                            *p
                        };
                        assert_eq!(back, pressed, "{} {class:?}: {got:?}", profile.name);
                    }
                    if !rewritten.contains(p) {
                        assert_eq!(
                            pressed_for(&profile, class, p),
                            *p,
                            "{} {class:?}: no row gives {p:?}",
                            profile.name
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn ctrl_line_editing_chords_move_and_delete_in_a_text_input() {
        let text = ClientTarget {
            text_input_active: true,
            ..target(ClientClass::Other)
        };
        let key = |evdev| Rewrite::Key {
            evdev,
            remove: CTRL,
            add: RawMods::default(),
        };
        assert_eq!(reference(&text, &letter(18, 0x65, CTRL)), key(KEY_END));
        assert_eq!(reference(&text, &letter(30, 0x61, CTRL)), key(KEY_HOME));
        assert_eq!(reference(&text, &letter(32, 0x64, CTRL)), key(KEY_DELETE));
        // Shift stays with the key, which then selects
        assert_eq!(
            reference(&text, &letter(18, 0x45, CTRL | SHIFT)),
            key(KEY_END)
        );
        // outside a text input, and for other letters, the chord reaches the client with Super
        let swapped = Rewrite::Mods {
            remove: CTRL,
            add: SUP,
        };
        assert_eq!(
            reference(&target(ClientClass::Other), &letter(18, 0x65, CTRL)),
            swapped
        );
        assert_eq!(reference(&text, &letter(KEY_C, SYM_C, CTRL)), swapped);
        // a terminal and a first-party client keep Ctrl
        for class in [ClientClass::Terminal, ClientClass::FirstParty] {
            assert_eq!(
                reference(&target(class), &letter(18, 0x65, CTRL)),
                Rewrite::Pass
            );
        }
    }

    #[test]
    fn a_ctrl_chord_of_a_third_party_client_is_pressed_with_super() {
        let cupertino = Profile::cupertino();
        let q = letter(16, 0x71, CTRL);
        assert_eq!(pressed_for(&cupertino, ClientClass::Other, &q).raw, SUP);
        let f5 = named(KEY_F5, SYM_F5, CTRL);
        assert_eq!(pressed_for(&cupertino, ClientClass::Other, &f5), f5);
        assert_eq!(pressed_for(&cupertino, ClientClass::FirstParty, &q), q);
        assert_eq!(pressed_for(&cupertino, ClientClass::Terminal, &q), q);
        assert_eq!(
            pressed_for(
                &cupertino,
                ClientClass::Terminal,
                &letter(KEY_C, SYM_C, CTRL | SHIFT)
            )
            .raw,
            SUP
        );
        assert_eq!(
            pressed_for(&Profile::classic_pc(), ClientClass::Other, &q),
            q
        );
    }
}
