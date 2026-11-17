//! Where dialogs, sheets and zoomed windows go: the pure placement functions.

use cupertino_wm::Size;
use cupertino_wm::{
    best_fit, child_frame, kept_inside, modal_panel_frame, sheet_frame, window_kind, Rect,
    SizeHints, WindowKind, ZoomConfig, MODAL_CASCADE,
};

const USABLE: Rect = Rect::new(0.0, 32.0, 1920.0, 1048.0);
const SCREEN: Rect = Rect::new(0.0, 0.0, 1920.0, 1080.0);

#[test]
fn modality_wins_over_the_utility_role() {
    assert_eq!(window_kind(true, true), WindowKind::AppModal);
    assert_eq!(window_kind(true, false), WindowKind::AppModal);
    assert_eq!(window_kind(false, true), WindowKind::Utility);
    assert_eq!(window_kind(false, false), WindowKind::Plain);
}

#[test]
fn a_sheet_is_centred_on_its_parent_and_hangs_from_the_title_bar() {
    let parent = Rect::new(100.0, 100.0, 800.0, 600.0);
    let f = sheet_frame(parent, Size::new(400.0, 200.0), 28.0);
    assert_eq!(f, Rect::new(300.0, 128.0, 400.0, 200.0));
}

#[test]
fn a_sheet_with_an_odd_width_difference_snaps_to_whole_points() {
    let parent = Rect::new(0.0, 0.0, 801.0, 600.0);
    let f = sheet_frame(parent, Size::new(400.0, 100.0), 0.0);
    assert_eq!(f.x.fract(), 0.0);
    assert!((f.x - 200.0).abs() <= 1.0);
}

#[test]
fn a_frame_is_moved_the_least_to_fit_and_pinned_to_the_corner_when_too_large() {
    let inside = Rect::new(100.0, 100.0, 300.0, 200.0);
    assert_eq!(kept_inside(inside, USABLE), inside);
    let off_right = Rect::new(1800.0, 100.0, 300.0, 200.0);
    assert_eq!(
        kept_inside(off_right, USABLE),
        Rect::new(1620.0, 100.0, 300.0, 200.0)
    );
    let off_top_left = Rect::new(-50.0, 0.0, 300.0, 200.0);
    assert_eq!(
        kept_inside(off_top_left, USABLE),
        Rect::new(0.0, 32.0, 300.0, 200.0)
    );
    let huge = Rect::new(500.0, 500.0, 3000.0, 2000.0);
    let k = kept_inside(huge, USABLE);
    assert_eq!(
        (k.x, k.y),
        (USABLE.x, USABLE.y),
        "larger than the area: top-left corner"
    );
}

#[test]
fn the_first_modal_panel_opens_centred_a_quarter_of_the_free_height_down() {
    let f = modal_panel_frame(USABLE, SCREEN, Size::new(400.0, 248.0), None);
    assert_eq!(f.x, 760.0);
    assert_eq!(f.y, 32.0 + (0.25 * (1048.0 - 248.0)));
    assert_eq!((f.w, f.h), (400.0, 248.0));
}

#[test]
fn a_second_modal_panel_cascades_from_the_first_and_stays_on_the_screen() {
    let size = Size::new(400.0, 300.0);
    let f = modal_panel_frame(USABLE, SCREEN, size, Some((700.0, 300.0)));
    assert_eq!(
        (f.x, f.y),
        (700.0 + MODAL_CASCADE.0, 300.0 + MODAL_CASCADE.1)
    );
    let f = modal_panel_frame(USABLE, SCREEN, size, Some((1900.0, 1000.0)));
    assert_eq!((f.x, f.y), (1520.0, 780.0));
}

#[test]
fn a_child_window_centres_on_its_parent_and_stays_inside_the_usable_area() {
    let size = Size::new(200.0, 100.0);
    let parent = Rect::new(400.0, 300.0, 600.0, 400.0);
    assert_eq!(
        child_frame(parent, size, USABLE),
        Rect::new(600.0, 450.0, 200.0, 100.0)
    );
    let corner = Rect::new(0.0, 32.0, 50.0, 50.0);
    let f = child_frame(corner, size, USABLE);
    assert!(f.x >= USABLE.x && f.y >= USABLE.y);
    let far = Rect::new(1900.0, 1070.0, 20.0, 10.0);
    let f = child_frame(far, size, USABLE);
    assert!(f.right() <= USABLE.right() && f.bottom() <= USABLE.bottom());
}

#[test]
fn without_hints_zoom_fills_the_usable_area() {
    let cfg = ZoomConfig::default();
    let current = Rect::new(300.0, 300.0, 500.0, 400.0);
    assert_eq!(
        best_fit(&cfg, USABLE, current, &SizeHints::default()),
        USABLE
    );
}

#[test]
fn zoom_uses_the_content_hint_and_stays_centred_on_the_window() {
    let cfg = ZoomConfig::default();
    let current = Rect::new(300.0, 300.0, 500.0, 400.0);
    let hints = SizeHints {
        content: Some(Size::new(900.0, 600.0)),
        ..SizeHints::default()
    };
    assert_eq!(
        best_fit(&cfg, USABLE, current, &hints),
        Rect::new(100.0, 200.0, 900.0, 600.0)
    );
}

#[test]
fn zoom_is_translated_into_the_area_when_the_centre_would_push_it_out() {
    let cfg = ZoomConfig::default();
    let current = Rect::new(1700.0, 900.0, 200.0, 150.0);
    let hints = SizeHints {
        content: Some(Size::new(900.0, 600.0)),
        ..SizeHints::default()
    };
    let r = best_fit(&cfg, USABLE, current, &hints);
    assert_eq!((r.w, r.h), (900.0, 600.0));
    assert_eq!(r.right(), USABLE.right());
    assert_eq!(r.bottom(), USABLE.bottom());
}

#[test]
fn zoom_respects_the_maximum_size_but_a_minimum_wins_over_the_area() {
    let cfg = ZoomConfig::default();
    let current = Rect::new(300.0, 300.0, 500.0, 400.0);
    let capped = SizeHints {
        max: Some(Size::new(1000.0, 0.0)),
        ..SizeHints::default()
    };
    let r = best_fit(&cfg, USABLE, current, &capped);
    assert_eq!(
        (r.w, r.h),
        (1000.0, 1048.0),
        "a zero maximum axis means unlimited"
    );

    let big_min = SizeHints {
        min: Some(Size::new(2400.0, 500.0)),
        ..SizeHints::default()
    };
    let r = best_fit(&cfg, USABLE, current, &big_min);
    assert_eq!(r.w, 2400.0, "a minimum larger than the area overflows");
}

#[test]
fn zoom_margin_and_fill_fraction_shrink_the_result() {
    let cfg = ZoomConfig {
        margin: 20.0,
        default_fill: 0.5,
    };
    let current = Rect::new(960.0, 556.0, 10.0, 10.0);
    let r = best_fit(&cfg, USABLE, current, &SizeHints::default());
    assert_eq!((r.w, r.h), (940.0, 504.0));
    assert!(r.x >= USABLE.x + 20.0 && r.right() <= USABLE.right() - 20.0);
}
