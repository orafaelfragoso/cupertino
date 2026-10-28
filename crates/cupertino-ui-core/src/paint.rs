//! The paint pass. Each node paints in its own coordinates into its own [`PaintNode`], whose transform places it in
//! the window. A node's clip is the one it puts on its descendants, in its coordinates; the renderer intersects the
//! clips along the parent chain. Bounds cover what the node and its whole subtree draw, in window coordinates. A
//! node repaints only when it changed or its size did, and keeps its paint revision otherwise.

use crate::cx::PaintCx;
use crate::route::Geometry;
use crate::tree::Tree;
use crate::{Environment, NodeId};
use cupertino_geometry::{Point, Rect, Size};
use cupertino_ui_platform::Timestamp;
use cupertino_ui_render::{
    DisplayList, Effect, PaintNode, PaintNodeId, PaintOp, PaintRevision, PathVerb, Shape, Transform,
};
use std::collections::{HashMap, HashSet};

struct Painted {
    source: u64,
    size: Size,
    revision: PaintRevision,
    ops: Vec<PaintOp>,
    /// What the ops may touch, in node coordinates.
    extent: Option<Rect>,
}

/// How far a blur reaches beyond the content it blurs, per unit of radius.
const BLUR_REACH: f64 = 3.0;

/// Everything `ops` may touch, in their own coordinates: strokes reach half their width outside the shape, effects
/// reach as far as their blur and offset, and a glyph is taken to fill its em square around the pen with room for
/// ascenders, descenders and overhang.
pub(crate) fn extent(ops: &[PaintOp]) -> Option<Rect> {
    // Each open layer collects its content's extent; its effects widen it when it closes.
    let mut layers: Vec<(Vec<Effect>, Option<Rect>)> = vec![(Vec::new(), None)];
    let add = |layers: &mut Vec<(Vec<Effect>, Option<Rect>)>, r: Rect| {
        let top = &mut layers.last_mut().expect("the base layer is never popped").1;
        *top = Some(top.map_or(r, |t| t.union(&r)));
    };
    for op in ops {
        match op {
            PaintOp::Fill { shape, .. } => {
                if let Some(r) = shape_bounds(shape) {
                    add(&mut layers, r);
                }
            }
            PaintOp::Stroke { shape, width, .. } => {
                if let Some(r) = shape_bounds(shape) {
                    add(&mut layers, r.outset(width / 2.0));
                }
            }
            PaintOp::Glyphs { origin, run, .. } => {
                let size = f64::from(run.size);
                for g in &run.glyphs {
                    let pen = Point::new(origin.x + f64::from(g.x), origin.y + f64::from(g.y));
                    add(
                        &mut layers,
                        Rect::from_ltrb(
                            pen.x - size,
                            pen.y - 1.5 * size,
                            pen.x + 2.0 * size,
                            pen.y + size,
                        ),
                    );
                }
            }
            PaintOp::Image { dst, .. } => add(&mut layers, *dst),
            PaintOp::PushLayer { effects, .. } => layers.push((effects.clone(), None)),
            PaintOp::PopLayer if layers.len() > 1 => {
                let (effects, content) = layers.pop().expect("checked above");
                if let Some(content) = content {
                    add(&mut layers, with_effects(content, &effects));
                }
            }
            _ => {}
        }
    }
    while layers.len() > 1 {
        let (effects, content) = layers.pop().expect("checked above");
        if let Some(content) = content {
            add(&mut layers, with_effects(content, &effects));
        }
    }
    layers.pop().and_then(|(_, r)| r)
}

fn with_effects(content: Rect, effects: &[Effect]) -> Rect {
    effects.iter().fold(content, |r, effect| match *effect {
        Effect::Blur { radius } => r.outset(radius.abs() * BLUR_REACH),
        Effect::DropShadow { offset, blur, .. } => r.union(
            &content
                .translate(offset.x, offset.y)
                .outset(blur.abs() * BLUR_REACH),
        ),
        Effect::InnerShadow { .. } => r,
    })
}

fn shape_bounds(shape: &Shape) -> Option<Rect> {
    match shape {
        Shape::Rect(r) | Shape::RoundedRect { rect: r, .. } => Some(*r),
        Shape::Path { verbs, .. } => {
            let points = verbs.iter().flat_map(|v| match *v {
                PathVerb::MoveTo(p) | PathVerb::LineTo(p) => vec![p],
                PathVerb::QuadTo(a, b) => vec![a, b],
                PathVerb::CubicTo(a, b, c) => vec![a, b, c],
                PathVerb::Close => vec![],
            });
            points.fold(None, |acc: Option<Rect>, p| {
                let r = Rect::new(p.x, p.y, 0.0, 0.0);
                Some(acc.map_or(r, |a| {
                    Rect::from_ltrb(
                        a.min_x().min(p.x),
                        a.min_y().min(p.y),
                        a.max_x().max(p.x),
                        a.max_y().max(p.y),
                    )
                }))
            })
        }
    }
}

/// One window's paint, kept between frames.
#[derive(Default)]
pub(crate) struct Painter {
    nodes: HashMap<NodeId, Painted>,
    revision: u64,
}

pub(crate) struct Painting {
    pub(crate) list: DisplayList,
    /// What each node itself draws, in window coordinates, in the list's order; damage is tracked per node.
    pub(crate) own: Vec<Rect>,
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
    ) -> Painting {
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
                        extent: extent(&ops),
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
                    .own_clip
                    .map(|c| Shape::Rect(c.translate(-origin.x, -origin.y))),
                bounds: bounds(placement.rect, painted.extent, placement.clip),
                hit: placement
                    .clip
                    .map_or(Some(placement.rect), |c| c.intersection(&placement.rect))
                    .unwrap_or(Rect::new(origin.x, origin.y, 0.0, 0.0)),
                ops: painted.ops.clone(),
            });
            seen.insert(id);
        }
        self.nodes.retain(|id, _| seen.contains(id));
        let own = list.nodes.iter().map(|n| n.bounds).collect();
        // Children come after their parent, so walking backwards widens each parent by its finished children.
        let index: HashMap<PaintNodeId, usize> = list
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.id, i))
            .collect();
        for i in (0..list.nodes.len()).rev() {
            let (bounds, parent) = (list.nodes[i].bounds, list.nodes[i].parent);
            if let Some(&p) = parent.and_then(|p| index.get(&p)) {
                list.nodes[p].bounds = list.nodes[p].bounds.union(&bounds);
            }
        }
        Painting { list, own }
    }
}

/// The node's layout rect together with what its ops touch, in window coordinates, cut by its clip.
fn bounds(rect: Rect, extent: Option<Rect>, clip: Option<Rect>) -> Rect {
    let reach = extent.map_or(rect, |e| {
        rect.union(&e.translate(rect.origin.x, rect.origin.y))
    });
    match clip {
        Some(clip) => {
            reach
                .intersection(&clip)
                .unwrap_or(Rect::new(rect.origin.x, rect.origin.y, 0.0, 0.0))
        }
        None => reach,
    }
}

#[cfg(test)]
mod tests;
