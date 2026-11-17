//! Window open, close, sheet and pop transforms, frame changes, full screen, workspace slides, overview and show desktop.

use cupertino_effects::transitions::{
    scale_about_center, APPEARANCE_FADE_DURATION, FRAME_CHANGE_MAX_DURATION, POP_SCALE_AMOUNT,
    SCALE_DOWN_AND_OUT_SCALE, TRANSFORM_SLOW_MOTION_FACTOR, WINDOW_CLOSE_SCALE, WINDOW_OPEN_SCALE,
};
use cupertino_effects::{
    frame_change_duration, frame_max_delta, server_move_driver, Driver, Ease, FullScreenTransition,
    OverviewZoom, ShowDesktop, TilingSprings, TransformKind, TransformSample, WindowKind,
    WindowZoom, WorkspaceSlide, SERVER_MOVE_DURATION, SHEET_SLIDE,
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
    assert_eq!(
        WindowKind::Plain.order_in(),
        Some(TransformKind::WindowOpen)
    );
    assert_eq!(
        WindowKind::Document.order_in(),
        Some(TransformKind::WindowOpen)
    );
    assert_eq!(
        WindowKind::Utility.order_in(),
        Some(TransformKind::WindowOpen)
    );
    assert_eq!(WindowKind::Alert.order_in(), Some(TransformKind::PopIn));
    assert_eq!(WindowKind::AppModal.order_in(), Some(TransformKind::PopIn));
    assert_eq!(WindowKind::Sheet.order_in(), Some(TransformKind::SheetIn));
    assert_eq!(
        WindowKind::Plain.order_out(),
        Some(TransformKind::WindowClose)
    );
    assert_eq!(
        WindowKind::Utility.order_out(),
        Some(TransformKind::FadeOut)
    );
    assert_eq!(
        WindowKind::AppModal.order_out(),
        Some(TransformKind::FadeOut)
    );
    assert_eq!(
        WindowKind::Alert.order_out(),
        Some(TransformKind::ScaleDownAndOut)
    );
    assert_eq!(WindowKind::Sheet.order_out(), Some(TransformKind::SheetOut));
    assert_eq!(WindowKind::default(), WindowKind::Plain);
}

#[test]
fn every_transform_samples_finite_values_over_and_beyond_its_range() {
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
        assert_eq!(kind.sample(-5.0), kind.sample(0.0));
        assert_eq!(kind.sample(5.0), kind.sample(1.0));
        assert!(kind.duration() > 0.0);
    }
}

#[test]
fn coming_in_ends_fully_visible_and_going_out_ends_gone() {
    for kind in [
        TransformKind::ScaleUpAndIn,
        TransformKind::PopIn,
        TransformKind::SheetIn,
        TransformKind::WindowOpen,
    ] {
        let end = kind.sample(1.0);
        assert!(
            close(end.scale, 1.0) && close(end.alpha, 1.0) && close(end.dy, 0.0),
            "{kind:?}"
        );
    }
    for kind in [
        TransformKind::PopOut,
        TransformKind::ScaleDownAndOut,
        TransformKind::FadeOut,
        TransformKind::SheetOut,
        TransformKind::WindowClose,
        TransformKind::AppearanceFade,
    ] {
        assert!(
            close(kind.sample(0.0).alpha, 1.0),
            "{kind:?} starts visible"
        );
    }
    for kind in [
        TransformKind::ScaleDownAndOut,
        TransformKind::FadeOut,
        TransformKind::SheetOut,
        TransformKind::WindowClose,
        TransformKind::AppearanceFade,
    ] {
        assert!(close(kind.sample(1.0).alpha, 0.0), "{kind:?} ends gone");
    }
}

