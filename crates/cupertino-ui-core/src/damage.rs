//! What a frame must repaint. A node that repainted, moved or changed its clip damages where it was and where it
//! is; a node that went away damages where it was. A group of opacity alone that changed damages, besides its own box,
//! the box of every node under it, which it composites anew, and not the space between them. A change in paint order,
//! a first frame, a resize or a new scale repaints the whole window. A node that draws nothing itself, before and
//! after (a container with no operation and no group), changes no pixel and damages nothing; its children damage what
//! they draw. A layout that moves nothing therefore damages nothing.
//!
//! A node in a kept scroll layer ([`scroll_layers`]) is compared in its layer's content coordinates: a scroll moves the
//! layer, which the renderer lays over again and paints only where content comes into view, so a node that only moved
//! with it damages nothing, nor does one of which only more or less shows (its box is cut by the layer's clip). What
//! changed in a layer damages where it is in the window now, and where it was in the content now shows.

use crate::paint::Painting;
use cupertino_geometry::Point;
use cupertino_geometry::Rect;
use cupertino_ui_render::{
    scroll_layers, DamageRegion, DisplayList, Layer, PaintNodeId, PaintRevision, Shape, Transform,
};
use std::collections::HashMap;

#[derive(Clone, PartialEq)]
struct Seen {
    revision: PaintRevision,
    /// In the content coordinates of the scroll layer the node is kept in, if any.
    transform: Transform,
    clip: Option<Shape>,
    /// As `transform`.
    own: Rect,
    /// It draws nothing itself, from [`Painting::blank`].
    blank: bool,
    /// A group of opacity alone: a change of it changes what its whole subtree shows.
    spreads: bool,
    /// Under another parent the node is composited with other groups.
    parent: Option<PaintNodeId>,
    /// The kept scroll layer the node lies in, and its box in the window.
    layer: Option<PaintNodeId>,
    window: Rect,
    /// A scroll layer's offset: it moving repaints no node's box.
    scroll: Option<Point>,
    /// The node or a node above it has a group, whose box reaches past what the node draws: in a layer, more or less
    /// of it showing changes what the group draws.
    grouped: bool,
}

/// How far two positions of one node may differ and still be one: a node moved into its layer's content coordinates
/// and back carries the rounding of the offset.
const SAME: f64 = 1e-6;

impl Seen {
    /// Whether `self` and `other` paint the same pixels of their layer's content, but for how much of the node shows.
    fn same_content(&self, other: &Seen) -> bool {
        let near = |a: f64, b: f64| (a - b).abs() <= SAME;
        self.revision == other.revision
            && self.clip == other.clip
            && self.blank == other.blank
            && self.spreads == other.spreads
            && self.parent == other.parent
            && self.layer == other.layer
            && self
                .transform
                .0
                .iter()
                .zip(&other.transform.0)
                .all(|(a, b)| near(*a, *b))
    }

