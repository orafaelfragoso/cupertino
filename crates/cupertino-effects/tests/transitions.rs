//! Window open, close, sheet and pop transforms, frame changes, full screen, workspace slides, overview and show desktop.

use cupertino_effects::transitions::{
    scale_about_center, FRAME_CHANGE_MAX_DURATION, POP_SCALE_AMOUNT, SCALE_DOWN_AND_OUT_SCALE,
    TRANSFORM_SLOW_MOTION_FACTOR, WINDOW_CLOSE_SCALE, WINDOW_OPEN_SCALE,
};
use cupertino_effects::{
    frame_change_duration, frame_max_delta, Driver, FullScreenTransition, OverviewZoom,
    ShowDesktop, TransformKind, WindowKind, WindowZoom, WorkspaceSlide, SHEET_SLIDE,
};
use cupertino_geometry::Rect;

const ALL: [TransformKind; 10] = [
    TransformKind::ScaleUpAndIn,
    TransformKind::PopIn,
    TransformKind::PopOut,
    TransformKind::ScaleDownAndOut,
    TransformKind::FadeOut,
    TransformKind::SheetIn,
    TransformKind::SheetOut,
    TransformKind::WindowOpen,
    TransformKind::WindowClose,
    TransformKind::AppearanceFade,
];

const SCREEN: Rect = Rect::new(0.0, 0.0, 1920.0, 1080.0);

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

fn rect_close(a: Rect, b: Rect) -> bool {
    close(a.x(), b.x())
        && close(a.y(), b.y())
        && close(a.width(), b.width())
        && close(a.height(), b.height())
}

fn duration_of(driver: Driver) -> f64 {
    match driver {
        Driver::Timed { duration, .. } => duration,
        Driver::Spring(_) => panic!("a timed driver"),
    }
}

#[test]
fn windows_order_in_and_out_by_kind() {
    use TransformKind::*;
    for (kind, order_in, order_out) in [
        (WindowKind::Plain, WindowOpen, WindowClose),
        (WindowKind::Document, WindowOpen, WindowClose),
        (WindowKind::Utility, WindowOpen, FadeOut),
        (WindowKind::Alert, PopIn, ScaleDownAndOut),
        (WindowKind::AppModal, PopIn, FadeOut),
        (WindowKind::Sheet, SheetIn, SheetOut),
    ] {
        assert_eq!(
            (kind.order_in(), kind.order_out()),
            (Some(order_in), Some(order_out)),
            "{kind:?}"
        );
    }
}

#[test]
fn every_transform_samples_finite_clamped_values() {
    for kind in ALL {
        for step in -2..=12 {
            let s = kind.sample(f64::from(step) / 10.0);
            assert!(s.scale.is_finite() && s.alpha.is_finite() && s.dy.is_finite());
            assert!(
                (0.0..=1.0 + 1e-9).contains(&s.alpha),
                "{kind:?}: {}",
                s.alpha
            );
            assert!(s.scale > 0.0 || kind == TransformKind::ScaleUpAndIn);
        }
        assert_eq!(kind.sample(-5.0), kind.sample(0.0), "{kind:?}");
        assert_eq!(kind.sample(5.0), kind.sample(1.0), "{kind:?}");
        assert!(kind.duration() > 0.0, "{kind:?}");
    }
}

#[test]
fn each_transform_starts_and_ends_where_its_window_appears_or_goes() {
    use TransformKind::*;
    // (kind, progress, scale, alpha, dy); None leaves that value unchecked.
    for (kind, t, scale, alpha, dy) in [
        (ScaleUpAndIn, 0.0, Some(0.0), Some(0.0), Some(0.0)),
        (ScaleUpAndIn, 1.0, Some(1.0), Some(1.0), Some(0.0)),
        (PopIn, 1.0, Some(1.0), Some(1.0), Some(0.0)),
        (SheetIn, 0.0, None, Some(0.0), Some(-SHEET_SLIDE)),
        (SheetIn, 0.5, None, Some(1.0), None),
        (SheetIn, 1.0, Some(1.0), Some(1.0), Some(0.0)),
        (WindowOpen, 0.0, Some(WINDOW_OPEN_SCALE), None, None),
        (WindowOpen, 1.0, Some(1.0), Some(1.0), Some(0.0)),
        (PopOut, 0.0, None, Some(1.0), None),
        (ScaleDownAndOut, 0.0, None, Some(1.0), None),
        (
            ScaleDownAndOut,
            1.0,
            Some(SCALE_DOWN_AND_OUT_SCALE),
            Some(0.0),
            None,
        ),
        (FadeOut, 0.0, None, Some(1.0), None),
        (FadeOut, 1.0, None, Some(0.0), None),
        (SheetOut, 0.0, None, Some(1.0), Some(0.0)),
        (SheetOut, 1.0, None, Some(0.0), Some(-SHEET_SLIDE)),
        (WindowClose, 0.0, None, Some(1.0), None),
        (WindowClose, 1.0, Some(WINDOW_CLOSE_SCALE), Some(0.0), None),
        (AppearanceFade, 0.0, None, Some(1.0), None),
        (AppearanceFade, 1.0, None, Some(0.0), None),
    ] {
        let s = kind.sample(t);
        for (what, expected, got) in [
            ("scale", scale, s.scale),
            ("alpha", alpha, s.alpha),
            ("dy", dy, s.dy),
        ] {
            if let Some(expected) = expected {
                assert!(close(got, expected), "{kind:?} at {t}: {what} {got}");
            }
        }
    }
}

