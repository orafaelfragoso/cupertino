//! Layout parsing and translation with dead keys, the key-number table and the keymap generator.

use cupertino_keyboard::keycodes::{
    evdev_of, is_character_key, layout_key_of, xkb_name_of, LAYOUT_KEY_TABLE,
};
use cupertino_keyboard::xkb::GenError;
use cupertino_keyboard::{generate, GenOptions, Keyboard, Layout, Level3Keys};

/// Keys 0..=3: `e`, a dead acute, a dead grave that composes with `e`, and `x`; modifier bit 2
/// (shift) selects the second table.
const LAYOUT: &str = r#"{
    "name": "Dead",
    "id": 3,
    "language": "xx",
    "sets": [{
        "first": 0,
        "last": 0,
        "modmap": [0, 0, 1],
        "default_table": 0,
        "tables": [
            [{"c": "e"}, {"dead": 0}, {"dead": 1}, {"c": "x"}],
            [{"c": "E"}, {"dead": 0}, {"dead": 1}, {"c": "X"}]
        ],
        "records": [
            {"next": 1, "entries": [{"state": 1, "out": {"c": "é"}, "next": 0}]},
            {"zero": {"c": "`"}, "next": 0}
        ],
        "terminators": [{"c": "´"}]
    }]
}"#;

fn layout() -> Layout {
    Layout::from_json(LAYOUT).expect("layout parses")
}

#[test]
fn layout_text_parses_and_malformed_text_is_an_error() {
    let l = layout();
    assert_eq!(
        (l.name.as_str(), l.id, l.language.as_str()),
        ("Dead", 3, "xx")
    );
    assert_eq!(l.sets.len(), 1);
    let without_language = Layout::from_json(r#"{"name": "T", "id": 1, "sets": []}"#).unwrap();
    assert_eq!(without_language.language, "");
    for bad in ["{\"name\": \"Test\"}", "not json"] {
        assert!(Layout::from_json(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn keys_translate_through_tables_and_dead_key_states() {
    let l = layout();
    let set = &l.sets[0];
    for ((key, mods, state), (text, next), row) in [
        ((0, 0, 0), ("e", 0), "plain key"),
        ((0, 2, 0), ("E", 0), "shift selects the second table"),
        ((1, 0, 0), ("", 1), "a dead key enters a state without text"),
        (
            (1, 0, 1),
            ("é", 0),
            "a dead key composes with the state entry",
        ),
        (
            (3, 0, 1),
            ("´x", 0),
            "no composition: the terminator, then the key",
        ),
        (
            (2, 0, 1),
            ("´`", 0),
            "a dead key without an entry: the terminator and its spacing form",
        ),
        (
            (99, 0, 1),
            ("", 1),
            "a key outside the table keeps the state",
        ),
    ] {
        let t = set.translate(key, mods, state);
        assert_eq!((t.text.as_str(), t.next_state), (text, next), "{row}");
    }
}

#[test]
fn layout_key_numbers_map_to_evdev_codes_and_xkb_names() {
    for (key, evdev, name) in [
        (0x00, Some(30), Some("AC01")),
        (0x24, Some(28), Some("RTRN")),
        (0x31, Some(57), Some("SPCE")),
        (0xff, None, None),
    ] {
        assert_eq!(evdev_of(key), evdev, "evdev of {key:#x}");
        assert_eq!(xkb_name_of(key), name, "xkb name of {key:#x}");
        if let Some(code) = evdev {
            assert_eq!(layout_key_of(code), Some(key), "inverse of {key:#x}");
        }
    }
    assert_eq!(layout_key_of(9999), None);
    let mut keys: Vec<u8> = LAYOUT_KEY_TABLE.iter().map(|(k, _, _)| *k).collect();
    let mut codes: Vec<u32> = LAYOUT_KEY_TABLE.iter().map(|(_, e, _)| *e).collect();
    keys.sort_unstable();
    codes.sort_unstable();
    keys.dedup();
    codes.dedup();
    assert_eq!(keys.len(), LAYOUT_KEY_TABLE.len(), "layout keys are unique");
    assert_eq!(
        codes.len(),
        LAYOUT_KEY_TABLE.len(),
        "evdev codes are unique"
    );
}

#[test]
fn character_keys_exclude_return_tab_and_everything_past_the_block() {
    for (key, character) in [
        (0x00, true),
        (0x31, true),
        (0x32, true),
        (0x24, false),
        (0x30, false),
        (0x33, false),
        (0x5d, true),
        (0x5f, true),
        (0x60, false),
    ] {
        assert_eq!(is_character_key(key), character, "key {key:#x}");
    }
}

#[test]
fn every_keyboard_family_and_level_three_choice_generates_symbols_keymap_and_compose() {
    for keyboard in [Keyboard::Ansi, Keyboard::Iso, Keyboard::Jis] {
        for level3 in [Level3Keys::BothAlt, Level3Keys::RightAlt, Level3Keys::None] {
            let opts = GenOptions {
                keyboard,
                set_index: 0,
                level3,
            };
            let out = generate(&layout(), &opts).expect("generates");
            let row = format!("{keyboard:?} {level3:?}");
            assert!(out.symbols.contains("xkb_symbols"), "{row}");
            assert!(out.symbols.contains("dead_acute"), "{row}");
            assert!(out.keymap.contains("xkb_keymap"), "{row}");
            assert!(!out.compose.is_empty(), "{row}");
            assert!(
                out.dead_keysyms.iter().any(|(_, n)| n == "dead_acute"),
                "{row}"
            );
        }
    }
}

#[test]
fn the_generator_rejects_a_missing_key_set() {
    let opts = GenOptions {
        set_index: 4,
        ..GenOptions::default()
    };
    assert!(matches!(
        generate(&layout(), &opts),
        Err(GenError::NoSuchSet(4))
    ));
}
