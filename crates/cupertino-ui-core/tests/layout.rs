//! The layout engine over a tree of its own: where stacks, grids, wraps and splits put their children, which trees
//! it refuses, and that reusing earlier work never changes the answer.

use cupertino_geometry::{Insets, Rect, Size};
use cupertino_ui_core::layout::{
    layout, layout_cached, Align, Axis, AxisConstraint, Constraints, ContainerKind, Flex,
    Generation, GridSpan, Justify, LayoutCache, LayoutDirection, LayoutError, LayoutOptions,
    LayoutSpec, LayoutTree, Measured, SizeSpec, SplitSpec, Track,
};

/// A tree of leaves of a given size and containers, by index.
#[derive(Default)]
struct Tree {
    specs: Vec<LayoutSpec>,
    children: Vec<Vec<usize>>,
    sizes: Vec<Size>,
}

impl Tree {
    fn add(&mut self, spec: LayoutSpec, size: Size, children: Vec<usize>) -> usize {
        self.specs.push(spec);
        self.sizes.push(size);
        self.children.push(children);
        self.specs.len() - 1
    }

    fn leaf(&mut self, width: f64, height: f64) -> usize {
        self.add(
            LayoutSpec::new(ContainerKind::Leaf),
            Size::new(width, height),
            vec![],
        )
    }

    fn leaf_with(
        &mut self,
        width: f64,
        height: f64,
        change: impl FnOnce(&mut LayoutSpec),
    ) -> usize {
        let mut spec = LayoutSpec::new(ContainerKind::Leaf);
        change(&mut spec);
        self.add(spec, Size::new(width, height), vec![])
    }

    fn container(&mut self, kind: ContainerKind, children: Vec<usize>) -> usize {
        self.add(LayoutSpec::new(kind), Size::ZERO, children)
    }
}

impl LayoutTree for Tree {
    type Id = usize;

    fn children(&self, id: usize) -> &[usize] {
        &self.children[id]
    }

    fn spec(&self, id: usize) -> &LayoutSpec {
        &self.specs[id]
    }

    fn generation(&self, _: usize) -> Generation {
        Generation(0)
    }

    fn measure_leaf(&self, id: usize, constraints: Constraints) -> Measured {
        let size = self.sizes[id];
        Measured {
            size: Size::new(
                constraints.width.clamp(size.width),
                constraints.height.clamp(size.height),
            ),
            baseline: None,
        }
    }
}

const OPTIONS: LayoutOptions = LayoutOptions {
    direction: LayoutDirection::Ltr,
    node_limit: 1000,
};

/// The rectangles of `ids` once `root` is laid out in exactly `size`.
fn placed(
    tree: &Tree,
    root: usize,
    size: Size,
    options: &LayoutOptions,
    ids: &[usize],
) -> Vec<[f64; 4]> {
    let result = layout(tree, root, Constraints::tight(size).unwrap(), options).unwrap();
    ids.iter()
        .map(|id| {
            let r = result.nodes[id].rect;
            [r.x(), r.y(), r.width(), r.height()]
        })
        .collect()
}

fn near(actual: &[[f64; 4]], expected: &[[f64; 4]], what: &str) {
    assert_eq!(actual.len(), expected.len(), "{what}");
    for (a, e) in actual.iter().zip(expected) {
        assert!(
            a.iter().zip(e).all(|(a, e)| (a - e).abs() < 1e-6),
            "{what}: {actual:?} is not {expected:?}"
        );
    }
}

