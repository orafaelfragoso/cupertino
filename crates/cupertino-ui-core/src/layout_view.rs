//! The bridge to layout: the node tree seen as a [`LayoutTree`], and layout's parent-relative result turned into
//! window placements. A node's clip limits its children, so a node is clipped by its ancestors' clips only.

use crate::cx::MeasureCx;
use crate::paint::unobscured;
use crate::route::{Geometry, Placement};
use crate::tree::Tree;
use crate::{Environment, NodeId};
use cupertino_geometry::{Point, Rect, Size};
use cupertino_ui_layout::{
    Constraints, ContainerKind, Generation, LayoutDirection, LayoutResult, LayoutSpec, LayoutTree,
    Measured,
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

/// Window placements from a layout of the subtree under `root`. A scroll container's children move by its scroll
/// offset, which counts from the start edge: in right-to-left layout, where layout mirrors the content, a horizontal
/// offset moves the content right. A scroll container outside the raised subtrees `cut` (window rects) clips its
/// content to the part of it they leave ([`unobscured`]): where raised nodes draw in place, what scrolls stays out
/// from under them.
pub(crate) fn placements<M: 'static>(
    tree: &Tree<M>,
    root: NodeId,
    layout: &LayoutResult<NodeId>,
    env: &Environment,
    cut: &[(NodeId, Rect)],
) -> Geometry {
    let mut geometry = Geometry::with_capacity(layout.nodes.len());
    let mut stack = vec![(root, Point::new(0.0, 0.0), None::<Rect>, false)];
    while let Some((id, parent_origin, clip, raised)) = stack.pop() {
        let Some(node) = layout.nodes.get(&id) else {
            continue;
        };
        let rect = node.rect.translate(parent_origin.x, parent_origin.y);
        let raised = raised || tree.get(id).is_some_and(|n| n.widget.raised(&*n.state));
        let scrolls = tree
            .get(id)
            .is_some_and(|n| matches!(n.spec.kind, ContainerKind::Scroll { .. }));
        let own_clip = node
            .clip
            .map(|c| c.translate(rect.origin.x, rect.origin.y))
            .map(|c| {
                if scrolls && !raised && !cut.is_empty() {
                    let shown = unobscured(rect, cut);
                    c.intersection(&shown).unwrap_or(Rect::new(
                        c.origin.x,
                        shown.min_y(),
                        c.width(),
                        0.0,
                    ))
                } else {
                    c
                }
            });
        let least = tree
            .get(id)
            .map_or(Size::ZERO, |n| n.widget.target(&*n.state, env));
        geometry.insert(
            id,
            Placement {
                rect,
                target: grown(rect, least),
                clip,
                own_clip,
            },
        );
        let inner = match own_clip {
            Some(own) => Some(match clip {
                Some(outer) => outer.intersection(&own).unwrap_or(Rect::new(
                    own.origin.x,
                    own.origin.y,
                    0.0,
                    0.0,
                )),
                None => own,
            }),
            None => clip,
        };
        if let Some(n) = tree.get(id) {
            let mut origin = rect.origin;
            if let ContainerKind::Scroll {
                horizontal,
                vertical,
            } = n.spec.kind
            {
                let offset = n.widget.scroll_offset(&*n.state);
                if horizontal {
                    origin.x += match env.direction {
                        LayoutDirection::Ltr => -offset.x,
                        LayoutDirection::Rtl => offset.x,
                    };
                }
                if vertical {
                    origin.y -= offset.y;
                }
            }
            stack.extend(n.children.iter().map(|&c| (c, origin, inner, raised)));
        }
    }
    geometry
}

/// `rect` grown evenly about its centre to at least `least`.
fn grown(rect: Rect, least: Size) -> Rect {
    let width = rect.size.width.max(least.width);
    let height = rect.size.height.max(least.height);
    Rect::new(
        rect.origin.x - (width - rect.size.width) / 2.0,
        rect.origin.y - (height - rect.size.height) / 2.0,
        width,
        height,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::route::hit_test;
    use crate::test_support::{env, group, leaf, Scroller};
    use crate::Element;
    use cupertino_ui_layout::{LayoutStats, NodeLayout};

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
            stats: LayoutStats::default(),
        };

        let g = placements(&tree, root, &layout, &env(), &[]);

        assert_eq!(g[&scroller].rect, Rect::new(10.0, 20.0, 100.0, 50.0));
        assert_eq!(
            g[&scroller].clip, None,
            "a node's own clip does not cut the node"
        );
        assert_eq!(g[&content].rect, Rect::new(10.0, -10.0, 100.0, 300.0));
        assert_eq!(g[&content].clip, Some(Rect::new(10.0, 20.0, 100.0, 50.0)));
    }

    /// A 100-point viewport over 300 points of content holding `first` and `last`, scrolled 200 points from the
    /// start edge, so `last` is in view; layout put the content at the start edge, mirrored in right-to-left.
    fn scrolled_to_the_end(direction: LayoutDirection) -> (Tree<String>, NodeId, Geometry) {
        let mut tree = Tree::<String>::new(10);
        let view = Element::new(Scroller {
            children: vec![Element::new(group(vec![
                Element::new(leaf("first")).key("first"),
                Element::new(leaf("last")).key("last"),
            ]))],
            offset: Point::new(200.0, 0.0),
        });
        let root = tree.reconcile(None, view).unwrap();
        let content = tree.get(root).unwrap().children[0];
        let [first, last] = tree.get(content).unwrap().children[..] else {
            unreachable!()
        };
        let at = |x, w, clip| NodeLayout {
            rect: Rect::new(x, 0.0, w, 10.0),
            clip,
            baseline: None,
        };
        let (content_x, first_x, last_x) = match direction {
            LayoutDirection::Ltr => (0.0, 0.0, 290.0),
            LayoutDirection::Rtl => (-200.0, 290.0, 0.0),
        };
        let layout = LayoutResult {
            nodes: [
                (root, at(0.0, 100.0, Some(Rect::new(0.0, 0.0, 100.0, 10.0)))),
                (content, at(content_x, 300.0, None)),
                (first, at(first_x, 10.0, None)),
                (last, at(last_x, 10.0, None)),
            ]
            .into(),
            stats: LayoutStats::default(),
        };
        let geometry = placements(
            &tree,
            root,
            &layout,
            &Environment { direction, ..env() },
            &[],
        );
        (tree, root, geometry)
    }

    #[test]
    fn a_scroll_offset_from_the_start_edge_moves_the_content_against_the_reading_direction() {
        for (direction, x) in [(LayoutDirection::Ltr, 95.0), (LayoutDirection::Rtl, 5.0)] {
            let (tree, root, geometry) = scrolled_to_the_end(direction);
            let hit = hit_test(&tree, root, &geometry, Point::new(x, 5.0)).unwrap();
            let target = tree.get(*hit.last().unwrap()).unwrap();
            assert_eq!(target.key, Some("last".into()), "{direction:?}");
            let outside = if x < 50.0 { 95.0 } else { 5.0 };
            let missed = hit_test(&tree, root, &geometry, Point::new(outside, 5.0)).unwrap();
            assert_eq!(
                missed.len(),
                2,
                "{direction:?}: only the viewport and the content are there"
            );
        }
    }
}
