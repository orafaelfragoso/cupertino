//! What a frame must repaint. A node that repainted, moved or changed its clip damages where it was and where it
//! is; a node that went away damages where it was. A change in paint order, a first frame, a resize or a new scale
//! repaints the whole window. Where a node's own operations tell (it has no group, whose opacity reaches its children,
//! and is under no group with effects), "where it was" is what its operations reached rather than its whole box, and a
//! node that only painted again damages only where its changed operations were and are: a container that draws
//! nothing damages nothing.

use crate::paint::Painting;
use cupertino_geometry::Rect;
use cupertino_ui_render::{DamageRegion, PaintNodeId, PaintRevision, Shape, Transform};
use std::collections::HashMap;

#[derive(Clone, PartialEq)]
struct Seen {
    revision: PaintRevision,
    transform: Transform,
    clip: Option<Shape>,
    own: Rect,
    /// What its operations reach, from [`Painting::reach`].
    reach: Option<Rect>,
}

#[derive(Default)]
pub(crate) struct DamageTracker {
    previous: HashMap<PaintNodeId, Seen>,
    order: Vec<PaintNodeId>,
    full: bool,
}

impl DamageTracker {
    /// The next frame repaints everything.
    pub(crate) fn invalidate(&mut self) {
        self.full = true;
    }

    pub(crate) fn damage(&mut self, painting: &Painting) -> DamageRegion {
        let order: Vec<PaintNodeId> = painting.list.nodes.iter().map(|n| n.id).collect();
        let mut current = HashMap::with_capacity(order.len());
        let mut rects = Vec::new();
        for (i, (node, &own)) in painting.list.nodes.iter().zip(&painting.own).enumerate() {
            let reach = painting.reach.get(i).copied().flatten();
            let changed = painting.changed.get(i).copied().flatten();
            let seen = Seen {
                revision: node.revision,
                transform: node.transform,
                clip: node.clip.clone(),
                own,
                reach,
            };
            match self.previous.get(&node.id) {
                Some(old) if *old == seen => {}
                Some(old) => match (old.reach, reach) {
                    // painted again in place: only what changed
                    (Some(was), Some(is))
                        if was == is
                            && old.transform == seen.transform
                            && old.clip == seen.clip
                            && changed.is_some() =>
                    {
                        rects.extend(changed);
                    }
                    (Some(was), Some(is)) if old.clip == seen.clip => rects.extend([was, is]),
                    _ => rects.extend([old.own, own]),
                },
                None => rects.push(reach.unwrap_or(own)),
            }
            current.insert(node.id, seen);
        }
        for (id, old) in &self.previous {
            if !current.contains_key(id) {
                rects.push(old.reach.unwrap_or(old.own));
            }
        }
        let reordered = {
            let kept_before = self.order.iter().filter(|id| current.contains_key(id));
            let kept_now = order.iter().filter(|id| self.previous.contains_key(id));
            !kept_before.eq(kept_now)
        };
        let full = std::mem::take(&mut self.full) || reordered;
        self.previous = current;
        self.order = order;
        if full {
            return DamageRegion::Full;
        }
        rects.retain(|r| !r.is_empty());
        DamageRegion::Rects(rects)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cupertino_ui_render::{DisplayList, PaintNode};

    fn node(id: u64, revision: u64, x: f64) -> PaintNode {
        PaintNode {
            id: PaintNodeId(id),
            parent: None,
            revision: PaintRevision(revision),
            transform: Transform([1.0, 0.0, 0.0, 1.0, x, 0.0]),
            clip: None,
            group: None,
            bounds: Rect::new(x, 0.0, 10.0, 10.0),
            hit: Rect::new(x, 0.0, 10.0, 10.0),
            ops: Vec::new(),
        }
    }

    fn painting(nodes: Vec<PaintNode>) -> Painting {
        let own = nodes.iter().map(|n| n.bounds).collect();
        Painting {
            reach: vec![None; nodes.len()],
            changed: vec![None; nodes.len()],
            list: DisplayList { nodes },
            own,
            animating: Vec::new(),
            resources: Default::default(),
            materials: Vec::new(),
        }
    }

    #[test]
    fn only_what_changed_is_damaged_where_it_was_and_where_it_is() {
        let mut tracker = DamageTracker::default();
        assert_eq!(
            tracker.damage(&painting(vec![node(1, 1, 0.0), node(2, 1, 20.0)])),
            DamageRegion::Rects(vec![
                Rect::new(0.0, 0.0, 10.0, 10.0),
                Rect::new(20.0, 0.0, 10.0, 10.0)
            ])
        );
        assert_eq!(
            tracker.damage(&painting(vec![node(1, 1, 0.0), node(2, 1, 20.0)])),
            DamageRegion::Rects(vec![])
        );
        assert_eq!(
            tracker.damage(&painting(vec![node(1, 1, 0.0), node(2, 1, 40.0)])),
            DamageRegion::Rects(vec![
                Rect::new(20.0, 0.0, 10.0, 10.0),
                Rect::new(40.0, 0.0, 10.0, 10.0)
            ])
        );
        assert_eq!(
            tracker.damage(&painting(vec![node(2, 1, 40.0)])),
            DamageRegion::Rects(vec![Rect::new(0.0, 0.0, 10.0, 10.0)])
        );
    }

    #[test]
    fn a_node_whose_operations_tell_damages_what_they_reach_and_in_place_only_what_changed() {
        let reach = Rect::new(2.0, 2.0, 4.0, 4.0);
        let with = |revision: u64, x: f64, changed: Option<Rect>| {
            let mut p = painting(vec![node(1, revision, x)]);
            p.reach = vec![Some(reach.translate(x, 0.0))];
            p.changed = vec![changed];
            p
        };
        let mut tracker = DamageTracker::default();
        assert_eq!(
            tracker.damage(&with(1, 0.0, None)),
            DamageRegion::Rects(vec![reach]),
            "a container that draws little damages only that"
        );
        let caret = Rect::new(3.0, 2.0, 1.0, 4.0);
        assert_eq!(
            tracker.damage(&with(2, 0.0, Some(caret))),
            DamageRegion::Rects(vec![caret])
        );
        assert_eq!(
            tracker.damage(&with(3, 0.0, Some(Rect::new(0.0, 0.0, 0.0, 0.0)))),
            DamageRegion::Rects(vec![]),
            "painted the same again: nothing"
        );
        assert_eq!(
            tracker.damage(&with(3, 20.0, None)),
            DamageRegion::Rects(vec![reach, reach.translate(20.0, 0.0)]),
            "moved: where it reached and where it reaches"
        );
        assert_eq!(
            tracker.damage(&painting(vec![])),
            DamageRegion::Rects(vec![reach.translate(20.0, 0.0)])
        );
    }

    #[test]
    fn a_new_paint_order_or_an_invalidation_repaints_everything() {
        let mut tracker = DamageTracker::default();
        tracker.damage(&painting(vec![node(1, 1, 0.0), node(2, 1, 20.0)]));
        assert_eq!(
            tracker.damage(&painting(vec![node(2, 1, 20.0), node(1, 1, 0.0)])),
            DamageRegion::Full
        );
        tracker.invalidate();
        assert_eq!(
            tracker.damage(&painting(vec![node(2, 1, 20.0), node(1, 1, 0.0)])),
            DamageRegion::Full
        );
    }
}
