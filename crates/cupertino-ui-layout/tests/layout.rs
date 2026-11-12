use crate::common;

use common::TestTree;
use cupertino_geometry::{Insets, Rect, Size};
use cupertino_ui_layout::{
    Align, Axis, ContainerKind, Flex, Justify, LayoutDirection, LayoutError, Overflow, SizeSpec,
    SplitSpec, Track,
};

const OPEN: Size = Size::new(f64::INFINITY, f64::INFINITY);

fn set_stack(tree: &mut TestTree, id: usize, align: Align, justify: Justify) {
    if let ContainerKind::Stack {
        align: a,
        justify: j,
        ..
    } = &mut tree.spec(id).kind
    {
        *a = align;
        *j = justify;
    }
}

#[test]
fn a_horizontal_stack_places_children_in_order_with_spacing() {
    let mut t = TestTree::default();
    let (a, b, c) = (t.leaf(10.0, 4.0), t.leaf(20.0, 8.0), t.leaf(5.0, 6.0));
    let root = t.stack(Axis::Horizontal, 3.0, vec![a, b, c]);
    let r = t.run(root, OPEN);
    assert_eq!(r.nodes[&root].rect, Rect::new(0.0, 0.0, 41.0, 8.0));
    assert_eq!(r.nodes[&a].rect, Rect::new(0.0, 0.0, 10.0, 4.0));
    assert_eq!(r.nodes[&b].rect, Rect::new(13.0, 0.0, 20.0, 8.0));
    assert_eq!(r.nodes[&c].rect, Rect::new(36.0, 0.0, 5.0, 6.0));
}

#[test]
fn leftover_space_is_shared_by_grow_weights() {
    let mut t = TestTree::default();
    let a = t.leaf(5.0, 10.0);
    let b = t.leaf(5.0, 10.0);
    t.flex(a, Flex::grow(1.0));
    t.flex(b, Flex::grow(2.0));
    let root = t.stack(Axis::Horizontal, 0.0, vec![a, b]);
    let r = t.run_tight(root, Size::new(300.0, 10.0), LayoutDirection::Ltr);
    assert_eq!(r.nodes[&a].rect, Rect::new(0.0, 0.0, 100.0, 10.0));
    assert_eq!(r.nodes[&b].rect, Rect::new(100.0, 0.0, 200.0, 10.0));
}

#[test]
fn a_fit_stack_does_not_grow_its_children_past_its_content() {
    let mut t = TestTree::default();
    let a = t.leaf(40.0, 10.0);
    t.flex(
        a,
        Flex {
            grow: 1.0,
            shrink: 1.0,
            basis: None,
        },
    );
    let root = t.stack(Axis::Horizontal, 0.0, vec![a]);
    let r = t.run(root, Size::new(500.0, 100.0));
    assert_eq!(r.nodes[&a].rect.size.width, 40.0);
}

#[test]
fn a_shortfall_is_taken_from_children_in_proportion_to_their_size() {
    let mut t = TestTree::default();
    let a = t.leaf(60.0, 10.0);
    let b = t.leaf(20.0, 10.0);
    t.flex(
        a,
        Flex {
            grow: 0.0,
            shrink: 1.0,
            basis: None,
        },
    );
    t.flex(
        b,
        Flex {
            grow: 0.0,
            shrink: 1.0,
            basis: None,
        },
    );
    let root = t.stack(Axis::Horizontal, 0.0, vec![a, b]);
    let r = t.run_tight(root, Size::new(40.0, 10.0), LayoutDirection::Ltr);
    assert_eq!(r.nodes[&a].rect.size.width, 30.0);
    assert_eq!(r.nodes[&b].rect.size.width, 10.0);
}

