use crate::common;

use common::TestTree;
use cupertino_geometry::Size;
use cupertino_ui_layout::{
    Align, Axis, ContainerKind, Flex, Justify, LayoutCache, LayoutDirection, LayoutResult, SizeSpec,
};
use proptest::prelude::*;

#[derive(Clone, Debug)]
enum Shape {
    Leaf {
        width: f64,
        height: f64,
    },
    Stack {
        vertical: bool,
        spacing: f64,
        align: u8,
        justify: u8,
        children: Vec<(Shape, f64, u8)>,
    },
}

fn size_spec(code: u8, value: f64) -> SizeSpec {
    match code % 4 {
        0 => SizeSpec::Fit,
        1 => SizeSpec::Fill,
        2 => SizeSpec::Fixed(value),
        _ => SizeSpec::Fraction(value / 100.0),
    }
}

fn shape() -> impl Strategy<Value = Shape> {
    let leaf =
        (0.0..80.0f64, 0.0..80.0f64).prop_map(|(width, height)| Shape::Leaf { width, height });
    leaf.prop_recursive(3, 24, 4, |inner| {
        (
            any::<bool>(),
            0.0..10.0f64,
            0u8..5,
            0u8..4,
            prop::collection::vec((inner, 0.0..3.0f64, any::<u8>()), 0..4),
        )
            .prop_map(
                |(vertical, spacing, align, justify, children)| Shape::Stack {
                    vertical,
                    spacing,
                    align,
                    justify,
                    children,
                },
            )
    })
}

fn build(tree: &mut TestTree, shape: &Shape, grow: f64, code: u8) -> usize {
    let id = match shape {
        Shape::Leaf { width, height } => tree.leaf(*width, *height),
        Shape::Stack {
            vertical,
            spacing,
            align,
            justify,
            children,
        } => {
            let kids = children
                .iter()
                .map(|(child, grow, code)| build(tree, child, *grow, *code))
                .collect();
            let id = tree.stack(
                if *vertical {
                    Axis::Vertical
                } else {
                    Axis::Horizontal
                },
                *spacing,
                kids,
            );
            if let ContainerKind::Stack {
                align: a,
                justify: j,
                ..
            } = &mut tree.spec(id).kind
            {
                *a = [
                    Align::Start,
                    Align::Center,
                    Align::End,
                    Align::Stretch,
                    Align::Baseline,
                ][*align as usize % 5];
                *j = [
                    Justify::Start,
                    Justify::Center,
                    Justify::End,
                    Justify::SpaceBetween,
                ][*justify as usize % 4];
            }
            id
        }
    };
    tree.flex(
        id,
        Flex {
            grow,
            shrink: grow,
            basis: if code.is_multiple_of(3) {
                Some(f64::from(code % 40))
            } else {
                None
            },
        },
    );
    tree.size(
        id,
        size_spec(code, f64::from(code)),
        size_spec(code / 4, f64::from(code)),
    );
    id
}

fn sizes(result: &LayoutResult<usize>) -> Vec<(usize, Size)> {
    let mut all: Vec<_> = result
        .nodes
        .iter()
        .map(|(id, n)| (*id, n.rect.size))
        .collect();
    all.sort_by_key(|(id, _)| *id);
    all
}