#[test]
fn a_stack_places_its_children_by_justify_align_spacing_direction_and_flex() {
    const ROW: Size = Size::new(200.0, 60.0);
    let stack = |axis, spacing, align, justify| ContainerKind::Stack {
        axis,
        spacing,
        align,
        justify,
    };
    type Rows = Vec<[f64; 4]>;
    // each row: what it shows, the stack, each child's flex, the direction, and the rectangles it comes to
    let rows: Vec<(&str, ContainerKind, [Flex; 3], LayoutDirection, Rows)> = vec![
        (
            "natural sizes from the start",
            stack(Axis::Horizontal, 0.0, Align::Start, Justify::Start),
            [Flex::NONE; 3],
            LayoutDirection::Ltr,
            vec![
                [0.0, 0.0, 40.0, 20.0],
                [40.0, 0.0, 60.0, 30.0],
                [100.0, 0.0, 30.0, 10.0],
            ],
        ),
        (
            "spacing between children only",
            stack(Axis::Horizontal, 10.0, Align::Start, Justify::Start),
            [Flex::NONE; 3],
            LayoutDirection::Ltr,
            vec![
                [0.0, 0.0, 40.0, 20.0],
                [50.0, 0.0, 60.0, 30.0],
                [120.0, 0.0, 30.0, 10.0],
            ],
        ),
        (
            "justified to the end",
            stack(Axis::Horizontal, 0.0, Align::Start, Justify::End),
            [Flex::NONE; 3],
            LayoutDirection::Ltr,
            vec![
                [70.0, 0.0, 40.0, 20.0],
                [110.0, 0.0, 60.0, 30.0],
                [170.0, 0.0, 30.0, 10.0],
            ],
        ),
        (
            "justified to the centre",
            stack(Axis::Horizontal, 0.0, Align::Start, Justify::Center),
            [Flex::NONE; 3],
            LayoutDirection::Ltr,
            vec![
                [35.0, 0.0, 40.0, 20.0],
                [75.0, 0.0, 60.0, 30.0],
                [135.0, 0.0, 30.0, 10.0],
            ],
        ),
        (
            "space between",
            stack(Axis::Horizontal, 0.0, Align::Start, Justify::SpaceBetween),
            [Flex::NONE; 3],
            LayoutDirection::Ltr,
            vec![
                [0.0, 0.0, 40.0, 20.0],
                [75.0, 0.0, 60.0, 30.0],
                [170.0, 0.0, 30.0, 10.0],
            ],
        ),
        (
            "centred across",
            stack(Axis::Horizontal, 0.0, Align::Center, Justify::Start),
            [Flex::NONE; 3],
            LayoutDirection::Ltr,
            vec![
                [0.0, 20.0, 40.0, 20.0],
                [40.0, 15.0, 60.0, 30.0],
                [100.0, 25.0, 30.0, 10.0],
            ],
        ),
        (
            "at the far side across",
            stack(Axis::Horizontal, 0.0, Align::End, Justify::Start),
            [Flex::NONE; 3],
            LayoutDirection::Ltr,
            vec![
                [0.0, 40.0, 40.0, 20.0],
                [40.0, 30.0, 60.0, 30.0],
                [100.0, 50.0, 30.0, 10.0],
            ],
        ),
        (
            "stretched across",
            stack(Axis::Horizontal, 0.0, Align::Stretch, Justify::Start),
            [Flex::NONE; 3],
            LayoutDirection::Ltr,
            vec![
                [0.0, 0.0, 40.0, 60.0],
                [40.0, 0.0, 60.0, 60.0],
                [100.0, 0.0, 30.0, 60.0],
            ],
        ),
        (
            "leftover room shared by grow",
            stack(Axis::Horizontal, 0.0, Align::Start, Justify::Start),
            [Flex::grow(1.0), Flex::NONE, Flex::grow(3.0)],
            LayoutDirection::Ltr,
            vec![
                [0.0, 0.0, 35.0, 20.0],
                [35.0, 0.0, 60.0, 30.0],
                [95.0, 0.0, 105.0, 10.0],
            ],
        ),
        (
            "mirrored right to left",
            stack(Axis::Horizontal, 0.0, Align::Start, Justify::Start),
            [Flex::NONE; 3],
            LayoutDirection::Rtl,
            vec![
                [160.0, 0.0, 40.0, 20.0],
                [100.0, 0.0, 60.0, 30.0],
                [70.0, 0.0, 30.0, 10.0],
            ],
        ),
        (
            "down a column",
            stack(Axis::Vertical, 5.0, Align::Start, Justify::Start),
            [Flex::NONE; 3],
            LayoutDirection::Ltr,
            vec![
                [0.0, 0.0, 40.0, 20.0],
                [0.0, 25.0, 60.0, 30.0],
                [0.0, 60.0, 30.0, 10.0],
            ],
        ),
    ];
    for (what, kind, flex, direction, expected) in rows {
        let mut tree = Tree::default();
        let kids: Vec<usize> = [(40.0, 20.0), (60.0, 30.0), (30.0, 10.0)]
            .into_iter()
            .zip(flex)
            .map(|((w, h), flex)| tree.leaf_with(w, h, |s| s.flex = flex))
            .collect();
        let root = tree.container(kind, kids.clone());
        let size = if matches!(
            tree.specs[root].kind,
            ContainerKind::Stack {
                axis: Axis::Vertical,
                ..
            }
        ) {
            Size::new(60.0, 200.0)
        } else {
            ROW
        };
        let options = LayoutOptions {
            direction,
            ..OPTIONS
        };
        near(&placed(&tree, root, size, &options, &kids), &expected, what);
    }
}

