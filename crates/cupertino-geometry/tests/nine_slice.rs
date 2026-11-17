//! Nine-slice scaling.

use cupertino_geometry::{Insets, NineSlice, Rect, Size};

fn slicer() -> NineSlice {
    NineSlice::new(Size::new(30.0, 30.0), Insets::uniform(10.0))
}

#[test]
fn corners_keep_their_scaled_size_and_the_centre_stretches() {
    let dest = Rect::new(100.0, 200.0, 90.0, 60.0);
    let s = slicer().slices(dest, 1.0);
    assert_eq!(s[0].dst, Rect::new(100.0, 200.0, 10.0, 10.0));
    assert_eq!(s[2].dst, Rect::new(180.0, 200.0, 10.0, 10.0));
    assert_eq!(s[6].dst, Rect::new(100.0, 250.0, 10.0, 10.0));
    assert_eq!(s[8].dst, Rect::new(180.0, 250.0, 10.0, 10.0));
    assert_eq!(s[4].dst, Rect::new(110.0, 210.0, 70.0, 40.0));
}

#[test]
fn source_regions_do_not_depend_on_the_destination() {
    let s = slicer().slices(Rect::new(0.0, 0.0, 90.0, 60.0), 1.0);
    assert_eq!(s[0].src, Rect::new(0.0, 0.0, 10.0, 10.0));
    assert_eq!(s[4].src, Rect::new(10.0, 10.0, 10.0, 10.0));
    assert_eq!(s[8].src, Rect::new(20.0, 20.0, 10.0, 10.0));
}

#[test]
fn slices_tile_the_destination_without_gaps() {
    let dest = Rect::new(5.0, 7.0, 90.0, 60.0);
    let total: f64 = slicer()
        .slices(dest, 1.0)
        .iter()
        .map(|s| s.dst.area())
        .sum();
    assert!((total - dest.area()).abs() < 1e-9);
}

#[test]
fn scale_multiplies_the_corner_size() {
    let s = slicer().slices(Rect::new(0.0, 0.0, 90.0, 90.0), 1.5);
    assert_eq!(s[0].dst, Rect::new(0.0, 0.0, 15.0, 15.0));
    assert_eq!(s[4].dst, Rect::new(15.0, 15.0, 60.0, 60.0));
}

#[test]
fn a_small_destination_shrinks_opposite_corners_proportionally() {
    let s = slicer().slices(Rect::new(0.0, 0.0, 10.0, 60.0), 1.0);
    assert_eq!(s[0].dst.width(), 5.0);
    assert_eq!(s[2].dst.width(), 5.0);
    assert_eq!(s[4].dst.width(), 0.0);
}

#[test]
fn min_size_is_the_scaled_inset_sum() {
    let n = NineSlice::new(Size::new(40.0, 30.0), Insets::new(4.0, 6.0, 8.0, 10.0));
    assert_eq!(n.min_size(1.0), Size::new(16.0, 12.0));
    assert_eq!(n.min_size(2.0), Size::new(32.0, 24.0));
}

#[test]
fn insets_larger_than_the_source_are_scaled_down() {
    let n = NineSlice::new(Size::new(20.0, 20.0), Insets::uniform(20.0));
    assert_eq!(n.min_size(1.0), Size::new(20.0, 20.0));
}

#[test]
fn negative_insets_count_as_zero() {
    let n = NineSlice::new(Size::new(20.0, 20.0), Insets::uniform(-4.0));
    assert_eq!(n.min_size(1.0), Size::ZERO);
}

#[test]
fn visible_slices_drop_the_empty_ones() {
    let three_part = NineSlice::new(Size::new(30.0, 10.0), Insets::new(0.0, 10.0, 0.0, 10.0));
    let visible = three_part.visible_slices(Rect::new(0.0, 0.0, 100.0, 10.0), 1.0);
    assert_eq!(visible.len(), 3);
    assert!(visible.iter().all(|s| !s.is_empty()));
}

#[test]
fn all_nine_slices_are_visible_for_a_roomy_destination() {
    assert_eq!(
        slicer()
            .visible_slices(Rect::new(0.0, 0.0, 90.0, 90.0), 1.0)
            .len(),
        9
    );
}

#[test]
fn uv_is_normalised_by_the_texture_size() {
    let s = slicer().slices(Rect::new(0.0, 0.0, 90.0, 90.0), 1.0);
    let uv = s[4].uv(Size::new(60.0, 30.0));
    assert!((uv[0] - 10.0 / 60.0).abs() < 1e-12);
    assert!((uv[1] - 10.0 / 30.0).abs() < 1e-12);
    assert!((uv[2] - 20.0 / 60.0).abs() < 1e-12);
    assert!((uv[3] - 20.0 / 30.0).abs() < 1e-12);
}
