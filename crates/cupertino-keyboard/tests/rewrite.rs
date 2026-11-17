//! Which Super, Ctrl and Alt chords are rewritten for third-party clients, and how a received
//! chord maps back to the keys pressed.

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
const KEY_RETURN: u32 = 28;
const SYM_LEFT: u32 = 0xff51;
const SYM_DOWN: u32 = 0xff54;
const SYM_BACKSPACE: u32 = 0xff08;
const SYM_TAB: u32 = 0xff09;

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

fn target(class: ClientClass, text_input_active: bool) -> ClientTarget {
    ClientTarget {
        class,
        text_input_active,
        opted_out: false,
    }
}

fn mods(remove: RawMods, add: RawMods) -> Rewrite {
    Rewrite::Mods { remove, add }
}

fn key(evdev: u32, remove: RawMods, add: RawMods) -> Rewrite {
    Rewrite::Key { evdev, remove, add }
}

#[test]
fn printable_characters_exclude_space_controls_and_none() {
    for (ch, printable) in [
        (Some('a'), true),
        (Some('é'), true),
        (Some(' '), false),
        (Some('\n'), false),
        (Some('\u{7}'), false),
        (None, false),
    ] {
        assert_eq!(is_printable(ch), printable, "{ch:?}");
    }
}

#[test]
fn cupertino_profile_rewrites_chords_by_client_class_and_text_input() {
    use ClientClass::{FirstParty, Other, Terminal};
    let (sup, ctrl, alt, shift, none) = (
        RawMods::SUPER,
        RawMods::CTRL,
        RawMods::ALT,
        RawMods::SHIFT,
        RawMods::default(),
    );
    let modifier_key = KeyPress {
        is_modifier: true,
        ..letter(KEY_C, 'c', sup)
    };
    let rows = [
        (
            "Super+letter is the Ctrl chord",
            target(Other, false),
            letter(KEY_C, 'c', sup),
            mods(sup, ctrl),
        ),
        (
            "Super+Shift+letter keeps Shift",
            target(Other, false),
            letter(KEY_A, 'z', sup | shift),
            mods(sup, ctrl),
        ),
        (
            "Super+Return is Ctrl+Return",
            target(Other, false),
            press(KEY_RETURN, SYM_RETURN, sup),
            mods(sup, ctrl),
        ),
        (
            "Ctrl+letter is not the command key: it reaches the app as Super",
            target(Other, false),
            letter(KEY_C, 'c', ctrl),
            mods(ctrl, sup),
        ),
        (
            "Ctrl+A outside a text input is the Super chord",
            target(Other, false),
            letter(KEY_A, 'a', ctrl),
            mods(ctrl, sup),
        ),
        (
            "Super+Left in a text input is Home",
            target(Other, true),
            press(KEY_LEFT, SYM_LEFT, sup),
            key(KEY_HOME, sup, none),
        ),
        (
            "Super+Down in a text input is Ctrl+End",
            target(Other, true),
            press(KEY_DOWN, SYM_DOWN, sup),
            key(KEY_END, sup, ctrl),
        ),
        (
            "Super+Left without a text input passes",
            target(Other, false),
            press(KEY_LEFT, SYM_LEFT, sup),
            Rewrite::Pass,
        ),
        (
            "Super+Backspace without a text input passes",
            target(Other, false),
            press(KEY_BACKSPACE, SYM_BACKSPACE, sup),
            Rewrite::Pass,
        ),
        (
            "Alt+Left in a text input is Ctrl+Left",
            target(Other, true),
            press(KEY_LEFT, SYM_LEFT, alt),
            mods(alt, ctrl),
        ),
        (
            "Alt+Left without a text input passes",
            target(Other, false),
            press(KEY_LEFT, SYM_LEFT, alt),
            Rewrite::Pass,
        ),
        (
            "Super+Tab passes",
            target(Other, false),
            press(KEY_TAB, SYM_TAB, sup),
            Rewrite::Pass,
        ),
        (
            "a modifier key passes",
            target(Other, false),
            modifier_key,
            Rewrite::Pass,
        ),
        (
            "Ctrl+Super passes",
            target(Other, false),
            letter(KEY_C, 'c', sup | ctrl),
            Rewrite::Pass,
        ),
        (
            "first-party clients get the chord as pressed",
            target(FirstParty, true),
            letter(KEY_C, 'c', sup),
            Rewrite::Pass,
        ),
        (
            "opted-out clients get the chord as pressed",
            ClientTarget {
                opted_out: true,
                ..target(Other, true)
            },
            letter(KEY_C, 'c', sup),
            Rewrite::Pass,
        ),
        (
            "terminal Super+letter is Ctrl+Shift",
            target(Terminal, false),
            letter(KEY_C, 'c', sup),
            mods(sup, ctrl | shift),
        ),
        (
            "terminal Super on a text size key is Ctrl",
            target(Terminal, false),
            press(KEY_MINUS, 0x2d, sup),
            mods(sup, ctrl),
        ),
        (
            "terminal Ctrl+letter reaches it unchanged",
            target(Terminal, true),
            letter(KEY_C, 'c', ctrl),
            Rewrite::Pass,
        ),
        (
            "terminal Super+Tab reaches it unchanged",
            target(Terminal, true),
            press(KEY_TAB, SYM_TAB, sup),
            Rewrite::Pass,
        ),
    ];
    let cup = Profile::cupertino();
    for (row, client, k, expected) in rows {
        assert_eq!(decide(&cup, &client, &k), expected, "{row}");
    }
    for (ch, evdev) in [
        ('a', KEY_HOME),
        ('e', KEY_END),
        ('b', KEY_LEFT),
        ('f', KEY_RIGHT),
        ('p', KEY_UP),
        ('n', KEY_DOWN),
        ('d', KEY_DELETE),
        ('h', KEY_BACKSPACE),
    ] {
        assert_eq!(
            decide(&cup, &target(Other, true), &letter(1, ch, ctrl)),
            key(evdev, ctrl, none),
            "Ctrl+{ch} in a text input is its line-editing key"
        );
    }
}

