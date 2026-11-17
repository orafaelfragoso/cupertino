//! The transport's value types: scale, buffer layout, outputs, window states, capabilities, input, text input and
//! errors.

use cupertino_wayland_client::{
    key, BufferFormat, BufferLayout, Button, Capability, CapabilitySet, Decorations, Error,
    Modifiers, OutputGeometry, OutputId, OutputInfo, OutputTransform, PixelRect, Preedit, Scale,
    SurroundingText, TextInputDone, WindowStates, MAX_SURROUNDING,
};

// scale

#[test]
fn an_integer_scale_is_a_multiple_of_120() {
    assert_eq!(Scale::from_integer(1), Some(Scale::ONE));
    assert_eq!(Scale::from_integer(2), Scale::from_120ths(240));
    assert_eq!(Scale::from_integer(3).unwrap().factor(), 3.0);
}

#[test]
fn a_scale_of_zero_or_less_does_not_exist() {
    assert_eq!(Scale::from_integer(0), None);
    assert_eq!(Scale::from_integer(-1), None);
    assert_eq!(Scale::from_integer(i32::MIN), None);
    assert_eq!(Scale::from_120ths(0), None);
}

#[test]
fn an_integer_scale_that_overflows_does_not_exist() {
    assert_eq!(Scale::from_integer(i32::MAX), None);
    assert!(Scale::from_integer(35_791_394).is_some());
    assert_eq!(Scale::from_integer(35_791_395), None);
}

#[test]
fn the_factor_is_the_value_over_120() {
    assert_eq!(Scale::ONE.factor(), 1.0);
    assert_eq!(Scale::from_120ths(180).unwrap().factor(), 1.5);
    assert_eq!(Scale::from_120ths(150).unwrap().factor(), 1.25);
    assert_eq!(Scale::from_120ths(1).unwrap().factor(), 1.0 / 120.0);
}

#[test]
fn only_a_whole_scale_has_an_integer_factor() {
    assert_eq!(Scale::ONE.integer(), Some(1));
    assert_eq!(Scale::from_120ths(360).unwrap().integer(), Some(3));
    assert_eq!(Scale::from_120ths(180).unwrap().integer(), None);
    assert_eq!(Scale::from_120ths(121).unwrap().integer(), None);
}

#[test]
fn ceil_rounds_up_to_a_whole_factor() {
    let ceil = |v| Scale::from_120ths(v).unwrap().ceil();
    assert_eq!(ceil(120), Scale::ONE);
    assert_eq!(ceil(121), Scale::from_integer(2).unwrap());
    assert_eq!(ceil(180), Scale::from_integer(2).unwrap());
    assert_eq!(ceil(240), Scale::from_integer(2).unwrap());
    assert_eq!(ceil(1), Scale::ONE);
}

#[test]
fn a_logical_extent_rounds_to_the_nearest_pixel_with_halves_up() {
    let scale = |v| Scale::from_120ths(v).unwrap();
    assert_eq!(scale(180).to_physical(3), Some(5));
    assert_eq!(scale(180).to_physical(1), Some(2));
    assert_eq!(scale(180).to_physical(2), Some(3));
    assert_eq!(scale(150).to_physical(10), Some(13));
    assert_eq!(scale(150).to_physical(9), Some(11));
    assert_eq!(scale(120).to_physical(0), Some(0));
    assert_eq!(scale(240).to_physical(100), Some(200));
    // 100 * 1/120 is 0.83: rounds up to one pixel
    assert_eq!(scale(1).to_physical(100), Some(1));
    assert_eq!(scale(1).to_physical(59), Some(0));
    assert_eq!(scale(1).to_physical(60), Some(1));
}

#[test]
fn a_physical_extent_that_does_not_fit_is_none() {
    let two = Scale::from_integer(2).unwrap();
    assert_eq!(two.to_physical(u32::MAX), None);
    assert_eq!(two.to_physical(u32::MAX / 2), Some(u32::MAX - 1));
    assert_eq!(Scale::ONE.to_physical(u32::MAX), Some(u32::MAX));
}

#[test]
fn scales_order_by_value() {
    assert!(Scale::from_120ths(150).unwrap() > Scale::ONE);
    assert!(Scale::from_120ths(150).unwrap() < Scale::from_integer(2).unwrap());
}

// buffers

