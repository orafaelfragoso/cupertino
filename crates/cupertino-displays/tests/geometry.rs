//! An output keeps its mode, logical extent and scale apart, and every constructor refuses a degenerate size.

use cupertino_displays::geometry::{
    raster, scale_in_120ths, Extent, GeometryError, OutputGeometry,
};

const UHD: Extent = Extent::new(3840, 2160);

#[test]
fn the_logical_extent_is_the_mode_over_the_scale() {
    for (name, mode, scale, rotated, expected) in [
        (
            "one and a half",
            UHD,
            1.5,
            false,
            Ok(Extent::new(2560, 1440)),
        ),
        ("native", UHD, 1.0, false, Ok(UHD)),
        ("rotated", UHD, 2.0, true, Ok(Extent::new(1080, 1920))),
        (
            "a fractional side rounds to the nearest point",
            Extent::new(1366, 768),
            1.25,
            false,
            Ok(Extent::new(1093, 614)),
        ),
        (
            "empty mode",
            Extent::new(0, 100),
            1.0,
            false,
            Err(GeometryError::Empty),
        ),
        (
            "zero scale",
            UHD,
            0.0,
            false,
            Err(GeometryError::InvalidScale),
        ),
        (
            "negative scale",
            UHD,
            -1.0,
            false,
            Err(GeometryError::InvalidScale),
        ),
        (
            "scale not a number",
            UHD,
            f64::NAN,
            false,
            Err(GeometryError::InvalidScale),
        ),
        (
            "infinite scale",
            UHD,
            f64::INFINITY,
            false,
            Err(GeometryError::InvalidScale),
        ),
        (
            "a side that rounds below one point",
            Extent::new(10, 10),
            100.0,
            false,
            Err(GeometryError::Empty),
        ),
        (
            "too large",
            Extent::new(u32::MAX, 10),
            1e-3,
            false,
            Err(GeometryError::Overflow),
        ),
    ] {
        let geometry = OutputGeometry::new(mode, scale, rotated);
        assert_eq!(geometry.map(|g| g.logical), expected, "{name}");
        if let Ok(g) = geometry {
            assert_eq!(g.mode, mode, "{name}: the mode stays in panel pixels");
        }
    }
}

#[test]
fn a_logical_width_gives_the_scale_that_reaches_it() {
    for (width, rotated, expected) in [
        (2560, false, Ok((1.5, Extent::new(2560, 1440)))),
        (1080, true, Ok((2.0, Extent::new(1080, 1920)))),
        (0, false, Err(GeometryError::Empty)),
    ] {
        assert_eq!(
            OutputGeometry::with_logical_width(UHD, width, rotated).map(|g| (g.scale, g.logical)),
            expected,
            "{width} rotated {rotated}"
        );
    }
}

#[test]
fn scales_travel_in_hundred_twentieths() {
    for (scale, expected) in [
        (1.0, Ok(120)),
        (1.5, Ok(180)),
        (1.25, Ok(150)),
        (1e-9, Ok(1)),
        (0.0, Err(GeometryError::InvalidScale)),
        (f64::NAN, Err(GeometryError::InvalidScale)),
        (1e12, Err(GeometryError::Overflow)),
    ] {
        assert_eq!(scale_in_120ths(scale), expected, "{scale}");
    }
}

#[test]
fn a_raster_is_the_logical_extent_at_the_buffer_scale() {
    for (logical, scale, expected) in [
        (Extent::new(2560, 1440), 180, Ok(Extent::new(3840, 2160))),
        (Extent::new(1000, 500), 120, Ok(Extent::new(1000, 500))),
        // 101 * 1.25 = 126.25 rounds to 126.
        (Extent::new(101, 101), 150, Ok(Extent::new(126, 126))),
        (Extent::new(0, 5), 120, Err(GeometryError::Empty)),
        (Extent::new(5, 5), 0, Err(GeometryError::InvalidScale)),
        (Extent::new(u32::MAX, 1), 240, Err(GeometryError::Overflow)),
    ] {
        assert_eq!(raster(logical, scale), expected, "{logical:?} at {scale}");
    }
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
fn points_convert_to_device_pixels_and_snap_onto_them() {
    let g = OutputGeometry::new(UHD, 1.5, false).unwrap();
    assert_eq!(g.to_pixels((100.0, 40.0)), (150.0, 60.0));
    assert_eq!(g.to_points((150.0, 60.0)), (100.0, 40.0));
    // One device pixel is 2/3 of a point: 1.5 device pixels round up to 2.
    assert!((g.snap(1.0) - 4.0 / 3.0).abs() < 1e-12);
    assert_eq!(g.snap(4.0), 4.0);
    let (x, y, w, h) = g.snap_outward((0.1, 0.1, 1.0, 1.0));
    assert_eq!((x, y), (0.0, 0.0));
    assert!((w - 4.0 / 3.0).abs() < 1e-12 && (h - 4.0 / 3.0).abs() < 1e-12);
    assert_eq!(
        g.snap_outward((4.0, 4.0, 8.0, 8.0)),
        (4.0, 4.0, 8.0, 8.0),
        "an aligned rect does not grow"
    );
}
