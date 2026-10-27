use std::time::{Duration, Instant};

use cupertino_keyboard::{BindingTable, ModifierProfile};

use crate::{KeyInput, Keymap, Response, TextField};

const A: u32 = 30;
const C: u32 = 46;
const E: u32 = 18;
const K: u32 = 37;
const T: u32 = 20;
const V: u32 = 47;
const X: u32 = 45;
const Y: u32 = 21;
const Z: u32 = 44;
const BACKSPACE: u32 = 14;
const RETURN: u32 = 28;
const ESCAPE: u32 = 1;
const HOME: u32 = 102;
const END: u32 = 107;
const LEFT: u32 = 105;
const RIGHT: u32 = 106;
const DELETE: u32 = 111;

/// Modifiers held for a key: `s` Shift, `c` Ctrl, `a` Alt, `m` Super (the primary modifier of the design profile).
fn key(code: u32, mods: &str, character: Option<char>) -> KeyInput {
    KeyInput::new(
        code,
        mods.contains('s'),
        mods.contains('c'),
        mods.contains('a'),
        mods.contains('m'),
        character,
    )
}

struct Fixture {
    field: TextField,
    keymap: Keymap,
    now: Instant,
}

impl Fixture {
    fn new(text: &str) -> Self {
        Fixture::with_keymap(text, Keymap::standard())
    }

    fn with_keymap(text: &str, keymap: Keymap) -> Self {
        let mut field = TextField::new();
        field.set_text(text);
        Fixture {
            field,
            keymap,
            now: Instant::now(),
        }
    }

    fn press(&mut self, code: u32, mods: &str, character: Option<char>) -> Response {
        self.now += Duration::from_millis(10);
        self.field
            .key(&mut self.keymap, key(code, mods, character), self.now)
    }

    fn typing(&mut self, text: &str) {
        for c in text.chars() {
            assert_eq!(self.press(0, "", Some(c)), Response::Edited);
        }
    }

    fn state(&self) -> (&str, usize, std::ops::Range<usize>) {
        (
            self.field.text(),
            self.field.caret(),
            self.field.selection(),
        )
    }
}

#[test]
fn typing_inserts_at_the_caret_and_arrows_move_by_grapheme() {
    let mut f = Fixture::new("");
    f.typing("cafe\u{301}!");
    assert_eq!(f.state(), ("cafe\u{301}!", 7, 7..7));
    f.press(LEFT, "", None);
    f.press(LEFT, "", None);
    assert_eq!(
        f.field.caret(),
        3,
        "e and its combining accent are one step"
    );
    f.typing("X");
    assert_eq!(f.state(), ("cafXe\u{301}!", 4, 4..4));
    f.press(RIGHT, "", None);
    assert_eq!(f.field.caret(), 7);
}

#[test]
fn shift_extends_and_a_plain_arrow_collapses_the_selection_to_its_side() {
    let mut f = Fixture::new("hello world");
    f.press(HOME, "", None);
    f.press(RIGHT, "s", None);
    f.press(RIGHT, "s", None);
    assert_eq!(f.state(), ("hello world", 2, 0..2));
    assert_eq!(f.press(RIGHT, "", None), Response::Moved);
    assert_eq!(f.state(), ("hello world", 2, 2..2));
    f.press(RIGHT, "s", None);
    f.press(LEFT, "", None);
    assert_eq!(f.state(), ("hello world", 2, 2..2));
}

#[test]
fn option_arrows_move_by_word_and_command_arrows_home_and_end_to_the_line_ends() {
    let mut f = Fixture::new("one two, three");
    f.press(LEFT, "a", None);
    assert_eq!(f.field.caret(), 9);
    f.press(LEFT, "a", None);
    assert_eq!(f.field.caret(), 4);
    f.press(RIGHT, "a", None);
    assert_eq!(
        f.field.caret(),
        7,
        "to the end of the word, before the comma"
    );
    f.press(RIGHT, "as", None);
    assert_eq!(f.state(), ("one two, three", 14, 7..14));
    f.press(LEFT, "m", None);
    assert_eq!(f.state(), ("one two, three", 0, 0..0));
    f.press(RIGHT, "ms", None);
    assert_eq!(f.field.selection(), 0..14);
    f.press(END, "", None);
    assert_eq!(f.state(), ("one two, three", 14, 14..14));
    f.press(HOME, "", None);
    assert_eq!(f.field.caret(), 0);
    f.press(END, "s", None);
    assert_eq!(f.field.selection(), 0..14);
}

