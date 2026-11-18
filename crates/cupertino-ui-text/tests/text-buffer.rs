//! The editing model of a text field: what its caret, selection, undo and kill buffer do for each edit, and how an
//! input method's batch lands in it.

use cupertino_ui_text::{ImeBatch, Preedit, Surrounding, TextBuffer};
use unicode_segmentation::UnicodeSegmentation;

const FLAG: &str = "\u{1F1E9}\u{1F1EA}";
const FAMILY: &str = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
const A_FAMILY: &str = "a\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
const FLAG_X: &str = "\u{1F1E9}\u{1F1EA}x";
const E_ACUTE_B: &str = "e\u{301}b";

/// A buffer holding `text` with its caret at `caret`.
fn at(text: &str, caret: usize) -> TextBuffer {
    let mut buffer = TextBuffer::new();
    buffer.set_text(text);
    buffer.select(caret..caret);
    buffer
}

#[test]
fn deleting_and_moving_go_by_grapheme_and_by_word() {
    type Edit = fn(&mut TextBuffer);
    let rows: Vec<(&str, String, usize, Edit, &str, usize)> = vec![
        (
            "backspace removes a whole family",
            A_FAMILY.into(),
            A_FAMILY.len(),
            |b| b.delete_backward(),
            "a",
            1,
        ),
        (
            "backspace removes a letter with its accent",
            E_ACUTE_B.into(),
            3,
            |b| b.delete_backward(),
            "b",
            0,
        ),
        (
            "delete removes a flag",
            FLAG_X.into(),
            0,
            |b| b.delete_forward(),
            "x",
            0,
        ),
        (
            "backspace at the start does nothing",
            "ab".into(),
            0,
            |b| b.delete_backward(),
            "ab",
            0,
        ),
        (
            "delete at the end does nothing",
            "ab".into(),
            2,
            |b| b.delete_forward(),
            "ab",
            2,
        ),
        (
            "left steps over a family",
            A_FAMILY.into(),
            A_FAMILY.len(),
            |b| b.move_char(false, false),
            A_FAMILY,
            1,
        ),
        (
            "right steps over a flag",
            FLAG_X.into(),
            0,
            |b| b.move_char(true, false),
            FLAG_X,
            8,
        ),
        (
            "a word left",
            "one two three".into(),
            13,
            |b| b.move_word(false, false),
            "one two three",
            8,
        ),
        (
            "a word right",
            "one two three".into(),
            0,
            |b| b.move_word(true, false),
            "one two three",
            3,
        ),
        (
            "delete the word before",
            "one two three".into(),
            13,
            |b| b.delete_word_backward(),
            "one two ",
            8,
        ),
        (
            "delete the word after",
            "one two three".into(),
            4,
            |b| b.delete_word_forward(),
            "one  three",
            4,
        ),
        (
            "delete to the line start",
            "one two".into(),
            4,
            |b| b.delete_to_line_start(),
            "two",
            0,
        ),
    ];
    for (what, text, caret, edit, expected, expected_caret) in rows {
        let mut buffer = at(&text, caret);
        edit(&mut buffer);
        assert_eq!(
            (buffer.text(), buffer.caret()),
            (expected, expected_caret),
            "{what}"
        );
    }
}

#[test]
fn a_selection_is_replaced_by_typing_extended_by_moving_and_taken_by_cut() {
    let mut buffer = at("hello world", 0);
    buffer.move_word(true, true);
    assert_eq!(buffer.selected_text(), "hello");
    buffer.move_char(true, true);
    assert_eq!(buffer.selected_text(), "hello ");
    buffer.insert("big ");
    assert_eq!((buffer.text(), buffer.caret()), ("big world", 4));

    buffer.select_all();
    assert_eq!(buffer.cut(), "big world");
    assert!(buffer.is_empty());
    buffer.paste("pasted");
    assert_eq!(buffer.text(), "pasted");

    buffer.select(2..4);
    buffer.delete_selection();
    assert_eq!(buffer.text(), "paed");
    buffer.move_to_start(true);
    assert_eq!(
        buffer.selection(),
        0..2,
        "the anchor stays where the caret was"
    );
    buffer.move_to_end(true);
    assert_eq!(buffer.selection(), 2..4);
}

#[test]
fn a_field_keeps_one_line_and_an_editor_keeps_its_line_breaks_and_both_obey_their_limit() {
    let mut field = TextBuffer::new();
    field.paste("one\ntwo\r\nthree");
    assert!(!field.text().contains(['\n', '\r']), "{:?}", field.text());
    assert!(field.text().starts_with("one") && field.text().ends_with("three"));
    field.set_text("line\none");
    assert!(!field.text().contains('\n'));

    let mut editor = TextBuffer::multiline();
    editor.paste("one\ntwo\tthree");
    assert_eq!(editor.text(), "one\ntwo\tthree");
    editor.move_to_line_start(false);
    assert_eq!(editor.caret(), 4, "the line starts after the break");

    let mut limited = TextBuffer::with_max_len(6);
    limited.insert(&format!("ab{FLAG}"));
    assert_eq!(
        limited.text(),
        "ab",
        "a flag would pass the limit and is cut whole"
    );
    assert_eq!(limited.cleaned("abcdefgh"), "abcdef");
}