#[test]
fn the_classic_pc_profile_is_never_rewritten() {
    for client in [ClientClass::Other, ClientClass::Terminal] {
        for raw in [RawMods::SUPER, RawMods::CTRL] {
            assert_eq!(
                decide(
                    &Profile::classic_pc(),
                    &target(client, true),
                    &letter(KEY_C, 'c', raw)
                ),
                Rewrite::Pass,
                "{client:?} {raw:?}"
            );
        }
    }
}

#[test]
fn pressed_for_inverts_the_rewrite() {
    let cup = Profile::cupertino();
    let pc = Profile::classic_pc();
    for (row, profile, class, received, pressed) in [
        (
            "other: Ctrl came from Super",
            &cup,
            ClientClass::Other,
            RawMods::CTRL,
            RawMods::SUPER,
        ),
        (
            "other: Super came from Ctrl",
            &cup,
            ClientClass::Other,
            RawMods::SUPER,
            RawMods::CTRL,
        ),
        (
            "terminal: Ctrl+Shift+C came from Super+C",
            &cup,
            ClientClass::Terminal,
            RawMods::CTRL | RawMods::SHIFT,
            RawMods::SUPER,
        ),
        (
            "first party: unchanged",
            &cup,
            ClientClass::FirstParty,
            RawMods::CTRL,
            RawMods::CTRL,
        ),
        (
            "classic profile: unchanged",
            &pc,
            ClientClass::Other,
            RawMods::CTRL,
            RawMods::CTRL,
        ),
    ] {
        let k = letter(KEY_C, 'c', received);
        let back = pressed_for(profile, class, &k);
        assert_eq!(back, KeyPress { raw: pressed, ..k }, "{row}");
    }
}
