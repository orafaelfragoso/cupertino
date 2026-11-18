//! A password field's buffer edits by grapheme, shows one bullet for each and keeps its text out of what is printed.

use cupertino_ui_text::{ImeBatch, Preedit, SecureBuffer, Surrounding, TextError};

const FLAG: &str = "\u{1F1E9}\u{1F1EA}";
const FAMILY: &str = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
const E_ACUTE: &str = "e\u{301}";

fn secret(buffer: &SecureBuffer) -> String {
    buffer.with_secret(str::to_owned)
}

#[test]
fn the_buffer_edits_masks_and_moves_by_grapheme() {
    let mut buffer = SecureBuffer::new();
    buffer.insert(&format!("p{FLAG}{E_ACUTE}{FAMILY}\tz\n"));
    assert_eq!(
        secret(&buffer),
        format!("p{FLAG}{E_ACUTE}{FAMILY}z"),
        "control characters are dropped"
    );
    assert_eq!(buffer.grapheme_count(), 5);
    assert_eq!(buffer.mask('•'), "•••••", "one bullet for each grapheme");
    assert_eq!(buffer.caret(), buffer.with_secret(str::len));

    buffer.delete_backward();
    buffer.delete_backward();
    assert_eq!(
        secret(&buffer),
        format!("p{FLAG}{E_ACUTE}"),
        "a family is one grapheme"
    );
    buffer.delete_backward();
    assert_eq!(
        secret(&buffer),
        format!("p{FLAG}"),
        "an accent goes with its letter"
    );

    buffer.move_to_start(false);
    buffer.move_char(true, false);
    buffer.delete_forward();
    assert_eq!(secret(&buffer), "p");

    buffer.insert(&format!("{E_ACUTE}{FLAG}x"));
    buffer.select_masked(1, 3);
    assert_eq!(
        buffer.masked_selection(),
        (3, 1..3),
        "the selection is where the bullets are"
    );
    buffer.insert("-");
    assert_eq!(secret(&buffer), "p-x", "typing replaces the selection");

    buffer.select_all();
    buffer.move_char(false, false);
    assert_eq!(
        buffer.selection(),
        0..0,
        "moving collapses a selection to its start"
    );
    buffer.select_masked(0, 99);
    assert_eq!(
        buffer.masked_selection().1,
        0..3,
        "a bullet past the end is the end"
    );

    assert_eq!(buffer.set("a\nb"), Err(TextError::InvalidText));
    assert_eq!(secret(&buffer), "p-x", "a refused text changes nothing");
    buffer.set("new").unwrap();
    assert_eq!((secret(&buffer), buffer.caret()), ("new".into(), 3));
    buffer.clear();
    assert!(buffer.is_empty());
}

#[test]
fn a_length_limit_cuts_an_insertion_at_a_grapheme_and_leaves_text_already_past_it() {
    let mut buffer = SecureBuffer::with_max_len(10);
    buffer.insert(&format!("ab{FLAG}{FLAG}"));
    assert_eq!(
        secret(&buffer),
        format!("ab{FLAG}"),
        "the second flag would pass 10 bytes"
    );
    buffer.insert("c");
    buffer.insert("d");
    assert_eq!(secret(&buffer), format!("ab{FLAG}"), "no room is left");
    buffer.select_all();
    buffer.insert(&format!("{E_ACUTE}{E_ACUTE}{E_ACUTE}{E_ACUTE}"));
    assert_eq!(
        secret(&buffer),
        format!("{E_ACUTE}{E_ACUTE}{E_ACUTE}"),
        "the selection's room is reused"
    );

    buffer.set_max_len(Some(2));
    assert_eq!(buffer.grapheme_count(), 3, "text past a lower limit stays");
    buffer.insert("x");
    assert_eq!(buffer.grapheme_count(), 3);
    buffer.set_max_len(None);
    buffer.insert("x");
    assert_eq!(buffer.grapheme_count(), 4);
}

#[test]
fn nothing_printed_for_a_buffer_or_the_input_method_shows_the_text() {
    let mut buffer = SecureBuffer::new();
    buffer.insert("hunter2");
    let preedit = Preedit {
        text: "hunter2".into(),
        cursor: Some(0..7),
    };
    let batch = ImeBatch {
        preedit: Some(preedit.clone()),
        commit: Some("hunter2".into()),
        delete: (1, 0),
        current: true,
    };
    let surrounding = Surrounding {
        text: "hunter2".into(),
        cursor: 3,
        anchor: 3,
    };
    for (what, printed) in [
        ("buffer", format!("{buffer:?} {buffer:#?}")),
        ("preedit", format!("{preedit:?} {preedit:#?}")),
        ("batch", format!("{batch:?} {batch:#?}")),
        ("surrounding", format!("{surrounding:?} {surrounding:#?}")),
    ] {
        assert!(!printed.contains("hunter"), "{what}: {printed}");
    }
}
