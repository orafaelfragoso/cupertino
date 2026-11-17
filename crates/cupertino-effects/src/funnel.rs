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
