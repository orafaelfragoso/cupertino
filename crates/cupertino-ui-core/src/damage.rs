//! What a frame must repaint. A node that repainted, moved or changed its clip damages where it was and where it
//! is; a node that went away damages where it was. A change in paint order, a first frame, a resize or a new scale
//! repaints the whole window. A node that draws nothing itself, before and after (a container with no operation and no
//! group), changes no pixel and damages nothing; its children damage what they draw.

use crate::paint::Painting;
use cupertino_geometry::Rect;
use cupertino_ui_render::{
    DamageRegion, DisplayList, PaintNodeId, PaintRevision, Shape, Transform,
};
use std::collections::HashMap;

#[derive(Clone, PartialEq)]
struct Seen {
    revision: PaintRevision,
    transform: Transform,
    clip: Option<Shape>,
    own: Rect,
    /// It draws nothing itself, from [`Painting::blank`].
    blank: bool,
}

#[derive(Default)]
pub(crate) struct DamageTracker {
    previous: HashMap<PaintNodeId, Seen>,
    order: Vec<PaintNodeId>,
    full: bool,
    /// The last frame painted, moved or removed a node with a box, whether or not it changed a pixel.
    repainted: bool,
}

impl DamageTracker {
    /// The next frame repaints everything.
    pub(crate) fn invalidate(&mut self) {
        self.full = true;
    }

    /// Whether the last frame painted, moved or removed a node with a box. Such a frame commits even when it changed
    /// no pixel: what an application sets on its surface outside the frame (a layer's zone or placement) and the next
    /// frame of an animation wait for a commit.
    pub(crate) fn repainted(&self) -> bool {
        self.repainted
    }

    pub(crate) fn damage(&mut self, painting: &Painting) -> DamageRegion {
        self.damage_of(&painting.list, &painting.own, &painting.blank)
    }

    /// [`Self::damage`] of a list with its damage boxes and blank flags, as a window's raised layer has its own.
    pub(crate) fn damage_of(
        &mut self,
        list: &DisplayList,
        own: &[Rect],
        blank: &[bool],
    ) -> DamageRegion {
        let order: Vec<PaintNodeId> = list.nodes.iter().map(|n| n.id).collect();
        let mut current = HashMap::with_capacity(order.len());
        let mut rects = Vec::new();
        let mut repainted = false;
        for (i, (node, &own)) in list.nodes.iter().zip(own).enumerate() {
            let blank = blank.get(i).copied().unwrap_or(false);
            let seen = Seen {
                revision: node.revision,
                transform: node.transform,
                clip: node.clip.clone(),
                own,
                blank,
            };
            match self.previous.get(&node.id) {
                Some(old) if *old == seen => {}
                Some(old) => {
                    repainted |= !old.own.is_empty() || !own.is_empty();
                    if !(old.blank && blank) {
                        rects.extend([old.own, own]);
                    }
                }
                None => {
                    repainted |= !own.is_empty();
                    if !blank {
                        rects.push(own);
                    }
                }
            }
            current.insert(node.id, seen);
        }
        for (id, old) in &self.previous {
            if !current.contains_key(id) {
                repainted |= !old.own.is_empty();
                if !old.blank {
                    rects.push(old.own);
                }
            }
        }
        let reordered = {
            let kept_before = self.order.iter().filter(|id| current.contains_key(id));
            let kept_now = order.iter().filter(|id| self.previous.contains_key(id));
            !kept_before.eq(kept_now)
        };
        let full = std::mem::take(&mut self.full) || reordered;
        self.repainted = repainted || full;
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
            blank: vec![false; nodes.len()],
            list: DisplayList { nodes },
            own,
            animating: Vec::new(),
            resources: Default::default(),
            materials: Vec::new(),
            layer: None,
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
    fn a_node_that_draws_nothing_damages_nothing_but_its_frame_commits() {
        let blank = |revision: u64, x: f64| {
            let mut p = painting(vec![node(1, revision, x)]);
            p.blank = vec![true];
            p
        };
        let mut tracker = DamageTracker::default();
        assert_eq!(tracker.damage(&blank(1, 0.0)), DamageRegion::Rects(vec![]));
        assert!(tracker.repainted(), "a new node is a change to commit");
        assert_eq!(
            tracker.damage(&blank(2, 20.0)),
            DamageRegion::Rects(vec![]),
            "painted again and moved, it still draws nothing"
        );
        assert!(tracker.repainted());
        assert_eq!(tracker.damage(&blank(2, 20.0)), DamageRegion::Rects(vec![]));
        assert!(!tracker.repainted(), "nothing painted: no frame");
        assert_eq!(
            tracker.damage(&painting(vec![node(1, 3, 20.0)])),
            DamageRegion::Rects(vec![
                Rect::new(20.0, 0.0, 10.0, 10.0),
                Rect::new(20.0, 0.0, 10.0, 10.0)
            ]),
            "a node that starts drawing damages its box"
        );
        assert_eq!(
            tracker.damage(&blank(4, 20.0)),
            DamageRegion::Rects(vec![
                Rect::new(20.0, 0.0, 10.0, 10.0),
                Rect::new(20.0, 0.0, 10.0, 10.0)
            ])
        );
        assert_eq!(
            tracker.damage(&painting(vec![])),
            DamageRegion::Rects(vec![])
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
