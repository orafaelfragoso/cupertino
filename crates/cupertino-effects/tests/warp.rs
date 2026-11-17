//! The warp minimize effect slides and stretches a window into the dock icon along the dock's edge direction.

use cupertino_effects::warp::WarpGeometry;
use cupertino_effects::{
    subdivide, warp_mesh, DockEdge, Landing, WarpDirection, WarpParams, WarpTiming,
};
use cupertino_geometry::Rect;

const WINDOW: Rect = Rect::new(300.0, 100.0, 500.0, 400.0);

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-3
}

fn bounds_match(mesh_bounds: Rect, expected: Rect) -> bool {
    close(mesh_bounds.min_x(), expected.min_x())
        && close(mesh_bounds.min_y(), expected.min_y())
        && close(mesh_bounds.max_x(), expected.max_x())
        && close(mesh_bounds.max_y(), expected.max_y())
}

/// A slot in the dock for each direction, outside the window along the direction's axis.
fn slot_for(direction: WarpDirection) -> Rect {
    match direction {
        WarpDirection::Down => Rect::new(520.0, 900.0, 60.0, 60.0),
        WarpDirection::Up => Rect::new(520.0, 10.0, 60.0, 60.0),
        WarpDirection::Right => Rect::new(1500.0, 250.0, 60.0, 60.0),
        WarpDirection::Left => Rect::new(10.0, 250.0, 60.0, 60.0),
    }
}

const DIRECTIONS: [WarpDirection; 4] = [
    WarpDirection::Down,
    WarpDirection::Up,
    WarpDirection::Right,
    WarpDirection::Left,
];

#[test]
fn the_bottom_dock_warps_up_or_down_by_where_the_slot_is() {
    let below = Rect::new(500.0, 900.0, 60.0, 60.0);
    let above = Rect::new(500.0, 0.0, 60.0, 60.0);
    assert_eq!(
        WarpDirection::for_dock(DockEdge::Bottom, WINDOW, below),
        WarpDirection::Down
    );
    assert_eq!(
        WarpDirection::for_dock(DockEdge::Bottom, WINDOW, above),
        WarpDirection::Up
    );
    assert_eq!(
        WarpDirection::for_dock(DockEdge::Left, WINDOW, below),
        WarpDirection::Left
    );
    assert_eq!(
        WarpDirection::for_dock(DockEdge::Right, WINDOW, below),
        WarpDirection::Right
    );
    assert!(WarpDirection::Left.is_horizontal() && WarpDirection::Right.is_horizontal());
    assert!(!WarpDirection::Up.is_horizontal() && !WarpDirection::Down.is_horizontal());
}

#[test]
fn a_warp_at_the_start_is_the_window() {
    let params = WarpParams::default();
    for direction in DIRECTIONS {
        let mesh = warp_mesh(WINDOW, slot_for(direction), direction, 0.0, &params);
        assert!(bounds_match(mesh.bounds(), WINDOW), "{direction:?}");
        assert!(mesh.is_finite());
    }
}

#[test]
fn a_warp_at_the_end_is_the_dock_icon() {
    let params = WarpParams::default();
    for direction in DIRECTIONS {
        let slot = slot_for(direction);
        let mesh = warp_mesh(WINDOW, slot, direction, 1.0, &params);
        assert!(
            bounds_match(mesh.bounds(), slot),
            "{direction:?}: {:?}",
            mesh.bounds()
        );
    }
}

#[test]
fn the_mesh_shape_follows_the_direction() {
    let params = WarpParams::default();
    for direction in DIRECTIONS {
        let mesh = warp_mesh(WINDOW, slot_for(direction), direction, 0.5, &params);
        if direction.is_horizontal() {
            assert_eq!(mesh.rows, 2, "{direction:?}");
            assert!(mesh.cols > 2);
        } else {
            assert_eq!(mesh.cols, 2, "{direction:?}");
            assert!(mesh.rows > 2);
        }
        assert!(mesh.vertex_count() <= 2 * params.max_samples);
    }
}

#[test]
fn texture_coordinates_cover_the_window_at_every_progress() {
    let params = WarpParams::default();
    for direction in DIRECTIONS {
        for step in 0..=10 {
            let p = f64::from(step) / 10.0;
            let mesh = warp_mesh(WINDOW, slot_for(direction), direction, p, &params);
            let us: Vec<f32> = mesh.vertices.iter().map(|v| v.uv[0]).collect();
            let vs: Vec<f32> = mesh.vertices.iter().map(|v| v.uv[1]).collect();
            for axis in [us, vs] {
                let lo = axis.iter().copied().fold(f32::MAX, f32::min);
                let hi = axis.iter().copied().fold(f32::MIN, f32::max);
                assert_eq!((lo, hi), (0.0, 1.0), "{direction:?} at {p}");
            }
            assert!(mesh.is_finite());
        }
    }
}

#[test]
fn the_edge_landing_ends_at_the_window_edge_beside_the_icon() {
    let params = WarpParams {
        landing: Landing::IconEdge,
        ..WarpParams::default()
    };
    let slot = slot_for(WarpDirection::Down);
    let end = warp_mesh(WINDOW, slot, WarpDirection::Down, 1.0, &params);
    assert!(end.is_finite());
    assert!(close(end.bounds().min_y(), slot.min_y()));
    let start = warp_mesh(WINDOW, slot, WarpDirection::Down, 0.0, &params);
    assert!(bounds_match(start.bounds(), WINDOW));
}