#[test]
fn a_window_opens_from_slightly_smaller_and_closes_to_slightly_smaller() {
    assert!(close(
        TransformKind::WindowOpen.sample(0.0).scale,
        WINDOW_OPEN_SCALE
    ));
    assert!(close(
        TransformKind::WindowClose.sample(1.0).scale,
        WINDOW_CLOSE_SCALE
    ));
    assert!(close(
        TransformKind::ScaleDownAndOut.sample(1.0).scale,
        SCALE_DOWN_AND_OUT_SCALE
    ));
    assert_eq!(
        TransformKind::ScaleUpAndIn.sample(0.0),
        TransformSample {
            scale: 0.0,
            alpha: 0.0,
            dy: 0.0
        }
    );
}

#[test]
fn a_pop_overshoots_and_settles() {
    let peak = (0..=100)
        .map(|i| TransformKind::PopIn.sample(f64::from(i) / 100.0).scale)
        .fold(0.0, f64::max);
    assert!(close(peak, 1.0 + POP_SCALE_AMOUNT), "{peak}");
    assert!(TransformKind::PopIn.sample(0.0).scale < 0.2);
    assert!(
        TransformKind::PopIn.sample(0.2).alpha >= 0.5,
        "fades in during the first part"
    );
    assert_eq!(TransformKind::PopIn.sample(0.5).alpha, 1.0);
}

#[test]
fn a_pop_out_is_a_pop_in_played_backwards() {
    for i in 0..=10 {
        let t = f64::from(i) / 10.0;
        let out = TransformKind::PopOut.sample(t);
        let back = TransformKind::PopIn.sample(1.0 - t);
        assert!(close(out.scale, back.scale) && close(out.alpha, back.alpha));
    }
}

#[test]
fn a_sheet_slides_up_out_of_its_parent_and_back() {
    assert!(close(TransformKind::SheetIn.sample(0.0).dy, -SHEET_SLIDE));
    assert!(close(TransformKind::SheetOut.sample(1.0).dy, -SHEET_SLIDE));
    assert!(close(TransformKind::SheetOut.sample(0.0).dy, 0.0));
    assert!(TransformKind::SheetIn.sample(0.0).alpha == 0.0);
    assert!(
        TransformKind::SheetIn.sample(0.5).alpha == 1.0,
        "fully opaque halfway"
    );
}

#[test]
fn a_window_openings_shadow_starts_part_way_and_other_shadows_follow_the_alpha() {
    let open = TransformKind::WindowOpen;
    assert!(open.shadow_factor(0.0) > 0.0 && open.shadow_factor(0.0) < 1.0);
    assert!(close(open.shadow_factor(1.0), 1.0));
    assert!(open.shadow_factor(0.1) < open.shadow_factor(0.9));
    assert_eq!(
        TransformKind::FadeOut.shadow_factor(0.3),
        TransformKind::FadeOut.sample(0.3).alpha
    );
    assert_eq!(
        TransformKind::WindowClose.shadow_factor(0.3),
        TransformKind::WindowClose.sample(0.3).alpha
    );
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
    assert_eq!(
        duration_of(TransformKind::AppearanceFade.driver(false)),
        APPEARANCE_FADE_DURATION
    );
}

#[test]
fn the_appearance_fade_has_its_own_curve() {
    let Driver::Timed { ease, .. } = TransformKind::AppearanceFade.driver(false) else {
        panic!("timed");
    };
    assert!(matches!(ease, Ease::Bezier(_)));
    let Driver::Timed { ease, .. } = TransformKind::FadeOut.driver(false) else {
        panic!("timed");
    };
    assert_eq!(ease, Ease::Cosine);
}

#[test]
fn an_animator_runs_a_transform_over_its_duration() {
    let kind = TransformKind::WindowOpen;
    let mut a = kind.animator(false);
    a.animate_to(1.0, 0.0);
    assert!(a.is_animating(kind.duration() * 0.5));
    assert!(!a.is_animating(kind.duration()));
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
    assert!(rect_close(
        TransformKind::PopIn.placement(rect, 1.0).rect,
        rect
    ));
    let mesh = p.mesh();
    assert_eq!(mesh.vertex_count(), 4);
}

