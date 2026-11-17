//! Meshes are row-major vertex grids drawn as triangle lists; the scale and funnel minimize effects build them.

use cupertino_effects::{
    funnel_cells, funnel_mesh, lerp_rect, scale_mesh, scale_rect, Ease, FunnelParams, Mesh, Vertex,
};
use cupertino_geometry::Rect;

fn v(x: f32, y: f32) -> Vertex {
    Vertex {
        position: [x, y],
        uv: [x / 10.0, y / 10.0],
    }
}

fn rect_eq(a: Rect, b: Rect) -> bool {
    (a.x() - b.x()).abs() < 1e-6
        && (a.y() - b.y()).abs() < 1e-6
        && (a.width() - b.width()).abs() < 1e-6
        && (a.height() - b.height()).abs() < 1e-6
}

const WINDOW: Rect = Rect::new(100.0, 100.0, 400.0, 300.0);
const SLOT: Rect = Rect::new(900.0, 700.0, 48.0, 48.0);

#[test]
fn a_quad_shows_the_whole_texture_in_its_rect_and_scales_without_changing_it() {
    let rect = Rect::new(10.0, 20.0, 30.0, 40.0);
    let m = Mesh::quad(rect);
    assert_eq!((m.cols, m.rows), (2, 2));
    for (col, row, position, uv) in [
        (0, 0, [10.0, 20.0], [0.0, 0.0]),
        (1, 0, [40.0, 20.0], [1.0, 0.0]),
        (0, 1, [10.0, 60.0], [0.0, 1.0]),
        (1, 1, [40.0, 60.0], [1.0, 1.0]),
    ] {
        assert_eq!(m.vertex(col, row).position, position, "({col}, {row})");
        assert_eq!(m.vertex(col, row).uv, uv, "({col}, {row})");
    }
    assert!(rect_eq(m.bounds(), rect));
    let scaled = m.scaled(2.0);
    assert_eq!(scaled.vertex(1, 1).position, [80.0, 120.0]);
    assert_eq!(scaled.vertex(1, 1).uv, [1.0, 1.0]);
}

#[test]
#[should_panic(expected = "cols * rows")]
fn a_mesh_must_hold_exactly_cols_times_rows_vertices() {
    Mesh::new(3, 2, vec![v(0.0, 0.0); 5]);
}

#[test]
fn a_grid_has_two_triangles_per_cell_and_a_degenerate_one_none() {
    let vertices: Vec<Vertex> = (0..3)
        .flat_map(|r| (0..3).map(move |c| v(c as f32, r as f32)))
        .collect();
    let m = Mesh::new(3, 3, vertices);
    assert_eq!((m.vertex_count(), m.cell_count()), (9, 4));
    let indices = m.indices();
    assert_eq!(indices.len(), 24);
    assert_eq!(&indices[..6], &[0, 1, 3, 1, 4, 3]);
    assert_eq!(&indices[18..], &[4, 5, 7, 5, 8, 7]);

    let line = Mesh::new(1, 4, vec![v(0.0, 0.0); 4]);
    assert_eq!(line.cell_count(), 0);
    assert!(line.indices().is_empty());
    let none = Mesh::new(0, 0, Vec::new());
    assert_eq!(none.cell_count(), 0);
    assert_eq!(none.bounds(), Rect::ZERO);
}

#[test]
fn a_mesh_knows_whether_it_is_finite_and_how_far_it_is_from_another() {
    let a = Mesh::quad(Rect::new(0.0, 0.0, 10.0, 10.0));
    let b = Mesh::quad(Rect::new(3.0, 4.0, 10.0, 10.0));
    assert!(a.is_finite());
    assert!((a.max_distance(&b) - 5.0).abs() < 1e-6);
    assert_eq!(a.max_distance(&a), 0.0);
    let mut bad = a.clone();
    bad.vertices[2].uv[1] = f32::NAN;
    assert!(!bad.is_finite(), "a texture coordinate");
    bad.vertices[2].uv[1] = 0.0;
    bad.vertices[3].position[0] = f32::INFINITY;
    assert!(!bad.is_finite(), "a position");
}

#[test]
fn the_scale_effect_eases_a_quad_from_the_window_to_the_slot() {
    assert!(rect_eq(
        lerp_rect(WINDOW, SLOT, 0.5),
        Rect::new(500.0, 400.0, 224.0, 174.0)
    ));
    for (progress, expected) in [
        (0.0, WINDOW),
        (0.5, lerp_rect(WINDOW, SLOT, 0.5)),
        (1.0, SLOT),
    ] {
        assert!(
            rect_eq(scale_rect(WINDOW, SLOT, progress, Ease::Cosine), expected),
            "{progress}"
        );
    }
    let quarter = scale_rect(WINDOW, SLOT, 0.25, Ease::Cosine);
    let linear = scale_rect(WINDOW, SLOT, 0.25, Ease::Linear);
    assert!(quarter.x() < linear.x(), "cosine starts slower");
    let mesh = scale_mesh(WINDOW, SLOT, 1.0, Ease::Linear);
    assert_eq!(mesh.vertex_count(), 4);
    assert!(rect_eq(mesh.bounds(), SLOT));
}

#[test]
fn the_funnel_grid_follows_the_window_size_within_bounds() {
    let p = FunnelParams::default();
    for (size, cells) in [
        ((700.0, 140.0), (10, 2)),
        ((10.0, 10.0), (1, 1)),
        ((99_999.0, 99_999.0), (p.max_cells, p.max_cells)),
    ] {
        assert_eq!(
            funnel_cells(Rect::new(0.0, 0.0, size.0, size.1), &p),
            cells,
            "{size:?}"
        );
    }
}

#[test]
fn the_funnel_runs_from_the_window_to_the_slot_over_the_whole_texture() {
    let p = FunnelParams::default();
    let (cols, rows) = funnel_cells(WINDOW, &p);
    for (progress, bounds) in [(-1.0, WINDOW), (0.0, WINDOW), (1.0, SLOT), (4.0, SLOT)] {
        let m = funnel_mesh(WINDOW, SLOT, progress, &p);
        assert!(rect_eq(m.bounds(), bounds), "{progress}");
        assert_eq!((m.cols, m.rows), (cols + 1, rows + 1), "{progress}");
    }
    let m = funnel_mesh(WINDOW, SLOT, 0.4, &p);
    assert_eq!(m.vertex(0, 0).uv, [0.0, 0.0]);
    assert_eq!(m.vertex(m.cols - 1, m.rows - 1).uv, [1.0, 1.0]);
    assert!(m.is_finite());
    let onto_itself = funnel_mesh(WINDOW, WINDOW, 0.5, &p);
    assert!(onto_itself.is_finite());
    assert!(rect_eq(onto_itself.bounds(), WINDOW), "onto its own window");
}

#[test]
fn the_funnel_moves_both_corners_and_never_overshoots() {
    let m = funnel_mesh(WINDOW, SLOT, 0.5, &FunnelParams::default());
    let first = m.vertex(0, 0).position;
    let last = m.vertex(m.cols - 1, m.rows - 1).position;
    let travelled = |from: [f64; 2], to: [f32; 2]| {
        (f64::from(to[0]) - from[0]).hypot(f64::from(to[1]) - from[1])
    };
    assert!(travelled([WINDOW.min_x(), WINDOW.min_y()], first) > 0.0);
    assert!(travelled([WINDOW.max_x(), WINDOW.max_y()], last) > 0.0);
    assert!(m.bounds().max_x() <= SLOT.max_x().max(WINDOW.max_x()) + 1e-3);
}
