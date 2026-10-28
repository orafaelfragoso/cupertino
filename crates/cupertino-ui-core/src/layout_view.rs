//! The bridge to layout: the node tree seen as a [`LayoutTree`], and layout's parent-relative result turned into
//! window placements. A node's clip limits its children, so a node is clipped by its ancestors' clips only.

use crate::cx::MeasureCx;
use crate::route::{Geometry, Placement};
use crate::tree::Tree;
use crate::{Environment, NodeId};
use cupertino_geometry::{Point, Rect};
use cupertino_ui_layout::{
    Constraints, Generation, LayoutResult, LayoutSpec, LayoutTree, Measured,
};

pub(crate) struct TreeView<'a, M> {
    pub(crate) tree: &'a Tree<M>,
    pub(crate) env: &'a Environment,
}

impl<M: 'static> LayoutTree for TreeView<'_, M> {
    type Id = NodeId;

    fn children(&self, id: NodeId) -> &[NodeId] {
        self.tree.get(id).map_or(&[], |n| &n.children)
    }

    fn spec(&self, id: NodeId) -> &LayoutSpec {
        &self
            .tree
            .get(id)
            .expect("layout asks only for nodes the tree handed it")
            .spec
    }

    fn generation(&self, id: NodeId) -> Generation {
        self.tree
            .get(id)
            .map_or(Generation::default(), |n| n.generation)
    }

    fn measure_leaf(&self, id: NodeId, constraints: Constraints) -> Measured {
        let node = self
            .tree
            .get(id)
            .expect("layout asks only for nodes the tree handed it");
        node.widget.measure(
            &*node.state,
            constraints,
            &mut MeasureCx {
                node: id,
                env: self.env,
            },
        )
    }
}

/// Window placements from a layout of the subtree under `root`.
pub(crate) fn placements<M: 'static>(
    tree: &Tree<M>,
    root: NodeId,
    layout: &LayoutResult<NodeId>,
) -> Geometry {
    let mut geometry = Geometry::with_capacity(layout.nodes.len());
    let mut stack = vec![(root, Point::new(0.0, 0.0), None::<Rect>)];
    while let Some((id, parent_origin, clip)) = stack.pop() {
        let Some(node) = layout.nodes.get(&id) else {
            continue;
        };
        let rect = node.rect.translate(parent_origin.x, parent_origin.y);
        geometry.insert(id, Placement { rect, clip });
        let inner = match node.clip {
            Some(own) => {
                let own = own.translate(rect.origin.x, rect.origin.y);
                Some(match clip {
                    Some(outer) => outer.intersection(&own).unwrap_or(Rect::new(
                        own.origin.x,
                        own.origin.y,
                        0.0,
                        0.0,
                    )),
                    None => own,
                })
            }
            None => clip,
        };
        if let Some(n) = tree.get(id) {
            stack.extend(n.children.iter().map(|&c| (c, rect.origin, inner)));
        }
    }
    geometry
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{group, leaf};
    use crate::Element;
    use cupertino_ui_layout::NodeLayout;

    #[test]
    fn placements_add_up_parent_origins_and_a_clip_limits_only_the_descendants() {
        let mut tree = Tree::<String>::new(10);
        let view = Element::new(group(vec![Element::new(group(vec![Element::new(leaf(
            "x",
        ))]))]));
        let root = tree.reconcile(None, view).unwrap();
        let scroller = tree.get(root).unwrap().children[0];
        let content = tree.get(scroller).unwrap().children[0];
        let at = |x, y, w, h, clip| NodeLayout {
            rect: Rect::new(x, y, w, h),
            clip,
            baseline: None,
        };
        let layout = LayoutResult {
            nodes: [
                (root, at(0.0, 0.0, 200.0, 200.0, None)),
                (
                    scroller,
                    at(
                        10.0,
                        20.0,
                        100.0,
                        50.0,
                        Some(Rect::new(0.0, 0.0, 100.0, 50.0)),
                    ),
                ),
                (content, at(0.0, -30.0, 100.0, 300.0, None)),
            ]
            .into(),
        };

        let g = placements(&tree, root, &layout);

        assert_eq!(g[&scroller].rect, Rect::new(10.0, 20.0, 100.0, 50.0));
        assert_eq!(
            g[&scroller].clip, None,
            "a node's own clip does not cut the node"
        );
        assert_eq!(g[&content].rect, Rect::new(10.0, -10.0, 100.0, 300.0));
        assert_eq!(g[&content].clip, Some(Rect::new(10.0, 20.0, 100.0, 50.0)));
    }
}
