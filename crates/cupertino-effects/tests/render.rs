use cupertino_effects::raster::{draw_mesh, window_test_pattern};
use cupertino_effects::*;
use cupertino_geometry::Rect;
use tiny_skia::Pixmap;

const W: u32 = 640;
const H: u32 = 400;
const WINDOW: Rect = Rect::new(80.0, 30.0, 280.0, 190.0);
const SLOT: Rect = Rect::new(420.0, 330.0, 40.0, 40.0);

fn render(kind: MinimizeKind, p: f64) -> Pixmap {
    let tex = window_test_pattern(280, 190);
    let mut pm = Pixmap::new(W, H).unwrap();
    let e = MinimizeEffect::new(kind, WINDOW, SLOT, DockEdge::Bottom);
    draw_mesh(&mut pm, &tex, &e.mesh(p), (0.0, 0.0), 1.0);
    pm
}

fn alpha(pm: &Pixmap, x: u32, y: u32) -> u8 {
    pm.data()[((y * pm.width() + x) * 4 + 3) as usize]
}

fn covered_bounds(pm: &Pixmap) -> Option<(u32, u32, u32, u32)> {
    let (mut l, mut t, mut r, mut b) = (u32::MAX, u32::MAX, 0, 0);
    for y in 0..pm.height() {
        for x in 0..pm.width() {
            if alpha(pm, x, y) > 127 {
                l = l.min(x);
                r = r.max(x);
                t = t.min(y);
                b = b.max(y);
            }
        }
    }
    (l != u32::MAX).then_some((l, t, r, b))
}

#[test]
fn rendering_at_progress_zero_covers_exactly_the_window() {
    for kind in MinimizeKind::ALL {
        let (l, t, r, b) = covered_bounds(&render(kind, 0.0)).unwrap();
        assert!(
            l.abs_diff(80) <= 1
                && t.abs_diff(30) <= 1
                && r.abs_diff(359) <= 1
                && b.abs_diff(219) <= 1,
            "{kind:?}: {l} {t} {r} {b}"
        );
    }
}

#[test]
fn rendering_at_progress_one_covers_exactly_the_slot() {
    for kind in MinimizeKind::ALL {
        let (l, t, r, b) = covered_bounds(&render(kind, 1.0)).unwrap();
        assert!(
            l.abs_diff(420) <= 1
                && t.abs_diff(330) <= 1
                && r.abs_diff(459) <= 1
                && b.abs_diff(369) <= 1,
            "{kind:?}: {l} {t} {r} {b}"
        );
    }
}

#[test]
fn rendered_warp_never_leaves_the_hull_of_window_and_slot_by_much() {
    let hull = WINDOW.union(&SLOT).outset(2.0);
    for k in 0..=8 {
        let pm = render(MinimizeKind::Warp, k as f64 / 8.0);
        for y in 0..H {
            for x in 0..W {
                if alpha(&pm, x, y) > 0 {
                    assert!(
                        hull.contains(cupertino_geometry::Point::new(
                            x as f64 + 0.5,
                            y as f64 + 0.5
                        )),
                        "step {k}: pixel ({x},{y})"
                    );
                }
            }
        }
    }
}

#[test]
fn warp_sequence_has_no_holes_in_the_window_body() {
    let pm = render(MinimizeKind::Warp, 0.35);
    let e = MinimizeEffect::new(MinimizeKind::Warp, WINDOW, SLOT, DockEdge::Bottom);
    let m = e.mesh(0.35);
    for r in 0..m.rows - 1 {
        let (a, b) = (m.vertex(0, r).position, m.vertex(1, r + 1).position);
        let (cx, cy) = (((a[0] + b[0]) / 2.0) as u32, ((a[1] + b[1]) / 2.0) as u32);
        assert!(
            alpha(&pm, cx.min(W - 1), cy.min(H - 1)) > 200,
            "row {r} at ({cx},{cy})"
        );
    }
}

#[test]
fn texture_orientation_survives_the_warp() {
    let tex = window_test_pattern(280, 190);
    let title = |pm: &Pixmap, x: u32, y: u32| -> [u8; 3] {
        let d = &pm.data()[((y * pm.width() + x) * 4) as usize..][..3];
        [d[0], d[1], d[2]]
    };
    let mut pm = Pixmap::new(W, H).unwrap();
    let e = MinimizeEffect::new(MinimizeKind::Warp, WINDOW, SLOT, DockEdge::Bottom);
    draw_mesh(&mut pm, &tex, &e.mesh(0.0), (0.0, 0.0), 1.0);
    let bar = title(&pm, 200, 36);
    assert!(
        bar[0] < 100 && bar[1] < 100,
        "dark title bar at the top: {bar:?}"
    );
}

#[test]
fn minimize_animation_renders_every_frame_without_panics() {
    let mut a = MinimizeAnimation::new(
        MinimizeEffect::new(MinimizeKind::Funnel, WINDOW, SLOT, DockEdge::Bottom),
        false,
    );
    a.minimize(0.0);
    let tex = window_test_pattern(280, 190);
    let mut pm = Pixmap::new(W, H).unwrap();
    for k in 0..12 {
        let t = k as f64 / 24.0;
        if k == 6 {
            a.reverse(t);
        }
        draw_mesh(&mut pm, &tex, &a.mesh(t), (0.0, 0.0), 1.0);
    }
}
