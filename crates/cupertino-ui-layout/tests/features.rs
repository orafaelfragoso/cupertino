use crate::common;

use common::TestTree;
use cupertino_geometry::{Point, Rect, Size};
use cupertino_ui_layout::{
    layout, Align, Axis, Constraints, ContainerKind, Flex, GridSpan, LayoutCache, LayoutDirection,
    LayoutError, LayoutOptions, Overflow, SizeSpec, Track,
};

const OPEN: Size = Size::new(f64::INFINITY, f64::INFINITY);

fn span(tree: &mut TestTree, id: usize, columns: usize, rows: usize) -> usize {
    tree.spec(id).span = GridSpan { columns, rows };
    id
}

#[test]
fn a_cell_spanning_columns_covers_their_width_and_the_next_cell_flows_after_it() {
    let mut t = TestTree::default();
    let wide = t.leaf(10.0, 5.0);
    span(&mut t, wide, 2, 1);
    let next = t.leaf(10.0, 5.0);
    let after = t.leaf(10.0, 5.0);
    let root = t.grid(
        vec![Track::Fixed(30.0), Track::Fixed(40.0)],
        vec![],
        Size::new(2.0, 0.0),
        vec![wide, next, after],
    );
    let r = t.run_tight(root, Size::new(72.0, 10.0), LayoutDirection::Ltr);
    assert_eq!(r.nodes[&wide].rect, Rect::new(0.0, 0.0, 72.0, 5.0));
    assert_eq!(r.nodes[&next].rect, Rect::new(0.0, 5.0, 30.0, 5.0));
    assert_eq!(r.nodes[&after].rect, Rect::new(32.0, 5.0, 40.0, 5.0));
}

#[test]
fn a_spanning_child_that_needs_more_widens_its_auto_tracks_equally() {
    let mut t = TestTree::default();
    let narrow = t.leaf(10.0, 5.0);
    let wide = t.leaf(100.0, 5.0);
    span(&mut t, wide, 2, 1);
    let root = t.grid(
        vec![Track::Auto, Track::Auto],
        vec![],
        Size::new(4.0, 0.0),
        vec![narrow, wide],
    );
    let r = t.run(root, OPEN);
    assert_eq!(r.nodes[&root].rect.size.width, 100.0);
    assert_eq!(r.nodes[&wide].rect.size.width, 100.0);
    assert_eq!(r.nodes[&narrow].rect.size.width, 53.0);
}

#[test]
fn a_cell_spanning_rows_takes_the_rows_below_and_later_cells_go_around_it() {
    let mut t = TestTree::default();
    let tall = t.leaf(10.0, 5.0);
    span(&mut t, tall, 1, 2);
    let (b, c, d) = (t.leaf(10.0, 5.0), t.leaf(10.0, 5.0), t.leaf(10.0, 5.0));
    let root = t.grid(
        vec![Track::Fixed(20.0), Track::Fixed(20.0)],
        vec![Track::Fixed(10.0), Track::Fixed(10.0)],
        Size::ZERO,
        vec![tall, b, c, d],
    );
    let r = t.run_tight(root, Size::new(40.0, 20.0), LayoutDirection::Ltr);
    assert_eq!(r.nodes[&tall].rect, Rect::new(0.0, 0.0, 20.0, 20.0));
    assert_eq!(r.nodes[&b].rect.origin, Point::new(20.0, 0.0));
    assert_eq!(r.nodes[&c].rect.origin, Point::new(20.0, 10.0));
    assert_eq!(r.nodes[&d].rect.origin, Point::new(0.0, 20.0));
}

#[test]
fn a_span_wider_than_the_grid_is_cut_to_it() {
    let mut t = TestTree::default();
    let a = t.leaf(1.0, 5.0);
    span(&mut t, a, 9, 0);
    let root = t.grid(
        vec![Track::Fixed(10.0), Track::Fixed(10.0)],
        vec![],
        Size::ZERO,
        vec![a],
    );
    let r = t.run(root, OPEN);
    assert_eq!(r.nodes[&a].rect.size.width, 20.0);
}