#[test]
fn a_pop_overshoots_and_settles_and_a_pop_out_plays_it_backwards() {
    let pop = TransformKind::PopIn;
    let peak = (0..=100)
        .map(|i| pop.sample(f64::from(i) / 100.0).scale)
        .fold(0.0, f64::max);
    assert!(close(peak, 1.0 + POP_SCALE_AMOUNT), "{peak}");
    assert!(pop.sample(0.0).scale < 0.2);
    assert!(
        pop.sample(0.2).alpha >= 0.5,
        "fades in during the first part"
    );
    assert_eq!(pop.sample(0.5).alpha, 1.0);
    for i in 0..=10 {
        let t = f64::from(i) / 10.0;
        let out = TransformKind::PopOut.sample(t);
        let back = pop.sample(1.0 - t);
        assert!(
            close(out.scale, back.scale) && close(out.alpha, back.alpha),
            "{t}"
        );
    }
}

#[test]
fn a_window_openings_shadow_starts_part_way_and_other_shadows_follow_the_alpha() {
    let open = TransformKind::WindowOpen;
    assert!(open.shadow_factor(0.0) > 0.0 && open.shadow_factor(0.0) < 1.0);
    assert!(close(open.shadow_factor(1.0), 1.0));
    assert!(open.shadow_factor(0.1) < open.shadow_factor(0.9));
    for kind in [TransformKind::FadeOut, TransformKind::WindowClose] {
        assert_eq!(kind.shadow_factor(0.3), kind.sample(0.3).alpha, "{kind:?}");
    }
}

#[test]
fn slow_motion_stretches_only_the_cosine_driven_transforms() {
    for kind in [
        TransformKind::PopIn,
        TransformKind::SheetIn,
        TransformKind::WindowOpen,
        TransformKind::WindowClose,
    ] {
        assert_eq!(duration_of(kind.driver(true)), kind.duration(), "{kind:?}");
    }
    let normal = duration_of(TransformKind::FadeOut.driver(false));
    let slow = duration_of(TransformKind::FadeOut.driver(true));
    assert_eq!(slow, normal * TRANSFORM_SLOW_MOTION_FACTOR);

    let kind = TransformKind::WindowOpen;
    let mut a = kind.animator(false);
    a.animate_to(1.0, 0.0);
    assert!(a.is_animating(kind.duration() * 0.5));
    assert!(
        !a.is_animating(kind.duration()),
        "the animator runs its duration"
    );
}

#[test]
fn a_placement_scales_about_the_centre_and_shifts_by_the_slide() {
    let rect = Rect::new(100.0, 200.0, 400.0, 300.0);
    let scaled = scale_about_center(rect, 0.5);
    assert!(rect_close(scaled, Rect::new(200.0, 275.0, 200.0, 150.0)));
    assert_eq!(scaled.center(), rect.center());
    let p = TransformKind::SheetIn.placement(rect, 0.0);
    assert!(rect_close(p.rect, rect.translate(0.0, -SHEET_SLIDE)));
    assert_eq!(p.opacity, 0.0);
    assert_eq!(p.mesh().vertex_count(), 4);
    assert!(rect_close(
        TransformKind::PopIn.placement(rect, 1.0).rect,
        rect
    ));
}

#[test]
fn a_frame_change_lasts_longer_the_more_it_changes_up_to_a_cap() {
    assert_eq!(frame_change_duration(0.0), 0.0);
    let (small, medium, large) = (
        frame_change_duration(10.0),
        frame_change_duration(100.0),
        frame_change_duration(500.0),
    );
    assert!(0.0 < small && small < medium && medium < large);
    assert_eq!(frame_change_duration(1e9), FRAME_CHANGE_MAX_DURATION);
    assert_eq!(frame_change_duration(-100.0), frame_change_duration(100.0));

    let from = Rect::new(0.0, 0.0, 100.0, 100.0);
    for (to, delta) in [
        (from, 0.0),
        (Rect::new(30.0, -50.0, 110.0, 100.0), 50.0),
        (Rect::new(0.0, 0.0, 100.0, 20.0), 80.0),
    ] {
        assert_eq!(frame_max_delta(from, to), delta, "{to:?}");
    }
}

#[test]
fn a_window_zoom_interpolates_the_frame_without_fading() {
    let (from, to) = (
        Rect::new(0.0, 0.0, 100.0, 100.0),
        Rect::new(100.0, 0.0, 300.0, 100.0),
    );
    let zoom = WindowZoom;
    assert_eq!(zoom.duration(from, to), frame_change_duration(200.0));
    assert_eq!(duration_of(zoom.driver(from, to)), zoom.duration(from, to));
    let p = zoom.placement(from, to, 0.5);
    assert!(rect_close(p.rect, Rect::new(50.0, 0.0, 200.0, 100.0)));
    assert_eq!(p.opacity, 1.0);
    assert_eq!(zoom.animator(from, to).target(), 0.0);
}

