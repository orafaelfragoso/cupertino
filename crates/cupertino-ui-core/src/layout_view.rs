//! The bridge to layout: the node tree seen as a [`LayoutTree`], and layout's parent-relative result turned into
//! window placements. A node's clip limits its children, so a node is clipped by its ancestors' clips only.

use crate::cx::MeasureCx;
use crate::paint::unobscured;
use crate::route::{Geometry, Placement, Raised};
use crate::tree::Tree;
use crate::{Environment, NodeId};
use cupertino_geometry::{Point, Rect, Size};
use cupertino_ui_layout::{
    Constraints, ContainerKind, Generation, IdMap, LayoutDirection, LayoutResult, LayoutSpec,
    LayoutTree, Measured,
};
use std::cell::{Cell, RefCell};

pub(crate) struct TreeView<'a, M> {
    pub(crate) tree: &'a Tree<M>,
    pub(crate) env: &'a Environment,
    /// The leaves that asked for idle time while they were measured ([`MeasureCx::request_idle`]).
    pub(crate) idle: RefCell<Vec<NodeId>>,
}

impl<'a, M> TreeView<'a, M> {
    pub(crate) fn new(tree: &'a Tree<M>, env: &'a Environment) -> Self {
        Self {
            tree,
            env,
            idle: RefCell::default(),
        }
    }
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
        let idle = Cell::new(false);
        let measured = node.widget.measure(
            &*node.state,
            constraints,
            &mut MeasureCx {
                node: id,
                env: self.env,
                idle: &idle,
            },
        );
        if idle.get() {
            self.idle.borrow_mut().push(id);
        }
        measured
    }
}

/// A scroll offset as it is drawn: a whole number of device pixels at `scale`, so content that moves by it lands on
/// the same pixel grid as content that does not, and a moved picture is the picture drawn again.
pub(crate) fn drawn_offset(offset: Point, scale: f64) -> Point {
    if !(scale > 0.0 && scale.is_finite()) {
        return offset;
    }
    let round = |v: f64| (v * scale).round() / scale;
    Point::new(round(offset.x), round(offset.y))
}

/// A node's origin on the window's device pixels at `scale` ([`Widget::on_pixels`](crate::Widget::on_pixels)): each
/// coordinate on the nearest device pixel edge, as a compositor places a layer's origin.
pub(crate) fn pixel_origin(origin: Point, scale: f64) -> Point {
    if !(scale > 0.0 && scale.is_finite()) {
        return origin;
    }
    let round = |v: f64| (v * scale).round() / scale;
    Point::new(round(origin.x), round(origin.y))
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
    placements_kept(tree, root, layout, env, cut, &mut KeptPlacements::default())
}

/// What a kept subtree's nodes were placed from, by [`placements_kept`]: each node in the order it was placed, with its
/// parent in the run, its rect and clip as layout gave them (relative to its parent) and its least hit size. Placed
/// again, the run gives each node the placement placing it from the layout would, by the same sums, without looking
/// the node up in the layout or the tree.
struct Run {
    /// The kept node's subtree revision and size when the run was made.
    revision: u64,
    size: Size,
    nodes: Vec<RunNode>,
}

struct RunNode {
    id: NodeId,
    /// The node's parent: an earlier node of the run, or [`ROOT`] for the kept node itself.
    parent: usize,
    rect: Rect,
    clip: Option<Rect>,
    least: Size,
}

/// The kept node a run hangs from, as a [`RunNode::parent`].
const ROOT: usize = usize::MAX;

/// The runs of a window's kept subtrees ([`Element::kept`](crate::Element::kept)) between frames: a kept node whose
/// subtree revision and size are those of its run has its descendants placed from the run, unwalked. Only subtrees
/// with no scroll container and no raised node are kept, since their placements follow scroll offsets and the raised
/// nodes over them, neither of which the subtree revision covers. Good for one environment: the window clears it when
/// the environment changes, as it clears its layout cache.
#[derive(Default)]
pub(crate) struct KeptPlacements {
    runs: IdMap<NodeId, Run>,
    /// How many nodes the last placements placed from runs.
    pub(crate) laid: usize,
}

impl KeptPlacements {
    pub(crate) fn clear(&mut self) {
        self.runs.clear();
    }
}

/// A run being made by the walk: its kept node, and whether it can still be kept.
struct Recording {
    root: NodeId,
    run: Run,
    keepable: bool,
}