#[test]
fn absolute_children_sit_at_their_position_and_the_container_grows_to_hold_them() {
    let mut t = TestTree::default();
    let (a, b) = (t.leaf(10.0, 10.0), t.leaf(30.0, 20.0));
    t.spec(a).position = Point::new(5.0, 7.0);
    t.spec(b).position = Point::new(40.0, 0.0);
    let root = t.container(ContainerKind::Absolute, vec![a, b]);
    let r = t.run(root, OPEN);
    assert_eq!(r.nodes[&a].rect, Rect::new(5.0, 7.0, 10.0, 10.0));
    assert_eq!(r.nodes[&b].rect, Rect::new(40.0, 0.0, 30.0, 20.0));
    assert_eq!(r.nodes[&root].rect.size, Size::new(70.0, 20.0));
    let r = t.run_tight(root, Size::new(100.0, 50.0), LayoutDirection::Rtl);
    assert_eq!(r.nodes[&a].rect.origin, Point::new(85.0, 7.0));
}

#[test]
fn an_absolute_child_of_a_size_of_its_own_keeps_it_past_a_smaller_container_that_crops_it() {
    let mut t = TestTree::default();
    let (a, b) = (t.leaf(10.0, 10.0), t.leaf(30.0, 20.0));
    t.size(a, SizeSpec::Fixed(120.0), SizeSpec::Fixed(80.0));
    t.spec(a).position = Point::new(-4.0, 2.0);
    let root = t.container(ContainerKind::Absolute, vec![a, b]);
    // a container that lets its children show past it, or only clips them, holds them to its size
    for overflow in [Overflow::Visible, Overflow::Clip] {
        t.spec(root).overflow = overflow;
        let r = t.run_tight(root, Size::new(100.0, 50.0), LayoutDirection::Ltr);
        assert_eq!(r.nodes[&a].rect, Rect::new(-4.0, 2.0, 100.0, 50.0));
    }
    t.spec(root).overflow = Overflow::Crop;
    let r = t.run_tight(root, Size::new(100.0, 50.0), LayoutDirection::Ltr);
    assert_eq!(r.nodes[&a].rect, Rect::new(-4.0, 2.0, 120.0, 80.0));
    assert_eq!(r.nodes[&root].rect.size, Size::new(100.0, 50.0));
    // and cuts them off at its edge
    assert_eq!(r.nodes[&root].clip, Some(Rect::new(0.0, 0.0, 100.0, 50.0)));
    // a child that fits itself to the room still takes no more than the container's
    t.size(b, SizeSpec::Fill, SizeSpec::Fit);
    let r = t.run_tight(root, Size::new(100.0, 50.0), LayoutDirection::Ltr);
    assert_eq!(r.nodes[&b].rect.size.width, 100.0);
}

#[test]
fn a_vertical_stack_reports_its_first_childs_baseline_and_start_aligns_baseline_children() {
    let mut t = TestTree::default();
    let label = t.leaf(40.0, 10.0);
    t.nodes[label].baseline = Some(8.0);
    let field = t.leaf(60.0, 20.0);
    t.nodes[field].baseline = Some(15.0);
    let root = t.stack(Axis::Vertical, 4.0, vec![label, field]);
    if let ContainerKind::Stack { align, .. } = &mut t.spec(root).kind {
        *align = Align::Baseline;
    }
    let r = t.run(root, OPEN);
    assert_eq!(r.nodes[&root].baseline, Some(8.0));
    assert_eq!(r.nodes[&field].rect.origin, Point::new(0.0, 14.0));
}

