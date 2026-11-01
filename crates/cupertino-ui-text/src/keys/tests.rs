use std::time::{Duration, Instant};

use cupertino_key_bindings::{BindingTable, TextAction};
use cupertino_modifiers::Profile;

use crate::keymap::Command;
use crate::{
    Affinity, Direction, KeyInput, Keymap, Response, TextBuffer as TextField, TextFieldKeys,
};

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
fn arrows_move_on_screen_through_right_to_left_text() {
    let mut f = Fixture::new("abc \u{5D0}\u{5D1}\u{5D2} def");
    f.press(HOME, "", None);
    let mut carets = Vec::new();
    for _ in 0..6 {
        f.press(RIGHT, "", None);
        carets.push(f.field.caret());
    }
    assert_eq!(
        carets,
        [1, 2, 3, 4, 8, 6],
        "the Hebrew is crossed from its last letter"
    );
    f.press(RIGHT, "s", None);
    assert_eq!(f.field.selection(), 4..6);
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
fn option_arrows_move_by_word_in_the_order_the_words_are_drawn() {
    let mut f = Fixture::new("abc \u{5D0}\u{5D1}\u{5D2} \u{5D3}\u{5D4}\u{5D5} xyz");
    f.field.select(3..3);
    let mut carets = Vec::new();
    for _ in 0..3 {
        f.press(RIGHT, "a", None);
        carets.push(f.field.caret());
    }
    for _ in 0..4 {
        f.press(LEFT, "a", None);
        carets.push(f.field.caret());
    }
    assert_eq!(
        carets,
        [11, 4, 21, 18, 10, 17, 0],
        "the Hebrew word read last is drawn first"
    );
    f.field.select(4..10);
    f.press(RIGHT, "a", None);
    assert_eq!(
        f.field.caret(),
        21,
        "from the selection's right edge, its start"
    );
    f.field.select(4..10);
    f.press(LEFT, "a", None);
    assert_eq!(f.field.caret(), 17, "from its left edge, its end");
    f.field.select(3..3);
    f.field.move_word(true, false);
    assert_eq!(f.field.caret(), 10, "a word forward stays logical");
}

#[test]
fn option_right_in_a_right_to_left_field_goes_to_the_start_of_the_word_before() {
    let mut f = Fixture::new("\u{5D0}\u{5D1}\u{5D2} \u{5D3}\u{5D4}\u{5D5}");
    f.field.set_direction(Some(Direction::Rtl));
    f.press(END, "", None);
    let mut carets = Vec::new();
    for _ in 0..3 {
        f.press(RIGHT, "a", None);
        carets.push(f.field.caret());
    }
    assert_eq!(carets, [7, 0, 0]);
}

#[test]
fn a_new_direction_draws_the_caret_on_the_side_of_its_own_text() {
    let mut f = Fixture::new("abc \u{5D0}\u{5D1}\u{5D2}");
    f.field.select(3..3);
    f.press(RIGHT, "", None);
    assert_eq!(f.field.caret_position().affinity, Affinity::Upstream);
    f.field.set_direction(Some(Direction::Rtl));
    assert_eq!(f.field.caret_position().affinity, Affinity::Downstream);
}

#[test]
fn the_primary_modifier_follows_the_users_profile() {
    let mut f = Fixture::new("abc");
    assert_eq!(f.press(A, "m", Some('a')), Response::Moved);
    assert_eq!(f.field.selection(), 0..3, "Super+A selects all");
    f.press(E, "c", Some('e'));
    f.press(A, "c", Some('a'));
    assert_eq!(f.state(), ("abc", 0, 0..0), "Ctrl+A goes to the start");
    let pc = Keymap::new(Profile::classic_pc(), BindingTable::standard());
    let mut f = Fixture::with_keymap("abc", pc);
    f.press(A, "c", Some('a'));
    assert_eq!(
        f.field.selection(),
        0..3,
        "with Ctrl as the command key, Ctrl+A selects all"
    );
    // physical Super is the Ctrl role under classic-pc: Super+A goes to the start of the line, as Ctrl+A does under cupertino
    f.press(E, "m", Some('e'));
    f.press(A, "m", Some('a'));
    assert_eq!(f.state(), ("abc", 0, 0..0));
}

#[test]
fn logical_modifiers_skip_the_profile_and_a_command_chord_types_nothing() {
    use cupertino_modifiers::Mods;
    let mut f = Fixture::new("abc");
    assert_eq!(
        f.field
            .logical_key(&mut f.keymap, A, Mods::PRIMARY, Some('a')),
        Response::Moved
    );
    assert_eq!(f.field.selection(), 0..3, "Primary+A selects all");
    f.field.logical_key(&mut f.keymap, E, Mods::CTRL, Some('e'));
    assert_eq!(f.state(), ("abc", 3, 3..3), "Ctrl+E goes to the end");
    assert_eq!(
        f.field.logical_key(&mut f.keymap, 0, Mods::CTRL, Some('q')),
        Response::Ignored
    );
    assert_eq!(
        f.field
            .logical_key(&mut f.keymap, 0, Mods::SHIFT, Some('D')),
        Response::Edited
    );
    assert_eq!(f.field.text(), "abcD");
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

#[test]
fn a_paste_or_composition_keeps_only_what_typing_could_produce() {
    let mut f = TextField::new();
    f.paste("a\tb\u{0}c\u{1B}[31md\u{202E}e\u{2066}f\r\ng");
    assert_eq!(f.text(), "a bc[31mdef g");
    f.set_composition("x\u{7}\u{2069}y");
    assert_eq!(f.text(), "a bc[31mdef gxy");
}

#[test]
fn chords_use_the_latin_letter_of_the_key_on_a_non_latin_layout() {
    let mut f = Fixture::new("привет");
    assert_eq!(f.press(A, "m", Some('ф')), Response::Moved);
    assert_eq!(
        f.field.selection(),
        0..12,
        "Super+A on a Cyrillic layout selects all"
    );
    assert_eq!(f.press(C, "m", Some('с')), Response::Copy("привет".into()));
    assert_eq!(f.press(X, "m", Some('ч')), Response::Cut("привет".into()));
    assert_eq!(f.press(Z, "m", Some('я')), Response::Edited);
    assert_eq!(f.field.text(), "привет");
    assert_eq!(f.press(V, "m", Some('м')), Response::Paste);
    f.press(E, "c", Some('у'));
    assert_eq!(
        f.field.caret(),
        12,
        "Ctrl+E on a Cyrillic layout goes to the end"
    );
    assert_eq!(
        f.press(A, "", Some('ф')),
        Response::Edited,
        "typing still types the layout's letter"
    );
    assert_eq!(f.field.text(), "приветф");
}

#[test]
fn a_composition_is_an_undo_step_of_its_own() {
    let mut f = Fixture::new("");
    f.typing("ab");
    f.field.set_composition("k");
    f.field.set_composition("ka");
    f.field.commit_composition();
    f.typing("c");
    assert_eq!(f.field.text(), "abkac");
    f.field.undo();
    assert_eq!(
        f.field.text(),
        "abka",
        "typing after the composition is a step"
    );
    f.field.undo();
    assert_eq!(f.field.text(), "ab", "the composition is a step");
    f.field.undo();
    assert_eq!(f.field.text(), "");

    let mut f = Fixture::new("");
    f.typing("ab");
    f.field.set_composition("k");
    f.field.cancel_composition();
    f.typing("c");
    f.field.undo();
    assert_eq!(
        f.field.text(),
        "ab",
        "a cancelled composition ends the typing step and leaves none"
    );
    f.field.undo();
    assert_eq!(f.field.text(), "");
}

#[test]
fn a_field_with_a_maximum_length_cuts_typing_pastes_and_compositions_at_a_grapheme() {
    let mut f = TextField::with_max_len(8);
    f.set_text("abcdefghij");
    assert_eq!(f.text(), "abcdefgh", "set text is cut too");
    f.select(2..5);
    f.paste("e\u{301}\u{1F600}xyz");
    assert_eq!(
        f.text(),
        "abe\u{301}fgh",
        "the accented e fits in 3 bytes, the emoji not"
    );
    f.insert("z");
    assert_eq!(
        f.text(),
        "abe\u{301}fgh",
        "a full field takes no more typing"
    );
    f.select(0..f.text().len());
    f.set_composition("123456789");
    assert_eq!(f.text(), "12345678");
    f.undo();
    assert_eq!(f.text(), "abe\u{301}fgh");
    f.undo();
    assert_eq!(f.text(), "abcdefgh", "the refused typing left no undo step");
    let mut big = TextField::with_max_len(1024);
    big.paste(&"x".repeat(10_000));
    assert_eq!(big.text().len(), 1024);
}

#[test]
fn the_key_views_are_on_physical_ctrl_tab_under_classic_pc() {
    const TAB: u32 = 15;
    let next = Command::Actions(vec![TextAction::SelectNextKeyView]);
    let previous = Command::Actions(vec![TextAction::SelectPreviousKeyView]);
    // the Ctrl role is on Super there, and the system keeps Super+Tab for the overview: the key is Ctrl+Tab
    let mut pc = Keymap::standard_for(Profile::classic_pc());
    assert_eq!(pc.command(key(TAB, "c", None)), next);
    assert_eq!(pc.command(key(TAB, "cs", None)), previous);
    // under cupertino it is Ctrl+Tab too, and Super+Tab is the system's, not a key view
    let mut mac = Keymap::standard();
    assert_eq!(mac.command(key(TAB, "c", None)), next);
    assert_eq!(mac.command(key(TAB, "cs", None)), previous);
    assert_eq!(mac.command(key(TAB, "m", None)), Command::Ignored);
}

#[test]
fn a_binding_of_the_user_on_the_key_view_chord_stays_under_classic_pc() {
    use cupertino_key_bindings::{Chord, Key, NamedKey};
    use cupertino_modifiers::Mods;
    let mut table = BindingTable::standard();
    table.bind(
        vec![Chord::new(Mods::PRIMARY, Key::Named(NamedKey::Tab))],
        vec![TextAction::InsertTab],
    );
    let mut pc = Keymap::new(Profile::classic_pc(), table);
    assert_eq!(
        pc.command(key(15, "c", None)),
        Command::Actions(vec![TextAction::InsertTab])
    );
}