#[test]
fn the_primary_modifier_follows_the_users_profile() {
    let mut f = Fixture::new("abc");
    assert_eq!(f.press(A, "m", Some('a')), Response::Moved);
    assert_eq!(f.field.selection(), 0..3, "Super+A selects all");
    f.press(E, "c", Some('e'));
    f.press(A, "c", Some('a'));
    assert_eq!(f.state(), ("abc", 0, 0..0), "Ctrl+A goes to the start");
    let pc = Keymap::new(ModifierProfile::classic_pc(), BindingTable::standard());
    let mut f = Fixture::with_keymap("abc", pc);
    f.press(A, "c", Some('a'));
    assert_eq!(
        f.field.selection(),
        0..3,
        "with Ctrl as the command key, Ctrl+A selects all"
    );
    assert_eq!(f.press(A, "m", Some('a')), Response::Ignored);
}

#[test]
fn copy_cut_and_paste_ask_the_window_for_the_clipboard() {
    let mut f = Fixture::new("hello world");
    assert_eq!(
        f.press(C, "m", Some('c')),
        Response::Moved,
        "nothing selected"
    );
    f.press(LEFT, "as", None);
    assert_eq!(f.press(C, "m", Some('c')), Response::Copy("world".into()));
    assert_eq!(f.press(X, "m", Some('x')), Response::Cut("world".into()));
    assert_eq!(f.state(), ("hello ", 6, 6..6));
    assert_eq!(f.press(V, "m", Some('v')), Response::Paste);
    f.field.paste("big\nworld");
    assert_eq!(f.state(), ("hello big world", 15, 15..15));
}

#[test]
fn undo_takes_back_a_run_of_typing_at_once_and_redo_brings_it_again() {
    let mut f = Fixture::new("");
    f.typing("ab");
    f.press(LEFT, "", None);
    f.typing("cd");
    assert_eq!(f.field.text(), "acdb");
    assert_eq!(f.press(Z, "m", Some('z')), Response::Edited);
    assert_eq!(f.state(), ("ab", 1, 1..1));
    f.press(Z, "m", Some('z'));
    assert_eq!(f.state(), ("", 0, 0..0));
    assert_eq!(
        f.press(Z, "m", Some('z')),
        Response::Moved,
        "nothing left to undo"
    );
    f.press(Z, "ms", Some('Z'));
    f.press(Z, "ms", Some('Z'));
    assert_eq!(f.state(), ("acdb", 3, 3..3));
}

#[test]
fn undo_restores_the_selection_an_edit_replaced() {
    let mut f = Fixture::new("hello");
    f.press(A, "m", Some('a'));
    f.typing("x");
    assert_eq!(f.state(), ("x", 1, 1..1));
    f.press(Z, "m", Some('z'));
    assert_eq!(f.state(), ("hello", 5, 0..5));
}

#[test]
fn deleting_keys_remove_a_grapheme_a_word_or_to_the_line_start() {
    let mut f = Fixture::new("one two three");
    f.press(BACKSPACE, "", None);
    assert_eq!(f.field.text(), "one two thre");
    f.press(BACKSPACE, "a", None);
    assert_eq!(f.field.text(), "one two ");
    f.press(HOME, "", None);
    f.press(DELETE, "", None);
    assert_eq!(f.field.text(), "ne two ");
    f.press(END, "", None);
    f.press(BACKSPACE, "m", None);
    assert_eq!(f.state(), ("", 0, 0..0));
}

#[test]
fn ctrl_k_kills_to_the_end_ctrl_y_yanks_and_ctrl_t_transposes() {
    let mut f = Fixture::new("abc def");
    f.press(HOME, "", None);
    f.press(RIGHT, "a", None);
    f.press(K, "c", Some('k'));
    assert_eq!(f.state(), ("abc", 3, 3..3));
    f.press(HOME, "", None);
    f.press(Y, "c", Some('y'));
    assert_eq!(f.state(), (" defabc", 4, 4..4));
    f.press(T, "c", Some('t'));
    assert_eq!(f.state(), (" deafbc", 5, 5..5));
}

#[test]
fn return_escape_and_unbound_shortcuts_go_to_the_window() {
    let mut f = Fixture::new("abc");
    assert_eq!(f.press(RETURN, "", Some('\r')), Response::Ignored);
    assert_eq!(f.press(ESCAPE, "", None), Response::Ignored);
    assert_eq!(f.press(0, "m", Some('f')), Response::Ignored);
    assert_eq!(f.field.text(), "abc");
}