#[test]
fn children_stop_shrinking_at_their_smallest_size() {
    let mut t = TestTree::default();
    let rigid = t.leaf(80.0, 10.0);
    t.floor(rigid, 30.0, 10.0);
    t.flex(
        rigid,
        Flex {
            grow: 0.0,
            shrink: 1.0,
            basis: None,
        },
    );
    let soft = t.leaf(80.0, 10.0);
    t.flex(
        soft,
        Flex {
            grow: 0.0,
            shrink: 1.0,
            basis: None,
        },
    );
    let root = t.stack(Axis::Horizontal, 0.0, vec![rigid, soft]);
    let r = t.run_tight(root, Size::new(100.0, 10.0), LayoutDirection::Ltr);
    assert_eq!(r.nodes[&rigid].rect.size.width, 50.0);
    let r = t.run_tight(root, Size::new(60.0, 10.0), LayoutDirection::Ltr);
    assert_eq!(r.nodes[&rigid].rect.size.width, 30.0);
    assert_eq!(r.nodes[&soft].rect.size.width, 30.0);
    let r = t.run_tight(root, Size::new(20.0, 10.0), LayoutDirection::Ltr);
    assert_eq!(r.nodes[&rigid].rect.size.width, 30.0);
    assert_eq!(r.nodes[&soft].rect.size.width, 0.0);
}

#[test]
fn sixty_children_that_all_reach_their_floor_are_shared_in_one_pass() {
    let mut t = TestTree::default();
    let children: Vec<usize> = (0..60)
        .map(|k| {
            let child = t.leaf(100.0, 10.0);
            t.floor(child, 40.0 + k as f64, 10.0);
            t.flex(
                child,
                Flex {
                    grow: 0.0,
                    shrink: 1.0 + k as f64 / 10.0,
                    basis: None,
                },
            )
        })
        .collect();
    let floors: f64 = (0..60).map(|k| 40.0 + k as f64).sum();
    let root = t.stack(Axis::Horizontal, 0.0, children.clone());
    let r = t.run_tight(root, Size::new(floors - 10.0, 10.0), LayoutDirection::Ltr);
    for (k, child) in children.iter().enumerate() {
        assert_eq!(r.nodes[child].rect.size.width, 40.0 + k as f64);
    }
    assert!(r.stats.flex_passes <= 32);
    assert_eq!(r.stats.flex_passes, 1, "one stack, one pass");
}

#[test]
fn a_container_child_cannot_shrink_below_what_its_own_children_need() {
    let mut t = TestTree::default();
    let (a, b) = (t.leaf(50.0, 10.0), t.leaf(50.0, 10.0));
    t.floor(a, 20.0, 10.0);
    t.floor(b, 15.0, 10.0);
    let inner = t.stack(Axis::Horizontal, 5.0, vec![a, b]);
    t.flex(
        inner,
        Flex {
            grow: 0.0,
            shrink: 1.0,
            basis: None,
        },
    );
    let other = t.leaf(100.0, 10.0);
    t.flex(
        other,
        Flex {
            grow: 0.0,
            shrink: 1.0,
            basis: None,
        },
    );
    let root = t.stack(Axis::Horizontal, 0.0, vec![inner, other]);
    let r = t.run_tight(root, Size::new(60.0, 10.0), LayoutDirection::Ltr);
    assert_eq!(r.nodes[&inner].rect.size.width, 40.0);
}

#[test]
fn a_second_layout_of_an_unchanged_tree_measures_nothing() {
    let mut t = TestTree::default();
    let leaves: Vec<usize> = (0..6).map(|i| t.leaf(10.0 + i as f64, 5.0)).collect();
    let inner = t.stack(Axis::Vertical, 1.0, leaves);
    let root = t.stack(Axis::Horizontal, 0.0, vec![inner]);
    let mut cache = LayoutCache::new();
    let first = t.run_cached(root, OPEN, &mut cache);
    assert!(first.stats.leaf_measures >= 6);
    let second = t.run_cached(root, OPEN, &mut cache);
    assert_eq!(second.stats.leaf_measures, 0);
    assert_eq!(second.stats.solves, 0);
    assert_eq!(first.nodes, second.nodes);
}

