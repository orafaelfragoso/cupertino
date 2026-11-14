//! Event delivery. An event for a target travels down the target's path from the root (capture), reaches the target,
//! and travels back up (bubble); any handler can stop it. Hit-testing picks the topmost node under a point,
//! respecting every clip on the way down.

use crate::cx::{Effects, EventCx, Status};
use crate::tree::Tree;
use crate::{Environment, Event, NodeId, Phase, Propagation, WindowKey};
use cupertino_geometry::{Point, Rect};
use cupertino_ui_layout::IdMap;

/// Where a node sits in its window, in logical points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Placement {
    pub(crate) rect: Rect,
    /// The rect grown evenly to the node's least hit size; it can reach past the rect but takes no layout space.
    pub(crate) target: Rect,
    /// The intersection of every ancestor's clip; it limits this node and its subtree.
    pub(crate) clip: Option<Rect>,
    /// The clip this node puts on its descendants.
    pub(crate) own_clip: Option<Rect>,
    /// Where the node stands to the raised nodes ([`Widget::raised`](crate::Widget::raised)).
    pub(crate) raised: Raised,
}

/// Where a node stands to the raised nodes, which lie over the rest of the window and take a point first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Raised {
    /// Neither raised nor above a raised node.
    #[default]
    No,
    /// Not raised, with a raised node under it.
    Holds,
    /// Raised, or under a raised node.
    In,
}

impl Placement {
    /// Whether `point` lands on the node: inside its hit target and every clip above it.
    pub(crate) fn hit(&self, point: Point) -> bool {
        self.target.contains(point) && self.clip.is_none_or(|clip| clip.contains(point))
    }
}

pub(crate) type Geometry = IdMap<NodeId, Placement>;

pub(crate) struct Route<'a, M> {
    pub(crate) tree: &'a mut Tree<M>,
    pub(crate) geometry: &'a Geometry,
    pub(crate) env: &'a Environment,
    pub(crate) status: Status,
    pub(crate) popups: &'a [(WindowKey, Rect)],
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

    /// Delivers `event` to `node` alone, and tells whether the node took it.
    pub(crate) fn offer(&mut self, node: NodeId, event: &Event) -> Propagation {
        self.deliver(node, node, Phase::Target, event)
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
            popups: self.popups,
            effects: self.effects,
        };
        n.widget.event(&mut *n.state, event, &mut cx)
    }
}

/// The path from `root` to the topmost node containing `point`; raised nodes lie over the rest, and later siblings paint
/// over earlier ones. Among
/// siblings, a rect that holds the point wins over a hit target that only reaches it, and of those targets the one
/// whose rect is nearest wins, the topmost on a tie. A node's subtree comes before the node itself.
pub(crate) fn hit_test<M: 'static>(
    tree: &Tree<M>,
    root: NodeId,
    geometry: &Geometry,
    point: Point,
) -> Option<Vec<NodeId>> {
    /// How the hit node reaches `point`, and the path up from that node; with `raised`, only a raised node's.
    fn visit<M: 'static>(
        tree: &Tree<M>,
        id: NodeId,
        geometry: &Geometry,
        point: Point,
        raised: bool,
    ) -> Option<(Reach, Vec<NodeId>)> {
        let placement = geometry.get(&id)?;
        if placement.clip.is_some_and(|clip| !clip.contains(point)) {
            return None;
        }
        if raised && placement.raised == Raised::No {
            return None;
        }
        let children = tree.get(id).map_or(&[][..], |n| &n.children[..]);
        let mut nearest: Option<(Reach, Vec<NodeId>)> = None;
        for &child in children.iter().rev() {
            let Some(found) = visit(tree, child, geometry, point, raised) else {
                continue;
            };
            let inside = found.0 == Reach::Rect;
            if nearest.as_ref().is_none_or(|(reach, _)| found.0 < *reach) {
                nearest = Some(found);
            }
            if inside {
                break;
            }
        }
        let (reach, mut path) = match nearest {
            Some(found) => found,
            // only a node above a raised one: not hit itself
            None if raised && placement.raised == Raised::Holds => return None,
            None if placement.rect.contains(point) => {
                let own = Point::new(
                    point.x - placement.rect.origin.x,
                    point.y - placement.rect.origin.y,
                );
                if !tree.get(id).is_none_or(|n| n.widget.hit(&*n.state, own)) {
                    return None;
                }
                (Reach::Rect, Vec::new())
            }
            None if placement.target.contains(point) => {
                (Reach::Target(distance(placement.rect, point)), Vec::new())
            }
            None => return None,
        };
        path.push(id);
        Some((reach, path))
    }
    // the raised nodes lie over the rest of the window: one whose rect holds the point takes it first
    let (_, mut path) = match visit(tree, root, geometry, point, true) {
        Some(found) if found.0 == Reach::Rect => found,
        _ => visit(tree, root, geometry, point, false)?,
    };
    path.reverse();
    Some(path)
}

/// How a hit node reaches a point; a rect beats any target, and a nearer target beats a farther one.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
enum Reach {
    Rect,
    Target(f64),
}

fn distance(rect: Rect, point: Point) -> f64 {
    let dx = (rect.min_x() - point.x)
        .max(point.x - rect.max_x())
        .max(0.0);
    let dy = (rect.min_y() - point.y)
        .max(point.y - rect.max_y())
        .max(0.0);
    dx.hypot(dy)
}

#[cfg(test)]
mod tests;