#[test]
fn a_frame_change_lasts_longer_the_farther_it_moves_up_to_a_cap() {
    assert_eq!(frame_change_duration(0.0), 0.0);
    let (small, medium, large) = (
        frame_change_duration(10.0),
        frame_change_duration(100.0),
        frame_change_duration(500.0),
    );
    assert!(0.0 < small && small < medium && medium < large);
    assert_eq!(frame_change_duration(1e9), FRAME_CHANGE_MAX_DURATION);
    assert_eq!(frame_change_duration(-100.0), frame_change_duration(100.0));
}

#[test]
fn the_largest_change_among_position_and_size_decides() {
    let from = Rect::new(0.0, 0.0, 100.0, 100.0);
    assert_eq!(frame_max_delta(from, from), 0.0);
    assert_eq!(
        frame_max_delta(from, Rect::new(30.0, -50.0, 110.0, 100.0)),
        50.0
    );
    assert_eq!(
        frame_max_delta(from, Rect::new(0.0, 0.0, 100.0, 20.0)),
        80.0
    );
}

#[test]
fn a_window_zoom_interpolates_the_frame_without_fading() {
    let (from, to) = (
        Rect::new(0.0, 0.0, 100.0, 100.0),
        Rect::new(100.0, 0.0, 300.0, 100.0),
    );
    let zoom = WindowZoom;
    assert_eq!(zoom.duration(from, to), frame_change_duration(200.0));
    let p = zoom.placement(from, to, 0.5);
    assert!(rect_close(p.rect, Rect::new(50.0, 0.0, 200.0, 100.0)));
    assert_eq!(p.opacity, 1.0);
    assert_eq!(duration_of(zoom.driver(from, to)), zoom.duration(from, to));
    assert!(zoom.animator(from, to).target() == 0.0);
}

#[test]
fn full_screen_moves_with_one_curve_and_fades_in_with_another() {
    let t = FullScreenTransition::default();
    let (from, to) = (
        Rect::new(100.0, 100.0, 400.0, 300.0),
        Rect::new(0.0, 0.0, 1920.0, 1080.0),
    );
    let start = t.placement(from, to, 0.0);
    assert!(rect_close(start.rect, from));
    assert!(close(start.opacity, 0.0));
    let end = t.placement(from, to, 1.0);
    assert!(rect_close(end.rect, to));
    assert!(close(end.opacity, 1.0));
    let fade_done = t.placement(from, to, t.fade_fraction);
    assert!(
        close(fade_done.opacity, 1.0),
        "the fade is over by its fraction"
    );
    assert_eq!(t.placement(from, to, -3.0), start);
    assert_eq!(t.placement(from, to, 3.0), end);
    assert_eq!(duration_of(t.animator().driver()), t.duration);
}

#[test]
fn workspaces_sit_one_screen_and_a_gap_apart() {
    let slide = WorkspaceSlide::new(1920.0);
    assert_eq!(slide.pitch(), 1920.0 + slide.gap);
    assert_eq!(slide.offset_x(2, 2.0), 0.0);
    assert_eq!(slide.offset_x(3, 2.0), slide.pitch());
    assert_eq!(slide.offset_x(0, 1.5), -1.5 * slide.pitch());
    let rect = Rect::new(10.0, 20.0, 100.0, 100.0);
    let p = slide.placement(rect, 1, 0.0);
    assert!(rect_close(p.rect, rect.translate(slide.pitch(), 0.0)));
}

#[test]
fn only_the_workspaces_that_reach_the_screen_are_visible() {
    let slide = WorkspaceSlide::new(1000.0);
    assert!(slide.is_visible(0, 0.0));
    assert!(!slide.is_visible(1, 0.0), "a neighbour sits behind the gap");
    assert!(!slide.is_visible(0, 1.0));
    assert!(slide.is_visible(0, 0.5) && slide.is_visible(1, 0.5));
    assert!(slide.is_visible(0, 0.9) && !slide.is_visible(0, 1.0));
    assert!(!slide.is_visible(5, 0.0));
}

