//! Minimal dependency-free benchmark for whole-tree layout: time per call, sizing cache misses and leaf measurements.
use cupertino_geometry::{Insets, Size};
use cupertino_ui_layout::{
    layout, layout_cached, Align, Axis, Constraints, ContainerKind, Flex, Generation, Justify,
    LayoutCache, LayoutDirection, LayoutOptions, LayoutSpec, LayoutTree, Measured, SizeSpec,
};
use std::hint::black_box;
use std::time::Instant;

struct Tree {
    nodes: Vec<(LayoutSpec, Vec<usize>)>,
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

    fn measure_leaf(&self, id: usize, c: Constraints) -> Measured {
        let width = (20.0 + (id % 13) as f64 * 7.0).min(c.width.max());
        Measured {
            size: Size::new(width, 12.0 + (id % 5) as f64),
            baseline: Some(10.0),
        }
    }
}

/// Rows of cells nested in columns, alternating axes, until about `target` nodes exist.
fn build(target: usize) -> Tree {
    let mut nodes: Vec<(LayoutSpec, Vec<usize>)> = Vec::new();
    let add = |nodes: &mut Vec<(LayoutSpec, Vec<usize>)>, spec: LayoutSpec, kids: Vec<usize>| {
        nodes.push((spec, kids));
        nodes.len() - 1
    };
    let mut rows = Vec::new();
    while nodes.len() < target {
        let cells: Vec<usize> = (0..8)
            .map(|i| {
                let mut spec = LayoutSpec::new(ContainerKind::Leaf);
                spec.flex = if i % 3 == 0 {
                    Flex::grow(1.0)
                } else {
                    Flex::NONE
                };
                add(&mut nodes, spec, Vec::new())
            })
            .collect();
        let mut row = LayoutSpec::new(ContainerKind::Stack {
            axis: Axis::Horizontal,
            spacing: 4.0,
            align: Align::Center,
            justify: Justify::Start,
        });
        row.insets = Insets::uniform(2.0);
        rows.push(add(&mut nodes, row, cells));
    }
    let mut column = LayoutSpec::new(ContainerKind::Stack {
        axis: Axis::Vertical,
        spacing: 2.0,
        align: Align::Stretch,
        justify: Justify::Start,
    });
    column.width = SizeSpec::Fill;
    add(&mut nodes, column, rows);
    Tree { nodes }
}

fn bench(name: &str, tree: &Tree, iterations: usize, warm: bool) {
    let root = tree.nodes.len() - 1;
    let options = LayoutOptions {
        direction: LayoutDirection::Ltr,
        node_limit: usize::MAX,
    };
    let available = Constraints::loose(Size::new(1280.0, f64::INFINITY)).unwrap();
    let mut cache = LayoutCache::new();
    if warm {
        layout_cached(tree, root, available, &options, &mut cache).unwrap();
    }
    let start = Instant::now();
    let mut stats = None;
    for _ in 0..iterations {
        let result = if warm {
            layout_cached(tree, root, available, &options, &mut cache)
        } else {
            layout(tree, root, available, &options)
        }
        .unwrap();
        stats = Some(black_box(result).stats);
    }
    let per_call = start.elapsed().as_secs_f64() / iterations as f64;
    let stats = stats.unwrap();
    println!(
        "{name:<24} {:>5} nodes {:>10.1} us/layout  {:>6} solves  {:>6} leaf measures",
        tree.nodes.len(),
        per_call * 1e6,
        stats.solves,
        stats.leaf_measures
    );
}

fn main() {
    for (name, nodes, iterations) in [("1k", 1_000, 200), ("10k", 10_000, 20)] {
        let tree = build(nodes);
        bench(&format!("rows of 8 ({name})"), &tree, iterations, false);
        bench(
            &format!("rows of 8 ({name}, warm)"),
            &tree,
            iterations,
            true,
        );
    }
}
