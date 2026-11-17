//! Geometry helpers, modifier sets and the key-name table that shortcut maps are written in.

use cupertino_input::geom::DRAG_START_THRESHOLD;
use cupertino_input::keysym::{self, ESCAPE, ISO_LEFT_TAB, TAB};
use cupertino_input::{drag_started, Modifiers, Point, Rect};

#[test]
fn a_rect_contains_its_top_left_but_not_its_far_edges() {
    let r = Rect::new(10.0, 20.0, 100.0, 50.0);
    assert!(r.contains(Point::new(10.0, 20.0)));
    assert!(r.contains(Point::new(109.9, 69.9)));
    assert!(!r.contains(Point::new(110.0, 30.0)));
    assert!(!r.contains(Point::new(50.0, 70.0)));
    assert!(!r.contains(Point::new(9.9, 30.0)));
    assert_eq!((r.right(), r.bottom()), (110.0, 70.0));
}

#[test]
fn inflating_grows_every_side() {
    let r = Rect::new(10.0, 20.0, 100.0, 50.0).inflate(5.0);
    assert_eq!(r, Rect::new(5.0, 15.0, 110.0, 60.0));
    assert_eq!(
        Rect::new(0.0, 0.0, 10.0, 10.0).inflate(-2.0),
        Rect::new(2.0, 2.0, 6.0, 6.0)
    );
}

#[test]
fn a_press_becomes_a_drag_past_the_threshold_in_either_axis() {
    let press = Point::new(100.0, 100.0);
    assert!(!drag_started(press, press));
    assert!(!drag_started(
        press,
        Point::new(100.0 + DRAG_START_THRESHOLD, 100.0)
    ));
    assert!(drag_started(
        press,
        Point::new(100.0 + DRAG_START_THRESHOLD + 0.1, 100.0)
    ));
    assert!(drag_started(
        press,
        Point::new(100.0, 100.0 - DRAG_START_THRESHOLD - 0.1)
    ));
    assert!(
        !drag_started(press, Point::new(103.0, 103.0)),
        "the distance is per axis"
    );
}

#[test]
fn modifier_sets_contain_what_they_require() {
    assert!(Modifiers::NONE.is_empty());
    let ctrl_shift = Modifiers::new(true, true, false, false);
    assert!(!ctrl_shift.is_empty());
    assert!(ctrl_shift.contains(Modifiers::NONE));
    assert!(ctrl_shift.contains(Modifiers::new(false, true, false, false)));
    assert!(!ctrl_shift.contains(Modifiers::new(false, false, true, false)));
    assert!(!Modifiers::NONE.contains(ctrl_shift));
    assert!(ctrl_shift.contains(ctrl_shift));
}

#[test]
fn names_resolve_to_keysyms_and_back() {
    for (name, sym) in [
        ("Escape", 0xff1b),
        ("Tab", 0xff09),
        ("space", 0x20),
        ("Left", 0xff51),
        ("Page_Up", 0xff55),
        ("grave", 0x60),
        ("XF86AudioMute", 0x1008ff12),
    ] {
        assert_eq!(keysym::from_name(name), Some(sym), "{name}");
        assert_eq!(keysym::name_of(sym).as_deref(), Some(name), "{sym:#x}");
    }
    assert_eq!(keysym::ESCAPE, ESCAPE);
}

#[test]
fn single_characters_are_their_own_keysyms() {
    assert_eq!(keysym::from_name("a"), Some(0x61));
    assert_eq!(keysym::from_name("A"), Some(0x41));
    assert_eq!(keysym::from_name("3"), Some(0x33));
    assert_eq!(keysym::from_name("/"), Some(0x2f));
    assert_eq!(keysym::name_of(0x61).as_deref(), Some("a"));
    assert_eq!(keysym::name_of(0x31).as_deref(), Some("1"));
}

#[test]
fn function_and_keypad_keys_are_numbered() {
    assert_eq!(keysym::from_name("F1"), Some(0xffbe));
    assert_eq!(keysym::from_name("F12"), Some(0xffc9));
    assert_eq!(keysym::from_name("F35"), Some(0xffbd + 35));
    assert_eq!(keysym::from_name("F0"), None);
    assert_eq!(keysym::from_name("F36"), None);
    assert_eq!(keysym::from_name("KP_0"), Some(0xffb0));
    assert_eq!(keysym::from_name("KP_9"), Some(0xffb9));
    assert_eq!(keysym::from_name("KP_10"), None);
    assert_eq!(keysym::name_of(0xffbe).as_deref(), Some("F1"));
    assert_eq!(keysym::name_of(0xffb5).as_deref(), Some("KP_5"));
}

#[test]
fn hexadecimal_and_unicode_names_are_accepted() {
    assert_eq!(keysym::from_name("0xff1b"), Some(ESCAPE));
    assert_eq!(keysym::from_name("0xzz"), None);
    assert_eq!(keysym::from_name("U00e9"), Some(0xe9));
    assert_eq!(keysym::from_name("U20ac"), Some(0x0100_20ac));
    assert_eq!(
        keysym::from_name("U110000"),
        None,
        "past the last code point"
    );
    assert_eq!(
        keysym::from_name("Ud800"),
        None,
        "a surrogate is no character"
    );
    assert_eq!(keysym::from_name("U12"), None, "at least four digits");
}

#[test]
fn unknown_names_and_unprintable_single_characters_are_refused() {
    assert_eq!(keysym::from_name(""), None);
    assert_eq!(keysym::from_name("NoSuchKey"), None);
    assert_eq!(keysym::from_name("é"), None);
    assert_eq!(keysym::name_of(0x7), None);
}

#[test]
fn every_listed_name_resolves_to_a_keysym_that_names_it_or_an_alias() {
    let mut count = 0;
    for name in keysym::names() {
        count += 1;
        let sym = keysym::from_name(name).unwrap_or_else(|| panic!("{name} resolves"));
        let back = keysym::name_of(sym).unwrap_or_else(|| panic!("{sym:#x} has a name"));
        assert_eq!(keysym::from_name(&back), Some(sym), "{name} via {back}");
    }
    assert!(count > 50);
}

#[test]
fn letters_fold_to_lower_case_and_other_symbols_stay() {
    assert_eq!(keysym::fold_case(u32::from(b'Q')), u32::from(b'q'));
    assert_eq!(keysym::fold_case(u32::from(b'q')), u32::from(b'q'));
    assert_eq!(keysym::fold_case(u32::from(b'@')), u32::from(b'@'));
    assert_eq!(keysym::fold_case(ESCAPE), ESCAPE);
    assert_ne!(ISO_LEFT_TAB, TAB);
}
