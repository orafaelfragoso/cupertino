//! Which Super and Alt chords are rewritten for third-party clients.

use cupertino_keyboard::modifiers::{Profile, RawMods};
use cupertino_keyboard::rewrite::{
    pressed_for, KEY_BACKSPACE, KEY_DELETE, KEY_DOWN, KEY_END, KEY_HOME, KEY_LEFT, KEY_RIGHT,
    KEY_UP, SYM_RETURN,
};
use cupertino_keyboard::{decide, is_printable, ClientClass, ClientTarget, KeyPress, Rewrite};

const KEY_C: u32 = 46;
const KEY_A: u32 = 30;
const KEY_TAB: u32 = 15;
const KEY_MINUS: u32 = 12;

fn target(class: ClientClass, text_input_active: bool) -> ClientTarget {
    ClientTarget {
        class,
        text_input_active,
        opted_out: false,
    }
}

fn press(evdev: u32, sym: u32, raw: RawMods) -> KeyPress {
    KeyPress {
        evdev,
        sym,
        printable: (0x21..0x7f).contains(&sym),
        is_modifier: false,
        raw,
    }
}

fn letter(evdev: u32, ch: char, raw: RawMods) -> KeyPress {
    press(evdev, ch as u32, raw)
}

fn other(text_input: bool) -> ClientTarget {
    target(ClientClass::Other, text_input)
}

fn cup() -> Profile {
    Profile::cupertino()
}

#[test]
fn printable_characters_exclude_space_controls_and_none() {
    assert!(is_printable(Some('a')));
    assert!(is_printable(Some('é')));
    assert!(!is_printable(Some(' ')));
    assert!(!is_printable(Some('\n')));
    assert!(!is_printable(Some('\u{7}')));
    assert!(!is_printable(None));
}

#[test]
fn super_letter_becomes_the_ctrl_chord_for_other_clients() {
    let r = decide(&cup(), &other(false), &letter(KEY_C, 'c', RawMods::SUPER));
    assert_eq!(
        r,
        Rewrite::Mods {
            remove: RawMods::SUPER,
            add: RawMods::CTRL
        }
    );
}

#[test]
fn super_chord_keeps_shift_and_alt() {
    let r = decide(
        &cup(),
        &other(false),
        &letter(KEY_A, 'z', RawMods::SUPER | RawMods::SHIFT),
    );
    assert_eq!(
        r,
        Rewrite::Mods {
            remove: RawMods::SUPER,
            add: RawMods::CTRL
        }
    );
}

#[test]
fn super_return_becomes_ctrl_return() {
    let r = decide(
        &cup(),
        &other(false),
        &press(28, SYM_RETURN, RawMods::SUPER),
    );
    assert_eq!(
        r,
        Rewrite::Mods {
            remove: RawMods::SUPER,
            add: RawMods::CTRL
        }
    );
}

#[test]
fn ctrl_chord_reaches_other_clients_as_super() {
    let r = decide(&cup(), &other(false), &letter(KEY_C, 'c', RawMods::CTRL));
    assert_eq!(
        r,
        Rewrite::Mods {
            remove: RawMods::CTRL,
            add: RawMods::SUPER
        }
    );
}

#[test]
fn ctrl_line_editing_chords_become_their_keys_in_a_text_input() {
    let cases = [
        ('a', KEY_HOME),
        ('e', KEY_END),
        ('b', KEY_LEFT),
        ('f', KEY_RIGHT),
        ('p', KEY_UP),
        ('n', KEY_DOWN),
        ('d', KEY_DELETE),
        ('h', KEY_BACKSPACE),
    ];
    for (ch, evdev) in cases {
        let r = decide(&cup(), &other(true), &letter(1, ch, RawMods::CTRL));
        assert_eq!(
            r,
            Rewrite::Key {
                evdev,
                remove: RawMods::CTRL,
                add: RawMods::default()
            },
            "Ctrl+{ch}"
        );
    }
}

#[test]
fn ctrl_line_editing_chord_outside_a_text_input_is_the_super_chord() {
    let r = decide(&cup(), &other(false), &letter(KEY_A, 'a', RawMods::CTRL));
    assert_eq!(
        r,
        Rewrite::Mods {
            remove: RawMods::CTRL,
            add: RawMods::SUPER
        }
    );
}

#[test]
fn super_arrows_become_home_and_end_in_a_text_input() {
    let left = decide(
        &cup(),
        &other(true),
        &press(KEY_LEFT, 0xff51, RawMods::SUPER),
    );
    assert_eq!(
        left,
        Rewrite::Key {
            evdev: KEY_HOME,
            remove: RawMods::SUPER,
            add: RawMods::default()
        }
    );
    let down = decide(
        &cup(),
        &other(true),
        &press(KEY_DOWN, 0xff54, RawMods::SUPER),
    );
    assert_eq!(
        down,
        Rewrite::Key {
            evdev: KEY_END,
            remove: RawMods::SUPER,
            add: RawMods::CTRL
        }
    );
}

