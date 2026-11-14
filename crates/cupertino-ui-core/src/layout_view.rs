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
            .into_iter()
            .collect(),
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
            .into_iter()
            .collect(),
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

    /// A row `width` points wide and 30 tall that clips its children.
    struct Clipped {
        width: f64,
        children: Vec<Element<String>>,
    }

    impl crate::Widget<String> for Clipped {
        type State = ();

        fn layout(&self) -> LayoutSpec {
            LayoutSpec {
                width: cupertino_ui_layout::SizeSpec::Fixed(self.width),
                height: cupertino_ui_layout::SizeSpec::Fixed(30.0),
                overflow: cupertino_ui_layout::Overflow::Clip,
                ..LayoutSpec::new(ContainerKind::Stack {
                    axis: cupertino_ui_layout::Axis::Vertical,
                    spacing: 3.0,
                    align: cupertino_ui_layout::Align::Start,
                    justify: cupertino_ui_layout::Justify::Start,
                })
            }
        }

        fn take_children(&mut self) -> Vec<Element<String>> {
            std::mem::take(&mut self.children)
        }

        fn state(&self) {}

        fn update(&self, old: &Self, _: &mut ()) -> crate::Change {
            if self.width == old.width {
                crate::Change::None
            } else {
                crate::Change::Layout
            }
        }

        fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
            Measured {
                size: Size::ZERO,
                baseline: None,
            }
        }

        fn event(
            &self,
            _: &mut (),
            _: &crate::Event,
            _: &mut crate::cx::EventCx<'_, String>,
        ) -> crate::Propagation {
            crate::Propagation::Continue
        }

        fn paint(&self, _: &(), _: &mut crate::cx::PaintCx<'_>) {}

        fn semantics(&self, _: &(), _: &mut crate::cx::SemanticsCx<'_>) {}
    }

    /// Twelve kept rows, row `i` of width `widths[i]` (its mark), each a clipping row over two leaves, in a view
    /// scrolled down by `offset`.
    fn kept_rows(widths: &[f64], offset: f64) -> Element<String> {
        let rows = widths
            .iter()
            .enumerate()
            .map(|(i, &width)| {
                Element::kept(i as u64, width.to_bits(), move || {
                    Element::new(Clipped {
                        width,
                        children: vec![Element::new(leaf("a")), Element::new(leaf("b"))],
                    })
                })
            })
            .collect();
        Element::new(Scroller {
            children: vec![Element::new(group(rows))],
            offset: Point::new(0.0, offset),
        })
    }

    /// A kept subtree that did not change is placed from its run, unwalked, and the placements after scrolls at whole
    /// and fractional offsets, a row made anew, a row dropped and a resize are those of placing every node.
    #[test]
    fn a_kept_subtree_that_did_not_change_is_placed_from_its_run_as_placing_every_node_places_it() {
        let env = env();
        let mut widths = vec![90.0; 12];
        let mut tree = Tree::<String>::new(100);
        let root = tree.reconcile(None, kept_rows(&widths, 0.0)).unwrap();
        let mut kept = KeptPlacements::default();
        let place = |tree: &Tree<String>, kept: &mut KeptPlacements, size: Size| {
            let result = cupertino_ui_layout::layout(
                &TreeView::new(tree, &env),
                root,
                Constraints::tight(size).unwrap(),
                &cupertino_ui_layout::LayoutOptions {
                    direction: env.direction,
                    node_limit: 1000,
                },
            )
            .unwrap();
            let got = placements_kept(tree, root, &result, &env, &[], kept);
            let whole = placements(tree, root, &result, &env, &[]);
            (got, whole)
        };
        let window = Size::new(100.0, 70.5);
        let (got, whole) = place(&tree, &mut kept, window);
        assert_eq!(got, whole);
        assert_eq!(kept.laid, 0, "nothing kept yet");
        // each row's two leaves come from its run
        let descendants = 12 * 2;
        for offset in [7.0, 13.5, 40.25, 0.0] {
            tree.reconcile(Some(root), kept_rows(&widths, offset))
                .unwrap();
            let (got, whole) = place(&tree, &mut kept, window);
            assert_eq!(got, whole, "at {offset}");
            assert_eq!(kept.laid, descendants, "at {offset}");
        }
        // a row made anew is walked, the others placed from their runs
        widths[3] = 60.0;
        tree.reconcile(Some(root), kept_rows(&widths, 13.5))
            .unwrap();
        let (got, whole) = place(&tree, &mut kept, window);
        assert_eq!(got, whole);
        assert_eq!(kept.laid, descendants - 2);
        // a row dropped: its run goes with it
        widths.pop();
        tree.reconcile(Some(root), kept_rows(&widths, 20.0))
            .unwrap();
        let (got, whole) = place(&tree, &mut kept, window);
        assert_eq!(got, whole);
        assert_eq!(kept.laid, 11 * 2);
        // the window resized: the rows keep their size, so their runs still hold
        let (got, whole) = place(&tree, &mut kept, Size::new(97.0, 50.0));
        assert_eq!(got, whole);
        assert_eq!(kept.laid, 11 * 2);
    }

    /// A group that lies on whole device pixels ([`crate::Widget::on_pixels`]) when `pixels` says so.
    struct Snapped {
        pixels: bool,
        children: Vec<Element<String>>,
    }

    impl crate::Widget<String> for Snapped {
        type State = ();

        fn layout(&self) -> LayoutSpec {
            LayoutSpec::new(ContainerKind::Overlay)
        }

        fn take_children(&mut self) -> Vec<Element<String>> {
            std::mem::take(&mut self.children)
        }

        fn state(&self) {}

        fn update(&self, old: &Self, _: &mut ()) -> crate::Change {
            if self.pixels == old.pixels {
                crate::Change::None
            } else {
                crate::Change::Layout
            }
        }

        fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
            Measured {
                size: Size::ZERO,
                baseline: None,
            }
        }

        fn event(
            &self,
            _: &mut (),
            _: &crate::Event,
            _: &mut crate::cx::EventCx<'_, String>,
        ) -> crate::Propagation {
            crate::Propagation::Continue
        }

        fn paint(&self, _: &(), _: &mut crate::cx::PaintCx<'_>) {}

        fn semantics(&self, _: &(), _: &mut crate::cx::SemanticsCx<'_>) {}

        fn on_pixels(&self, _: &()) -> bool {
            self.pixels
        }
    }

    /// A node on whole device pixels moves to the nearest device pixel corner at the window's scale, and its subtree
    /// with it, keeping where each descendant lies in it; a node that is not keeps its fraction.
    #[test]
    fn a_node_on_whole_device_pixels_moves_its_subtree_to_the_nearest_pixel_corner() {
        for pixels in [false, true] {
            for scale in [1.0, 1.5, 2.0] {
                let mut tree = Tree::<String>::new(10);
                let view = Element::new(group(vec![Element::new(Snapped {
                    pixels,
                    children: vec![Element::new(leaf("x"))],
                })]));
                let root = tree.reconcile(None, view).unwrap();
                let snapped = tree.get(root).unwrap().children[0];
                let inside = tree.get(snapped).unwrap().children[0];
                let at = |x, y| NodeLayout {
                    rect: Rect::new(x, y, 40.0, 20.0),
                    clip: None,
                    baseline: None,
                };
                let layout = LayoutResult {
                    nodes: [
                        (root, at(0.0, 0.0)),
                        (snapped, at(10.3, 20.7)),
                        (inside, at(0.25, 1.0)),
                    ]
                    .into_iter()
                    .collect(),
                    stats: LayoutStats::default(),
                };
                let g = placements(&tree, root, &layout, &Environment { scale, ..env() }, &[]);
                let origin = g[&snapped].rect.origin;
                let expected = if pixels {
                    Point::new(
                        (10.3 * scale).round() / scale,
                        (20.7 * scale).round() / scale,
                    )
                } else {
                    Point::new(10.3, 20.7)
                };
                assert!(
                    (origin.x - expected.x).abs() < 1e-9 && (origin.y - expected.y).abs() < 1e-9,
                    "{pixels} at {scale}: {origin:?}"
                );
                if pixels {
                    for v in [origin.x * scale, origin.y * scale] {
                        assert!((v - v.round()).abs() < 1e-9, "at {scale}: {v}");
                    }
                }
                let child = g[&inside].rect.origin;
                assert!(
                    (child.x - origin.x - 0.25).abs() < 1e-9
                        && (child.y - origin.y - 1.0).abs() < 1e-9
                );
            }
        }
    }

    #[test]
    fn a_drawn_scroll_offset_is_a_whole_number_of_device_pixels() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            for v in [0.0, 0.3, 10.4, 10.6, 333.333, -7.77] {
                let d = drawn_offset(Point::new(v, v * 2.0), scale);
                for (got, given) in [(d.x, v), (d.y, v * 2.0)] {
                    let px = got * scale;
                    assert!((px - px.round()).abs() < 1e-9, "{v} at {scale}: {px}");
                    assert!((got - given).abs() <= 0.5 / scale + 1e-9, "{v} at {scale}");
                }
            }
        }
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
