//! Output transforms and buffer pixel areas.

use cupertino_geometry::{area_in_buffer, OutputTransform, PixelArea, Point, Rect, Size};

#[test]
fn wayland_values_map_in_order_and_reject_out_of_range() {
    for (i, t) in OutputTransform::ALL.iter().enumerate() {
        assert_eq!(OutputTransform::from_wayland(i as u32), Some(*t));
    }
    assert_eq!(
        OutputTransform::from_wayland(3),
        Some(OutputTransform::Rotated270)
    );
    assert_eq!(OutputTransform::from_wayland(8), None);
}

#[test]
fn quarter_turns_swap_the_sides_and_invert_the_other_way() {
    use OutputTransform::*;
    // (transform, swaps, inverse)
    for (t, swaps, inverse) in [
        (Normal, false, Normal),
        (Rotated90, true, Rotated270),
        (Rotated180, false, Rotated180),
        (Rotated270, true, Rotated90),
        (Flipped, false, Flipped),
        (Flipped90, true, Flipped90),
        (Flipped180, false, Flipped180),
        (Flipped270, true, Flipped270),
    ] {
        assert_eq!(t.swaps_sides(), swaps, "{t:?}");
        assert_eq!(t.inverse(), inverse, "{t:?}");
        let (size, pixels) = if swaps {
            (Size::new(1080.0, 1920.0), (1080, 1920))
        } else {
            (Size::new(1920.0, 1080.0), (1920, 1080))
        };
        assert_eq!(t.turn_size(Size::new(1920.0, 1080.0)), size, "{t:?}");
        assert_eq!(t.turn_pixels((1920, 1080)), pixels, "{t:?}");
    }
}

#[test]
fn buffer_points_follow_the_transform() {
    let up = Size::new(400.0, 200.0);
    // a counter-clockwise quarter turn moves the upright top-right to the buffer's top-left
    for (t, p, expected) in [
        (
            OutputTransform::Rotated90,
            Point::new(400.0, 0.0),
            Point::new(0.0, 0.0),
        ),
        (
            OutputTransform::Rotated90,
            Point::new(0.0, 0.0),
            Point::new(0.0, 400.0),
        ),
        (
            OutputTransform::Flipped,
            Point::new(10.0, 20.0),
            Point::new(390.0, 20.0),
        ),
        (
            OutputTransform::Rotated180,
            Point::new(10.0, 20.0),
            Point::new(390.0, 180.0),
        ),
    ] {
        assert_eq!(t.buffer_point(p, up), expected, "{t:?} {p:?}");
    }
}

#[test]
fn every_transform_round_trips_points_and_rects() {
    let up = Size::new(400.0, 200.0);
    let p = Point::new(37.5, 120.25);
    let r = Rect::new(10.0, 20.0, 30.0, 40.0);
    for t in OutputTransform::ALL {
        let buffer = t.turn_size(up);
        let back = t.upright_point(t.buffer_point(p, up), buffer);
        assert!(
            (back.x - p.x).abs() < 1e-9 && (back.y - p.y).abs() < 1e-9,
            "{t:?}"
        );
        assert_eq!(t.upright_rect(&t.buffer_rect(&r, up), buffer), r, "{t:?}");
    }
    let turned = OutputTransform::Rotated90.buffer_rect(&Rect::new(0.0, 0.0, 100.0, 50.0), up);
    assert_eq!((turned.width(), turned.height()), (50.0, 100.0));
    assert!(Rect::new(0.0, 0.0, 200.0, 400.0)
        .intersection(&turned)
        .is_some());
}

#[test]
fn every_buffer_pixel_maps_to_a_distinct_upright_pixel() {
    // a 4x2 upright picture held at Rotated90 is a 2x4 buffer; its pixel (0, 0) shows the upright top-right pixel
    for (t, pixel, buffer, upright) in [
        (OutputTransform::Rotated90, (0, 0), (2, 4), (3, 0)),
        (OutputTransform::Normal, (1, 1), (4, 2), (1, 1)),
        (OutputTransform::Flipped, (0, 0), (4, 2), (3, 0)),
    ] {
        assert_eq!(t.upright_pixel(pixel, buffer), upright, "{t:?}");
    }
    for t in OutputTransform::ALL {
        let buffer = (6u32, 4u32);
        let upright = t.turn_pixels(buffer);
        let mut seen = std::collections::HashSet::new();
        for y in 0..buffer.1 {
            for x in 0..buffer.0 {
                let (ux, uy) = t.upright_pixel((x, y), buffer);
                assert!(ux < upright.0 && uy < upright.1, "{t:?}");
                assert!(seen.insert((ux, uy)), "{t:?} repeats ({ux}, {uy})");
            }
        }
    }
}