#[test]
fn the_server_move_is_a_timed_ease() {
    let Driver::Timed { duration, ease } = server_move_driver() else {
        panic!("timed");
    };
    assert_eq!(duration, SERVER_MOVE_DURATION);
    assert!(matches!(ease, Ease::Bezier(_)));
}

#[test]
fn tiling_uses_springs_for_every_property() {
    let s = TilingSprings::default();
    for driver in [s.frame, s.alpha, s.corner_radius, s.quick] {
        assert!(matches!(driver, Driver::Spring(_)));
        assert!(driver.settle_time() > 0.0);
    }
    assert!(
        s.quick.settle_time() < s.frame.settle_time(),
        "the quick spring is the stiffer one"
    );
}

#[test]
fn the_overview_zoom_runs_longer_in_slow_motion_and_clamps_progress() {
    let z = OverviewZoom::default();
    assert!(duration_of(z.driver(true)) > duration_of(z.driver(false)));
    let (window, slot) = (
        Rect::new(0.0, 0.0, 800.0, 600.0),
        Rect::new(100.0, 100.0, 200.0, 150.0),
    );
    assert!(rect_close(z.placement(window, slot, 0.0).rect, window));
    assert!(rect_close(z.placement(window, slot, 1.0).rect, slot));
    assert!(rect_close(z.placement(window, slot, 7.0).rect, slot));
    assert!(rect_close(z.placement(window, slot, -7.0).rect, window));
    let a = z.animator(false, 1.0);
    assert_eq!(a.value(0.0), 1.0);
}

#[test]
fn show_desktop_slides_a_window_off_by_the_nearest_horizontal_edge() {
    let screen = Rect::new(0.0, 0.0, 1920.0, 1080.0);
    let sd = ShowDesktop::default();
    let left_window = Rect::new(100.0, 200.0, 400.0, 300.0);
    assert!(rect_close(
        sd.hidden_rect(left_window, screen),
        left_window.translate(-500.0, 0.0)
    ));
    let right_window = Rect::new(1400.0, 200.0, 400.0, 300.0);
    assert!(rect_close(
        sd.hidden_rect(right_window, screen),
        right_window.translate(1920.0 - 1400.0, 0.0)
    ));
}

#[test]
fn show_desktop_may_leave_a_sliver_and_may_go_vertical() {
    let screen = Rect::new(0.0, 0.0, 1920.0, 1080.0);
    let window = Rect::new(800.0, 20.0, 400.0, 300.0);
    let peek = ShowDesktop {
        peek: 40.0,
        ..ShowDesktop::default()
    };
    let hidden = peek.hidden_rect(window, screen);
    assert!(close(hidden.min_x(), 1920.0 - 40.0), "{hidden:?}");
    let vertical = ShowDesktop {
        allow_vertical: true,
        ..ShowDesktop::default()
    };
    let up = vertical.hidden_rect(window, screen);
    assert!(close(up.max_y(), 0.0), "{up:?}");
    assert_eq!(up.x(), window.x());
}

#[test]
fn show_desktop_placement_runs_between_the_window_and_its_hidden_place() {
    let screen = Rect::new(0.0, 0.0, 1920.0, 1080.0);
    let sd = ShowDesktop::default();
    let window = Rect::new(100.0, 200.0, 400.0, 300.0);
    assert!(rect_close(sd.placement(window, screen, 0.0).rect, window));
    assert!(rect_close(
        sd.placement(window, screen, 1.0).rect,
        sd.hidden_rect(window, screen)
    ));
    assert!(rect_close(
        sd.placement(window, screen, 0.5).rect,
        window.translate(-250.0, 0.0)
    ));
    assert_eq!(sd.animator(0.25).value(0.0), 0.25);
}

#[test]
fn an_off_screen_window_is_not_pulled_back_by_show_desktop() {
    let screen = Rect::new(0.0, 0.0, 1920.0, 1080.0);
    let window = Rect::new(-600.0, 0.0, 400.0, 300.0);
    let hidden = ShowDesktop::default().hidden_rect(window, screen);
    assert_eq!(hidden, window, "already clear of the screen, it stays put");
}