#[test]
fn a_layout_holds_extents_of_one_to_the_maximum() {
    let layout = BufferLayout::new(3, 2, BufferFormat::Argb8888).unwrap();
    assert_eq!((layout.width(), layout.height()), (3, 2));
    assert_eq!(layout.format(), BufferFormat::Argb8888);
    assert_eq!(layout.stride(), 12);
    assert_eq!(layout.buffer_len(), 24);
}

#[test]
fn a_layout_refuses_a_zero_or_oversized_extent() {
    let f = BufferFormat::Xrgb8888;
    assert_eq!(BufferLayout::MAX_EXTENT, 16384);
    assert!(BufferLayout::new(0, 1, f).is_none());
    assert!(BufferLayout::new(1, 0, f).is_none());
    assert!(BufferLayout::new(BufferLayout::MAX_EXTENT, 1, f).is_some());
    assert!(BufferLayout::new(1, BufferLayout::MAX_EXTENT, f).is_some());
    assert!(BufferLayout::new(BufferLayout::MAX_EXTENT + 1, 1, f).is_none());
    assert!(BufferLayout::new(1, BufferLayout::MAX_EXTENT + 1, f).is_none());
    assert!(BufferLayout::new(u32::MAX, u32::MAX, f).is_none());
}

#[test]
fn the_largest_layout_has_a_length_that_fits_a_wire_size() {
    let layout = BufferLayout::new(
        BufferLayout::MAX_EXTENT,
        BufferLayout::MAX_EXTENT,
        BufferFormat::Argb8888,
    )
    .unwrap();
    assert_eq!(layout.buffer_len(), 16384 * 16384 * 4);
    assert!(layout.buffer_len() <= i32::MAX as usize);
}

#[test]
fn layouts_of_different_format_are_not_equal() {
    let a = BufferLayout::new(2, 2, BufferFormat::Argb8888);
    let b = BufferLayout::new(2, 2, BufferFormat::Xrgb8888);
    assert_ne!(a, b);
    assert_eq!(a, BufferLayout::new(2, 2, BufferFormat::Argb8888));
}

#[test]
fn a_pixel_rect_is_plain_data() {
    let r = PixelRect {
        x: 1,
        y: 2,
        width: 3,
        height: 4,
    };
    assert_eq!(r, r);
    assert_ne!(r, PixelRect { x: 0, ..r });
}

// outputs

fn output(
    transform: OutputTransform,
    mode: Option<(u32, u32)>,
    geometry: Option<(u32, u32)>,
) -> OutputInfo {
    OutputInfo {
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
    }
}

#[test]
fn an_output_id_round_trips_its_registry_name() {
    assert_eq!(OutputId::from_registry_name(42).registry_name(), 42);
    assert_eq!(
        OutputId::from_registry_name(1),
        OutputId::from_registry_name(1)
    );
    assert!(OutputId::from_registry_name(1) < OutputId::from_registry_name(2));
}

#[test]
fn rotations_by_a_quarter_turn_swap_the_sides() {
    use OutputTransform::*;
    let swapping: Vec<_> = [
        Normal, Rotated90, Rotated180, Rotated270, Flipped, Flipped90, Flipped180, Flipped270,
    ]
    .into_iter()
    .filter(|t| t.swaps_sides())
    .collect();
    assert_eq!(swapping, [Rotated90, Rotated270, Flipped90, Flipped270]);
    assert_eq!(OutputTransform::default(), Normal);
}

#[test]
fn the_effective_scale_is_the_mode_over_the_logical_size() {
    let o = output(
        OutputTransform::Normal,
        Some((3840, 2160)),
        Some((2560, 1440)),
    );
    assert_eq!(o.effective_scale(), Some(1.5));
    let o = output(
        OutputTransform::Normal,
        Some((3840, 2160)),
        Some((1920, 1080)),
    );
    assert_eq!(o.effective_scale(), Some(2.0));
}

#[test]
fn a_rotated_output_compares_the_mode_height_with_the_logical_width() {
    let o = output(
        OutputTransform::Rotated90,
        Some((3840, 2160)),
        Some((1080, 1920)),
    );
    assert_eq!(o.effective_scale(), Some(2.0));
    let o = output(
        OutputTransform::Flipped270,
        Some((3840, 2160)),
        Some((1080, 1920)),
    );
    assert_eq!(o.effective_scale(), Some(2.0));
    let o = output(
        OutputTransform::Rotated180,
        Some((3840, 2160)),
        Some((1920, 1080)),
    );
    assert_eq!(o.effective_scale(), Some(2.0));
}