#[test]
fn a_warp_outside_the_range_is_clamped() {
    let params = WarpParams::default();
    let slot = slot_for(WarpDirection::Down);
    assert_eq!(
        warp_mesh(WINDOW, slot, WarpDirection::Down, -2.0, &params),
        warp_mesh(WINDOW, slot, WarpDirection::Down, 0.0, &params)
    );
    assert_eq!(
        warp_mesh(WINDOW, slot, WarpDirection::Down, 9.0, &params),
        warp_mesh(WINDOW, slot, WarpDirection::Down, 1.0, &params)
    );
}

#[test]
fn the_geometry_measures_the_slide_and_the_sideways_offsets() {
    let slot = Rect::new(520.0, 900.0, 60.0, 60.0);
    let g = WarpGeometry::new(WINDOW, slot, WarpDirection::Down, &WarpParams::default());
    assert_eq!(g.length, 400.0);
    assert_eq!(g.width, 500.0);
    assert_eq!(
        g.slide, 400.0,
        "the icon starts 800 below the top; the window is 400 tall"
    );
    assert_eq!(g.dx_low, 220.0);
    assert_eq!(
        g.dx_high, -220.0,
        "the icon's right edge is 220 left of the window's"
    );
}

#[test]
fn a_slot_inside_the_window_needs_no_slide() {
    let slot = Rect::new(520.0, 300.0, 60.0, 60.0);
    let g = WarpGeometry::new(WINDOW, slot, WarpDirection::Down, &WarpParams::default());
    assert_eq!(g.slide, 0.0);
}

#[test]
fn the_timing_never_runs_faster_than_one_time_unit() {
    let params = WarpParams::default();
    let t = WarpTiming::compute(0.0, 0.0, 0.0, 400.0, &params);
    assert_eq!(t.slide, params.time_unit);
    assert_eq!(t.stretch, params.time_unit);
    assert!(t.delay >= 0.0);
    assert!(t.total() >= t.stretch);
}

#[test]
fn a_longer_trip_takes_longer() {
    let params = WarpParams::default();
    let short = WarpTiming::compute(100.0, 100.0, 100.0, 400.0, &params);
    let long = WarpTiming::compute(2000.0, 100.0, 2000.0, 400.0, &params);
    assert!(long.slide > short.slide);
    assert!(long.stretch > short.stretch);
}

#[test]
fn the_phases_run_from_nothing_to_everything() {
    let t = WarpTiming::compute(400.0, 300.0, 800.0, 400.0, &WarpParams::default());
    assert_eq!(t.phases(0.0), (0.0, 0.0));
    let (e1, e2) = t.phases(1.0);
    assert!((e1 - 1.0).abs() < 1e-12 && (e2 - 1.0).abs() < 1e-12);
    assert_eq!(t.phases(-1.0), t.phases(0.0));
    assert_eq!(t.phases(2.0), t.phases(1.0));
    let mut last = (0.0, 0.0);
    for i in 0..=100 {
        let now = t.phases(f64::from(i) / 100.0);
        assert!(now.0 >= last.0 && now.1 >= last.1, "never goes back");
        last = now;
    }
}

#[test]
fn the_stretch_phase_waits_for_the_slide_when_the_window_must_move_first() {
    let t = WarpTiming::compute(1000.0, 0.0, 800.0, 400.0, &WarpParams::default());
    assert!(t.delay > 0.0);
    let (_, early) = t.phases(t.delay / t.total() * 0.5);
    assert_eq!(early, 0.0);
}

#[test]
fn subdividing_always_spans_zero_to_one_in_ascending_order() {
    let params = WarpParams::default();
    for (extent, amplitude) in [(800.0, 400.0), (50.0, 5.0), (3000.0, 900.0), (0.0, 0.0)] {
        let cuts = subdivide(extent, amplitude, &params);
        assert_eq!(cuts[0], 0.0);
        assert_eq!(*cuts.last().unwrap(), 1.0);
        assert!(cuts.windows(2).all(|w| w[0] < w[1]), "{extent} {amplitude}");
    }
}

#[test]
fn a_straight_short_run_needs_no_extra_cuts_and_a_curved_long_one_does() {
    let params = WarpParams::default();
    assert_eq!(subdivide(100.0, 0.0, &params), [0.0, 1.0]);
    assert!(subdivide(800.0, 400.0, &params).len() > 8);
}

#[test]
fn the_cut_count_respects_the_sample_budget() {
    let params = WarpParams {
        max_samples: 16,
        ..WarpParams::default()
    };
    let cuts = subdivide(5000.0, 5000.0, &params);
    assert!(cuts.len() <= 16, "{}", cuts.len());
    let shallow = WarpParams {
        max_depth: 2,
        ..WarpParams::default()
    };
    assert!(subdivide(5000.0, 5000.0, &shallow).len() <= 5);
}

#[test]
fn negative_extents_and_amplitudes_cut_like_their_magnitudes() {
    let params = WarpParams::default();
    assert_eq!(
        subdivide(-800.0, -400.0, &params),
        subdivide(800.0, 400.0, &params)
    );
}