#[test]
fn grids_wraps_and_splits_place_their_children_in_tracks_rows_and_by_the_split_rule() {
    // a grid: a fixed column, one that takes what is left, one as wide as its widest cell; the second row is fixed, and
    // a cell fills the tracks it covers, a span of two columns the gap between them too
    let mut tree = Tree::default();
    let cells: Vec<usize> = vec![
        tree.leaf(10.0, 12.0),
        tree.leaf(10.0, 12.0),
        tree.leaf(30.0, 12.0),
        tree.leaf_with(10.0, 8.0, |s| {
            s.span = GridSpan {
                columns: 2,
                rows: 1,
            }
        }),
        tree.leaf(10.0, 8.0),
    ];
    let grid = ContainerKind::Grid {
        columns: vec![Track::Fixed(50.0), Track::Fraction(1.0), Track::Auto],
        rows: vec![Track::Auto, Track::Fixed(20.0)],
        gaps: Size::new(5.0, 3.0),
    };
    let root = tree.container(grid, cells.clone());
    near(
        &placed(&tree, root, Size::new(200.0, 100.0), &OPTIONS, &cells),
        &[
            [0.0, 0.0, 50.0, 12.0],
            [55.0, 0.0, 110.0, 12.0],
            [170.0, 0.0, 30.0, 12.0],
            [0.0, 15.0, 165.0, 20.0],
            [170.0, 15.0, 30.0, 20.0],
        ],
        "a grid",
    );

    // a wrap: as many at their natural size in a row as fit; a full row spreads what is left between its children and
    // the last row keeps that spacing, so equal children line up in columns
    let mut tree = Tree::default();
    let items: Vec<usize> = (0..5).map(|_| tree.leaf(30.0, 10.0)).collect();
    let root = tree.container(
        ContainerKind::Wrap {
            gaps: Size::new(4.0, 2.0),
        },
        items.clone(),
    );
    near(
        &placed(&tree, root, Size::new(100.0, 40.0), &OPTIONS, &items),
        &[
            [0.0, 0.0, 30.0, 10.0],
            [35.0, 0.0, 30.0, 10.0],
            [70.0, 0.0, 30.0, 10.0],
            [0.0, 12.0, 30.0, 10.0],
            [35.0, 12.0, 30.0, 10.0],
        ],
        "a wrap",
    );

    // a split: the leading child at its asked extent within what leaves the trailing one its minimum, hidden when
    // the two minimums do not fit
    let split = SplitSpec {
        axis: Axis::Horizontal,
        leading: 80.0,
        gap: 4.0,
        min_leading: 40.0,
        min_trailing: 50.0,
    };
    for (extent, shown) in [
        (300.0, Some(80.0)),
        (100.0, Some(46.0)),
        (94.0, Some(40.0)),
        (93.0, None),
    ] {
        assert_eq!(split.shown(extent), shown, "a split {extent} wide");
    }
    let mut tree = Tree::default();
    let (leading, trailing) = (tree.leaf(10.0, 10.0), tree.leaf(10.0, 10.0));
    let root = tree.container(ContainerKind::Split(split), vec![leading, trailing]);
    let both = placed(
        &tree,
        root,
        Size::new(100.0, 30.0),
        &OPTIONS,
        &[leading, trailing],
    );
    near(
        &both,
        &[[0.0, 0.0, 46.0, 30.0], [50.0, 0.0, 50.0, 30.0]],
        "a split that shows both",
    );
}