#[test]
fn full_screen_moves_with_one_curve_and_fades_in_with_another() {
    let t = FullScreenTransition::default();
    let from = Rect::new(100.0, 100.0, 400.0, 300.0);
    let start = t.placement(from, SCREEN, 0.0);
    assert!(rect_close(start.rect, from));
    assert!(close(start.opacity, 0.0));
    let end = t.placement(from, SCREEN, 1.0);
    assert!(rect_close(end.rect, SCREEN));
    assert!(close(end.opacity, 1.0));
    assert!(
        close(t.placement(from, SCREEN, t.fade_fraction).opacity, 1.0),
        "the fade is over by its fraction"
    );
    assert_eq!(t.placement(from, SCREEN, -3.0), start);
    assert_eq!(t.placement(from, SCREEN, 3.0), end);
    assert_eq!(duration_of(t.animator().driver()), t.duration);
}

#[test]
fn workspaces_sit_one_screen_and_a_gap_apart() {
    let slide = WorkspaceSlide::new(1920.0);
    assert_eq!(slide.pitch(), 1920.0 + slide.gap);
    for (index, position, offset) in [
        (2, 2.0, 0.0),
        (3, 2.0, slide.pitch()),
        (0, 1.5, -1.5 * slide.pitch()),
    ] {
        assert_eq!(
            slide.offset_x(index, position),
            offset,
            "{index} at {position}"
        );
    }
    let rect = Rect::new(10.0, 20.0, 100.0, 100.0);
    let p = slide.placement(rect, 1, 0.0);
    assert!(rect_close(p.rect, rect.translate(slide.pitch(), 0.0)));
}

#[test]
fn only_the_workspaces_that_reach_the_screen_are_visible() {
    let slide = WorkspaceSlide::new(1000.0);
    for (index, position, visible) in [
        (0, 0.0, true),
        (1, 0.0, false),
        (0, 1.0, false),
        (0, 0.5, true),
        (1, 0.5, true),
        (0, 0.9, true),
        (5, 0.0, false),
    ] {
        assert_eq!(
            slide.is_visible(index, position),
            visible,
            "{index} at {position}"
        );
    }
}

#[test]
fn the_overview_zoom_runs_longer_in_slow_motion_and_clamps_progress() {
    let z = OverviewZoom::default();
    assert!(duration_of(z.driver(true)) > duration_of(z.driver(false)));
    let (window, slot) = (
        Rect::new(0.0, 0.0, 800.0, 600.0),
        Rect::new(100.0, 100.0, 200.0, 150.0),
    );
    for (progress, expected) in [(-7.0, window), (0.0, window), (1.0, slot), (7.0, slot)] {
        assert!(
            rect_close(z.placement(window, slot, progress).rect, expected),
            "{progress}"
        );
    }
    assert_eq!(z.animator(false, 1.0).value(0.0), 1.0);
}

#[test]
fn show_desktop_hides_a_window_past_the_nearest_edge() {
    let middle = Rect::new(800.0, 20.0, 400.0, 300.0);
    let peek = ShowDesktop {
        peek: 40.0,
        ..ShowDesktop::default()
    };
    let vertical = ShowDesktop {
        allow_vertical: true,
        ..ShowDesktop::default()
    };
    let left = Rect::new(100.0, 200.0, 400.0, 300.0);
    let right = Rect::new(1400.0, 200.0, 400.0, 300.0);
    let off_screen = Rect::new(-600.0, 0.0, 400.0, 300.0);
    for (name, show, window, hidden) in [
        (
            "left half",
            ShowDesktop::default(),
            left,
            left.translate(-500.0, 0.0),
        ),
        (
            "right half",
            ShowDesktop::default(),
            right,
            right.translate(1920.0 - 1400.0, 0.0),
        ),
        (
            "with a sliver left",
            peek,
            middle,
            middle.translate(1920.0 - 40.0 - 800.0, 0.0),
        ),
        (
            "up, when vertical is allowed",
            vertical,
            middle,
            middle.translate(0.0, -320.0),
        ),
        (
            "already off screen",
            ShowDesktop::default(),
            off_screen,
            off_screen,
        ),
    ] {
        let got = show.hidden_rect(window, SCREEN);
        assert!(rect_close(got, hidden), "{name}: {got:?}");
    }
}

#[test]
fn show_desktop_placement_runs_between_the_window_and_its_hidden_place() {
    let sd = ShowDesktop::default();
    let window = Rect::new(100.0, 200.0, 400.0, 300.0);
    for (progress, expected) in [
        (0.0, window),
        (0.5, window.translate(-250.0, 0.0)),
        (1.0, sd.hidden_rect(window, SCREEN)),
    ] {
        assert!(
            rect_close(sd.placement(window, SCREEN, progress).rect, expected),
            "{progress}"
        );
    }
    assert_eq!(sd.animator(0.25).value(0.0), 0.25);
}
