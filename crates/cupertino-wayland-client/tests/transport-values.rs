//! The transport's value types: scale, buffer layout, outputs, window states, input and text input.

use cupertino_wayland_client::{
    BufferFormat, BufferLayout, Button, OutputGeometry, OutputId, OutputInfo, OutputTransform,
    Preedit, Scale, SurroundingText, TextInputDone, WindowStates, MAX_SURROUNDING,
};

fn scale(v: u32) -> Scale {
    Scale::from_120ths(v).unwrap()
}

#[test]
fn a_scale_is_a_positive_number_of_120ths() {
    for (row, got, expected) in [
        ("1", Scale::from_integer(1), Some(Scale::ONE)),
        ("2", Scale::from_integer(2), Scale::from_120ths(240)),
        ("0", Scale::from_integer(0), None),
        ("-1", Scale::from_integer(-1), None),
        ("i32::MIN", Scale::from_integer(i32::MIN), None),
        ("i32::MAX", Scale::from_integer(i32::MAX), None),
        (
            "the largest",
            Scale::from_integer(35_791_394),
            Scale::from_120ths(35_791_394 * 120),
        ),
        (
            "one over the largest",
            Scale::from_integer(35_791_395),
            None,
        ),
        ("0/120", Scale::from_120ths(0), None),
    ] {
        assert_eq!(got, expected, "{row}");
    }
}

#[test]
fn a_scale_has_a_factor_an_integer_only_when_whole_a_ceiling_and_an_order() {
    for (v, factor, integer, ceil) in [
        (120, 1.0, Some(1), 120),
        (360, 3.0, Some(3), 360),
        (180, 1.5, None, 240),
        (150, 1.25, None, 240),
        (121, 121.0 / 120.0, None, 240),
        (240, 2.0, Some(2), 240),
        (1, 1.0 / 120.0, None, 120),
    ] {
        let s = scale(v);
        assert_eq!(s.factor(), factor, "{v}/120");
        assert_eq!(s.integer(), integer, "{v}/120");
        assert_eq!(s.ceil(), scale(ceil), "{v}/120");
    }
    assert!(scale(150) > Scale::ONE);
    assert!(scale(150) < scale(240));
}

#[test]
fn a_logical_extent_rounds_to_the_nearest_pixel_with_halves_up_or_is_none_if_it_does_not_fit() {
    for (v, logical, physical) in [
        (180, 3, Some(5)),
        (180, 1, Some(2)),
        (180, 2, Some(3)),
        (150, 10, Some(13)),
        (150, 9, Some(11)),
        (120, 0, Some(0)),
        (240, 100, Some(200)),
        // 100 * 1/120 is 0.83: rounds up to one pixel
        (1, 100, Some(1)),
        (1, 59, Some(0)),
        (1, 60, Some(1)),
        (240, u32::MAX, None),
        (240, u32::MAX / 2, Some(u32::MAX - 1)),
        (120, u32::MAX, Some(u32::MAX)),
    ] {
        assert_eq!(
            scale(v).to_physical(logical),
            physical,
            "{logical} at {v}/120"
        );
    }
}

#[test]
fn a_buffer_layout_holds_extents_of_one_to_the_maximum() {
    let layout = BufferLayout::new(3, 2, BufferFormat::Argb8888).unwrap();
    assert_eq!((layout.width(), layout.height()), (3, 2));
    assert_eq!(layout.format(), BufferFormat::Argb8888);
    assert_eq!(layout.stride(), 12);
    assert_eq!(layout.buffer_len(), 24);

    let max = BufferLayout::MAX_EXTENT;
    for (width, height, held) in [
        (0, 1, false),
        (1, 0, false),
        (max, 1, true),
        (1, max, true),
        (max + 1, 1, false),
        (1, max + 1, false),
        (u32::MAX, u32::MAX, false),
    ] {
        assert_eq!(
            BufferLayout::new(width, height, BufferFormat::Xrgb8888).is_some(),
            held,
            "{width}x{height}"
        );
    }

    let largest = BufferLayout::new(max, max, BufferFormat::Argb8888).unwrap();
    assert!(
        largest.buffer_len() <= i32::MAX as usize,
        "the largest layout's length fits a wire size"
    );
}

#[test]
fn the_effective_scale_is_the_mode_over_the_logical_size_with_quarter_turns_swapping_sides() {
    use OutputTransform::*;
    let output = |transform, mode, geometry: Option<(u32, u32)>| OutputInfo {
        id: OutputId::from_registry_name(7),
        name: None,
        description: None,
        make: None,
        model: None,
        physical_size_mm: None,
        transform,
        scale: Scale::ONE,
        mode,
        refresh_mhz: None,
        geometry: geometry.map(|(width, height)| OutputGeometry {
            x: 0,
            y: 0,
            width,
            height,
        }),
    };
    let uhd = Some((3840, 2160));
    for (transform, mode, geometry, effective) in [
        (Normal, uhd, Some((2560, 1440)), Some(1.5)),
        (Normal, uhd, Some((1920, 1080)), Some(2.0)),
        (Rotated90, uhd, Some((1080, 1920)), Some(2.0)),
        (Flipped270, uhd, Some((1080, 1920)), Some(2.0)),
        (Rotated180, uhd, Some((1920, 1080)), Some(2.0)),
        (Normal, None, Some((1, 1)), None),
        (Normal, Some((1, 1)), None, None),
    ] {
        assert_eq!(
            output(transform, mode, geometry).effective_scale(),
            effective,
            "{transform:?} {mode:?} {geometry:?}"
        );
    }
    let swapping: Vec<_> = [
        Normal, Rotated90, Rotated180, Rotated270, Flipped, Flipped90, Flipped180, Flipped270,
    ]
    .into_iter()
    .filter(|t| t.swaps_sides())
    .collect();
    assert_eq!(swapping, [Rotated90, Rotated270, Flipped90, Flipped270]);
}

