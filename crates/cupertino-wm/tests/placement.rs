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
    for (modal, utility, kind) in [
        (true, true, WindowKind::AppModal),
        (true, false, WindowKind::AppModal),
        (false, true, WindowKind::Utility),
        (false, false, WindowKind::Plain),
    ] {
        assert_eq!(window_kind(modal, utility), kind, "{modal} {utility}");
    }
}

#[test]
fn a_sheet_is_centred_on_its_parent_on_whole_points_and_hangs_from_the_title_bar() {
    let parent = Rect::new(100.0, 100.0, 800.0, 600.0);
    assert_eq!(
        sheet_frame(parent, Size::new(400.0, 200.0), 28.0),
        Rect::new(300.0, 128.0, 400.0, 200.0)
    );
    let odd = sheet_frame(
        Rect::new(0.0, 0.0, 801.0, 600.0),
        Size::new(400.0, 100.0),
        0.0,
    );
    assert_eq!(odd.x.fract(), 0.0);
    assert!((odd.x - 200.0).abs() <= 1.0);
}

#[test]
fn a_frame_is_moved_the_least_to_fit_and_pinned_to_the_corner_when_too_large() {
    for (name, frame, expected) in [
        (
            "inside",
            Rect::new(100.0, 100.0, 300.0, 200.0),
            Rect::new(100.0, 100.0, 300.0, 200.0),
        ),
        (
            "off the right",
            Rect::new(1800.0, 100.0, 300.0, 200.0),
            Rect::new(1620.0, 100.0, 300.0, 200.0),
        ),
        (
            "off the top left",
            Rect::new(-50.0, 0.0, 300.0, 200.0),
            Rect::new(0.0, 32.0, 300.0, 200.0),
        ),
    ] {
        assert_eq!(kept_inside(frame, USABLE), expected, "{name}");
    }
    let k = kept_inside(Rect::new(500.0, 500.0, 3000.0, 2000.0), USABLE);
    assert_eq!((k.x, k.y), (USABLE.x, USABLE.y), "larger than the area");
}

#[test]
fn modal_panels_open_centred_a_quarter_down_then_cascade_on_the_screen() {
    let first = modal_panel_frame(USABLE, SCREEN, Size::new(400.0, 248.0), None);
    assert_eq!(
        first,
        Rect::new(760.0, 32.0 + 0.25 * (1048.0 - 248.0), 400.0, 248.0)
    );
    let size = Size::new(400.0, 300.0);
    for (previous, expected) in [
        (
            (700.0, 300.0),
            (700.0 + MODAL_CASCADE.0, 300.0 + MODAL_CASCADE.1),
        ),
        ((1900.0, 1000.0), (1520.0, 780.0)),
    ] {
        let f = modal_panel_frame(USABLE, SCREEN, size, Some(previous));
        assert_eq!((f.x, f.y), expected, "after {previous:?}");
    }
}

#[test]
fn a_child_window_centres_on_its_parent_and_stays_inside_the_usable_area() {
    let size = Size::new(200.0, 100.0);
    assert_eq!(
        child_frame(Rect::new(400.0, 300.0, 600.0, 400.0), size, USABLE),
        Rect::new(600.0, 450.0, 200.0, 100.0)
    );
    for parent in [
        Rect::new(0.0, 32.0, 50.0, 50.0),
        Rect::new(1900.0, 1070.0, 20.0, 10.0),
    ] {
        let f = child_frame(parent, size, USABLE);
        assert!(
            f.x >= USABLE.x
                && f.y >= USABLE.y
                && f.right() <= USABLE.right()
                && f.bottom() <= USABLE.bottom(),
            "{parent:?}: {f:?}"
        );
    }
}

#[test]
fn zoom_finds_the_best_fit_for_the_hints_inside_the_area() {
    let cfg = ZoomConfig::default();
    let current = Rect::new(300.0, 300.0, 500.0, 400.0);
    let content = || SizeHints {
        content: Some(Size::new(900.0, 600.0)),
        ..SizeHints::default()
    };
    // A zero maximum axis means unlimited.
    let capped = SizeHints {
        max: Some(Size::new(1000.0, 0.0)),
        ..SizeHints::default()
    };
    // A minimum larger than the area overflows it.
    let big_min = SizeHints {
        min: Some(Size::new(2400.0, 500.0)),
        ..SizeHints::default()
    };
    // (name, hints, width, height, origin); None leaves that value unchecked.
    for (name, hints, w, h, origin) in [
        (
            "no hints fill the area",
            SizeHints::default(),
            USABLE.w,
            Some(USABLE.h),
            Some((USABLE.x, USABLE.y)),
        ),
        (
            "the content hint, centred on the window",
            content(),
            900.0,
            Some(600.0),
            Some((100.0, 200.0)),
        ),
        ("a maximum", capped, 1000.0, Some(1048.0), None),
        ("a minimum wins over the area", big_min, 2400.0, None, None),
    ] {
        let r = best_fit(&cfg, USABLE, current, &hints);
        assert_eq!(r.w, w, "{name}");
        if let Some(h) = h {
            assert_eq!(r.h, h, "{name}");
        }
        if let Some(origin) = origin {
            assert_eq!((r.x, r.y), origin, "{name}");
        }
    }
    let near_corner = Rect::new(1700.0, 900.0, 200.0, 150.0);
    let r = best_fit(&cfg, USABLE, near_corner, &content());
    assert_eq!(
        (r.w, r.h, r.right(), r.bottom()),
        (900.0, 600.0, USABLE.right(), USABLE.bottom()),
        "translated into the area when the centre would push it out"
    );
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