#[test]
fn a_fixed_child_keeps_its_size_while_others_shrink() {
    let mut t = TestTree::default();
    let fixed = t.leaf(1.0, 10.0);
    t.size(fixed, SizeSpec::Fixed(60.0), SizeSpec::Fit);
    t.flex(
        fixed,
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
    let root = t.stack(Axis::Horizontal, 0.0, vec![fixed, soft]);
    let r = t.run_tight(root, Size::new(100.0, 10.0), LayoutDirection::Ltr);
    assert_eq!(r.nodes[&fixed].rect.size.width, 60.0);
    assert_eq!(r.nodes[&soft].rect.size.width, 40.0);
}

#[test]
fn justify_moves_the_run_or_spreads_it() {
    for (justify, expected) in [
        (Justify::Start, [0.0, 10.0]),
        (Justify::Center, [40.0, 50.0]),
        (Justify::End, [80.0, 90.0]),
        (Justify::SpaceBetween, [0.0, 90.0]),
    ] {
        let mut t = TestTree::default();
        let (a, b) = (t.leaf(10.0, 10.0), t.leaf(10.0, 10.0));
        let root = t.stack(Axis::Horizontal, 0.0, vec![a, b]);
        set_stack(&mut t, root, Align::Start, justify);
        let r = t.run_tight(root, Size::new(100.0, 10.0), LayoutDirection::Ltr);
        assert_eq!(
            [r.nodes[&a].rect.origin.x, r.nodes[&b].rect.origin.x],
            expected,
            "{justify:?}"
        );
    }
}

#[test]
fn cross_axis_alignment_places_or_stretches_children() {
    for (align, y, height) in [
        (Align::Start, 0.0, 10.0),
        (Align::Center, 15.0, 10.0),
        (Align::End, 30.0, 10.0),
        (Align::Stretch, 0.0, 40.0),
    ] {
        let mut t = TestTree::default();
        let a = t.leaf(10.0, 10.0);
        let root = t.stack(Axis::Horizontal, 0.0, vec![a]);
        set_stack(&mut t, root, align, Justify::Start);
        let r = t.run_tight(root, Size::new(100.0, 40.0), LayoutDirection::Ltr);
        assert_eq!(
            (r.nodes[&a].rect.origin.y, r.nodes[&a].rect.size.height),
            (y, height),
            "{align:?}"
        );
    }
}

#[test]
fn stretch_leaves_a_child_with_a_fixed_cross_size_alone() {
    let mut t = TestTree::default();
    let a = t.leaf(10.0, 10.0);
    t.size(a, SizeSpec::Fit, SizeSpec::Fixed(12.0));
    let root = t.stack(Axis::Horizontal, 0.0, vec![a]);
    set_stack(&mut t, root, Align::Stretch, Justify::Start);
    let r = t.run_tight(root, Size::new(100.0, 40.0), LayoutDirection::Ltr);
    assert_eq!(r.nodes[&a].rect.size.height, 12.0);
}

#[test]
fn baseline_alignment_lines_up_text_of_different_sizes() {
    let mut t = TestTree::default();
    let small = t.leaf(10.0, 12.0);
    t.nodes[small].baseline = Some(9.0);
    let large = t.leaf(10.0, 24.0);
    t.nodes[large].baseline = Some(18.0);
    let root = t.stack(Axis::Horizontal, 0.0, vec![small, large]);
    set_stack(&mut t, root, Align::Baseline, Justify::Start);
    let r = t.run(root, OPEN);
    assert_eq!(r.nodes[&small].rect.origin.y, 9.0);
    assert_eq!(r.nodes[&large].rect.origin.y, 0.0);
    assert_eq!(r.nodes[&root].baseline, Some(18.0));
    assert_eq!(r.nodes[&root].rect.size.height, 24.0);
}

#[test]
fn a_text_child_wraps_to_the_width_a_vertical_stack_offers() {
    let mut t = TestTree::default();
    let text = t.text(200.0, 2000.0);
    let root = t.stack(Axis::Vertical, 0.0, vec![text]);
    let r = t.run(root, Size::new(100.0, f64::INFINITY));
    assert_eq!(r.nodes[&text].rect.size, Size::new(100.0, 20.0));
}

#[test]
fn right_to_left_mirrors_the_main_axis_and_start_alignment() {
    let mut t = TestTree::default();
    let (a, b) = (t.leaf(10.0, 10.0), t.leaf(20.0, 10.0));
    let row = t.stack(Axis::Horizontal, 5.0, vec![a, b]);
    let r = t.run_tight(row, Size::new(100.0, 10.0), LayoutDirection::Rtl);
    assert_eq!(r.nodes[&a].rect.origin.x, 90.0);
    assert_eq!(r.nodes[&b].rect.origin.x, 65.0);

    let mut t = TestTree::default();
    let a = t.leaf(10.0, 10.0);
    let column = t.stack(Axis::Vertical, 0.0, vec![a]);
    let r = t.run_tight(column, Size::new(100.0, 10.0), LayoutDirection::Rtl);
    assert_eq!(r.nodes[&a].rect.origin.x, 90.0);
}

#[test]
fn insets_pad_the_content_and_fill_and_fraction_use_the_offered_size() {
    let mut t = TestTree::default();
    let a = t.leaf(10.0, 10.0);
    t.size(a, SizeSpec::Fraction(0.5), SizeSpec::Fill);
    let root = t.container(ContainerKind::Overlay, vec![a]);
    t.insets(root, Insets::new(1.0, 2.0, 3.0, 4.0));
    t.size(root, SizeSpec::Fill, SizeSpec::Fill);
    let r = t.run(root, Size::new(100.0, 50.0));
    assert_eq!(r.nodes[&root].rect, Rect::new(0.0, 0.0, 100.0, 50.0));
    assert_eq!(r.nodes[&a].rect, Rect::new(2.0, 1.0, 47.0, 46.0));
}

#[test]
fn grid_tracks_mix_fixed_auto_and_fraction_and_flow_row_by_row() {
    let mut t = TestTree::default();
    let cells: Vec<usize> = (0..4)
        .map(|i| t.leaf(10.0 + i as f64 * 10.0, 5.0))
        .collect();
    let root = t.grid(
        vec![Track::Fixed(30.0), Track::Auto, Track::Fraction(1.0)],
        vec![Track::Auto],
        Size::new(2.0, 1.0),
        cells.clone(),
    );
    let r = t.run_tight(root, Size::new(200.0, 20.0), LayoutDirection::Ltr);
    let rect = |i: usize| r.nodes[&cells[i]].rect;
    assert_eq!(rect(0), Rect::new(0.0, 0.0, 30.0, 5.0));
    assert_eq!(rect(1), Rect::new(32.0, 0.0, 20.0, 5.0));
    assert_eq!(rect(2), Rect::new(54.0, 0.0, 146.0, 5.0));
    assert_eq!(rect(3), Rect::new(0.0, 6.0, 30.0, 5.0));
}

#[test]
fn a_grid_without_a_definite_size_gives_fractions_the_room_their_content_needs() {
    let mut t = TestTree::default();
    let (a, b) = (t.leaf(40.0, 5.0), t.leaf(10.0, 5.0));
    let root = t.grid(
        vec![Track::Fraction(1.0), Track::Fraction(1.0)],
        vec![],
        Size::ZERO,
        vec![a, b],
    );
    let r = t.run(root, Size::new(1000.0, 1000.0));
    assert_eq!(r.nodes[&root].rect.size.width, 80.0);
}

#[test]
fn absolute_children_keep_their_size_at_the_origin_and_overlay_children_stretch() {
    let mut t = TestTree::default();
    let (a, b) = (t.leaf(10.0, 10.0), t.leaf(30.0, 20.0));
    let absolute = t.container(ContainerKind::Absolute, vec![a, b]);
    let r = t.run(absolute, OPEN);
    assert_eq!(r.nodes[&absolute].rect.size, Size::new(30.0, 20.0));
    assert_eq!(r.nodes[&a].rect, Rect::new(0.0, 0.0, 10.0, 10.0));

    let mut t = TestTree::default();
    let (a, b) = (t.leaf(10.0, 10.0), t.leaf(30.0, 20.0));
    t.spec(b).align_self = Some(Align::End);
    let overlay = t.container(ContainerKind::Overlay, vec![a, b]);
    let r = t.run(overlay, OPEN);
    assert_eq!(r.nodes[&a].rect, Rect::new(0.0, 0.0, 30.0, 20.0));
    assert_eq!(r.nodes[&b].rect, Rect::new(0.0, 0.0, 30.0, 20.0));
}

#[test]
fn a_scroll_view_clips_and_keeps_its_content_at_full_size() {
    let mut t = TestTree::default();
    let content = t.leaf(100.0, 1000.0);
    let scroll = t.container(
        ContainerKind::Scroll {
            horizontal: false,
            vertical: true,
        },
        vec![content],
    );
    t.size(scroll, SizeSpec::Fill, SizeSpec::Fill);
    let r = t.run(scroll, Size::new(100.0, 300.0));
    assert_eq!(r.nodes[&scroll].rect.size, Size::new(100.0, 300.0));
    assert_eq!(
        r.nodes[&scroll].clip,
        Some(Rect::new(0.0, 0.0, 100.0, 300.0))
    );
    assert_eq!(r.nodes[&content].rect, Rect::new(0.0, 0.0, 100.0, 1000.0));
}

#[test]
fn scroll_content_stretches_across_the_axis_that_does_not_scroll() {
    let mut t = TestTree::default();
    let content = t.leaf(10.0, 1000.0);
    let scroll = t.container(
        ContainerKind::Scroll {
            horizontal: false,
            vertical: true,
        },
        vec![content],
    );
    t.size(scroll, SizeSpec::Fill, SizeSpec::Fill);
    let r = t.run(scroll, Size::new(100.0, 300.0));
    assert_eq!(r.nodes[&content].rect.size.width, 100.0);
}

#[test]
fn only_a_clipping_container_has_a_clip() {
    let mut t = TestTree::default();
    let a = t.leaf(10.0, 10.0);
    let root = t.stack(Axis::Horizontal, 0.0, vec![a]);
    assert_eq!(t.run(root, OPEN).nodes[&root].clip, None);
    t.spec(root).overflow = Overflow::Clip;
    assert_eq!(
        t.run(root, OPEN).nodes[&root].clip,
        Some(Rect::new(0.0, 0.0, 10.0, 10.0))
    );
}

#[test]
fn an_empty_container_is_empty() {
    let mut t = TestTree::default();
    let root = t.stack(Axis::Vertical, 4.0, vec![]);
    assert_eq!(t.run(root, OPEN).nodes[&root].rect.size, Size::ZERO);
}

#[test]
fn invalid_input_is_a_typed_error() {
    let mut t = TestTree::default();
    let a = t.leaf(10.0, 10.0);
    t.size(a, SizeSpec::Fixed(f64::NAN), SizeSpec::Fit);
    let root = t.stack(Axis::Horizontal, 0.0, vec![a]);
    assert_eq!(
        t.try_run(root, OPEN, LayoutDirection::Ltr).unwrap_err(),
        LayoutError::NonFiniteValue
    );

    t.size(a, SizeSpec::Fixed(-1.0), SizeSpec::Fit);
    assert_eq!(
        t.try_run(root, OPEN, LayoutDirection::Ltr).unwrap_err(),
        LayoutError::InvalidConstraint
    );

    let mut t = TestTree::default();
    let a = t.leaf(10.0, 10.0);
    let root = t.stack(Axis::Horizontal, 0.0, vec![a, a]);
    assert_eq!(
        t.try_run(root, OPEN, LayoutDirection::Ltr).unwrap_err(),
        LayoutError::CycleOrMissingNode
    );

    let mut t = TestTree::default();
    let a = t.leaf(f64::NAN, 10.0);
    let root = t.stack(Axis::Horizontal, 0.0, vec![a]);
    assert_eq!(
        t.try_run(root, OPEN, LayoutDirection::Ltr).unwrap_err(),
        LayoutError::NonFiniteValue
    );
}

#[test]
fn a_cycle_and_an_oversized_tree_are_refused() {
    let mut t = TestTree::default();
    let a = t.stack(Axis::Horizontal, 0.0, vec![]);
    t.nodes[a].children.push(a);
    assert_eq!(
        t.try_run(a, OPEN, LayoutDirection::Ltr).unwrap_err(),
        LayoutError::CycleOrMissingNode
    );

    let mut t = TestTree::default();
    let leaves: Vec<usize> = (0..11).map(|_| t.leaf(1.0, 1.0)).collect();
    let root = t.stack(Axis::Horizontal, 0.0, leaves);
    let options = cupertino_ui_layout::LayoutOptions {
        direction: LayoutDirection::Ltr,
        node_limit: 10,
    };
    let available = cupertino_ui_layout::Constraints::loose(OPEN).unwrap();
    assert_eq!(
        cupertino_ui_layout::layout(&t, root, available, &options).unwrap_err(),
        LayoutError::NodeLimit
    );

    let mut t = TestTree::default();
    let mut inner = t.leaf(1.0, 1.0);
    for _ in 0..300 {
        inner = t.stack(Axis::Horizontal, 0.0, vec![inner]);
    }
    assert_eq!(
        t.try_run(inner, OPEN, LayoutDirection::Ltr).unwrap_err(),
        LayoutError::NodeLimit
    );
}

/// Alternating axes with every flex and alignment mode mixed in: `depth` levels, each holding a leaf and the next level.
fn nested(depth: usize) -> (TestTree, usize) {
    let mut t = TestTree::default();
    let mut inner = t.leaf(10.0, 10.0);
    for level in 0..depth {
        let sibling = t.leaf(5.0 + (level % 7) as f64, 4.0);
        t.flex(
            sibling,
            match level % 3 {
                0 => Flex::grow(1.0),
                1 => Flex {
                    grow: 0.0,
                    shrink: 1.0,
                    basis: None,
                },
                _ => Flex::NONE,
            },
        );
        t.flex(
            inner,
            if level % 2 == 0 {
                Flex::grow(2.0)
            } else {
                Flex {
                    grow: 1.0,
                    shrink: 1.0,
                    basis: None,
                }
            },
        );
        let axis = if level % 2 == 0 {
            Axis::Horizontal
        } else {
            Axis::Vertical
        };
        inner = t.stack(axis, 1.0, vec![sibling, inner]);
        let align = [Align::Center, Align::Stretch, Align::Start, Align::End][level % 4];
        set_stack(&mut t, inner, align, Justify::Center);
        if level % 5 == 0 {
            t.size(inner, SizeSpec::Fill, SizeSpec::Fill);
        }
    }
    (t, inner)
}

#[test]
fn layout_work_grows_linearly_with_depth_on_mixed_nested_stacks() {
    let mut calls = Vec::new();
    for depth in [25, 50, 100, 200] {
        let (t, root) = nested(depth);
        let r = t.run_tight(root, Size::new(900.0, 700.0), LayoutDirection::Ltr);
        assert_eq!(r.nodes.len(), 2 * depth + 1);
        calls.push((depth, t.leaf_measures.get(), r.stats.solves));
    }
    for (depth, measures, solves) in &calls {
        let nodes = 2 * depth + 1;
        assert!(
            *measures <= 4 * nodes,
            "{depth} levels took {measures} leaf measurements"
        );
        assert!(
            *solves <= 5 * nodes,
            "{depth} levels took {solves} sizing cache misses"
        );
    }
}

#[test]
fn a_horizontal_scroll_view_starts_at_its_right_edge_in_right_to_left() {
    let mut t = TestTree::default();
    let content = t.leaf(500.0, 20.0);
    let scroll = t.container(
        ContainerKind::Scroll {
            horizontal: true,
            vertical: false,
        },
        vec![content],
    );
    t.size(scroll, SizeSpec::Fill, SizeSpec::Fill);
    let r = t.run_tight(scroll, Size::new(200.0, 20.0), LayoutDirection::Rtl);
    assert_eq!(r.nodes[&content].rect, Rect::new(-300.0, 0.0, 500.0, 20.0));
    let r = t.run_tight(scroll, Size::new(200.0, 20.0), LayoutDirection::Ltr);
    assert_eq!(r.nodes[&content].rect, Rect::new(0.0, 0.0, 500.0, 20.0));
}

#[test]
fn many_children_that_each_hit_a_bound_still_settle() {
    let mut t = TestTree::default();
    let kids: Vec<usize> = (0..60)
        .map(|i| {
            let leaf = t.leaf(1.0 + i as f64, 5.0);
            t.flex(
                leaf,
                Flex {
                    grow: 0.0,
                    shrink: 1.0 + (i % 7) as f64,
                    basis: None,
                },
            )
        })
        .collect();
    for i in (0..60).step_by(2) {
        t.size(kids[i], SizeSpec::Fixed(40.0), SizeSpec::Fit);
    }
    let root = t.stack(Axis::Horizontal, 0.0, kids.clone());
    let r = t.run_tight(root, Size::new(300.0, 10.0), LayoutDirection::Ltr);
    let total: f64 = kids.iter().map(|k| r.nodes[k].rect.size.width).sum();
    assert!(total.is_finite() && r.nodes.len() == 61);
}

#[test]
fn the_result_reports_the_work_it_did() {
    let mut t = TestTree::default();
    let (a, b) = (t.leaf(10.0, 10.0), t.leaf(20.0, 10.0));
    let root = t.stack(Axis::Horizontal, 0.0, vec![a, b]);
    let r = t.run(root, OPEN);
    assert_eq!(r.stats.leaf_measures, t.leaf_measures.get());
    assert!(r.stats.solves >= 3);
}

fn split(t: &mut TestTree, leading: f64) -> (usize, usize, usize) {
    let (a, b) = (t.leaf(10.0, 10.0), t.leaf(10.0, 10.0));
    let root = t.container(
        ContainerKind::Split(SplitSpec {
            axis: Axis::Horizontal,
            leading,
            gap: 1.0,
            min_leading: 128.0,
            min_trailing: 128.0,
        }),
        vec![a, b],
    );
    (root, a, b)
}

#[test]
fn a_split_keeps_the_trailing_minimum_and_then_hides_the_leading_child() {
    let mut t = TestTree::default();
    let (root, a, b) = split(&mut t, 200.0);
    let at = |width: f64| {
        let r = t.run_tight(root, Size::new(width, 50.0), LayoutDirection::Ltr);
        (r.nodes[&a].rect, r.nodes[&b].rect)
    };
    assert_eq!(
        at(500.0),
        (
            Rect::new(0.0, 0.0, 200.0, 50.0),
            Rect::new(201.0, 0.0, 299.0, 50.0)
        )
    );
    assert_eq!(
        at(300.0),
        (
            Rect::new(0.0, 0.0, 171.0, 50.0),
            Rect::new(172.0, 0.0, 128.0, 50.0)
        ),
        "the leading child gives way first"
    );
    assert_eq!(
        at(257.0),
        (
            Rect::new(0.0, 0.0, 128.0, 50.0),
            Rect::new(129.0, 0.0, 128.0, 50.0)
        )
    );
    assert_eq!(
        at(250.0),
        (
            Rect::new(0.0, 0.0, 0.0, 50.0),
            Rect::new(0.0, 0.0, 250.0, 50.0)
        ),
        "both minimums do not fit"
    );
}

#[test]
fn a_split_raises_a_short_leading_child_to_its_minimum_and_mirrors_in_right_to_left() {
    let mut t = TestTree::default();
    let (root, a, b) = split(&mut t, 40.0);
    let r = t.run_tight(root, Size::new(400.0, 50.0), LayoutDirection::Rtl);
    assert_eq!(r.nodes[&a].rect, Rect::new(272.0, 0.0, 128.0, 50.0));
    assert_eq!(r.nodes[&b].rect, Rect::new(0.0, 0.0, 271.0, 50.0));
}