#[test]
fn the_effective_scale_needs_a_mode_and_a_logical_size() {
    assert_eq!(
        output(OutputTransform::Normal, None, Some((1, 1))).effective_scale(),
        None
    );
    assert_eq!(
        output(OutputTransform::Normal, Some((1, 1)), None).effective_scale(),
        None
    );
}

// window states

#[test]
fn a_window_that_floats_has_no_fixed_size() {
    assert!(!WindowStates::default().size_is_fixed());
    let activated = WindowStates {
        activated: true,
        suspended: true,
        ..Default::default()
    };
    assert!(!activated.size_is_fixed());
}

#[test]
fn maximized_fullscreen_resizing_and_tiled_windows_have_a_fixed_size() {
    let d = WindowStates::default;
    let fixed = [
        WindowStates {
            maximized: true,
            ..d()
        },
        WindowStates {
            fullscreen: true,
            ..d()
        },
        WindowStates {
            resizing: true,
            ..d()
        },
        WindowStates {
            tiled_left: true,
            ..d()
        },
        WindowStates {
            tiled_right: true,
            ..d()
        },
        WindowStates {
            tiled_top: true,
            ..d()
        },
        WindowStates {
            tiled_bottom: true,
            ..d()
        },
    ];
    for states in fixed {
        assert!(states.size_is_fixed(), "{states:?}");
    }
}

#[test]
fn the_window_frame_is_drawn_by_the_client_unless_told_otherwise() {
    assert_eq!(Decorations::default(), Decorations::Client);
}

// capabilities

#[test]
fn every_capability_is_listed_once_in_order() {
    let all = Capability::ALL;
    assert_eq!(all.len(), 13);
    let mut sorted = all;
    sorted.sort();
    assert_eq!(all, sorted);
    sorted.windows(2).for_each(|w| assert_ne!(w[0], w[1]));
}

#[test]
fn an_empty_set_has_no_capability_and_no_extension() {
    let set = CapabilitySet::default();
    assert!(Capability::ALL.iter().all(|c| !set.contains(*c)));
    assert_eq!(set.iter().count(), 0);
    assert!(set.extensions().is_empty());
    assert_eq!(set.extension("cupertino_layers_v1"), None);
}

// input

#[test]
fn evdev_codes_name_the_three_buttons() {
    assert_eq!(Button::from_evdev(0x110), Button::Left);
    assert_eq!(Button::from_evdev(0x111), Button::Right);
    assert_eq!(Button::from_evdev(0x112), Button::Middle);
    assert_eq!(Button::from_evdev(0x113), Button::Other(0x113));
    assert_eq!(Button::from_evdev(0), Button::Other(0));
}

#[test]
fn the_key_constants_are_evdev_codes() {
    assert_eq!(key::ESCAPE, 1);
    assert_eq!(key::BACKSPACE, 14);
    assert_eq!(key::TAB, 15);
    assert_eq!(key::ENTER, 28);
    assert_eq!(key::SPACE, 57);
    assert_eq!(key::KEYPAD_ENTER, 96);
    assert_eq!(key::HOME, 102);
    assert_eq!(key::UP, 103);
    assert_eq!(key::PAGE_UP, 104);
    assert_eq!(key::LEFT, 105);
    assert_eq!(key::RIGHT, 106);
    assert_eq!(key::END, 107);
    assert_eq!(key::DOWN, 108);
    assert_eq!(key::PAGE_DOWN, 109);
    assert_eq!(key::DELETE, 111);
}

#[test]
fn no_modifier_is_held_by_default() {
    assert_eq!(
        Modifiers::default(),
        Modifiers {
            ctrl: false,
            alt: false,
            shift: false,
            logo: false,
            caps_lock: false
        }
    );
}

// text input

#[test]
fn a_short_text_is_kept_whole() {
    let s = SurroundingText::new("héllo", 3, 3).unwrap();
    assert_eq!(s.text(), "héllo");
    assert_eq!((s.cursor(), s.anchor()), (3, 3));
}

