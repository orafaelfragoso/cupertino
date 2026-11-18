//! A field driven by key presses: the standard chords resolve under the user's modifier profile, and a key that is no
//! field key is left to the window.

use cupertino_keyboard::modifiers::Profile;
use cupertino_ui_text::{KeyInput, Keymap, Response, TextBuffer, TextFieldKeys};
use std::time::Instant;

const KEY_A: u32 = 30;
const KEY_C: u32 = 46;
const KEY_X: u32 = 45;
const KEY_V: u32 = 47;
const KEY_BACKSPACE: u32 = 14;
const KEY_LEFT: u32 = 105;
const KEY_RETURN: u32 = 28;
const KEY_TAB: u32 = 15;

type Selection = std::ops::Range<usize>;

/// The physical shift, control, alt and super held with a key.
type Held = [bool; 4];
const NONE: Held = [false; 4];
const CTRL: Held = [false, true, false, false];
const ALT: Held = [false, false, true, false];
const SUPER: Held = [false, false, false, true];

fn press(code: u32, held: Held, character: Option<char>) -> KeyInput {
    KeyInput::new(code, held[0], held[1], held[2], held[3], character)
}

fn field(selection: std::ops::Range<usize>) -> TextBuffer {
    let mut buffer = TextBuffer::new();
    buffer.set_text("hello");
    buffer.select(selection);
    buffer
}

#[test]
fn the_standard_chords_edit_a_field_by_the_role_the_users_profile_gives_each_modifier() {
    let copy = |text: &str| Response::Copy(text.into());
    type Case = (
        &'static str,
        Profile,
        KeyInput,
        Selection,
        Response,
        &'static str,
        Selection,
    );
    let rows: Vec<Case> = vec![
        (
            "a letter",
            Profile::cupertino(),
            press(KEY_A, NONE, Some('x')),
            5..5,
            Response::Edited,
            "hellox",
            6..6,
        ),
        (
            "select all",
            Profile::cupertino(),
            press(KEY_A, SUPER, Some('a')),
            5..5,
            Response::Moved,
            "hello",
            0..5,
        ),
        (
            "copy",
            Profile::cupertino(),
            press(KEY_C, SUPER, Some('c')),
            1..3,
            copy("el"),
            "hello",
            1..3,
        ),
        (
            "copy of no selection",
            Profile::cupertino(),
            press(KEY_C, SUPER, Some('c')),
            2..2,
            Response::Moved,
            "hello",
            2..2,
        ),
        (
            "cut",
            Profile::cupertino(),
            press(KEY_X, SUPER, Some('x')),
            0..2,
            Response::Cut("he".into()),
            "llo",
            0..0,
        ),
        (
            "paste asks for the clipboard",
            Profile::cupertino(),
            press(KEY_V, SUPER, Some('v')),
            5..5,
            Response::Paste,
            "hello",
            5..5,
        ),
        (
            "backspace",
            Profile::cupertino(),
            press(KEY_BACKSPACE, NONE, None),
            5..5,
            Response::Edited,
            "hell",
            4..4,
        ),
        (
            "option and backspace",
            Profile::cupertino(),
            press(KEY_BACKSPACE, ALT, None),
            5..5,
            Response::Edited,
            "",
            0..0,
        ),
        (
            "an arrow",
            Profile::cupertino(),
            press(KEY_LEFT, NONE, None),
            5..5,
            Response::Moved,
            "hello",
            4..4,
        ),
        (
            "return is the window's",
            Profile::cupertino(),
            press(KEY_RETURN, NONE, None),
            5..5,
            Response::Ignored,
            "hello",
            5..5,
        ),
        (
            "tab is the window's",
            Profile::cupertino(),
            press(KEY_TAB, NONE, None),
            5..5,
            Response::Ignored,
            "hello",
            5..5,
        ),
        (
            "control is not the command key",
            Profile::cupertino(),
            press(KEY_C, CTRL, Some('c')),
            1..3,
            Response::Ignored,
            "hello",
            1..3,
        ),
        (
            "copy on a classic keyboard",
            Profile::classic_pc(),
            press(KEY_C, CTRL, Some('c')),
            1..3,
            copy("el"),
            "hello",
            1..3,
        ),
        (
            "super is not the command key there",
            Profile::classic_pc(),
            press(KEY_C, SUPER, Some('c')),
            1..3,
            Response::Ignored,
            "hello",
            1..3,
        ),
    ];
    for (what, profile, key, selection, response, text, after) in rows {
        let mut buffer = field(selection);
        let mut keymap = Keymap::standard_for(profile);
        assert_eq!(
            buffer.key(&mut keymap, key, Instant::now()),
            response,
            "{what}"
        );
        assert_eq!((buffer.text(), buffer.selection()), (text, after), "{what}");
    }
}