#[test]
fn changing_one_leaf_leaves_the_measurements_of_untouched_subtrees_alone() {
    let mut t = TestTree::default();
    let leaves: Vec<usize> = (0..6).map(|i| t.leaf(10.0 + i as f64, 5.0)).collect();
    let inner = t.stack(Axis::Vertical, 1.0, leaves.clone());
    let sibling = t.leaf(7.0, 7.0);
    let root = t.stack(Axis::Horizontal, 0.0, vec![inner, sibling]);
    let mut cache = LayoutCache::new();
    t.run_cached(root, OPEN, &mut cache);

    t.nodes[leaves[2]].leaf = Size::new(14.0, 6.0);
    t.touch(leaves[2]);
    let after = t.run_cached(root, OPEN, &mut cache);
    assert!(after.stats.leaf_measures <= 4, "{:?}", after.stats);
    assert_eq!(
        after.nodes[&inner].rect.size,
        Size::new(15.0, 5.0 * 5.0 + 6.0 + 5.0)
    );
    assert_eq!(after.nodes, t.run(root, OPEN).nodes);
}

#[test]
fn removed_nodes_and_a_new_direction_do_not_leave_stale_results() {
    let mut t = TestTree::default();
    let (a, b) = (t.leaf(10.0, 5.0), t.leaf(20.0, 5.0));
    let root = t.stack(Axis::Horizontal, 0.0, vec![a, b]);
    let mut cache = LayoutCache::new();
    t.run_cached(root, OPEN, &mut cache);

    t.nodes[root].children = vec![a];
    t.touch(root);
    let r = t.run_cached(root, OPEN, &mut cache);
    assert_eq!(r.nodes.len(), 2);
    assert_eq!(r.nodes[&root].rect.size.width, 10.0);

    t.nodes[root].children = vec![a, b];
    t.touch(root);
    let options = cupertino_ui_layout::LayoutOptions {
        direction: LayoutDirection::Rtl,
        node_limit: 100,
    };
    let tight = cupertino_ui_layout::Constraints::tight(Size::new(100.0, 5.0)).unwrap();
    let rtl = cupertino_ui_layout::layout_cached(&t, root, tight, &options, &mut cache).unwrap();
    assert_eq!(rtl.nodes[&a].rect.origin.x, 90.0);
}

#[test]
fn spans_and_positions_must_be_finite() {
    let mut t = TestTree::default();
    let a = t.leaf(1.0, 1.0);
    t.spec(a).position = Point::new(f64::NAN, 0.0);
    let root = t.container(ContainerKind::Absolute, vec![a]);
    assert!(t.try_run(root, OPEN, LayoutDirection::Ltr).is_err());
    t.spec(a).position = Point::ZERO;
    t.size(a, SizeSpec::Fit, SizeSpec::Fit);
    assert!(t.try_run(root, OPEN, LayoutDirection::Ltr).is_ok());
}

fn tall_spans(count: usize) -> (TestTree, usize) {
    let mut t = TestTree::default();
    let kids: Vec<usize> = (0..count)
        .map(|_| {
            let leaf = t.leaf(1.0, 1.0);
            span(&mut t, leaf, 1, count)
        })
        .collect();
    let root = t.grid(vec![Track::Auto], vec![], Size::ZERO, kids);
    (t, root)
}

#[test]
fn grid_spans_beyond_the_node_limit_fail_before_the_occupancy_grid_is_built() {
    let (t, root) = tall_spans(50_000);
    let options = LayoutOptions {
        direction: LayoutDirection::Ltr,
        node_limit: 100_000,
    };
    let available = Constraints::loose(OPEN).unwrap();
    assert_eq!(
        layout(&t, root, available, &options).unwrap_err(),
        LayoutError::NodeLimit
    );
}

#[test]
fn grid_spans_within_the_node_limit_still_stack_in_order() {
    let (t, root) = tall_spans(40);
    let r = t.run(root, OPEN);
    assert_eq!(r.nodes.len(), 41);
    let tops: Vec<f64> = (0..40).map(|i| r.nodes[&i].rect.origin.y).collect();
    assert!(tops.windows(2).all(|w| w[0] < w[1]));
}
