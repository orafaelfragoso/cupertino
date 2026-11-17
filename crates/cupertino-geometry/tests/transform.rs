//! Output transforms and buffer pixel areas.

use cupertino_geometry::{area_in_buffer, OutputTransform, PixelArea, Point, Rect, Size};

#[test]
fn wayland_values_map_in_order_and_reject_out_of_range() {
    for (i, t) in OutputTransform::ALL.iter().enumerate() {
        assert_eq!(OutputTransform::from_wayland(i as u32), Some(*t));
    }
    assert_eq!(
        OutputTransform::from_wayland(0),
        Some(OutputTransform::Normal)
    );
    assert_eq!(
        OutputTransform::from_wayland(3),
        Some(OutputTransform::Rotated270)
    );
    assert_eq!(OutputTransform::from_wayland(8), None);
    assert_eq!(OutputTransform::default(), OutputTransform::Normal);
}

#[test]
fn quarter_turns_swap_the_sides() {
    use OutputTransform::*;
    for t in [Rotated90, Rotated270, Flipped90, Flipped270] {
        assert!(t.swaps_sides(), "{t:?}");
        assert_eq!(
            t.turn_size(Size::new(1920.0, 1080.0)),
            Size::new(1080.0, 1920.0)
        );
        assert_eq!(t.turn_pixels((1920, 1080)), (1080, 1920));
    }
    for t in [Normal, Rotated180, Flipped, Flipped180] {
        assert!(!t.swaps_sides(), "{t:?}");
        assert_eq!(t.turn_pixels((1920, 1080)), (1920, 1080));
    }
}

#[test]
fn inverse_turns_quarters_the_other_way_and_leaves_the_rest() {
    use OutputTransform::*;
    assert_eq!(Rotated90.inverse(), Rotated270);
    assert_eq!(Rotated270.inverse(), Rotated90);
    for t in [
        Normal, Rotated180, Flipped, Flipped90, Flipped180, Flipped270,
    ] {
        assert_eq!(t.inverse(), t);
    }
}

#[test]
fn a_quarter_turn_counter_clockwise_moves_the_top_right_to_the_top_left() {
    // upright 400x200; its top-right corner (400, 0) lands at the buffer's top-left (0, 0)
    let up = Size::new(400.0, 200.0);
    assert_eq!(
        OutputTransform::Rotated90.buffer_point(Point::new(400.0, 0.0), up),
        Point::new(0.0, 0.0)
    );
    assert_eq!(
        OutputTransform::Rotated90.buffer_point(Point::new(0.0, 0.0), up),
        Point::new(0.0, 400.0)
    );
}

#[test]
fn flipped_mirrors_left_to_right() {
    let up = Size::new(400.0, 200.0);
    assert_eq!(
        OutputTransform::Flipped.buffer_point(Point::new(10.0, 20.0), up),
        Point::new(390.0, 20.0)
    );
}

#[test]
fn half_turn_is_a_point_reflection() {
    let up = Size::new(400.0, 200.0);
    assert_eq!(
        OutputTransform::Rotated180.buffer_point(Point::new(10.0, 20.0), up),
        Point::new(390.0, 180.0)
    );
}

#[test]
fn every_transform_round_trips_points_and_rects() {
    let up = Size::new(400.0, 200.0);
    let p = Point::new(37.5, 120.25);
    let r = Rect::new(10.0, 20.0, 30.0, 40.0);
    for t in OutputTransform::ALL {
        let buffer = t.turn_size(up);
        let there = t.buffer_point(p, up);
        let back = t.upright_point(there, buffer);
        assert!(
            (back.x - p.x).abs() < 1e-9 && (back.y - p.y).abs() < 1e-9,
            "{t:?}"
        );
        let rr = t.upright_rect(&t.buffer_rect(&r, up), buffer);
        assert_eq!(rr, r, "{t:?}");
    }
}

#[test]
fn buffer_rect_swaps_its_sides_under_a_quarter_turn() {
    let up = Size::new(400.0, 200.0);
    let r = OutputTransform::Rotated90.buffer_rect(&Rect::new(0.0, 0.0, 100.0, 50.0), up);
    assert_eq!((r.width(), r.height()), (50.0, 100.0));
    assert!(Rect::new(0.0, 0.0, 200.0, 400.0).intersection(&r).is_some());
}

#[test]
fn upright_pixel_turns_a_buffer_one_pixel_at_a_time() {
    // a 4x2 upright picture held at Rotated90 is a 2x4 buffer; its pixel (0, 0) shows the upright top-right pixel
    assert_eq!(
        OutputTransform::Rotated90.upright_pixel((0, 0), (2, 4)),
        (3, 0)
    );
    assert_eq!(
        OutputTransform::Normal.upright_pixel((1, 1), (4, 2)),
        (1, 1)
    );
    assert_eq!(
        OutputTransform::Flipped.upright_pixel((0, 0), (4, 2)),
        (3, 0)
    );
}

