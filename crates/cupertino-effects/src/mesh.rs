//! Vertex grids with texture coordinates, the common output of every mesh effect.
//!
//! A [`Mesh`] is a row-major grid of `cols × rows` vertices (vertex counts, not cells). Each vertex
//! carries its destination position in output coordinates and the texture coordinate of the window
//! content it shows, so any renderer can draw it as a triangle list ([`Mesh::indices`]).

use cupertino_geometry::Rect;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vertex {
    pub position: [f32; 2],
    pub uv: [f32; 2],
}

#[derive(Clone, Debug, PartialEq)]
pub struct Mesh {
    /// Vertices per row.
    pub cols: usize,
    /// Vertex rows.
    pub rows: usize,
    /// Row-major, `cols * rows` entries.
    pub vertices: Vec<Vertex>,
}

impl Mesh {
    pub fn new(cols: usize, rows: usize, vertices: Vec<Vertex>) -> Self {
        assert_eq!(
            vertices.len(),
            cols * rows,
            "vertex count must equal cols * rows"
        );
        Self {
            cols,
            rows,
            vertices,
        }
    }

    /// Two-by-two mesh showing the whole texture in `rect`.
    pub fn quad(rect: Rect) -> Self {
        let (l, t, r, b) = (
            rect.min_x() as f32,
            rect.min_y() as f32,
            rect.max_x() as f32,
            rect.max_y() as f32,
        );
        Self::new(
            2,
            2,
            vec![
                Vertex {
                    position: [l, t],
                    uv: [0.0, 0.0],
                },
                Vertex {
                    position: [r, t],
                    uv: [1.0, 0.0],
                },
                Vertex {
                    position: [l, b],
                    uv: [0.0, 1.0],
                },
                Vertex {
                    position: [r, b],
                    uv: [1.0, 1.0],
                },
            ],
        )
    }

    pub fn vertex(&self, col: usize, row: usize) -> Vertex {
        self.vertices[row * self.cols + col]
    }

    pub fn vertex_count(&self) -> usize {
        self.vertices.len()
    }

    pub fn cell_count(&self) -> usize {
        self.cols.saturating_sub(1) * self.rows.saturating_sub(1)
    }

    /// Triangle list, two triangles per cell, indices into [`Mesh::vertices`].
    pub fn indices(&self) -> Vec<u32> {
        let mut out = Vec::with_capacity(self.cell_count() * 6);
        for r in 0..self.rows.saturating_sub(1) {
            for c in 0..self.cols.saturating_sub(1) {
                let i = (r * self.cols + c) as u32;
                let (a, b, d, e) = (i, i + 1, i + self.cols as u32, i + self.cols as u32 + 1);
                out.extend_from_slice(&[a, b, d, b, e, d]);
            }
        }
        out
    }

    /// Bounding box of the destination positions.
    pub fn bounds(&self) -> Rect {
        let (mut l, mut t, mut r, mut b) = (
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        );
        for v in &self.vertices {
            l = l.min(v.position[0] as f64);
            r = r.max(v.position[0] as f64);
            t = t.min(v.position[1] as f64);
            b = b.max(v.position[1] as f64);
        }
        if self.vertices.is_empty() {
            return Rect::ZERO;
        }
        Rect::from_ltrb(l, t, r, b)
    }

    /// Copy with every position multiplied by `s` (texture coordinates unchanged).
    pub fn scaled(&self, s: f32) -> Mesh {
        let vertices = self
            .vertices
            .iter()
            .map(|v| Vertex {
                position: [v.position[0] * s, v.position[1] * s],
                uv: v.uv,
            })
            .collect();
        Mesh {
            cols: self.cols,
            rows: self.rows,
            vertices,
        }
    }

    /// All coordinates are finite numbers.
    pub fn is_finite(&self) -> bool {
        self.vertices
            .iter()
            .all(|v| v.position.iter().chain(&v.uv).all(|x| x.is_finite()))
    }

    /// Largest distance between corresponding vertices of two meshes of the same shape.
    pub fn max_distance(&self, other: &Mesh) -> f64 {
        assert_eq!((self.cols, self.rows), (other.cols, other.rows));
        self.vertices
            .iter()
            .zip(&other.vertices)
            .map(|(a, b)| {
                ((a.position[0] - b.position[0]) as f64)
                    .hypot((a.position[1] - b.position[1]) as f64)
            })
            .fold(0.0, f64::max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quad_covers_rect_and_texture() {
        let m = Mesh::quad(Rect::new(10.0, 20.0, 100.0, 50.0));
        assert_eq!(m.bounds(), Rect::new(10.0, 20.0, 100.0, 50.0));
        assert_eq!(m.vertex(1, 1).uv, [1.0, 1.0]);
        assert_eq!(m.indices().len(), 6);
        assert!(m.is_finite());
    }

    #[test]
    fn scaling_moves_positions_only() {
        let m = Mesh::quad(Rect::new(10.0, 20.0, 100.0, 50.0)).scaled(0.5);
        assert_eq!(m.bounds(), Rect::new(5.0, 10.0, 50.0, 25.0));
        assert_eq!(m.vertex(1, 1).uv, [1.0, 1.0]);
    }

    #[test]
    fn indices_cover_every_cell() {
        let verts = (0..12)
            .map(|i| Vertex {
                position: [i as f32, 0.0],
                uv: [0.0, 0.0],
            })
            .collect();
        let m = Mesh::new(4, 3, verts);
        assert_eq!(m.cell_count(), 6);
        let idx = m.indices();
        assert_eq!(idx.len(), 36);
        assert!(idx.iter().all(|&i| (i as usize) < m.vertex_count()));
    }
}
