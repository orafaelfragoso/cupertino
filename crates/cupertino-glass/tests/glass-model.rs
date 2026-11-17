//! The glass model the compositor draws containers with: shape distance, the union of shapes, shading and the
//! uniform block.

use cupertino_glass::glass::{
    union_field, Container, Cut, GlassCorner, GlassEdge, GlassMode, GlassShape, GlassUniform, Rgb,
    MAX_SHAPES, MAX_SPACING,
};

const BACKDROP: Rgb = [0.3, 0.5, 0.7];

fn flat(_level: usize, _at: [f32; 2]) -> Rgb {
    BACKDROP
}

fn container(shapes: Vec<GlassShape>, spacing: f32) -> Container {
    Container::new(shapes, spacing, GlassMode::default())
}

#[test]
fn a_shape_is_negative_inside_zero_on_its_edge_and_the_distance_outside() {
    let rect = [10.0, 20.0, 100.0, 60.0];
    let corners = [GlassCorner::Circular, GlassCorner::Continuous];
    for corner in corners {
        for radius in [0.0, 12.0, f32::INFINITY] {
            let shape = GlassShape::new(rect, radius).with_corner(corner);
            let name = format!("{corner:?} radius {radius}");
            assert!(
                shape.sdf([60.0, 50.0]) < -25.0,
                "{name}: the centre is deep inside"
            );
            assert!(
                shape.sdf([60.0, 20.0]).abs() < 1e-3,
                "{name}: the middle of the top edge"
            );
            assert!(
                (shape.sdf([60.0, 10.0]) - 10.0).abs() < 1e-3,
                "{name}: above the top edge"
            );
            assert!(
                (shape.sdf([140.0, 50.0]) - 30.0).abs() < 1e-3,
                "{name}: beside the right edge"
            );
        }
    }
    let capsule = GlassShape::capsule(rect);
    assert_eq!(capsule.fitted_radius(), 30.0);
    assert!(
        capsule.sdf([10.0, 20.0]) > 5.0,
        "the capsule's corner is cut away"
    );
}

#[test]
fn a_blended_union_joins_close_shapes_and_a_hard_one_does_not() {
    let shapes = [
        GlassShape::new([0.0, 0.0, 50.0, 50.0], 25.0),
        GlassShape::new([60.0, 0.0, 50.0, 50.0], 25.0),
    ];
    let gap = [55.0, 25.0];
    let (hard, _) = union_field(&shapes, 0.0, gap);
    let (blended, weights) = union_field(&shapes, 20.0, gap);
    assert!(hard > 0.0, "a hard union leaves the gap outside");
    assert!(blended < hard, "blending pulls the gap into the glass");
    assert_eq!(weights.len(), 2);
    assert!(
        (weights[0] - weights[1]).abs() < 1e-3,
        "the gap is as near to either shape"
    );

    let (inside, _) = union_field(&shapes, 20.0, [25.0, 25.0]);
    assert!(inside < -20.0);
}

#[test]
fn a_container_keeps_its_limits() {
    let many = (0..MAX_SHAPES + 4)
        .map(|i| GlassShape::new([i as f32 * 10.0, 0.0, 8.0, 8.0], 2.0))
        .collect();
    assert_eq!(container(many, 0.0).shapes.len(), MAX_SHAPES);
    let one = vec![GlassShape::new([0.0, 0.0, 40.0, 40.0], 8.0)];
    for (asked, kept) in [
        (1e6, MAX_SPACING),
        (-3.0, 0.0),
        (f32::NAN, 0.0),
        (10.0, 10.0),
    ] {
        assert_eq!(
            container(one.clone(), asked).spacing,
            kept,
            "spacing {asked}"
        );
    }
    assert_eq!(container(Vec::new(), 0.0).bounds(), None);
}

#[test]
fn the_bounds_cover_every_shape_and_the_backdrop_area_reaches_past_them() {
    let c = container(
        vec![
            GlassShape::new([10.0, 20.0, 40.0, 40.0], 8.0),
            GlassShape::new([100.0, 0.0, 30.0, 30.0], 8.0),
        ],
        16.0,
    );
    assert_eq!(c.bounds(), Some([10.0, 0.0, 120.0, 60.0]));
    let [x, y, w, h] = c.sample_area().unwrap();
    assert!(x < 10.0 && y < 0.0 && x + w > 130.0 && y + h > 60.0);
    let with_edge = c.clone().with_edge(GlassEdge {
        width: 400.0,
        color: [0.0, 0.0, 0.0, 1.0],
        soft: 0.0,
    });
    assert!(
        with_edge.sample_area().unwrap()[2] > w,
        "a wide edge needs more of the backdrop"
    );
}

