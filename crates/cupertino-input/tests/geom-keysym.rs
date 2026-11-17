//! Geometry helpers, modifier sets and the key-name table that shortcut maps are written in.

use cupertino_input::geom::DRAG_START_THRESHOLD;
use cupertino_input::keysym::{self, ESCAPE};
use cupertino_input::{drag_started, Modifiers, Point, Rect};

#[test]
fn a_rect_contains_its_top_left_but_not_its_far_edges_and_inflates_on_every_side() {
    let r = Rect::new(10.0, 20.0, 100.0, 50.0);
    for (point, inside) in [
        ((10.0, 20.0), true),
        ((109.9, 69.9), true),
        ((110.0, 30.0), false),
        ((50.0, 70.0), false),
        ((9.9, 30.0), false),
    ] {
        assert_eq!(
            r.contains(Point::new(point.0, point.1)),
            inside,
            "{point:?}"
        );
    }
    assert_eq!((r.right(), r.bottom()), (110.0, 70.0));
    assert_eq!(r.inflate(5.0), Rect::new(5.0, 15.0, 110.0, 60.0));
    assert_eq!(
        Rect::new(0.0, 0.0, 10.0, 10.0).inflate(-2.0),
        Rect::new(2.0, 2.0, 6.0, 6.0)
    );
}

#[test]
fn a_press_becomes_a_drag_past_the_threshold_in_either_axis() {
    let press = Point::new(100.0, 100.0);
    for (name, to, drag) in [
        ("no motion", press, false),
        (
            "at the threshold",
            Point::new(100.0 + DRAG_START_THRESHOLD, 100.0),
            false,
        ),
        (
            "past it across",
            Point::new(100.0 + DRAG_START_THRESHOLD + 0.1, 100.0),
            true,
        ),
        (
            "past it up",
            Point::new(100.0, 100.0 - DRAG_START_THRESHOLD - 0.1),
            true,
        ),
        ("the distance is per axis", Point::new(103.0, 103.0), false),
    ] {
        assert_eq!(drag_started(press, to), drag, "{name}");
    }
}

#[test]
fn modifier_sets_contain_what_they_require() {
    let ctrl_shift = Modifiers::new(true, true, false, false);
    assert!(Modifiers::NONE.is_empty() && !ctrl_shift.is_empty());
    for (set, required, contains) in [
        (ctrl_shift, Modifiers::NONE, true),
        (ctrl_shift, Modifiers::new(false, true, false, false), true),
        (ctrl_shift, Modifiers::new(false, false, true, false), false),
        (Modifiers::NONE, ctrl_shift, false),
        (ctrl_shift, ctrl_shift, true),
    ] {
        assert_eq!(set.contains(required), contains, "{set:?} {required:?}");
    }
}

#[test]
fn names_resolve_to_keysyms() {
    for (name, sym) in [
        ("Escape", Some(0xff1b)),
        ("Tab", Some(0xff09)),
        ("space", Some(0x20)),
        ("Left", Some(0xff51)),
        ("Page_Up", Some(0xff55)),
        ("grave", Some(0x60)),
        ("XF86AudioMute", Some(0x1008ff12)),
        ("a", Some(0x61)),
        ("A", Some(0x41)),
        ("3", Some(0x33)),
        ("/", Some(0x2f)),
        ("F1", Some(0xffbe)),
        ("F12", Some(0xffc9)),
        ("F35", Some(0xffbd + 35)),
        ("F0", None),
        ("F36", None),
        ("KP_0", Some(0xffb0)),
        ("KP_9", Some(0xffb9)),
        ("KP_10", None),
        ("0xff1b", Some(ESCAPE)),
        ("0xzz", None),
        ("U00e9", Some(0xe9)),
        ("U20ac", Some(0x0100_20ac)),
        ("U110000", None),
        ("Ud800", None),
        ("U12", None),
        ("", None),
        ("NoSuchKey", None),
        ("é", None),
    ] {
        assert_eq!(keysym::from_name(name), sym, "{name}");
    }
}

#[test]
fn keysyms_have_names_that_resolve_back_to_them() {
    for (sym, name) in [
        (0xff1b, Some("Escape")),
        (0x1008ff12, Some("XF86AudioMute")),
        (0x61, Some("a")),
        (0x31, Some("1")),
        (0xffbe, Some("F1")),
        (0xffb5, Some("KP_5")),
        (0x7, None),
    ] {
        assert_eq!(keysym::name_of(sym).as_deref(), name, "{sym:#x}");
    }
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
    for (sym, folded) in [
        (u32::from(b'Q'), u32::from(b'q')),
        (u32::from(b'q'), u32::from(b'q')),
        (u32::from(b'@'), u32::from(b'@')),
        (ESCAPE, ESCAPE),
    ] {
        assert_eq!(keysym::fold_case(sym), folded, "{sym:#x}");
    }
}