#[test]
fn an_area_becomes_the_buffer_pixels_it_covers() {
    fn px(x: u32, y: u32, width: u32, height: u32) -> Option<PixelArea> {
        Some(PixelArea {
            x,
            y,
            width,
            height,
        })
    }
    let normal = OutputTransform::Normal;
    // (row, area, output, buffer, transform, pixels)
    for (row, area, output, buffer, t, expected) in [
        (
            "scale 1",
            Rect::new(100.0, 50.0, 300.0, 200.0),
            Rect::new(0.0, 0.0, 1920.0, 1080.0),
            (1920, 1080),
            normal,
            px(100, 50, 300, 200),
        ),
        (
            "scale 1.5",
            Rect::new(100.0, 50.0, 300.0, 200.0),
            Rect::new(0.0, 0.0, 1280.0, 720.0),
            (1920, 1080),
            normal,
            px(150, 75, 450, 300),
        ),
        (
            "output origin subtracted",
            Rect::new(1930.0, 120.0, 30.0, 40.0),
            Rect::new(1920.0, 100.0, 1920.0, 1080.0),
            (1920, 1080),
            normal,
            px(10, 20, 30, 40),
        ),
        (
            "cut to the buffer",
            Rect::new(-20.0, 90.0, 50.0, 50.0),
            Rect::new(0.0, 0.0, 100.0, 100.0),
            (100, 100),
            normal,
            px(0, 90, 30, 10),
        ),
        (
            "turned buffer",
            Rect::new(0.0, 0.0, 100.0, 50.0),
            Rect::new(0.0, 0.0, 400.0, 200.0),
            (200, 400),
            OutputTransform::Rotated90,
            px(0, 300, 50, 100),
        ),
        (
            "outside the output",
            Rect::new(200.0, 0.0, 10.0, 10.0),
            Rect::new(0.0, 0.0, 100.0, 100.0),
            (100, 100),
            normal,
            None,
        ),
        (
            "thinner than a pixel",
            Rect::new(10.0, 10.0, 0.3, 10.0),
            Rect::new(0.0, 0.0, 100.0, 100.0),
            (100, 100),
            normal,
            None,
        ),
        (
            "empty output",
            Rect::new(0.0, 0.0, 10.0, 10.0),
            Rect::ZERO,
            (100, 100),
            normal,
            None,
        ),
    ] {
        assert_eq!(area_in_buffer(&area, &output, buffer, t), expected, "{row}");
    }
}

#[test]
fn fractional_scale_edges_round_to_the_nearest_pixel_and_neighbours_share_an_edge() {
    let output = Rect::new(0.0, 0.0, 1280.0, 720.0);
    let cut = |r: Rect| area_in_buffer(&r, &output, (1920, 1080), OutputTransform::Normal).unwrap();
    let left = cut(Rect::new(0.0, 0.0, 100.3, 50.0));
    let right = cut(Rect::new(100.3, 0.0, 100.0, 50.0));
    // 100.3 * 1.5 = 150.45 -> 150
    assert_eq!(left.width, 150);
    assert_eq!(right.x, left.x + left.width);
}

#[test]
fn cutting_the_area_and_turning_it_back_gives_the_upright_area() {
    let output = Rect::new(0.0, 0.0, 6.0, 4.0);
    let area = Rect::new(1.0, 1.0, 3.0, 2.0);
    for t in OutputTransform::ALL {
        let buffer = t.turn_pixels((6, 4));
        let pa = area_in_buffer(&area, &output, buffer, t).unwrap();
        assert_eq!(
            t.inverse().turn_pixels((pa.width, pa.height)),
            (3, 2),
            "{t:?}"
        );
        // the cut-out's corner pixels show the upright area's corners
        let ups: Vec<_> = [(pa.x, pa.y), (pa.x + pa.width - 1, pa.y + pa.height - 1)]
            .into_iter()
            .map(|p| t.upright_pixel(p, buffer))
            .collect();
        let min_x = ups.iter().map(|p| p.0).min().unwrap();
        let min_y = ups.iter().map(|p| p.1).min().unwrap();
        assert_eq!((min_x, min_y), (1, 1), "{t:?}");
    }
}
