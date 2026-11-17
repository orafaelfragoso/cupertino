//! Instruction counts of a layout solve of a file manager's window: a toolbar, a sidebar of 24 places beside a list of
//! 64 visible rows of five columns under a header, and a status bar, about 480 nodes. Solved cold (a new cache), again
//! with nothing changed (every size cached), and at a new width (the cache keyed to the old one).
//!
//! The wall-clock bench beside it (`cargo bench -p cupertino-ui-layout
//! --bench layout`) times synthetic trees of 1 000 and 10 000 nodes.

use cupertino_geometry::{Insets, Size};
use cupertino_ui_layout::{
    layout, layout_cached, Align, Axis, Constraints, ContainerKind, Flex, Generation, Justify,
    LayoutCache, LayoutDirection, LayoutOptions, LayoutResult, LayoutSpec, LayoutTree, Measured,
    SizeSpec, SplitSpec,
};
use gungraun::prelude::*;
use std::hint::black_box;

/// The window's size in points.
const WINDOW: Size = Size {
    width: 1000.0,
    height: 640.0,
};
/// The list rows on screen and the places in the sidebar.
const ROWS: usize = 64;
const PLACES: usize = 24;

struct Tree {
    nodes: Vec<(LayoutSpec, Vec<usize>)>,
    root: usize,
}

impl LayoutTree for Tree {
    type Id = usize;

    fn children(&self, id: usize) -> &[usize] {
        &self.nodes[id].1
    }

    fn spec(&self, id: usize) -> &LayoutSpec {
        &self.nodes[id].0
    }

    fn generation(&self, _: usize) -> Generation {
        Generation(0)
    }

    /// A label's width follows its node, like text of varying length; every line is 16 points tall.
    fn measure_leaf(&self, id: usize, c: Constraints) -> Measured {
        let width = (24.0 + (id % 17) as f64 * 9.0).min(c.width.max());
        Measured {
            size: Size::new(width, 16.0),
            baseline: Some(12.0),
        }
    }
}

fn stack(axis: Axis, spacing: f64, align: Align) -> LayoutSpec {
    LayoutSpec::new(ContainerKind::Stack {
        axis,
        spacing,
        align,
        justify: Justify::Start,
    })
}

fn leaf(width: SizeSpec, flex: Flex) -> LayoutSpec {
    let mut spec = LayoutSpec::new(ContainerKind::Leaf);
    spec.width = width;
    spec.flex = flex;
    spec
}

impl Tree {
    fn add(&mut self, spec: LayoutSpec, children: Vec<usize>) -> usize {
        self.nodes.push((spec, children));
        self.nodes.len() - 1
    }

    /// A row of `cells`, each a leaf of the given width and flex, 6 points apart, inset by 4.
    fn row(&mut self, cells: &[(SizeSpec, Flex)]) -> usize {
        let kids = cells
            .iter()
            .map(|&(width, flex)| self.add(leaf(width, flex), Vec::new()))
            .collect();
        let mut spec = stack(Axis::Horizontal, 6.0, Align::Center);
        spec.insets = Insets::uniform(4.0);
        spec.width = SizeSpec::Fill;
        self.add(spec, kids)
    }

    fn files_window() -> Tree {
        let mut t = Tree {
            nodes: Vec::new(),
            root: 0,
        };
        let fit = (SizeSpec::Fit, Flex::NONE);
        let grow = (SizeSpec::Fit, Flex::grow(1.0));
        let fixed = |w| (SizeSpec::Fixed(w), Flex::NONE);
        let mut toolbar = vec![fit; 12];
        toolbar[6] = grow;
        let toolbar = t.row(&toolbar);
        let places = (0..PLACES).map(|_| t.row(&[fixed(16.0), grow])).collect();
        let mut sidebar = stack(Axis::Vertical, 2.0, Align::Stretch);
        sidebar.insets = Insets::uniform(10.0);
        let sidebar = t.add(sidebar, places);
        let columns = [grow, fixed(140.0), fixed(80.0), fixed(120.0)];
        let header = t.row(&columns);
        let mut rows = vec![header];
        for _ in 0..ROWS {
            let mut cells = vec![fixed(16.0)];
            cells.extend(columns);
            rows.push(t.row(&cells));
        }
        let mut list = stack(Axis::Vertical, 0.0, Align::Stretch);
        list.width = SizeSpec::Fill;
        let list = t.add(list, rows);
        let split = t.add(
            LayoutSpec::new(ContainerKind::Split(SplitSpec {
                axis: Axis::Horizontal,
                leading: 220.0,
                gap: 1.0,
                min_leading: 150.0,
                min_trailing: 300.0,
            })),
            vec![sidebar, list],
        );
        let status = t.row(&[grow, fit]);
        let mut window = stack(Axis::Vertical, 0.0, Align::Stretch);
        window.width = SizeSpec::Fill;
        window.height = SizeSpec::Fill;
        t.root = t.add(window, vec![toolbar, split, status]);
        t
    }
}

fn options() -> LayoutOptions {
    LayoutOptions {
        direction: LayoutDirection::Ltr,
        node_limit: usize::MAX,
    }
}

fn window(width: f64) -> Constraints {
    Constraints::tight(Size::new(width, WINDOW.height)).unwrap()
}

/// The window's tree and a cache that holds its solve at the window's width.
fn solved() -> (Tree, LayoutCache<usize>) {
    let tree = Tree::files_window();
    let mut cache = LayoutCache::new();
    layout_cached(
        &tree,
        tree.root,
        window(WINDOW.width),
        &options(),
        &mut cache,
    )
    .unwrap();
    (tree, cache)
}

#[library_benchmark]
#[bench::files_window(setup = Tree::files_window)]
fn cold(tree: Tree) -> LayoutResult<usize> {
    black_box(layout(&tree, tree.root, window(WINDOW.width), &options()).unwrap())
}

#[library_benchmark]
#[bench::unchanged(args = (WINDOW.width), setup = with_solved)]
#[bench::resized(args = (WINDOW.width - 40.0), setup = with_solved)]
fn cached((tree, mut cache, width): (Tree, LayoutCache<usize>, f64)) -> LayoutResult<usize> {
    black_box(layout_cached(&tree, tree.root, window(width), &options(), &mut cache).unwrap())
}

fn with_solved(width: f64) -> (Tree, LayoutCache<usize>, f64) {
    let (tree, cache) = solved();
    (tree, cache, width)
}

library_benchmark_group!(name = solve, benchmarks = [cold, cached]);
main!(library_benchmark_groups = solve);
