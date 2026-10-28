//! The paint pass. Each node paints in its own coordinates into its own [`PaintNode`]; the transform places it in
//! the window and the clip is every clipping ancestor's, intersected, in the node's coordinates. A node repaints
//! only when it changed or its size did, and keeps its paint revision otherwise, so the renderer can skip it.

use crate::cx::PaintCx;
use crate::route::Geometry;
use crate::tree::Tree;
use crate::{Environment, NodeId};
use cupertino_geometry::{Rect, Size};
use cupertino_ui_platform::Timestamp;
use cupertino_ui_render::{
    DisplayList, PaintNode, PaintNodeId, PaintOp, PaintRevision, Shape, Transform,
};
use std::collections::{HashMap, HashSet};

struct Painted {
    source: u64,
    size: Size,
    revision: PaintRevision,
    ops: Vec<PaintOp>,
}

/// One window's paint, kept between frames.
#[derive(Default)]
pub(crate) struct Painter {
    nodes: HashMap<NodeId, Painted>,
    revision: u64,
}

pub(crate) struct PaintInput<'a> {
    pub(crate) env: &'a Environment,
    pub(crate) focus: Option<NodeId>,
    pub(crate) focus_visible: bool,
    pub(crate) window_active: bool,
    pub(crate) now: Timestamp,
}

pub(crate) fn paint_node_id(id: NodeId) -> PaintNodeId {
    PaintNodeId(id.0.get())
}

impl Painter {
    pub(crate) fn paint<M: 'static>(
        &mut self,
        tree: &Tree<M>,
        root: NodeId,
        geometry: &Geometry,
        input: &PaintInput<'_>,
    ) -> DisplayList {
        let mut list = DisplayList::default();
        let mut seen = HashSet::with_capacity(self.nodes.len());
        for id in tree.preorder(root) {
            let (Some(node), Some(placement)) = (tree.get(id), geometry.get(&id)) else {
                continue;
            };
            let size = placement.rect.size;
            let focused = input.focus == Some(id);
            // Focus and the window's active state change how a node paints, though nothing in the node changed.
            let source = node.paint_revision << 3
                | u64::from(input.window_active) << 2
                | u64::from(focused) << 1
                | u64::from(focused && input.focus_visible);
            let fresh = self
                .nodes
                .get(&id)
                .is_some_and(|p| p.source == source && p.size == size);
            if !fresh {
                let mut ops = Vec::new();
                node.widget.paint(
                    &*node.state,
                    &mut PaintCx {
                        node: id,
                        size,
                        env: input.env,
                        focused,
                        focus_visible: focused && input.focus_visible,
                        window_active: input.window_active,
                        now: input.now,
                        ops: &mut ops,
                    },
                );
                self.revision += 1;
                self.nodes.insert(
                    id,
                    Painted {
                        source,
                        size,
                        revision: PaintRevision(self.revision),
                        ops,
                    },
                );
            }
            let painted = &self.nodes[&id];
            let origin = placement.rect.origin;
            list.nodes.push(PaintNode {
                id: paint_node_id(id),
                parent: node.parent.map(paint_node_id),
                revision: painted.revision,
                transform: Transform([1.0, 0.0, 0.0, 1.0, origin.x, origin.y]),
                clip: placement
                    .clip
                    .map(|c| Shape::Rect(c.translate(-origin.x, -origin.y))),
                bounds: placement.rect,
                hit: placement
                    .clip
                    .map_or(Some(placement.rect), |c| c.intersection(&placement.rect))
                    .unwrap_or(Rect::new(origin.x, origin.y, 0.0, 0.0)),
                ops: painted.ops.clone(),
            });
            seen.insert(id);
        }
        self.nodes.retain(|id, _| seen.contains(id));
        list
    }
}

#[cfg(test)]
mod tests;
