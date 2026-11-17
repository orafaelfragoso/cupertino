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
fn the_direction_follows_the_dock_edge_and_for_a_bottom_dock_where_the_slot_is() {
    let below = Rect::new(500.0, 900.0, 60.0, 60.0);
    let above = Rect::new(500.0, 0.0, 60.0, 60.0);
    for (edge, slot, direction) in [
        (DockEdge::Bottom, below, WarpDirection::Down),
        (DockEdge::Bottom, above, WarpDirection::Up),
        (DockEdge::Left, below, WarpDirection::Left),
        (DockEdge::Right, below, WarpDirection::Right),
    ] {
        assert_eq!(
            WarpDirection::for_dock(edge, WINDOW, slot),
            direction,
            "{edge:?} {slot:?}"
        );
        assert_eq!(
            direction.is_horizontal(),
            matches!(direction, WarpDirection::Left | WarpDirection::Right)
        );
    }
}

#[test]
fn a_warp_runs_from_the_window_to_the_dock_icon_and_clamps_its_progress() {
    let params = WarpParams::default();
    for direction in DIRECTIONS {
        let slot = slot_for(direction);
        for (progress, bounds) in [(-2.0, WINDOW), (0.0, WINDOW), (1.0, slot), (9.0, slot)] {
            let mesh = warp_mesh(WINDOW, slot, direction, progress, &params);
            assert!(
                bounds_match(mesh.bounds(), bounds),
                "{direction:?} at {progress}: {:?}",
                mesh.bounds()
            );
        }
    }
}

#[test]
fn the_mesh_is_cut_along_the_direction_and_covers_the_whole_texture() {
    let params = WarpParams::default();
    for direction in DIRECTIONS {
        let mid = warp_mesh(WINDOW, slot_for(direction), direction, 0.5, &params);
        let (across, along) = if direction.is_horizontal() {
            (mid.rows, mid.cols)
        } else {
            (mid.cols, mid.rows)
        };
        assert_eq!(across, 2, "{direction:?}");
        assert!(along > 2, "{direction:?}");
        assert!(mid.vertex_count() <= 2 * params.max_samples);
        for step in 0..=10 {
            let p = f64::from(step) / 10.0;
            let mesh = warp_mesh(WINDOW, slot_for(direction), direction, p, &params);
            assert!(mesh.is_finite(), "{direction:?} at {p}");
            for axis in 0..2 {
                let values = mesh.vertices.iter().map(|v| v.uv[axis]);
                let lo = values.clone().fold(f32::MAX, f32::min);
                let hi = values.fold(f32::MIN, f32::max);
                assert_eq!((lo, hi), (0.0, 1.0), "{direction:?} at {p}");
            }
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
fn the_geometry_measures_the_slide_and_the_sideways_offsets() {
    let params = WarpParams::default();
    let g = WarpGeometry::new(
        WINDOW,
        Rect::new(520.0, 900.0, 60.0, 60.0),
        WarpDirection::Down,
        &params,
    );
    assert_eq!((g.length, g.width), (400.0, 500.0));
    assert_eq!(
        g.slide, 400.0,
        "the icon starts 800 below the top; the window is 400 tall"
    );
    assert_eq!(
        (g.dx_low, g.dx_high),
        (220.0, -220.0),
        "the icon's edges are 220 in from the window's"
    );
    let inside = WarpGeometry::new(
        WINDOW,
        Rect::new(520.0, 300.0, 60.0, 60.0),
        WarpDirection::Down,
        &params,
    );
    assert_eq!(inside.slide, 0.0, "a slot inside the window needs no slide");
}

#[test]
fn the_timing_grows_with_the_trip_but_never_runs_faster_than_one_time_unit() {
    let params = WarpParams::default();
    let t = WarpTiming::compute(0.0, 0.0, 0.0, 400.0, &params);
    assert_eq!((t.slide, t.stretch), (params.time_unit, params.time_unit));
    assert!(t.delay >= 0.0 && t.total() >= t.stretch);
    let short = WarpTiming::compute(100.0, 100.0, 100.0, 400.0, &params);
    let long = WarpTiming::compute(2000.0, 100.0, 2000.0, 400.0, &params);
    assert!(long.slide > short.slide && long.stretch > short.stretch);
}

#[test]
fn the_phases_run_from_nothing_to_everything_without_going_back() {
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
fn subdividing_spans_zero_to_one_in_ascending_order_with_cuts_where_the_run_curves() {
    let params = WarpParams::default();
    for (extent, amplitude) in [(800.0, 400.0), (50.0, 5.0), (3000.0, 900.0), (0.0, 0.0)] {
        let cuts = subdivide(extent, amplitude, &params);
        assert_eq!(cuts[0], 0.0);
        assert_eq!(*cuts.last().unwrap(), 1.0);
        assert!(cuts.windows(2).all(|w| w[0] < w[1]), "{extent} {amplitude}");
    }
    assert_eq!(
        subdivide(100.0, 0.0, &params),
        [0.0, 1.0],
        "straight and short"
    );
    assert!(
        subdivide(800.0, 400.0, &params).len() > 8,
        "curved and long"
    );
    assert_eq!(
        subdivide(-800.0, -400.0, &params),
        subdivide(800.0, 400.0, &params),
        "negative values cut like their magnitudes"
    );
}

#[test]
fn the_cut_count_respects_the_sample_budget_and_depth() {
    let budget = WarpParams {
        max_samples: 16,
        ..WarpParams::default()
    };
    let cuts = subdivide(5000.0, 5000.0, &budget);
    assert!(cuts.len() <= 16, "{}", cuts.len());
    let shallow = WarpParams {
        max_depth: 2,
        ..WarpParams::default()
    };
    assert!(subdivide(5000.0, 5000.0, &shallow).len() <= 5);
}