#[test]
fn super_arrows_and_backspace_pass_without_a_text_input() {
    assert_eq!(
        decide(
            &cup(),
            &other(false),
            &press(KEY_LEFT, 0xff51, RawMods::SUPER)
        ),
        Rewrite::Pass
    );
    assert_eq!(
        decide(
            &cup(),
            &other(false),
            &press(KEY_BACKSPACE, 0xff08, RawMods::SUPER)
        ),
        Rewrite::Pass
    );
}

#[test]
fn alt_word_chords_become_ctrl_in_a_text_input() {
    let r = decide(&cup(), &other(true), &press(KEY_LEFT, 0xff51, RawMods::ALT));
    assert_eq!(
        r,
        Rewrite::Mods {
            remove: RawMods::ALT,
            add: RawMods::CTRL
        }
    );
    assert_eq!(
        decide(
            &cup(),
            &other(false),
            &press(KEY_LEFT, 0xff51, RawMods::ALT)
        ),
        Rewrite::Pass
    );
}

#[test]
fn super_tab_and_unprintable_keys_pass() {
    assert_eq!(
        decide(
            &cup(),
            &other(false),
            &press(KEY_TAB, 0xff09, RawMods::SUPER)
        ),
        Rewrite::Pass
    );
}

#[test]
fn first_party_clients_and_opt_outs_are_never_rewritten() {
    let k = letter(KEY_C, 'c', RawMods::SUPER);
    assert_eq!(
        decide(&cup(), &target(ClientClass::FirstParty, true), &k),
        Rewrite::Pass
    );
    let opted = ClientTarget {
        opted_out: true,
        ..other(true)
    };
    assert_eq!(decide(&cup(), &opted, &k), Rewrite::Pass);
}

#[test]
fn other_profiles_are_never_rewritten() {
    let k = letter(KEY_C, 'c', RawMods::SUPER);
    assert_eq!(
        decide(&Profile::classic_pc(), &other(true), &k),
        Rewrite::Pass
    );
}

#[test]
fn modifier_keys_and_ctrl_super_chords_pass() {
    let mut k = letter(KEY_C, 'c', RawMods::SUPER);
    k.is_modifier = true;
    assert_eq!(decide(&cup(), &other(false), &k), Rewrite::Pass);
    let both = letter(KEY_C, 'c', RawMods::SUPER | RawMods::CTRL);
    assert_eq!(decide(&cup(), &other(false), &both), Rewrite::Pass);
}

#[test]
fn terminal_super_letter_is_ctrl_shift() {
    let t = target(ClientClass::Terminal, false);
    let r = decide(&cup(), &t, &letter(KEY_C, 'c', RawMods::SUPER));
    assert_eq!(
        r,
        Rewrite::Mods {
            remove: RawMods::SUPER,
            add: RawMods::CTRL | RawMods::SHIFT
        }
    );
}

#[test]
fn terminal_super_size_key_is_ctrl() {
    let t = target(ClientClass::Terminal, false);
    let r = decide(&cup(), &t, &press(KEY_MINUS, 0x2d, RawMods::SUPER));
    assert_eq!(
        r,
        Rewrite::Mods {
            remove: RawMods::SUPER,
            add: RawMods::CTRL
        }
    );
}

#[test]
fn terminal_ctrl_chords_and_other_super_keys_reach_it_unchanged() {
    let t = target(ClientClass::Terminal, true);
    assert_eq!(
        decide(&cup(), &t, &letter(KEY_C, 'c', RawMods::CTRL)),
        Rewrite::Pass
    );
    assert_eq!(
        decide(&cup(), &t, &press(KEY_TAB, 0xff09, RawMods::SUPER)),
        Rewrite::Pass
    );
}

#[test]
fn pressed_for_inverts_the_super_row_for_other_clients() {
    let received = letter(KEY_C, 'c', RawMods::CTRL);
    assert_eq!(
        pressed_for(&cup(), ClientClass::Other, &received).raw,
        RawMods::SUPER
    );
}

#[test]
fn pressed_for_inverts_the_ctrl_row_for_other_clients() {
    let received = letter(KEY_C, 'c', RawMods::SUPER);
    assert_eq!(
        pressed_for(&cup(), ClientClass::Other, &received).raw,
        RawMods::CTRL
    );
}

#[test]
fn pressed_for_a_terminal_copy_is_super_c() {
    let received = letter(KEY_C, 'c', RawMods::CTRL | RawMods::SHIFT);
    assert_eq!(
        pressed_for(&cup(), ClientClass::Terminal, &received).raw,
        RawMods::SUPER
    );
}

#[test]
fn pressed_for_leaves_first_party_and_classic_chords_alone() {
    let received = letter(KEY_C, 'c', RawMods::CTRL);
    assert_eq!(
        pressed_for(&cup(), ClientClass::FirstParty, &received),
        received
    );
    assert_eq!(
        pressed_for(&Profile::classic_pc(), ClientClass::Other, &received),
        received
    );
}