/// [`placements`], placing the descendants of kept subtrees that did not change from `kept`'s runs and keeping a run
/// for every other kept subtree it walks. The placements are those of walking every node.
pub(crate) fn placements_kept<M: 'static>(
    tree: &Tree<M>,
    root: NodeId,
    layout: &LayoutResult<NodeId>,
    env: &Environment,
    cut: &[(NodeId, Rect)],
    kept: &mut KeptPlacements,
) -> Geometry {
    let mut geometry = Geometry::with_capacity_and_hasher(layout.nodes.len(), Default::default());
    let mut old = std::mem::take(&mut kept.runs);
    kept.laid = 0;
    let mut recordings: Vec<Recording> = Vec::new();
    // per node to place: its parent's origin and clip, whether it is raised, and the recording and parent in it of the
    // run it is placed into
    let mut stack = vec![(
        root,
        Point::new(0.0, 0.0),
        None::<Rect>,
        false,
        None::<(usize, usize)>,
    )];
    while let Some((id, parent_origin, clip, raised, recording)) = stack.pop() {
        let Some(node) = layout.nodes.get(&id) else {
            continue;
        };
        let n = tree.get(id);
        let on_pixels = n.is_some_and(|n| n.widget.on_pixels(&*n.state));
        let mut rect = node.rect.translate(parent_origin.x, parent_origin.y);
        if on_pixels {
            rect.origin = pixel_origin(rect.origin, env.scale);
        }
        let raised = raised || n.is_some_and(|n| n.widget.raised(&*n.state));
        let scrolls = n.is_some_and(|n| matches!(n.spec.kind, ContainerKind::Scroll { .. }));
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
        let least = n.map_or(Size::ZERO, |n| n.widget.target(&*n.state, env));
        geometry.insert(
            id,
            Placement {
                rect,
                target: grown(rect, least),
                clip,
                own_clip,
                raised: if raised { Raised::In } else { Raised::No },
            },
        );
        let inner = inner_clip(clip, own_clip);
        // the run this node is placed into, if one is being made: its descendants go into it too
        let placed_into = recording.map(|(r, parent)| {
            let rec = &mut recordings[r];
            rec.keepable &= !scrolls && !raised && !on_pixels;
            rec.run.nodes.push(RunNode {
                id,
                parent,
                rect: node.rect,
                clip: node.clip,
                least,
            });
            (r, rec.run.nodes.len() - 1)
        });
        let Some(n) = n else { continue };
        let candidate =
            recording.is_none() && n.kept.is_some() && id != root && !raised && !scrolls;
        if candidate {
            if let Some(run) = old
                .remove(&id)
                .filter(|run| run.revision == n.subtree_revision && run.size == rect.size)
            {
                lay_run(&run, rect.origin, inner, &mut geometry);
                kept.laid += run.nodes.len();
                kept.runs.insert(id, run);
                continue;
            }
        }
        let mut origin = rect.origin;
        if let ContainerKind::Scroll {
            horizontal,
            vertical,
        } = n.spec.kind
        {
            let offset = drawn_offset(n.widget.scroll_offset(&*n.state), env.scale);
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
        let into = match placed_into {
            Some(into) => Some(into),
            None if candidate => {
                recordings.push(Recording {
                    root: id,
                    run: Run {
                        revision: n.subtree_revision,
                        size: rect.size,
                        nodes: Vec::new(),
                    },
                    keepable: true,
                });
                Some((recordings.len() - 1, ROOT))
            }
            None => None,
        };
        stack.extend(n.children.iter().map(|&c| (c, origin, inner, raised, into)));
    }
    for rec in recordings.into_iter().filter(|r| r.keepable) {
        kept.runs.insert(rec.root, rec.run);
    }
    // the nodes above a raised one, which hit-testing goes through to reach it first
    let roots: Vec<NodeId> = geometry
        .iter()
        .filter(|(id, p)| {
            p.raised == Raised::In
                && tree.get(**id).and_then(|n| n.parent).is_none_or(|parent| {
                    geometry.get(&parent).is_none_or(|p| p.raised != Raised::In)
                })
        })
        .map(|(id, _)| *id)
        .collect();
    for id in roots {
        let mut up = tree.get(id).and_then(|n| n.parent);
        while let Some(parent) = up {
            match geometry.get_mut(&parent) {
                Some(p) if p.raised == Raised::No => p.raised = Raised::Holds,
                _ => break,
            }
            up = tree.get(parent).and_then(|n| n.parent);
        }
    }
    geometry
}

/// The clip a node leaves its descendants: its own met with the one it is under.
fn inner_clip(clip: Option<Rect>, own: Option<Rect>) -> Option<Rect> {
    match own {
        Some(own) => Some(match clip {
            Some(outer) => {
                outer
                    .intersection(&own)
                    .unwrap_or(Rect::new(own.origin.x, own.origin.y, 0.0, 0.0))
            }
            None => own,
        }),
        None => clip,
    }
}

/// Places `run`'s nodes under a kept node placed at `origin` that leaves its descendants `clip`, by the sums the walk
/// makes: none of them scrolls or is raised.
fn lay_run(run: &Run, origin: Point, clip: Option<Rect>, geometry: &mut Geometry) {
    // each node's origin and the clip it leaves its descendants, as its children read them
    let mut placed: Vec<(Point, Option<Rect>)> = Vec::with_capacity(run.nodes.len());
    for n in &run.nodes {
        let (parent_origin, clip) = match n.parent {
            ROOT => (origin, clip),
            p => placed[p],
        };
        let rect = n.rect.translate(parent_origin.x, parent_origin.y);
        let own_clip = n.clip.map(|c| c.translate(rect.origin.x, rect.origin.y));
        geometry.insert(
            n.id,
            Placement {
                rect,
                target: grown(rect, n.least),
                clip,
                own_clip,
                raised: Raised::No,
            },
        );
        placed.push((rect.origin, inner_clip(clip, own_clip)));
    }
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
