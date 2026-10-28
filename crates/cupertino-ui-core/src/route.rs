//! Event delivery. An event for a target travels down the target's path from the root (capture), reaches the target,
//! and travels back up (bubble); any handler can stop it. Hit-testing picks the topmost node under a point,
//! respecting every clip on the way down.

use crate::cx::{Effects, EventCx, Status};
use crate::tree::Tree;
use crate::{Environment, Event, NodeId, Phase, Propagation};
use cupertino_geometry::{Point, Rect};
use std::collections::HashMap;

/// Where a node sits in its window, in logical points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Placement {
    pub(crate) rect: Rect,
    /// The intersection of every ancestor's clip; it limits this node and its subtree.
    pub(crate) clip: Option<Rect>,
    /// The clip this node puts on its descendants.
    pub(crate) own_clip: Option<Rect>,
}

impl Placement {
    /// Whether `point` lands on the node: inside its rect and every clip above it.
    pub(crate) fn hit(&self, point: Point) -> bool {
        self.rect.contains(point) && self.clip.is_none_or(|clip| clip.contains(point))
    }
}

pub(crate) type Geometry = HashMap<NodeId, Placement>;

pub(crate) struct Route<'a, M> {
    pub(crate) tree: &'a mut Tree<M>,
    pub(crate) geometry: &'a Geometry,
    pub(crate) env: &'a Environment,
    pub(crate) status: Status,
    pub(crate) effects: &'a mut Effects<M>,
}

impl<M: 'static> Route<'_, M> {
    /// Delivers `event` along `path` (root first, target last).
    pub(crate) fn dispatch(&mut self, path: &[NodeId], event: &Event) -> Propagation {
        let Some((&target, ancestors)) = path.split_last() else {
            return Propagation::Continue;
        };
        let steps = ancestors
            .iter()
            .map(|&n| (n, Phase::Capture))
            .chain([(target, Phase::Target)])
            .chain(ancestors.iter().rev().map(|&n| (n, Phase::Bubble)));
        for (node, phase) in steps {
            if self.deliver(node, target, phase, event) == Propagation::Stop {
                return Propagation::Stop;
            }
        }
        Propagation::Continue
    }

    /// Delivers `event` to `node` alone.
    pub(crate) fn notify(&mut self, node: NodeId, event: &Event) {
        self.deliver(node, node, Phase::Target, event);
    }

    fn deliver(
        &mut self,
        node: NodeId,
        target: NodeId,
        phase: Phase,
        event: &Event,
    ) -> Propagation {
        let Some(n) = self.tree.get_mut(node) else {
            return Propagation::Continue;
        };
        let mut cx = EventCx {
            node,
            target,
            phase,
            placement: self.geometry.get(&node).copied(),
            env: self.env,
            status: self.status,
            effects: self.effects,
        };
        n.widget.event(&mut *n.state, event, &mut cx)
    }
}

/// The path from `root` to the topmost node containing `point`; later siblings paint over earlier ones.
pub(crate) fn hit_test<M: 'static>(
    tree: &Tree<M>,
    root: NodeId,
    geometry: &Geometry,
    point: Point,
) -> Option<Vec<NodeId>> {
    fn visit<M: 'static>(
        tree: &Tree<M>,
        id: NodeId,
        geometry: &Geometry,
        point: Point,
        path: &mut Vec<NodeId>,
    ) -> bool {
        let Some(placement) = geometry.get(&id) else {
            return false;
        };
        if placement.clip.is_some_and(|clip| !clip.contains(point)) {
            return false;
        }
        path.push(id);
        let children = tree.get(id).map_or(&[][..], |n| &n.children[..]);
        if children
            .iter()
            .rev()
            .any(|&child| visit(tree, child, geometry, point, path))
        {
            return true;
        }
        if placement.rect.contains(point) {
            return true;
        }
        path.pop();
        false
    }
    let mut path = Vec::new();
    visit(tree, root, geometry, point, &mut path).then_some(path)
}

#[cfg(test)]
mod tests;