#[test]
fn shading_covers_the_inside_and_leaves_the_far_outside_alone() {
    let c = container(vec![GlassShape::new([0.0, 0.0, 200.0, 100.0], 20.0)], 0.0);
    let (face, coverage) = c.shade([100.0, 50.0], &flat);
    assert_eq!(coverage, 1.0);
    assert!(face.iter().all(|v| (0.0..=1.0).contains(v)));
    assert_ne!(face, BACKDROP, "the glass is not the bare backdrop");

    let (_, far) = c.shade([100.0, 400.0], &flat);
    assert_eq!(far, 0.0);
    let (_, edge) = c.shade([100.0, 0.0], &flat);
    assert!(
        edge > 0.0 && edge < 1.0,
        "the edge pixel is partly covered: {edge}"
    );
}

#[test]
fn a_cut_limits_what_the_container_shows() {
    let shape = GlassShape::new([0.0, 0.0, 200.0, 100.0], 20.0);
    let cut = Cut {
        rect: [0.0, 0.0, 100.0, 100.0],
        extent: 0.0,
        exponent: 2.0,
    };
    let cut_container = container(vec![shape], 0.0).cut_to(cut);
    assert_eq!(cut_container.shade([50.0, 50.0], &flat).1, 1.0);
    assert_eq!(cut_container.shade([150.0, 50.0], &flat).1, 0.0);
    assert_eq!(cut.coverage([50.0, 50.0]), 1.0);
    assert_eq!(cut.coverage([150.0, 50.0]), 0.0);
    assert!(cut.sdf([100.0, 50.0]).abs() < 1e-3);
}

#[test]
fn the_edge_line_is_drawn_outside_the_shape_and_where_shapes_overlap_only_around_the_whole() {
    let edge = GlassEdge {
        width: 2.0,
        color: [1.0, 1.0, 1.0, 0.5],
        soft: 0.0,
    };
    let single =
        container(vec![GlassShape::new([0.0, 0.0, 100.0, 100.0], 10.0)], 0.0).with_edge(edge);
    assert!(
        (single.edge_alpha_at([50.0, -1.0]) - 0.5).abs() < 1e-3,
        "just outside"
    );
    assert_eq!(single.edge_alpha_at([50.0, 50.0]), 0.0, "not inside");
    assert_eq!(
        single.edge_alpha_at([50.0, -10.0]),
        0.0,
        "not beyond its width"
    );
    assert_eq!(
        container(vec![GlassShape::new([0.0, 0.0, 100.0, 100.0], 10.0)], 0.0)
            .edge_alpha_at([50.0, -1.0]),
        0.0
    );

    let overlapping = container(
        vec![
            GlassShape::new([0.0, 0.0, 100.0, 100.0], 10.0),
            GlassShape::new([50.0, 0.0, 100.0, 100.0], 10.0),
        ],
        0.0,
    )
    .with_edge(edge);
    assert!(
        overlapping.edge_alpha_at([75.0, 101.0]) > 0.0,
        "around the whole"
    );
    assert_eq!(
        overlapping.edge_alpha_at([100.0, 50.0]),
        0.0,
        "no line inside the other shape"
    );
}

#[test]
fn the_uniform_carries_the_container_in_target_pixels() {
    let c = container(
        vec![
            GlassShape::new([10.0, 20.0, 40.0, 30.0], 8.0),
            GlassShape::new([60.0, 20.0, 40.0, 30.0], 8.0),
        ],
        12.0,
    );
    let uv = [[0.001, 0.0, 0.0], [0.0, 0.001, 0.0]];
    let u = GlassUniform::new(&c, 2.0, uv, [0.0, 0.0, 400.0, 200.0], [800.0, 600.0], 0.75);
    assert_eq!(u.info[0], 2.0, "two shapes");
    assert_eq!(u.info[1], 24.0, "the blend width in pixels");
    assert_eq!(u.info[3], 0.75, "the opacity");
    assert_eq!(u.shapes[0], [20.0, 40.0, 80.0, 60.0]);
    assert_eq!(u.shapes[1], [120.0, 40.0, 80.0, 60.0]);
    assert_eq!(u.uv_x[..3], [0.001, 0.0, 0.0]);

    let odd = GlassUniform::new(&c, 1.5, uv, [0.0; 4], [800.0, 600.0], 1.0);
    let snapped = odd.on_pixels();
    let rounded = GlassUniform::new(
        &container(vec![GlassShape::new([10.3, 20.3, 40.0, 30.0], 0.0)], 0.0),
        1.0,
        uv,
        [0.0; 4],
        [800.0, 600.0],
        1.0,
    )
    .on_pixels();
    assert_eq!(
        snapped.shapes[0], odd.shapes[0],
        "rounded shapes keep their edges"
    );
    assert_eq!(
        rounded.shapes[0],
        [10.0, 20.0, 40.0, 30.0],
        "square shapes land on whole pixels"
    );
}
