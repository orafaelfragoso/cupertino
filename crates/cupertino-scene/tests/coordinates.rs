//! Global rectangles are moved onto an output's own coordinates, and a content blur keeps its radius in thousandths.

use cupertino_geometry::Rect;
use cupertino_scene::{ContentBlur, GlobalRect, OutputRect};

#[test]
fn a_global_rect_is_expressed_relative_to_its_output() {
    let output = GlobalRect(Rect::new(1920.0, 100.0, 2560.0, 1440.0));
    let window = GlobalRect(Rect::new(2000.0, 150.0, 300.0, 200.0));
    let OutputRect(local) = window.on_output(output);
    assert_eq!(local, Rect::new(80.0, 50.0, 300.0, 200.0));
}

#[test]
fn a_rect_left_of_the_output_origin_has_negative_coordinates() {
    let output = GlobalRect(Rect::new(0.0, 0.0, 1920.0, 1080.0));
    let window = GlobalRect(Rect::new(-40.0, -10.0, 100.0, 100.0));
    assert_eq!(
        window.on_output(output),
        OutputRect(Rect::new(-40.0, -10.0, 100.0, 100.0))
    );
}

#[test]
fn a_rect_on_the_output_origin_does_not_move() {
    let output = GlobalRect(Rect::new(-1280.0, 0.0, 1280.0, 720.0));
    assert_eq!(
        output.on_output(output),
        OutputRect(Rect::new(0.0, 0.0, 1280.0, 720.0))
    );
}

#[test]
fn a_blur_radius_round_trips_through_thousandths() {
    let blur = ContentBlur::of_points(12.5);
    assert_eq!(blur.sigma_milli, 12_500);
    assert_eq!(blur.sigma(), 12.5);
    assert_eq!(ContentBlur::of_points(0.0004).sigma_milli, 0);
    assert_eq!(ContentBlur::of_points(0.0006).sigma_milli, 1);
}

#[test]
fn a_negative_or_huge_blur_radius_is_clamped() {
    assert_eq!(ContentBlur::of_points(-3.0).sigma_milli, 0);
    assert_eq!(ContentBlur::of_points(f64::NAN).sigma_milli, 0);
    assert_eq!(ContentBlur::of_points(1e12).sigma_milli, u32::MAX);
    assert_eq!(ContentBlur::of_points(f64::INFINITY).sigma_milli, u32::MAX);
}
