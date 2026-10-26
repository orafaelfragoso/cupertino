//! Funnel minimize: a regular warp grid whose vertices fly straight to their slot points.

use crate::ease::cosine;
use crate::mesh::{Mesh, Vertex};
use cupertino_geometry::Rect;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FunnelParams {
    /// Cell size in points (70).
    pub cell: f64,
    /// Largest number of cells per axis (19).
    pub max_cells: usize,
}

impl Default for FunnelParams {
    fn default() -> Self {
        Self {
            cell: 70.0,
            max_cells: 19,
        }
    }
}

/// Cells per axis `(cols, rows)` of the warp grid for a window.
pub fn funnel_cells(window: Rect, params: &FunnelParams) -> (usize, usize) {
    let n = |len: f64| ((len / params.cell) as i64).clamp(1, params.max_cells as i64) as usize;
    (n(window.width()), n(window.height()))
}

pub fn funnel_mesh(window: Rect, slot: Rect, p: f64, params: &FunnelParams) -> Mesh {
    let (cols, rows) = funnel_cells(window, params);
    let e = cosine(p.clamp(0.0, 1.0));
    let corners = [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)];
    let at = |r: Rect, u: f64, v: f64| (r.min_x() + u * r.width(), r.min_y() + v * r.height());
    let d_max = corners
        .iter()
        .map(|&(u, v)| {
            let (a, b) = (at(window, u, v), at(slot, u, v));
            (b.0 - a.0).hypot(b.1 - a.1)
        })
        .fold(0.0, f64::max);
    let mut vertices = Vec::with_capacity((cols + 1) * (rows + 1));
    for j in 0..=rows {
        for i in 0..=cols {
            let (u, v) = (i as f64 / cols as f64, j as f64 / rows as f64);
            let (a, b) = (at(window, u, v), at(slot, u, v));
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let dist = dx.hypot(dy);
            let (x, y) = if dist > 0.0 {
                let moved = dist - (dist - d_max * e).clamp(0.0, d_max);
                (a.0 + dx / dist * moved, a.1 + dy / dist * moved)
            } else {
                a
            };
            vertices.push(Vertex {
                position: [x as f32, y as f32],
                uv: [u as f32, v as f32],
            });
        }
    }
    Mesh::new(cols + 1, rows + 1, vertices)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win() -> Rect {
        Rect::new(200.0, 100.0, 900.0, 600.0)
    }

    fn slot() -> Rect {
        Rect::new(1300.0, 1000.0, 64.0, 64.0)
    }

    #[test]
    fn grid_size_follows_the_cell_rule() {
        let p = FunnelParams::default();
        assert_eq!(funnel_cells(Rect::new(0.0, 0.0, 69.0, 69.0), &p), (1, 1));
        assert_eq!(funnel_cells(Rect::new(0.0, 0.0, 140.0, 210.0), &p), (2, 3));
        assert_eq!(funnel_cells(Rect::new(0.0, 0.0, 900.0, 600.0), &p), (12, 8));
        assert_eq!(
            funnel_cells(Rect::new(0.0, 0.0, 5000.0, 5000.0), &p),
            (19, 19)
        );
        assert_eq!(
            funnel_cells(Rect::new(0.0, 0.0, f64::NAN, 10.0), &p),
            (1, 1)
        );
    }

    #[test]
    fn vertex_budget_is_20_by_20() {
        let m = funnel_mesh(
            Rect::new(0.0, 0.0, 8000.0, 8000.0),
            slot(),
            0.5,
            &FunnelParams::default(),
        );
        assert_eq!((m.cols, m.rows), (20, 20));
        assert_eq!(m.vertex_count(), 400);
    }

    #[test]
    fn endpoints_are_window_and_slot() {
        let p = FunnelParams::default();
        let m0 = funnel_mesh(win(), slot(), 0.0, &p);
        assert_eq!(m0.bounds(), win());
        let m1 = funnel_mesh(win(), slot(), 1.0, &p);
        let b = m1.bounds();
        assert!(
            (b.min_x() - slot().min_x()).abs() < 1e-3 && (b.max_x() - slot().max_x()).abs() < 1e-3
        );
        assert!(
            (b.min_y() - slot().min_y()).abs() < 1e-3 && (b.max_y() - slot().max_y()).abs() < 1e-3
        );
    }

    #[test]
    fn uv_is_a_regular_grid_and_independent_of_progress() {
        let p = FunnelParams::default();
        let a = funnel_mesh(win(), slot(), 0.2, &p);
        let b = funnel_mesh(win(), slot(), 0.8, &p);
        for (x, y) in a.vertices.iter().zip(&b.vertices) {
            assert_eq!(x.uv, y.uv);
        }
        assert_eq!(a.vertex(0, 0).uv, [0.0, 0.0]);
        assert_eq!(a.vertex(a.cols - 1, a.rows - 1).uv, [1.0, 1.0]);
    }

    #[test]
    fn nearest_vertices_arrive_first() {
        let p = FunnelParams::default();
        let m0 = funnel_mesh(win(), slot(), 0.0, &p);
        let slot_c = slot();
        let m = funnel_mesh(win(), slot_c, 0.6, &p);
        let target = |uv: [f32; 2]| {
            (
                slot_c.min_x() + uv[0] as f64 * slot_c.width(),
                slot_c.min_y() + uv[1] as f64 * slot_c.height(),
            )
        };
        let remaining = |v: &Vertex| {
            (v.position[0] as f64 - target(v.uv).0).hypot(v.position[1] as f64 - target(v.uv).1)
        };
        let start = |v: &Vertex| {
            (m0.vertices.iter().find(|w| w.uv == v.uv).unwrap(), 0)
                .0
                .position
        };
        let mut near_done = 0;
        let mut far_done = 0;
        let mut dists: Vec<(f64, f64)> = m
            .vertices
            .iter()
            .map(|v| {
                let s = start(v);
                (
                    (s[0] as f64 - target(v.uv).0).hypot(s[1] as f64 - target(v.uv).1),
                    remaining(v),
                )
            })
            .collect();
        dists.sort_by(|a, b| a.0.total_cmp(&b.0));
        let half = dists.len() / 2;
        for d in &dists[..half] {
            if d.1 < 1e-3 {
                near_done += 1;
            }
        }
        for d in &dists[half..] {
            if d.1 < 1e-3 {
                far_done += 1;
            }
        }
        assert!(near_done > far_done, "{near_done} vs {far_done}");
    }

    #[test]
    fn vertices_never_move_backwards_and_stay_on_their_line() {
        let p = FunnelParams::default();
        let m0 = funnel_mesh(win(), slot(), 0.0, &p);
        let mut prev: Vec<f64> = vec![0.0; m0.vertex_count()];
        for k in 1..=100 {
            let m = funnel_mesh(win(), slot(), k as f64 / 100.0, &p);
            assert!(m.is_finite());
            for (i, (v, s)) in m.vertices.iter().zip(&m0.vertices).enumerate() {
                let moved = ((v.position[0] - s.position[0]) as f64)
                    .hypot((v.position[1] - s.position[1]) as f64);
                assert!(moved >= prev[i] - 1e-3, "vertex {i} step {k}");
                prev[i] = moved;
            }
        }
    }

    #[test]
    fn coincident_window_and_slot_do_not_produce_nan() {
        let m = funnel_mesh(win(), win(), 0.5, &FunnelParams::default());
        assert!(m.is_finite());
        assert!(m.max_distance(&funnel_mesh(win(), win(), 0.0, &FunnelParams::default())) < 1e-3);
    }
}
