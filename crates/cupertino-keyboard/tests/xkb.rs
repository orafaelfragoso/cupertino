//! Layout translation with dead keys, the key-number table and the keymap generator.

use cupertino_keyboard::keycodes::{
    evdev_of, is_character_key, layout_key_of, xkb_name_of, LAYOUT_KEY_TABLE,
};
use cupertino_keyboard::xkb::GenError;
use cupertino_keyboard::{generate, GenOptions, Keyboard, Layout, Level3Keys};

/// Keys 0..=3: `e`, a dead acute, a dead grave that composes with `e`, and `x`; modifier bit 2 (shift)
/// selects the second table.
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
fn a_layout_carries_its_language() {
    assert_eq!(layout().language, "xx");
}

#[test]
fn shift_selects_the_second_table() {
    let set = &layout().sets[0];
    assert_eq!(set.translate(0, 0, 0).text, "e");
    assert_eq!(set.translate(0, 2, 0).text, "E");
}

#[test]
fn a_dead_key_enters_a_state_without_text() {
    let t = layout().sets[0].translate(1, 0, 0);
    assert_eq!(t.text, "");
    assert_eq!(t.next_state, 1);
}

#[test]
fn a_dead_key_composes_with_the_following_state_entry() {
    let set = &layout().sets[0];
    let t = set.translate(1, 0, 1);
    assert_eq!(t.text, "é");
    assert_eq!(t.next_state, 0);
}

#[test]
fn a_dead_state_that_does_not_compose_prints_the_terminator_then_the_key() {
    let set = &layout().sets[0];
    let t = set.translate(3, 0, 1);
    assert_eq!(t.text, "´x");
    assert_eq!(t.next_state, 0);
}

#[test]
fn a_dead_key_without_a_matching_entry_prints_the_terminator_and_its_own_spacing_form() {
    let set = &layout().sets[0];
    let t = set.translate(2, 0, 1);
    assert_eq!(t.text, "´`");
}

#[test]
fn a_key_outside_the_table_produces_nothing_and_keeps_the_state() {
    let set = &layout().sets[0];
    let t = set.translate(99, 0, 1);
    assert_eq!(t.text, "");
    assert_eq!(t.next_state, 1);
}

#[test]
fn layout_key_numbers_map_to_evdev_codes_and_back() {
    assert_eq!(evdev_of(0x00), Some(30));
    assert_eq!(layout_key_of(30), Some(0x00));
    assert_eq!(evdev_of(0x31), Some(57));
    assert_eq!(evdev_of(0xff), None);
    assert_eq!(layout_key_of(9999), None);
}

#[test]
fn layout_key_numbers_map_to_xkb_names() {
    assert_eq!(xkb_name_of(0x00), Some("AC01"));
    assert_eq!(xkb_name_of(0x24), Some("RTRN"));
    assert_eq!(xkb_name_of(0xff), None);
}

#[test]
fn the_key_table_has_no_duplicate_keys_or_codes() {
    let mut keys: Vec<u8> = LAYOUT_KEY_TABLE.iter().map(|(k, _, _)| *k).collect();
    let mut codes: Vec<u32> = LAYOUT_KEY_TABLE.iter().map(|(_, e, _)| *e).collect();
    keys.sort_unstable();
    codes.sort_unstable();
    keys.dedup();
    codes.dedup();
    assert_eq!(keys.len(), LAYOUT_KEY_TABLE.len());
    assert_eq!(codes.len(), LAYOUT_KEY_TABLE.len());
}

#[test]
fn character_keys_exclude_return_tab_and_everything_past_the_block() {
    assert!(is_character_key(0x00));
    assert!(is_character_key(0x31));
    assert!(is_character_key(0x32));
    assert!(!is_character_key(0x24));
    assert!(!is_character_key(0x30));
    assert!(!is_character_key(0x33));
    assert!(is_character_key(0x5d));
    assert!(is_character_key(0x5f));
    assert!(!is_character_key(0x60));
}

#[test]
fn the_generator_produces_symbols_a_keymap_and_compose_text() {
    let out = generate(&layout(), &GenOptions::default()).expect("generates");
    assert!(out.symbols.contains("xkb_symbols"));
    assert!(out.keymap.contains("xkb_keymap"));
    assert!(out.symbols.contains("dead_acute"));
    assert!(!out.compose.is_empty());
    assert!(out
        .dead_keysyms
        .iter()
        .any(|(_, name)| name == "dead_acute"));
}

#[test]
fn the_generator_rejects_a_missing_key_set() {
    let opts = GenOptions {
        set_index: 4,
        ..GenOptions::default()
    };
    let err = generate(&layout(), &opts).unwrap_err();
    assert!(matches!(err, GenError::NoSuchSet(4)));
    assert_eq!(err.to_string(), "layout has no key set 4");
}

#[test]
fn every_keyboard_family_and_level_three_choice_generates() {
    for keyboard in [Keyboard::Ansi, Keyboard::Iso, Keyboard::Jis] {
        for level3 in [Level3Keys::BothAlt, Level3Keys::RightAlt, Level3Keys::None] {
            let opts = GenOptions {
                keyboard,
                set_index: 0,
                level3,
            };
            assert!(
                generate(&layout(), &opts).is_ok(),
                "{keyboard:?} {level3:?}"
            );
        }
    }
}
