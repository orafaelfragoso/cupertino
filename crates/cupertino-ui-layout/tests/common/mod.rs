#![allow(dead_code)]

use cupertino_geometry::{Insets, Size};
use cupertino_ui_layout::{
    layout, Align, Axis, Constraints, ContainerKind, Flex, Generation, Justify, LayoutDirection,
    LayoutError, LayoutOptions, LayoutResult, LayoutSpec, LayoutTree, Measured, SizeSpec, Track,
};

pub struct Node {
    pub spec: LayoutSpec,
    pub children: Vec<usize>,
    pub leaf: Size,
    pub baseline: Option<f64>,
    /// Text-like leaf: its width is capped by the constraint and its height follows from this area.
    pub area: Option<f64>,
}

#[derive(Default)]
pub struct TestTree {
    pub nodes: Vec<Node>,
}

impl LayoutTree for TestTree {
    type Id = usize;

    fn children(&self, id: usize) -> &[usize] {
        &self.nodes[id].children
    }

    fn spec(&self, id: usize) -> &LayoutSpec {
        &self.nodes[id].spec
    }

    fn generation(&self, _: usize) -> Generation {
        Generation(0)
    }

    fn measure_leaf(&self, id: usize, c: Constraints) -> Measured {
        let node = &self.nodes[id];
        let size = match node.area {
            Some(area) => {
                let width = node.leaf.width.min(c.width.max());
                Size::new(width, area / width)
            }
            None => node.leaf,
        };
        Measured {
            size,
            baseline: node.baseline,
        }
    }
}

impl TestTree {
    fn add(&mut self, kind: ContainerKind, children: Vec<usize>, leaf: Size) -> usize {
        self.nodes.push(Node {
            spec: LayoutSpec::new(kind),
            children,
            leaf,
            baseline: None,
            area: None,
        });
        self.nodes.len() - 1
    }

    pub fn leaf(&mut self, width: f64, height: f64) -> usize {
        self.add(ContainerKind::Leaf, Vec::new(), Size::new(width, height))
    }

    pub fn text(&mut self, width: f64, area: f64) -> usize {
        let id = self.leaf(width, 0.0);
        self.nodes[id].area = Some(area);
        id
    }

    pub fn stack(&mut self, axis: Axis, spacing: f64, children: Vec<usize>) -> usize {
        self.add(
            ContainerKind::Stack {
                axis,
                spacing,
                align: Align::Start,
                justify: Justify::Start,
            },
            children,
            Size::ZERO,
        )
    }

    pub fn grid(
        &mut self,
        columns: Vec<Track>,
        rows: Vec<Track>,
        gaps: Size,
        children: Vec<usize>,
    ) -> usize {
        self.add(
            ContainerKind::Grid {
                columns,
                rows,
                gaps,
            },
            children,
            Size::ZERO,
        )
    }

    pub fn container(&mut self, kind: ContainerKind, children: Vec<usize>) -> usize {
        self.add(kind, children, Size::ZERO)
    }

    pub fn spec(&mut self, id: usize) -> &mut LayoutSpec {
        &mut self.nodes[id].spec
    }

    pub fn flex(&mut self, id: usize, flex: Flex) -> usize {
        self.nodes[id].spec.flex = flex;
        id
    }

    pub fn size(&mut self, id: usize, width: SizeSpec, height: SizeSpec) -> usize {
        self.nodes[id].spec.width = width;
        self.nodes[id].spec.height = height;
        id
    }

    pub fn insets(&mut self, id: usize, insets: Insets) -> usize {
        self.nodes[id].spec.insets = insets;
        id
    }

    pub fn run(&self, root: usize, available: Size) -> LayoutResult<usize> {
        self.try_run(root, available, LayoutDirection::Ltr).unwrap()
    }

    pub fn try_run(
        &self,
        root: usize,
        available: Size,
        direction: LayoutDirection,
    ) -> Result<LayoutResult<usize>, LayoutError> {
        let options = LayoutOptions {
            direction,
            node_limit: 10_000,
        };
        layout(self, root, Constraints::loose(available)?, &options)
    }

    pub fn run_tight(
        &self,
        root: usize,
        size: Size,
        direction: LayoutDirection,
    ) -> LayoutResult<usize> {
        let options = LayoutOptions {
            direction,
            node_limit: 10_000,
        };
        layout(self, root, Constraints::tight(size).unwrap(), &options).unwrap()
    }
}