#[test]
fn only_a_maximized_fullscreen_resizing_or_tiled_window_has_a_fixed_size() {
    let d = WindowStates::default;
    for (row, states, fixed) in [
        ("floating", d(), false),
        (
            "activated and suspended",
            WindowStates {
                activated: true,
                suspended: true,
                ..d()
            },
            false,
        ),
        (
            "maximized",
            WindowStates {
                maximized: true,
                ..d()
            },
            true,
        ),
        (
            "fullscreen",
            WindowStates {
                fullscreen: true,
                ..d()
            },
            true,
        ),
        (
            "resizing",
            WindowStates {
                resizing: true,
                ..d()
            },
            true,
        ),
        (
            "tiled left",
            WindowStates {
                tiled_left: true,
                ..d()
            },
            true,
        ),
        (
            "tiled right",
            WindowStates {
                tiled_right: true,
                ..d()
            },
            true,
        ),
        (
            "tiled top",
            WindowStates {
                tiled_top: true,
                ..d()
            },
            true,
        ),
        (
            "tiled bottom",
            WindowStates {
                tiled_bottom: true,
                ..d()
            },
            true,
        ),
    ] {
        assert_eq!(states.size_is_fixed(), fixed, "{row}");
    }
}

#[test]
fn evdev_codes_name_the_three_buttons() {
    for (code, button) in [
        (0x110, Button::Left),
        (0x111, Button::Right),
        (0x112, Button::Middle),
        (0x113, Button::Other(0x113)),
        (0, Button::Other(0)),
    ] {
        assert_eq!(Button::from_evdev(code), button, "{code:#x}");
    }
}

#[test]
fn surrounding_text_keeps_a_short_text_whole_and_refuses_offsets_inside_a_character_or_past_the_end(
) {
    let s = SurroundingText::new("héllo", 3, 3).unwrap();
    assert_eq!(s.text(), "héllo");
    assert_eq!((s.cursor(), s.anchor()), (3, 3));
    let s = SurroundingText::new("hello world", 5, 0).unwrap();
    assert_eq!((s.cursor(), s.anchor()), (5, 0), "a selection");

    // "é" is two bytes
    for (text, cursor, anchor, held) in [
        ("é", 1, 1, false),
        ("é", 0, 1, false),
        ("é", 2, 3, false),
        ("ab", 3, 3, false),
        ("", 0, 0, true),
    ] {
        assert_eq!(
            SurroundingText::new(text, cursor, anchor).is_some(),
            held,
            "{text:?} {cursor} {anchor}"
        );
    }
}

#[test]
fn a_long_surrounding_text_is_cut_around_the_cursor_at_character_boundaries() {
    let text = "ß".repeat(MAX_SURROUNDING);
    let cursor = text.len() / 2;
    let s = SurroundingText::new(&text, cursor, cursor).unwrap();
    assert!(s.text().len() <= MAX_SURROUNDING);
    assert!(s.text().len() > MAX_SURROUNDING - 2);
    assert!(s.text().is_char_boundary(s.cursor()));
    assert_eq!(s.cursor(), s.anchor());
    // the cursor sits where it did in the text: the same character follows it
    assert!(s.text()[s.cursor()..].starts_with('ß'));
    assert!(s.cursor() > 1000 && s.cursor() < MAX_SURROUNDING - 1000);

    let text = "a".repeat(3 * MAX_SURROUNDING);
    let end = text.len();
    for (cursor, anchor) in [
        (0, 0),
        (1, 1),
        (MAX_SURROUNDING, MAX_SURROUNDING),
        (2 * MAX_SURROUNDING, 2 * MAX_SURROUNDING),
        (end - 1, end - 1),
        (end, end),
        (7000, 5000),
        (9000, 100),
    ] {
        let s = SurroundingText::new(&text, cursor, anchor).unwrap();
        let row = format!("cursor {cursor}, anchor {anchor}");
        assert!(s.text().len() <= MAX_SURROUNDING, "{row}");
        assert!(s.cursor() <= s.text().len(), "{row}");
        assert!(s.anchor() <= s.text().len(), "{row}");
    }
    assert_eq!(SurroundingText::new(&text, 0, 0).unwrap().cursor(), 0);
    let s = SurroundingText::new(&text, end, end).unwrap();
    assert_eq!(
        s.cursor(),
        s.text().len(),
        "a cursor at the end stays there"
    );
    let s = SurroundingText::new(&text, 7000, 5000).unwrap();
    assert_eq!(
        s.cursor() - s.anchor(),
        2000,
        "a selection that fits stays whole"
    );
}

#[test]
fn typed_text_never_appears_in_a_debug_print() {
    let s = SurroundingText::new("secret", 0, 0).unwrap();
    assert!(!format!("{s:?}").contains("secret"));
    assert!(format!("{s:?}").contains("<6 bytes>"));
    let preedit = Preedit {
        text: "secret".into(),
        cursor: None,
    };
    assert!(!format!("{preedit:?}").contains("secret"));
    let done = TextInputDone {
        preedit: Some(preedit),
        commit: Some("secret".into()),
        delete: (0, 0),
        current: true,
    };
    let printed = format!("{done:?}");
    assert!(!printed.contains("secret"), "{printed}");
}
