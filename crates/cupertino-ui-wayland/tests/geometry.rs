//! The arithmetic a window system does before the transport: where a popup goes, what a frame redraws and how its
//! pixels are laid out in a buffer.

use cupertino_geometry::Rect;
use cupertino_ui_wayland::{copy_rects, popup, rgba_to_argb8888, Damage, Error, PixelRect};
use cupertino_wayland_client::{Anchor, Gravity};
use tiny_skia::Pixmap;

#[test]
fn a_popup_covers_its_rectangle_rounded_outwards_with_its_corner_at_the_rectangles() {
    let rows = [
        (
            "whole points",
            Rect::new(10.0, 20.0, 100.0, 40.0),
            Rect::new(10.0, 20.0, 100.0, 40.0),
        ),
        (
            "fractions are rounded outwards",
            Rect::new(10.4, 20.6, 99.2, 39.6),
            Rect::new(10.0, 20.0, 100.0, 41.0),
        ),
        (
            "a corner left of the parent",
            Rect::new(-5.5, -0.5, 30.0, 10.0),
            Rect::new(-6.0, -1.0, 31.0, 11.0),
        ),
    ];
    for (what, rect, whole) in rows {
        let placement = popup::placement(&rect, false).unwrap();
        assert_eq!(placement.anchor_rect, whole, "{what}");
        assert_eq!(
            placement.size,
            (whole.width() as u32, whole.height() as u32),
            "{what}"
        );
        assert_eq!(
            (placement.anchor, placement.gravity),
            (Anchor::TopLeft, Gravity::BottomRight),
            "{what}"
        );
        assert!(!placement.grab, "{what}");
    }
    assert!(
        popup::placement(&Rect::new(0.0, 0.0, 10.0, 10.0), true)
            .unwrap()
            .grab,
        "a popup that takes the seat's grab"
    );

    let refused = [
        ("no width", Rect::new(0.0, 0.0, 0.0, 10.0)),
        ("no height", Rect::new(0.0, 0.0, 10.0, 0.0)),
        (
            "a width that is not a number",
            Rect::new(0.0, 0.0, f64::NAN, 10.0),
        ),
        (
            "an infinite corner",
            Rect::new(f64::INFINITY, 0.0, 10.0, 10.0),
        ),
    ];
    for (what, rect) in refused {
        assert!(
            matches!(popup::placement(&rect, false), Err(Error::InvalidShape)),
            "{what}"
        );
    }
}

#[test]
fn damage_is_the_whole_surface_until_a_frame_takes_it_and_then_what_changed_in_order() {
    let mut damage = Damage::new();
    assert!(damage.is_pending(), "a new surface is all to draw");
    damage.add(Rect::new(1.0, 1.0, 5.0, 5.0));
    assert_eq!(
        damage.take(),
        None,
        "and rectangles added to the whole are the whole"
    );
    assert!(!damage.is_pending());
    assert_eq!(
        damage.take(),
        Some(vec![]),
        "a frame after that has nothing unless something changed"
    );

    damage.add(Rect::new(40.0, 50.0, 5.0, 5.0));
    damage.add(Rect::new(0.0, 0.0, 10.0, 10.0));
    damage.add(Rect::new(5.0, 5.0, 10.0, 10.0));
    damage.add(Rect::new(100.0, 0.0, 0.0, 10.0));
    assert!(damage.is_pending());
    assert_eq!(
        damage.take(),
        Some(vec![
            Rect::new(0.0, 0.0, 15.0, 15.0),
            Rect::new(40.0, 50.0, 5.0, 5.0)
        ]),
        "overlapping rectangles merge, an empty one is nothing, and they come top to bottom"
    );

    for i in 0..9 {
        damage.add(Rect::new(f64::from(i) * 20.0, 0.0, 5.0, 5.0));
    }
    assert_eq!(
        damage.take(),
        Some(vec![Rect::new(0.0, 0.0, 165.0, 5.0)]),
        "a ninth rectangle apart makes all of them the rectangle round them"
    );
    damage.add_whole();
    damage.add(Rect::new(1.0, 1.0, 1.0, 1.0));
    assert_eq!(damage.take(), None);
}

#[test]
fn rgba_pixels_become_argb_by_swapping_red_and_blue_and_only_the_rectangles_asked_for_are_copied() {
    let rgba = [10, 20, 30, 40, 50, 60, 70, 80];
    let mut argb = [0; 8];
    rgba_to_argb8888(&rgba, &mut argb);
    assert_eq!(argb, [30, 20, 10, 40, 70, 60, 50, 80]);
    assert!(
        std::panic::catch_unwind(|| rgba_to_argb8888(&[0; 8], &mut [0; 4])).is_err(),
        "buffers of different sizes"
    );

    let mut from = Pixmap::new(4, 3).unwrap();
    for (i, byte) in from.data_mut().iter_mut().enumerate() {
        *byte = i as u8 + 1;
    }
    let mut to = Pixmap::new(4, 3).unwrap();
    copy_rects(
        &mut to,
        &from,
        &[PixelRect {
            x: 1,
            y: 1,
            width: 2,
            height: 2,
        }],
    );
    let copied: Vec<usize> = (0..12)
        .filter(|i| to.data()[i * 4..i * 4 + 4] != [0; 4])
        .collect();
    assert_eq!(
        copied,
        [5, 6, 9, 10],
        "the pixels of the rectangle and no others"
    );
    assert_eq!(
        to.data()[5 * 4..6 * 4],
        from.data()[5 * 4..6 * 4],
        "as they were"
    );
}