proptest! {
    #[test]
    fn any_tree_lays_out_to_finite_non_negative_rects_inside_its_constraints(
        shape in shape(),
        width in 0.0..400.0f64,
        height in 0.0..400.0f64,
        tight in any::<bool>(),
    ) {
        let mut tree = TestTree::default();
        let root = build(&mut tree, &shape, 0.0, 1);
        let available = Size::new(width, height);
        let result = if tight {
            tree.run_tight(root, available, LayoutDirection::Ltr)
        } else {
            tree.run(root, available)
        };
        prop_assert_eq!(result.nodes.len(), tree.nodes.len());
        for node in result.nodes.values() {
            let r = node.rect;
            prop_assert!([r.origin.x, r.origin.y, r.size.width, r.size.height].iter().all(|v| v.is_finite()));
            prop_assert!(r.size.width >= 0.0 && r.size.height >= 0.0);
        }
        let root_size = result.nodes[&root].rect.size;
        prop_assert!(root_size.width <= width + 1e-9 && root_size.height <= height + 1e-9);
        if tight {
            prop_assert_eq!(root_size, available);
        }
    }

    #[test]
    fn a_stack_without_shrink_never_overlaps_its_children_on_the_main_axis(
        widths in prop::collection::vec(0.0..50.0f64, 0..8),
        spacing in 0.0..8.0f64,
        room in 0.0..600.0f64,
        justify in 0u8..4,
    ) {
        let mut tree = TestTree::default();
        let kids: Vec<usize> = widths.iter().map(|w| tree.leaf(*w, 5.0)).collect();
        let root = tree.stack(Axis::Horizontal, spacing, kids.clone());
        if let ContainerKind::Stack { justify: j, .. } = &mut tree.spec(root).kind {
            *j = [Justify::Start, Justify::Center, Justify::End, Justify::SpaceBetween][justify as usize];
        }
        let result = tree.run_tight(root, Size::new(room, 10.0), LayoutDirection::Ltr);
        let rects: Vec<_> = kids.iter().map(|k| result.nodes[k].rect).collect();
        for pair in rects.windows(2) {
            prop_assert!(pair[0].max_x() + spacing <= pair[1].min_x() + 1e-9);
        }
    }

    #[test]
    fn grow_fills_the_room_exactly_when_the_children_can_grow(
        weights in prop::collection::vec(0.1..5.0f64, 1..6),
        room in 50.0..800.0f64,
    ) {
        let mut tree = TestTree::default();
        let kids: Vec<usize> = weights.iter().map(|w| {
            let leaf = tree.leaf(1.0, 5.0);
            tree.flex(leaf, Flex::grow(*w))
        }).collect();
        let root = tree.stack(Axis::Horizontal, 0.0, kids.clone());
        let result = tree.run_tight(root, Size::new(room, 10.0), LayoutDirection::Ltr);
        let total: f64 = kids.iter().map(|k| result.nodes[k].rect.size.width).sum();
        prop_assert!((total - room).abs() < 1e-6, "{total} vs {room}");
    }

    #[test]
    fn right_to_left_is_the_mirror_image_of_left_to_right(shape in shape(), width in 1.0..400.0f64, height in 1.0..400.0f64) {
        let mut tree = TestTree::default();
        let root = build(&mut tree, &shape, 0.0, 1);
        let size = Size::new(width, height);
        let ltr = tree.run_tight(root, size, LayoutDirection::Ltr);
        let rtl = tree.run_tight(root, size, LayoutDirection::Rtl);
        prop_assert_eq!(sizes(&ltr), sizes(&rtl));
        for (id, node) in &tree_children(&tree) {
            let parent_width = ltr.nodes[id].rect.size.width;
            for child in node {
                let l = ltr.nodes[child].rect;
                let r = rtl.nodes[child].rect;
                prop_assert!((r.origin.x - (parent_width - l.max_x())).abs() < 1e-9);
                prop_assert_eq!(r.origin.y, l.origin.y);
            }
        }
    }

    #[test]
    fn a_cached_layout_after_edits_equals_a_fresh_one(
        shape in shape(),
        edits in prop::collection::vec((any::<u8>(), 0.0..80.0f64, 0.0..80.0f64), 1..5),
        width in 50.0..400.0f64,
    ) {
        let mut tree = TestTree::default();
        let root = build(&mut tree, &shape, 0.0, 1);
        let size = Size::new(width, 300.0);
        let mut cache = LayoutCache::new();
        tree.run_cached(root, size, &mut cache);
        for (pick, w, h) in edits {
            let leaves: Vec<usize> = (0..tree.nodes.len()).filter(|i| tree.nodes[*i].children.is_empty()).collect();
            let leaf = leaves[pick as usize % leaves.len()];
            tree.nodes[leaf].leaf = Size::new(w, h);
            tree.touch(leaf);
            let cached = tree.run_cached(root, size, &mut cache);
            let fresh = tree.run(root, size);
            prop_assert_eq!(cached.nodes, fresh.nodes);
        }
    }

    #[test]
    fn layout_is_deterministic(shape in shape(), width in 0.0..400.0f64) {
        let mut tree = TestTree::default();
        let root = build(&mut tree, &shape, 0.0, 1);
        let size = Size::new(width, 300.0);
        let first = tree.run(root, size);
        let second = tree.run(root, size);
        for (id, node) in &first.nodes {
            prop_assert_eq!(*node, second.nodes[id]);
        }
    }
}

fn tree_children(tree: &TestTree) -> Vec<(usize, Vec<usize>)> {
    tree.nodes
        .iter()
        .enumerate()
        .map(|(id, n)| (id, n.children.clone()))
        .collect()
}
