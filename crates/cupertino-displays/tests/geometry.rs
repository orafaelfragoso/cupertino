//! An output keeps its mode, logical extent and scale apart, and every constructor refuses a degenerate size.

use cupertino_displays::geometry::{
    raster, scale_in_120ths, Extent, GeometryError, OutputGeometry, SCALE_DENOMINATOR,
};

const UHD: Extent = Extent::new(3840, 2160);

#[test]
fn the_logical_extent_is_the_mode_over_the_scale() {
    let g = OutputGeometry::new(UHD, 1.5, false).unwrap();
    assert_eq!(g.logical, Extent::new(2560, 1440));
    let g = OutputGeometry::new(UHD, 1.0, false).unwrap();
    assert_eq!(g.logical, UHD);
}

#[test]
fn a_rotated_output_lays_out_with_its_sides_swapped() {
    let g = OutputGeometry::new(UHD, 2.0, true).unwrap();
    assert_eq!(g.logical, Extent::new(1080, 1920));
    assert_eq!(g.mode, UHD, "the mode stays in panel pixels");
}

#[test]
fn a_fractional_logical_side_rounds_to_the_nearest_point() {
    let g = OutputGeometry::new(Extent::new(1366, 768), 1.25, false).unwrap();
    assert_eq!(g.logical, Extent::new(1093, 614));
}

#[test]
fn degenerate_inputs_are_errors() {
    assert_eq!(
        OutputGeometry::new(Extent::new(0, 100), 1.0, false),
        Err(GeometryError::Empty)
    );
    for scale in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert_eq!(
            OutputGeometry::new(UHD, scale, false),
            Err(GeometryError::InvalidScale),
            "scale {scale}"
        );
    }
    assert_eq!(
        OutputGeometry::new(Extent::new(10, 10), 100.0, false),
        Err(GeometryError::Empty),
        "a side that rounds below one point"
    );
    assert_eq!(
        OutputGeometry::new(Extent::new(u32::MAX, 10), 1e-3, false),
        Err(GeometryError::Overflow)
    );
}

#[test]
fn a_logical_width_gives_the_scale_that_reaches_it() {
    let g = OutputGeometry::with_logical_width(UHD, 2560, false).unwrap();
    assert_eq!(g.scale, 1.5);
    assert_eq!(g.logical, Extent::new(2560, 1440));
    let turned = OutputGeometry::with_logical_width(UHD, 1080, true).unwrap();
    assert_eq!(turned.scale, 2.0);
    assert_eq!(turned.logical, Extent::new(1080, 1920));
    assert_eq!(
        OutputGeometry::with_logical_width(UHD, 0, false),
        Err(GeometryError::Empty)
    );
}

#[test]
fn scales_travel_in_hundred_twentieths() {
    assert_eq!(SCALE_DENOMINATOR, 120);
    assert_eq!(scale_in_120ths(1.0), Ok(120));
    assert_eq!(scale_in_120ths(1.5), Ok(180));
    assert_eq!(scale_in_120ths(1.25), Ok(150));
    assert_eq!(scale_in_120ths(1e-9), Ok(1), "never zero");
    assert_eq!(scale_in_120ths(0.0), Err(GeometryError::InvalidScale));
    assert_eq!(scale_in_120ths(f64::NAN), Err(GeometryError::InvalidScale));
    assert_eq!(scale_in_120ths(1e12), Err(GeometryError::Overflow));
}

#[test]
fn a_raster_is_the_logical_extent_at_the_buffer_scale() {
    assert_eq!(
        raster(Extent::new(2560, 1440), 180),
        Ok(Extent::new(3840, 2160))
    );
    assert_eq!(
        raster(Extent::new(1000, 500), 120),
        Ok(Extent::new(1000, 500))
    );
    // 101 * 1.25 = 126.25 rounds to 126.
    assert_eq!(
        raster(Extent::new(101, 101), 150),
        Ok(Extent::new(126, 126))
    );
    assert_eq!(raster(Extent::new(0, 5), 120), Err(GeometryError::Empty));
    assert_eq!(
        raster(Extent::new(5, 5), 0),
        Err(GeometryError::InvalidScale)
    );
    assert_eq!(
        raster(Extent::new(u32::MAX, 1), 240),
        Err(GeometryError::Overflow)
    );
}

#[test]
fn the_output_raster_matches_the_mode_and_the_whole_raster_rounds_the_scale_up() {
    let g = OutputGeometry::new(UHD, 1.5, false).unwrap();
    assert_eq!(g.buffer_scale_120ths(), 180);
    assert_eq!(g.raster(), Ok(UHD));
    assert_eq!(g.whole_raster(), Ok(Extent::new(5120, 2880)));
    let whole = OutputGeometry::new(UHD, 2.0, false).unwrap();
    assert_eq!(whole.whole_raster(), whole.raster());
}

#[test]
fn points_and_pixels_convert_both_ways() {
    let g = OutputGeometry::new(UHD, 1.5, false).unwrap();
    assert_eq!(g.to_pixels((100.0, 40.0)), (150.0, 60.0));
    assert_eq!(g.to_points((150.0, 60.0)), (100.0, 40.0));
}

#[test]
fn snapping_lands_on_device_pixels() {
    let g = OutputGeometry::new(UHD, 1.5, false).unwrap();
    // One device pixel is 2/3 of a point.
    assert!(
        (g.snap(1.0) - 4.0 / 3.0).abs() < 1e-12,
        "1.5 device pixels round up to 2"
    );
    assert_eq!(g.snap(4.0), 4.0);
    let (x, y, w, h) = g.snap_outward((0.1, 0.1, 1.0, 1.0));
    assert_eq!((x, y), (0.0, 0.0));
    assert!((w - 4.0 / 3.0).abs() < 1e-12 && (h - 4.0 / 3.0).abs() < 1e-12);
    // An already aligned rect does not grow.
    assert_eq!(g.snap_outward((4.0, 4.0, 8.0, 8.0)), (4.0, 4.0, 8.0, 8.0));
}

#[test]
fn an_extent_knows_its_area_and_its_swap() {
    let e = Extent::new(3, 4);
    assert_eq!(e.swapped(), Extent::new(4, 3));
    assert_eq!(e.area(), 12);
    assert_eq!(
        Extent::new(u32::MAX, u32::MAX).area(),
        u64::from(u32::MAX).pow(2)
    );
    assert_eq!(e.to_string(), "3\u{202f}×\u{202f}4");
}

#[test]
fn errors_describe_themselves() {
    assert_eq!(GeometryError::Empty.to_string(), "the size is empty");
    assert_eq!(GeometryError::Overflow.to_string(), "the size is too large");
}