#[test]
fn clicks_place_the_caret_select_a_word_or_everything_and_drags_extend() {
    let mut f = Fixture::new("one two three");
    let t = f.now;
    f.field.press(5, false, t);
    f.field.release();
    assert_eq!(f.state(), ("one two three", 5, 5..5));
    f.field.press(5, false, t + Duration::from_millis(300));
    f.field.release();
    assert_eq!(f.field.selection(), 4..7, "double click: the word");
    f.field.press(5, false, t + Duration::from_millis(600));
    assert_eq!(f.field.selection(), 0..13, "triple click: everything");
    f.field.release();
    f.field.press(5, false, t + Duration::from_millis(1500));
    f.field.drag_to(10);
    f.field.release();
    assert_eq!(f.state(), ("one two three", 10, 5..10));
    f.field.press(2, true, t + Duration::from_millis(1600));
    assert_eq!(f.field.selection(), 2..5, "Shift extends from the anchor");
    f.field.release();
    f.field.press(5, false, t + Duration::from_millis(2500));
    f.field.press(5, false, t + Duration::from_millis(2700));
    f.field.drag_to(10);
    assert_eq!(
        f.state(),
        ("one two three", 13, 4..13),
        "a word drag extends by words"
    );
}

#[test]
fn a_press_maps_to_the_nearest_caret_position() {
    let mut f = Fixture::new("a\u{1F600}b");
    assert_eq!(f.field.boundaries(), vec![0, 1, 5, 6]);
    let offsets = [0.0, 8.0, 24.0, 32.0];
    assert_eq!(f.field.index_at(&offsets, 3.9), 0);
    assert_eq!(f.field.index_at(&offsets, 17.0), 5);
    assert_eq!(f.field.index_at(&offsets, 90.0), 6);
    f.field.press(3, false, f.now);
    assert_eq!(f.field.caret(), 1, "inside the emoji goes to its start");
}

#[test]
fn the_caret_blinks_after_a_full_on_phase_and_hides_while_text_is_selected() {
    let mut f = Fixture::new("abc");
    let t = f.now + Duration::from_millis(10);
    f.press(LEFT, "", None);
    assert!(f.field.caret_visible(t + Duration::from_millis(499)));
    assert!(!f.field.caret_visible(t + Duration::from_millis(500)));
    assert!(!f.field.caret_visible(t + Duration::from_millis(999)));
    assert!(f.field.caret_visible(t + Duration::from_millis(1000)));
    assert_eq!(
        f.field.next_blink(t + Duration::from_millis(200)),
        Some(Duration::from_millis(300))
    );
    assert_eq!(
        f.field.next_blink(t + Duration::from_millis(700)),
        Some(Duration::from_millis(300))
    );
    f.press(A, "m", Some('a'));
    assert!(!f.field.caret_visible(f.now));
    assert_eq!(f.field.next_blink(f.now), None);
}

#[test]
fn a_composition_replaces_itself_until_committed_or_cancelled() {
    let mut f = Fixture::new("ab");
    f.field.set_composition("k");
    f.field.set_composition("ka");
    assert_eq!(f.field.text(), "abka");
    assert_eq!(f.field.composition(), Some(2..4));
    f.field.cancel_composition();
    assert_eq!((f.field.text(), f.field.composition()), ("ab", None));
    f.field.set_composition("\u{304B}");
    f.field.commit_composition();
    assert_eq!(f.state(), ("ab\u{304B}", 5, 5..5));
    assert_eq!(f.field.composition(), None);
}

#[test]
fn the_scroll_keeps_the_caret_inside_the_field() {
    let mut f = TextField::new();
    f.follow_caret(150.0, 200.0, 100.0);
    assert_eq!(f.scroll(), 50.0);
    f.follow_caret(120.0, 200.0, 100.0);
    assert_eq!(f.scroll(), 50.0, "still in view");
    f.follow_caret(20.0, 200.0, 100.0);
    assert_eq!(f.scroll(), 20.0);
    f.follow_caret(20.0, 60.0, 100.0);
    assert_eq!(f.scroll(), 0.0, "text narrower than the field");
}

#[test]
fn blur_drops_the_selection_and_keeps_the_text() {
    let mut f = Fixture::new("Sound");
    f.press(A, "m", Some('a'));
    f.field.blur();
    f.typing("s");
    assert_eq!(f.state(), ("Sounds", 6, 6..6));
}
