//! Nine-slice scaling.

use cupertino_geometry::{Insets, NineSlice, Rect, Size};

fn slicer() -> NineSlice {
    NineSlice::new(Size::new(30.0, 30.0), Insets::uniform(10.0))
}

#[test]
fn corners_keep_their_scaled_size_the_centre_stretches_and_the_slices_tile() {
    // (row, destination, scale, [top-left, top-right, bottom-left, bottom-right, centre])
    for (row, dest, scale, expected) in [
        (
            "scale 1",
            Rect::new(100.0, 200.0, 90.0, 60.0),
            1.0,
            [
                Rect::new(100.0, 200.0, 10.0, 10.0),
                Rect::new(180.0, 200.0, 10.0, 10.0),
                Rect::new(100.0, 250.0, 10.0, 10.0),
                Rect::new(180.0, 250.0, 10.0, 10.0),
                Rect::new(110.0, 210.0, 70.0, 40.0),
            ],
        ),
        (
            "scale 1.5",
            Rect::new(0.0, 0.0, 90.0, 90.0),
            1.5,
            [
                Rect::new(0.0, 0.0, 15.0, 15.0),
                Rect::new(75.0, 0.0, 15.0, 15.0),
                Rect::new(0.0, 75.0, 15.0, 15.0),
                Rect::new(75.0, 75.0, 15.0, 15.0),
                Rect::new(15.0, 15.0, 60.0, 60.0),
            ],
        ),
    ] {
        let s = slicer().slices(dest, scale);
        for (i, rect) in [0, 2, 6, 8, 4].into_iter().zip(expected) {
            assert_eq!(s[i].dst, rect, "{row}: slice {i}");
        }
        // the source regions do not depend on the destination
        assert_eq!(s[0].src, Rect::new(0.0, 0.0, 10.0, 10.0), "{row}");
        assert_eq!(s[4].src, Rect::new(10.0, 10.0, 10.0, 10.0), "{row}");
        assert_eq!(s[8].src, Rect::new(20.0, 20.0, 10.0, 10.0), "{row}");
        let total: f64 = s.iter().map(|s| s.dst.area()).sum();
        assert!((total - dest.area()).abs() < 1e-9, "{row}: gaps");
    }
}

#[test]
fn a_small_destination_shrinks_opposite_corners_proportionally() {
    let s = slicer().slices(Rect::new(0.0, 0.0, 10.0, 60.0), 1.0);
    assert_eq!(s[0].dst.width(), 5.0);
    assert_eq!(s[2].dst.width(), 5.0);
    assert_eq!(s[4].dst.width(), 0.0);
}

#[test]
fn min_size_is_the_scaled_inset_sum_within_the_source() {
    // (row, source, insets, scale, minimum)
    for (row, source, insets, scale, min) in [
        (
            "scale 1",
            Size::new(40.0, 30.0),
            Insets::new(4.0, 6.0, 8.0, 10.0),
            1.0,
            Size::new(16.0, 12.0),
        ),
        (
            "scale 2",
            Size::new(40.0, 30.0),
            Insets::new(4.0, 6.0, 8.0, 10.0),
            2.0,
            Size::new(32.0, 24.0),
        ),
        (
            "insets beyond the source",
            Size::new(20.0, 20.0),
            Insets::uniform(20.0),
            1.0,
            Size::new(20.0, 20.0),
        ),
        (
            "negative insets",
            Size::new(20.0, 20.0),
            Insets::uniform(-4.0),
            1.0,
            Size::ZERO,
        ),
    ] {
        assert_eq!(NineSlice::new(source, insets).min_size(scale), min, "{row}");
    }
}

#[test]
fn visible_slices_drop_the_empty_ones() {
    let three_part = NineSlice::new(Size::new(30.0, 10.0), Insets::new(0.0, 10.0, 0.0, 10.0));
    let visible = three_part.visible_slices(Rect::new(0.0, 0.0, 100.0, 10.0), 1.0);
    assert_eq!(visible.len(), 3);
    assert!(visible.iter().all(|s| !s.is_empty()));
    let roomy = slicer().visible_slices(Rect::new(0.0, 0.0, 90.0, 90.0), 1.0);
    assert_eq!(roomy.len(), 9);
}

#[test]
fn uv_is_normalised_by_the_texture_size() {
    let s = slicer().slices(Rect::new(0.0, 0.0, 90.0, 90.0), 1.0);
    let uv = s[4].uv(Size::new(60.0, 30.0));
    for (got, want) in uv
        .into_iter()
        .zip([10.0 / 60.0, 10.0 / 30.0, 20.0 / 60.0, 20.0 / 30.0])
    {
        assert!((got - want).abs() < 1e-12, "{uv:?}");
    }
}