#[test]
fn undo_takes_back_runs_of_typing_and_deleting_as_steps_and_redo_gives_them_again() {
    let mut buffer = TextBuffer::new();
    assert!(!buffer.undo(), "nothing to undo");
    for c in ["a", "b", "c"] {
        buffer.insert(c);
    }
    buffer.paste("XY");
    buffer.delete_backward();
    buffer.delete_backward();
    assert_eq!(buffer.text(), "abc");

    let mut states = Vec::new();
    while buffer.undo() {
        states.push(buffer.text().to_owned());
    }
    assert_eq!(
        states,
        ["abcXY", "abc", ""],
        "typing, a paste and deleting are a step each"
    );
    assert!(buffer.is_empty());

    assert!(buffer.redo());
    assert_eq!(buffer.text(), "abc");
    assert!(buffer.redo());
    assert_eq!(buffer.text(), "abcXY");
    buffer.move_to_end(false);
    buffer.insert("!");
    assert_eq!(buffer.text(), "abcXY!");
    assert!(!buffer.redo(), "an edit ends what could be redone");
}

#[test]
fn a_kill_keeps_what_it_took_for_a_yank_and_a_line_end_takes_the_break() {
    let mut buffer = TextBuffer::multiline();
    buffer.set_text("one two\nthree");
    buffer.select(4..4);
    buffer.kill_to_end();
    assert_eq!(buffer.text(), "one \nthree");
    buffer.kill_to_end();
    assert_eq!(
        buffer.text(),
        "one three",
        "at the end of a line the break goes"
    );
    buffer.move_to_end(false);
    buffer.yank();
    assert_eq!(
        buffer.text(),
        "one three\n",
        "the kill buffer holds the last kill"
    );

    let mut other = at("abc", 1);
    other.yank();
    assert_eq!(
        other.text(),
        "abc",
        "a buffer that killed nothing yanks nothing"
    );
}

#[test]
fn an_input_method_batch_lands_in_the_order_of_its_protocol() {
    let batch =
        |delete, commit: Option<&str>, preedit: Option<(&str, Option<std::ops::Range<usize>>)>| {
            ImeBatch {
                preedit: preedit.map(|(text, cursor)| Preedit {
                    text: text.into(),
                    cursor,
                }),
                commit: commit.map(str::to_owned),
                delete,
                current: true,
            }
        };
    let mut buffer = at("hello", 5);
    buffer.apply_ime(&batch((0, 0), None, Some(("に", Some(0..3)))));
    assert_eq!(buffer.text(), "helloに");
    assert_eq!(buffer.composition(), Some(5..8));
    assert_eq!(buffer.composition_cursor(), Some(5..8));
    assert_eq!(
        buffer.committed(),
        "hello",
        "the composition is not committed text"
    );
    assert_eq!(buffer.surrounding(100).text, "hello");
    assert_eq!(
        buffer.surrounding(100).cursor,
        5,
        "the caret is where the composition starts"
    );

    // the composition goes first, then the deletion around the caret, then the commit, then the new composition
    buffer.apply_ime(&batch((2, 0), Some("p!"), Some(("ほ", None))));
    assert_eq!(buffer.committed(), "help!");
    assert_eq!(buffer.text(), "help!ほ");
    assert_eq!(buffer.composition(), Some(5..8));
    assert_eq!(
        buffer.composition_cursor(),
        None,
        "the input method hid its cursor"
    );

    buffer.apply_ime(&batch((0, 0), Some("ほ"), None));
    assert_eq!((buffer.text(), buffer.composition()), ("help!ほ", None));
    assert_eq!(buffer.caret(), buffer.text().len());

    // the bytes the input method asks to delete are cut inward to whole characters: one byte of a 3-byte character
    // deletes none of it, and the character goes when all its bytes are asked for
    buffer.apply_ime(&batch((1, 0), None, None));
    assert_eq!(buffer.text(), "help!ほ");
    buffer.apply_ime(&batch((3, 0), None, None));
    assert_eq!(buffer.text(), "help!");
}

#[test]
fn the_text_around_a_caret_sent_to_the_input_method_is_bounded_and_cut_between_graphemes() {
    let text = format!("{}{FAMILY}{}", "ab".repeat(20), "cd".repeat(20));
    let caret = text.find(FAMILY).unwrap();
    for max_bytes in [4, 12, 30, 200] {
        for (cursor, anchor) in [
            (caret, caret),
            (caret, caret + FAMILY.len()),
            (0, 0),
            (text.len(), text.len()),
        ] {
            let around = Surrounding::around(&text, cursor, anchor, max_bytes);
            assert!(
                around.text.len() <= max_bytes.max(FAMILY.len()),
                "{max_bytes}: {}",
                around.text.len()
            );
            assert!(text.contains(&around.text));
            let start = text.find(&around.text).unwrap();
            assert!(
                text.grapheme_indices(true).any(|(i, _)| i == start) || start == text.len(),
                "starts between graphemes"
            );
            assert!(
                around.text.graphemes(true).collect::<String>() == around.text,
                "whole graphemes"
            );
            assert!(around.cursor <= around.text.len() && around.anchor <= around.text.len());
            if max_bytes == 200 {
                assert_eq!(around.text, text, "a text that fits is all there");
            }
        }
    }
}