#[test]
fn every_buffer_pixel_maps_to_a_distinct_upright_pixel() {
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
fn an_area_of_a_normal_output_at_scale_one_is_its_own_pixels() {
    let output = Rect::new(0.0, 0.0, 1920.0, 1080.0);
    let a = area_in_buffer(
        &Rect::new(100.0, 50.0, 300.0, 200.0),
        &output,
        (1920, 1080),
        OutputTransform::Normal,
    );
    assert_eq!(
        a,
        Some(PixelArea {
            x: 100,
            y: 50,
            width: 300,
            height: 200
        })
    );
}

#[test]
fn an_area_is_scaled_to_the_buffers_pixels_per_point() {
    let output = Rect::new(0.0, 0.0, 1280.0, 720.0);
    let a = area_in_buffer(
        &Rect::new(100.0, 50.0, 300.0, 200.0),
        &output,
        (1920, 1080),
        OutputTransform::Normal,
    );
    assert_eq!(
        a,
        Some(PixelArea {
            x: 150,
            y: 75,
            width: 450,
            height: 300
        })
    );
}

#[test]
fn fractional_scale_edges_round_to_the_nearest_pixel_and_neighbours_share_an_edge() {
    let output = Rect::new(0.0, 0.0, 1280.0, 720.0);
    let left = area_in_buffer(
        &Rect::new(0.0, 0.0, 100.3, 50.0),
        &output,
        (1920, 1080),
        OutputTransform::Normal,
    )
    .unwrap();
    let right = area_in_buffer(
        &Rect::new(100.3, 0.0, 100.0, 50.0),
        &output,
        (1920, 1080),
        OutputTransform::Normal,
    )
    .unwrap();
    // 100.3 * 1.5 = 150.45 -> 150
    assert_eq!(left.width, 150);
    assert_eq!(right.x, left.x + left.width);
}

#[test]
fn the_output_origin_is_subtracted_before_scaling() {
    let output = Rect::new(1920.0, 100.0, 1920.0, 1080.0);
    let a = area_in_buffer(
        &Rect::new(1920.0 + 10.0, 100.0 + 20.0, 30.0, 40.0),
        &output,
        (1920, 1080),
        OutputTransform::Normal,
    );
    assert_eq!(
        a,
        Some(PixelArea {
            x: 10,
            y: 20,
            width: 30,
            height: 40
        })
    );
}

#[test]
fn an_area_is_cut_to_the_buffer() {
    let output = Rect::new(0.0, 0.0, 100.0, 100.0);
    let a = area_in_buffer(
        &Rect::new(-20.0, 90.0, 50.0, 50.0),
        &output,
        (100, 100),
        OutputTransform::Normal,
    );
    assert_eq!(
        a,
        Some(PixelArea {
            x: 0,
            y: 90,
            width: 30,
            height: 10
        })
    );
}

#[test]
fn an_area_outside_the_output_or_thinner_than_a_pixel_is_none() {
    let output = Rect::new(0.0, 0.0, 100.0, 100.0);
    let n = OutputTransform::Normal;
    assert_eq!(
        area_in_buffer(&Rect::new(200.0, 0.0, 10.0, 10.0), &output, (100, 100), n),
        None
    );
    assert_eq!(
        area_in_buffer(&Rect::new(10.0, 10.0, 0.3, 10.0), &output, (100, 100), n),
        None
    );
    assert_eq!(
        area_in_buffer(&Rect::new(0.0, 0.0, 10.0, 10.0), &Rect::ZERO, (100, 100), n),
        None
    );
}

#[test]
fn a_turned_buffer_gives_the_turned_pixel_area() {
    // a 400x200 upright output held at Rotated90 is a 200x400 buffer
    let output = Rect::new(0.0, 0.0, 400.0, 200.0);
    let a = area_in_buffer(
        &Rect::new(0.0, 0.0, 100.0, 50.0),
        &output,
        (200, 400),
        OutputTransform::Rotated90,
    )
    .unwrap();
    assert_eq!((a.width, a.height), (50, 100));
    assert_eq!((a.x, a.y), (0, 300));
}

#[test]
fn cutting_the_area_and_turning_it_back_gives_the_upright_area() {
    let output = Rect::new(0.0, 0.0, 6.0, 4.0);
    let area = Rect::new(1.0, 1.0, 3.0, 2.0);
    for t in OutputTransform::ALL {
        let buffer = t.turn_pixels((6, 4));
        let pa = area_in_buffer(&area, &output, buffer, t).unwrap();
        let (w, h) = t.inverse().turn_pixels((pa.width, pa.height));
        assert_eq!((w, h), (3, 2), "{t:?}");
        // the cut-out's corner pixels show the upright area's corners
        let mut ups = Vec::new();
        for (x, y) in [(pa.x, pa.y), (pa.x + pa.width - 1, pa.y + pa.height - 1)] {
            ups.push(t.upright_pixel((x, y), buffer));
        }
        let min_x = ups.iter().map(|p| p.0).min().unwrap();
        let min_y = ups.iter().map(|p| p.1).min().unwrap();
        assert_eq!((min_x, min_y), (1, 1), "{t:?}");
    }
}
