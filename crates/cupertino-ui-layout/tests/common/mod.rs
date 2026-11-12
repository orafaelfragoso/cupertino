use cupertino_geometry::{Insets, Size};
use cupertino_ui_layout::{
    layout, layout_cached, Align, Axis, Constraints, ContainerKind, Flex, Generation, Justify,
    LayoutCache, LayoutDirection, LayoutError, LayoutOptions, LayoutResult, LayoutSpec, LayoutTree,
    Measured, SizeSpec, Track,
};

pub struct Node {
    pub spec: LayoutSpec,
    pub children: Vec<usize>,
    pub leaf: Size,
    pub baseline: Option<f64>,
    /// Text-like leaf: its width is capped by the constraint and its height follows from this area.
    pub area: Option<f64>,
    /// What the leaf answers when asked for its smallest size, a maximum of zero on an axis.
    pub floor: Size,
    pub generation: u64,
}

#[derive(Default)]
pub struct TestTree {
    pub nodes: Vec<Node>,
    pub leaf_measures: std::cell::Cell<usize>,
}

impl LayoutTree for TestTree {
    type Id = usize;

    fn children(&self, id: usize) -> &[usize] {
        &self.nodes[id].children
    }

    fn spec(&self, id: usize) -> &LayoutSpec {
        &self.nodes[id].spec
    }

    fn generation(&self, id: usize) -> Generation {
        Generation(self.nodes[id].generation)
    }

    fn measure_leaf(&self, id: usize, c: Constraints) -> Measured {
        self.leaf_measures.set(self.leaf_measures.get() + 1);
        let node = &self.nodes[id];
        let size = match node.area {
            Some(area) => {
                let width = node.leaf.width.min(c.width.max());
                Size::new(width, area / width)
            }
            None => node.leaf,
        };
        let size = Size::new(
            if c.width.max() == 0.0 {
                node.floor.width
            } else {
                size.width
            },
            if c.height.max() == 0.0 {
                node.floor.height
            } else {
                size.height
            },
        );
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
            floor: Size::ZERO,
            generation: 0,
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

    /// Marks the node changed, as its owner would after editing its spec, children or content.
    pub fn touch(&mut self, id: usize) {
        self.nodes[id].generation += 1;
    }

    pub fn floor(&mut self, id: usize, width: f64, height: f64) -> usize {
        self.nodes[id].floor = Size::new(width, height);
        id
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

    pub fn run_cached(
        &self,
        root: usize,
        available: Size,
        cache: &mut LayoutCache<usize>,
    ) -> LayoutResult<usize> {
        let options = LayoutOptions {
            direction: LayoutDirection::Ltr,
            node_limit: 10_000,
        };
        layout_cached(
            self,
            root,
            Constraints::loose(available).unwrap(),
            &options,
            cache,
        )
        .unwrap()
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