#[test]
fn insets_clip_and_fixed_sizes_hold_whatever_the_content_asks() {
    let mut tree = Tree::default();
    let big = tree.leaf_with(500.0, 500.0, |s| {
        s.width = SizeSpec::Fixed(70.0);
        s.height = SizeSpec::Fraction(0.5);
    });
    let root = {
        let mut spec = LayoutSpec::new(ContainerKind::Stack {
            axis: Axis::Vertical,
            spacing: 0.0,
            align: Align::Start,
            justify: Justify::Start,
        });
        spec.insets = Insets::new(10.0, 20.0, 5.0, 4.0);
        spec.overflow = cupertino_ui_core::layout::Overflow::Clip;
        tree.add(spec, Size::ZERO, vec![big])
    };
    let result = layout(
        &tree,
        root,
        Constraints::tight(Size::new(120.0, 100.0)).unwrap(),
        &OPTIONS,
    )
    .unwrap();
    let r = result.nodes[&big].rect;
    assert_eq!(
        (r.x(), r.y(), r.width(), r.height()),
        (20.0, 10.0, 70.0, 42.5),
        "inside the insets"
    );
    assert_eq!(
        result.nodes[&root].clip,
        Some(Rect::new(0.0, 0.0, 120.0, 100.0)),
        "a clipping node says what it clips to"
    );
    assert_eq!(result.nodes[&big].clip, None);
}

#[test]
fn a_tree_or_a_constraint_that_cannot_be_laid_out_is_refused_without_a_partial_result() {
    let size = Constraints::tight(Size::new(100.0, 100.0)).unwrap();
    let mut chain = Tree::default();
    let mut last = chain.leaf(1.0, 1.0);
    for _ in 0..20 {
        last = chain.container(ContainerKind::Absolute, vec![last]);
    }
    let few = LayoutOptions {
        node_limit: 10,
        ..OPTIONS
    };
    assert_eq!(
        layout(&chain, last, size, &few).err(),
        Some(LayoutError::NodeLimit)
    );
    assert!(layout(&chain, last, size, &OPTIONS).is_ok());

    let mut shared = Tree::default();
    let leaf = shared.leaf(1.0, 1.0);
    let root = shared.container(ContainerKind::Absolute, vec![leaf, leaf]);
    assert_eq!(
        layout(&shared, root, size, &OPTIONS).err(),
        Some(LayoutError::CycleOrMissingNode)
    );

    let mut cycle = Tree::default();
    let a = cycle.container(ContainerKind::Absolute, vec![]);
    cycle.children[a] = vec![a];
    assert_eq!(
        layout(&cycle, a, size, &OPTIONS).err(),
        Some(LayoutError::CycleOrMissingNode)
    );

    assert_eq!(
        AxisConstraint::new(5.0, 3.0, 9.0),
        Err(LayoutError::InvalidConstraint)
    );
    assert_eq!(
        AxisConstraint::new(-1.0, 3.0, 9.0),
        Err(LayoutError::InvalidConstraint)
    );
    assert_eq!(
        AxisConstraint::new(0.0, f64::NAN, 9.0),
        Err(LayoutError::NonFiniteValue)
    );
    assert_eq!(
        AxisConstraint::loose(f64::NAN),
        Err(LayoutError::NonFiniteValue)
    );
    assert!(
        AxisConstraint::loose(f64::INFINITY).is_ok(),
        "only the maximum may be infinite"
    );
}

#[test]
fn reusing_the_work_of_an_earlier_layout_does_not_change_the_answer() {
    let mut tree = Tree::default();
    let kids: Vec<usize> = (0..6)
        .map(|i| tree.leaf(10.0 + f64::from(i), 8.0))
        .collect();
    let root = tree.container(
        ContainerKind::Stack {
            axis: Axis::Horizontal,
            spacing: 2.0,
            align: Align::Center,
            justify: Justify::SpaceBetween,
        },
        kids,
    );
    let size = Constraints::tight(Size::new(150.0, 30.0)).unwrap();
    let fresh = layout(&tree, root, size, &OPTIONS).unwrap();
    let mut cache = LayoutCache::new();
    let first = layout_cached(&tree, root, size, &OPTIONS, &mut cache).unwrap();
    let again = layout_cached(&tree, root, size, &OPTIONS, &mut cache).unwrap();
    for result in [&first, &again] {
        assert_eq!(result.nodes.len(), fresh.nodes.len());
        assert!(fresh
            .nodes
            .iter()
            .all(|(id, node)| result.nodes[id] == *node));
    }
    assert!(first.stats.leaf_measures > 0);
    assert_eq!(again.stats.leaf_measures, 0, "nothing is measured again");
    assert!(again.stats.solves < first.stats.solves);
    assert!(!cache.is_empty());
}