#[test]
fn an_offset_inside_a_character_or_past_the_end_is_refused() {
    // "é" is two bytes
    assert!(SurroundingText::new("é", 1, 1).is_none());
    assert!(SurroundingText::new("é", 0, 1).is_none());
    assert!(SurroundingText::new("é", 2, 3).is_none());
    assert!(SurroundingText::new("ab", 3, 3).is_none());
    assert!(SurroundingText::new("", 0, 0).is_some());
}

#[test]
fn a_selection_keeps_its_cursor_and_anchor() {
    let s = SurroundingText::new("hello world", 5, 0).unwrap();
    assert_eq!((s.cursor(), s.anchor()), (5, 0));
}

#[test]
fn a_long_text_is_cut_around_the_cursor_at_character_boundaries() {
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
}

#[test]
fn a_long_text_cut_at_the_start_or_end_keeps_the_cursor_inside() {
    let text = "a".repeat(3 * MAX_SURROUNDING);
    for cursor in [
        0,
        1,
        MAX_SURROUNDING,
        2 * MAX_SURROUNDING,
        text.len() - 1,
        text.len(),
    ] {
        let s = SurroundingText::new(&text, cursor, cursor).unwrap();
        assert!(s.text().len() <= MAX_SURROUNDING);
        assert!(s.cursor() <= s.text().len(), "cursor {cursor}");
    }
    let s = SurroundingText::new(&text, 0, 0).unwrap();
    assert_eq!(s.cursor(), 0);
    let s = SurroundingText::new(&text, text.len(), text.len()).unwrap();
    assert_eq!(s.cursor(), s.text().len());
}

#[test]
fn a_selection_that_fits_stays_whole_inside_the_cut() {
    let text = "a".repeat(3 * MAX_SURROUNDING);
    let (anchor, cursor) = (5000, 7000);
    let s = SurroundingText::new(&text, cursor, anchor).unwrap();
    assert_eq!(s.cursor() - s.anchor(), cursor - anchor);
    assert!(s.cursor() <= s.text().len());
}

#[test]
fn a_selection_longer_than_the_limit_is_cut_around_the_cursor() {
    let text = "a".repeat(3 * MAX_SURROUNDING);
    let s = SurroundingText::new(&text, 9000, 100).unwrap();
    assert!(s.text().len() <= MAX_SURROUNDING);
    assert!(s.cursor() <= s.text().len());
    assert!(s.anchor() <= s.text().len());
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

// errors

#[test]
fn errors_say_what_went_wrong() {
    let missing = Error::MissingRequiredProtocol {
        interface: "xdg_wm_base",
        minimum_version: 2,
    };
    assert_eq!(
        missing.to_string(),
        "the compositor does not offer xdg_wm_base version 2 or newer"
    );
    assert_eq!(
        Error::InvalidSerial {
            serial: 3,
            latest: 5
        }
        .to_string(),
        "configure 3 arrived after configure 5"
    );
    assert_eq!(
        Error::UnsupportedFormat("text/x".into()).to_string(),
        "format text/x is not offered"
    );
    assert_eq!(Error::InvalidScale.to_string(), "invalid scale");
    assert_eq!(Error::DeviceLost.to_string(), "the buffer device was lost");
    assert_eq!(
        Error::PermissionDenied("a popup grab").to_string(),
        "a popup grab needs a user input event first"
    );
    assert_eq!(Error::Cancelled.to_string(), "cancelled");
    assert_eq!(Error::UnknownSurface.to_string(), "unknown surface");
    assert_eq!(
        Error::NotConfigured.to_string(),
        "the surface has not been configured yet"
    );
    assert_eq!(
        Error::NoFreeBuffer.to_string(),
        "the compositor holds every buffer of the surface"
    );
    assert_eq!(
        Error::BufferRejected.to_string(),
        "the compositor could not import the dmabuf"
    );
    assert_eq!(
        Error::Unsupported(Capability::Presentation).to_string(),
        "the compositor does not offer Presentation"
    );
    assert_eq!(
        Error::Protocol("a bad value").to_string(),
        "protocol violation by the compositor: a bad value"
    );
    let io = std::io::Error::other("no memory");
    assert_eq!(Error::Io(io).to_string(), "no memory");
}

#[test]
fn the_error_is_a_std_error() {
    fn takes(_: &dyn std::error::Error) {}
    takes(&Error::Cancelled);
}
