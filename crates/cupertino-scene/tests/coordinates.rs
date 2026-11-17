//! Global rectangles are moved onto an output's own coordinates, and a content blur keeps its radius in thousandths.

use cupertino_geometry::Rect;
use cupertino_scene::{ContentBlur, GlobalRect, OutputRect};

#[test]
fn a_global_rect_is_expressed_relative_to_its_output() {
    for (name, output, window, local) in [
        (
            "inside an output right of the origin",
            Rect::new(1920.0, 100.0, 2560.0, 1440.0),
            Rect::new(2000.0, 150.0, 300.0, 200.0),
            Rect::new(80.0, 50.0, 300.0, 200.0),
        ),
        (
            "left of the output's origin",
            Rect::new(0.0, 0.0, 1920.0, 1080.0),
            Rect::new(-40.0, -10.0, 100.0, 100.0),
            Rect::new(-40.0, -10.0, 100.0, 100.0),
        ),
        (
            "the output itself",
            Rect::new(-1280.0, 0.0, 1280.0, 720.0),
            Rect::new(-1280.0, 0.0, 1280.0, 720.0),
            Rect::new(0.0, 0.0, 1280.0, 720.0),
        ),
    ] {
        assert_eq!(
            GlobalRect(window).on_output(GlobalRect(output)),
            OutputRect(local),
            "{name}"
        );
    }
}

#[test]
fn a_blur_radius_is_kept_in_thousandths_and_clamped() {
    for (points, milli) in [
        (12.5, 12_500),
        (0.0004, 0),
        (0.0006, 1),
        (-3.0, 0),
        (f64::NAN, 0),
        (1e12, u32::MAX),
        (f64::INFINITY, u32::MAX),
    ] {
        assert_eq!(
            ContentBlur::of_points(points).sigma_milli,
            milli,
            "{points}"
        );
    }
    assert_eq!(ContentBlur::of_points(12.5).sigma(), 12.5);
}