    /// Whether only the node's position in its layer's content moved by rounding, and its scroll offset if it has one.
    fn same(&self, other: &Seen) -> bool {
        let near = |a: f64, b: f64| (a - b).abs() <= SAME;
        let rect = |a: &Rect, b: &Rect| {
            near(a.min_x(), b.min_x())
                && near(a.min_y(), b.min_y())
                && near(a.max_x(), b.max_x())
                && near(a.max_y(), b.max_y())
        };
        self.same_content(other) && rect(&self.own, &other.own)
    }
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
        let ends = subtree_ends(list);
        let kept_in = scroll_layers(list);
        let offset_of = |l: usize| match list.nodes[l].layer {
            Some(Layer::Scroll { offset }) => offset,
            _ => Point::ZERO,
        };
        // where each kept layer's content is now, by node
        let offsets: HashMap<PaintNodeId, Point> = kept_in
            .iter()
            .flatten()
            .map(|&l| (list.nodes[l].id, offset_of(l)))
            .collect();
        let index: HashMap<PaintNodeId, usize> =
            order.iter().enumerate().map(|(i, id)| (*id, i)).collect();
        let mut grouped = vec![false; list.nodes.len()];
        for (i, node) in list.nodes.iter().enumerate() {
            let above = node
                .parent
                .and_then(|p| index.get(&p))
                .is_some_and(|&p| p < i && grouped[p]);
            grouped[i] = node.group.is_some() || above;
        }
        // where a box seen in an earlier frame shows in this one
        let now = |seen: &Seen| match seen.layer.and_then(|l| offsets.get(&l)) {
            Some(o) => seen.own.translate(-o.x, -o.y),
            None => seen.window,
        };
        let mut current = HashMap::with_capacity(order.len());
        let mut rects = Vec::new();
        let mut repainted = false;
        let blank_at = |i: usize| blank.get(i).copied().unwrap_or(false);
        let boxes = own;
        for (i, (node, &own)) in list.nodes.iter().zip(own).enumerate() {
            let blank = blank_at(i);
            let shift = kept_in[i].map_or(Point::ZERO, offset_of);
            let mut transform = node.transform;
            transform.0[4] += shift.x;
            transform.0[5] += shift.y;
            let scroll = match (node.layer, kept_in.contains(&Some(i))) {
                (Some(Layer::Scroll { offset }), true) => Some(offset),
                _ => None,
            };
            let seen = Seen {
                revision: node.revision,
                transform,
                clip: node.clip.clone(),
                own: own.translate(shift.x, shift.y),
                blank,
                spreads: node.group.as_ref().is_some_and(|g| g.effects.is_empty()),
                parent: node.parent,
                layer: kept_in[i].map(|l| list.nodes[l].id),
                window: own,
                scroll,
                grouped: grouped[i],
            };
            match self.previous.get(&node.id) {
                Some(old) if old.same(&seen) => {
                    // a scroll layer that moved: the renderer lays it over again, and the frame commits
                    repainted |= old.scroll != seen.scroll;
                }
                // in a layer, only what shows of it changed: what came into view is painted with the layer's new rows
                Some(old) if seen.layer.is_some() && !seen.grouped && old.same_content(&seen) => {
                    repainted = true;
                }
                Some(old) => {
                    repainted |= !old.own.is_empty() || !own.is_empty();
                    // where it drew and where it draws: a node that draws nothing damages nothing where it is
                    if !old.blank {
                        rects.push(now(old));
                    }
                    if !blank {
                        rects.push(own);
                    }
                    // the subtree is composited anew: where its nodes draw now (where they drew before, when that
                    // differs, they damage themselves)
                    if old.spreads || seen.spreads {
                        for j in i + 1..ends[i] {
                            if let (false, Some(&drawn)) = (blank_at(j), boxes.get(j)) {
                                repainted |= !drawn.is_empty();
                                rects.push(drawn);
                            }
                        }
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
                    rects.push(now(old));
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

/// For each node of a list in preorder, the index past the last node of its subtree.
fn subtree_ends(list: &DisplayList) -> Vec<usize> {
    let len = list.nodes.len();
    let mut ends = vec![len; len];
    let mut open: Vec<usize> = Vec::new();
    for (j, node) in list.nodes.iter().enumerate() {
        while let Some(&top) = open.last() {
            if Some(list.nodes[top].id) == node.parent {
                break;
            }
            ends[top] = j;
            open.pop();
        }
        open.push(j);
    }
    ends
}

#[cfg(test)]
mod tests {
    use super::*;
    use cupertino_ui_render::{DisplayList, PaintNode};

    fn node(id: u64, revision: u64, x: f64) -> PaintNode {
        PaintNode {
            layer: None,
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
            pins: false,
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
            DamageRegion::Rects(vec![Rect::new(20.0, 0.0, 10.0, 10.0)]),
            "a node that starts drawing damages its box"
        );
        assert_eq!(
            tracker.damage(&blank(4, 30.0)),
            DamageRegion::Rects(vec![Rect::new(20.0, 0.0, 10.0, 10.0)]),
            "and one that stops drawing where it drew"
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

    /// A scroll layer at `offset` clipping to 0..100 x 0..50, with a row at y 10 and one at y 40 of its content, the
    /// second at `revision`; boxes cut by the clip as the painter cuts them.
    fn scrolled(offset: f64, revision: u64) -> Painting {
        let mut layer = node(1, 1, 0.0);
        layer.clip = Some(Shape::Rect(Rect::new(0.0, 0.0, 100.0, 50.0)));
        layer.layer = Some(Layer::Scroll {
            offset: Point::new(0.0, offset),
        });
        layer.bounds = Rect::new(0.0, 0.0, 100.0, 50.0);
        let row = |id, revision, y: f64| {
            let mut n = node(id, revision, 0.0);
            n.parent = Some(PaintNodeId(1));
            n.transform.0[5] = y - offset;
            let shown = Rect::new(0.0, y - offset, 100.0, 10.0)
                .intersection(&Rect::new(0.0, -1.0, 100.0, 52.0))
                .unwrap_or(Rect::new(0.0, y - offset, 0.0, 0.0));
            n.bounds = shown;
            n
        };
        let mut p = painting(vec![layer, row(2, 1, 10.0), row(3, revision, 40.0)]);
        p.blank[0] = true;
        p
    }

    #[test]
    fn a_scroll_damages_nothing_its_layer_moves_and_a_change_in_it_where_it_shows_now() {
        let mut tracker = DamageTracker::default();
        tracker.damage(&scrolled(0.0, 1));
        // the rows moved with the layer, the second half out of view: nothing to paint but what comes into view,
        // which the renderer finds itself, and a frame to commit
        assert_eq!(
            tracker.damage(&scrolled(6.0, 1)),
            DamageRegion::Rects(vec![])
        );
        assert!(tracker.repainted());
        assert_eq!(
            tracker.damage(&scrolled(6.0, 1)),
            DamageRegion::Rects(vec![])
        );
        assert!(!tracker.repainted(), "nothing moved: no frame");
        // the second row painted again while the layer moved on: its box where it shows now, as it was and as it is
        assert_eq!(
            tracker.damage(&scrolled(9.0, 2)),
            DamageRegion::Rects(vec![
                Rect::new(0.0, 31.0, 100.0, 10.0),
                Rect::new(0.0, 31.0, 100.0, 10.0)
            ])
        );
    }
}
