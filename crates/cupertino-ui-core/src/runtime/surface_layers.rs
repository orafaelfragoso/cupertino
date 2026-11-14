//! A window's surface layers ([`Widget::surface_layer`](crate::Widget::surface_layer)): the marked subtrees leave the
//! window's paint for lists of their own, each drawn at its layout size into a frame of its own, with its own damage,
//! and handed to the window system with the window's frame
//! ([`WindowSystem::present_surface_layers`](cupertino_ui_platform::WindowSystem::present_surface_layers)). The
//! compositor places and animates them; the application changes their properties with
//! [`WindowRequest::Layer`](cupertino_ui_platform::WindowRequest). Layout, input and semantics never see the split.

use std::collections::HashMap;

use cupertino_geometry::{Point, Rect};
use cupertino_ui_platform::{
    PresentOutcome, SurfaceLayer, SurfaceLayerLook, WindowId, WindowSystem,
};
use cupertino_ui_render::{
    CpuFrame, DamageRegion, DisplayList, PaintNodeId, PaintResources, RenderRequest, UiRenderer,
};

use crate::damage::DamageTracker;
use crate::paint::Painting;
use crate::route::Geometry;
use crate::tree::Tree;
use crate::{NodeId, UiError};

/// A widget's mark: its subtree draws into the surface layer `key`, unique within its window, looking as `look` says.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceLayerMark {
    pub key: u64,
    pub look: SurfaceLayerLook,
}

/// A marked node of a window: its id, its layout rect in the window and its mark.
pub(crate) type Marked = (NodeId, Rect, SurfaceLayerMark);

/// The marked nodes under `root` in paint order; a mark inside a marked subtree is part of the outer layer, and a key
/// marked twice keeps its first node.
pub(crate) fn marked<M: 'static>(tree: &Tree<M>, root: NodeId, geometry: &Geometry) -> Vec<Marked> {
    let mut out: Vec<Marked> = Vec::new();
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        let Some(node) = tree.get(id) else { continue };
        if let Some(mark) = node.widget.surface_layer(&*node.state) {
            if let Some(placement) = geometry.get(&id) {
                if !out.iter().any(|(_, _, m)| m.key == mark.key) {
                    out.push((id, placement.rect, mark));
                }
            }
            continue;
        }
        stack.extend(node.children.iter().rev());
    }
    out
}

/// One surface layer's paint, in the layer's coordinates (its top left at the origin).
pub(crate) struct LayerPaint {
    pub(crate) key: u64,
    pub(crate) rect: Rect,
    pub(crate) look: SurfaceLayerLook,
    pub(crate) list: DisplayList,
    pub(crate) own: Vec<Rect>,
    pub(crate) blank: Vec<bool>,
}

fn moved(r: Rect, by: Point) -> Rect {
    r.translate(-by.x, -by.y)
}

/// Takes the marked subtrees out of `painting`, each into a list of its own moved to its layer's origin, in the marks'
/// order. A subtree's root loses its ancestors' clips and groups: the layer's shape and its properties stand for
/// them.
pub(crate) fn split(painting: &mut Painting, marks: &[Marked]) -> Vec<LayerPaint> {
    let mut out: Vec<LayerPaint> = marks
        .iter()
        .map(|(_, rect, mark)| LayerPaint {
            key: mark.key,
            rect: *rect,
            look: mark.look,
            list: DisplayList::default(),
            own: Vec::new(),
            blank: Vec::new(),
        })
        .collect();
    if marks.is_empty() {
        return out;
    }
    let roots: HashMap<PaintNodeId, usize> = marks
        .iter()
        .enumerate()
        .map(|(i, (id, ..))| (PaintNodeId::from(*id), i))
        .collect();
    let mut owner: HashMap<PaintNodeId, usize> = HashMap::new();
    let nodes = std::mem::take(&mut painting.list.nodes);
    let own = std::mem::take(&mut painting.own);
    let blank = std::mem::take(&mut painting.blank);
    for ((mut node, own), blank) in nodes.into_iter().zip(own).zip(blank) {
        let layer = roots.get(&node.id).copied().or_else(|| {
            node.parent
                .and_then(|p| owner.get(&p).or_else(|| roots.get(&p)).copied())
        });
        let Some(i) = layer else {
            painting.list.nodes.push(node);
            painting.own.push(own);
            painting.blank.push(blank);
            continue;
        };
        owner.insert(node.id, i);
        let origin = out[i].rect.origin;
        if roots.get(&node.id) == Some(&i) || node.parent.is_none_or(|p| !owner.contains_key(&p)) {
            node.parent = None;
        }
        node.transform.0[4] -= origin.x;
        node.transform.0[5] -= origin.y;
        node.bounds = moved(node.bounds, origin);
        node.hit = moved(node.hit, origin);
        let to = &mut out[i];
        to.list.nodes.push(node);
        to.own.push(moved(own, origin));
        to.blank.push(blank);
    }
    out
}

/// What a window's surface layer last showed and how to draw its next frame.
#[derive(Default)]
pub(crate) struct LayerState {
    pub(crate) damage: DamageTracker,
    /// Made on the layer's first frame.
    pub(crate) renderer: Option<Box<dyn UiRenderer<Frame = CpuFrame>>>,
    pub(crate) list: DisplayList,
    /// The rect and look it was last shown with.
    pub(crate) shown: Option<(Rect, SurfaceLayerLook)>,
}

/// A window's surface layers, by key, and the keys the window system shows, in order.
#[derive(Default)]
pub(crate) struct SurfaceLayers {
    pub(crate) layers: HashMap<u64, LayerState>,
    pub(crate) shown: Vec<u64>,
}

impl SurfaceLayers {
    /// The next frame draws every layer whole.
    pub(crate) fn invalidate(&mut self) {
        for l in self.layers.values_mut() {
            l.damage.invalidate();
        }
    }

    /// Lets the renderers go of the layers they keep, as a window nobody sees does.
    pub(crate) fn drop_kept(&mut self) {
        for r in self.layers.values_mut().filter_map(|l| l.renderer.as_mut()) {
            r.drop_layers();
        }
    }

    /// The renderers are of another kind from now on: each layer makes a new one.
    pub(crate) fn drop_renderers(&mut self) {
        for l in self.layers.values_mut() {
            l.renderer = None;
            l.damage.invalidate();
        }
    }
}

/// A frame's surface layers with what each must repaint, ready to present.
pub(crate) struct Prepared {
    paints: Vec<(LayerPaint, Option<DamageRegion>)>,
    /// Something differs from what the window system shows: a layer's pixels, rect or look, or which layers it shows.
    pub(crate) changed: bool,
}

impl SurfaceLayers {
    /// Works out what each layer of `paints` must repaint against what it showed; a layer whose rect or look changed,
    /// or that is new, draws whole.
    pub(crate) fn prepare(&mut self, paints: Vec<LayerPaint>) -> Prepared {
        let keys: Vec<u64> = paints.iter().map(|p| p.key).collect();
        let mut changed = keys != self.shown;
        self.layers.retain(|k, _| keys.contains(k));
        let paints = paints
            .into_iter()
            .map(|p| {
                let state = self.layers.entry(p.key).or_default();
                if state.shown != Some((p.rect, p.look)) {
                    state.damage.invalidate();
                    changed = true;
                }
                let damage = state
                    .damage
                    .damage_of(&p.list, &p.own, &p.blank, &Default::default());
                let repaint = !matches!(&damage, DamageRegion::Rects(r) if r.is_empty())
                    || state.damage.repainted();
                changed |= repaint;
                (p, repaint.then_some(damage))
            })
            .collect();
        Prepared { paints, changed }
    }

    /// Draws the layers of `prepared` that changed and hands them all to the window system for `window`'s next
    /// present, at `scale`. A layer's renderer is made by `renderers` on its first frame.
    pub(crate) fn present(
        &mut self,
        prepared: Prepared,
        system: &mut (impl WindowSystem + ?Sized),
        window: WindowId,
        scale: f64,
        resources: &PaintResources,
        renderers: &mut dyn FnMut() -> Box<dyn UiRenderer<Frame = CpuFrame>>,
    ) -> Result<PresentOutcome, UiError> {
        let mut drawn = Vec::with_capacity(prepared.paints.len());
        for (paint, damage) in prepared.paints {
            let state = self.layers.entry(paint.key).or_default();
            state.list = paint.list;
            let id = match damage {
                Some(damage) => {
                    let renderer = state.renderer.get_or_insert_with(&mut *renderers);
                    let id = renderer
                        .render(&RenderRequest {
                            list: &state.list,
                            resources,
                            scale,
                            width: super::physical(paint.rect.width(), scale).max(1),
                            height: super::physical(paint.rect.height(), scale).max(1),
                            damage: &damage,
                        })
                        .map_err(UiError::Render)?;
                    Some(id)
                }
                None => None,
            };
            drawn.push((paint.key, paint.rect, paint.look, id));
        }
        let mut layers = Vec::with_capacity(drawn.len());
        for &(key, rect, look, id) in &drawn {
            let frame = match (id, self.layers.get(&key).and_then(|s| s.renderer.as_ref())) {
                (Some(id), Some(r)) => Some(r.frame(id).map_err(UiError::Render)?),
                _ => None,
            };
            layers.push(SurfaceLayer {
                key,
                rect,
                look,
                frame,
            });
        }
        let outcome = system
            .present_surface_layers(window, &layers)
            .map_err(UiError::Platform)?;
        drop(layers);
        if outcome == PresentOutcome::NotReady {
            self.shown.clear();
            for s in self.layers.values_mut() {
                s.shown = None;
                s.damage.invalidate();
            }
            return Ok(outcome);
        }
        self.shown = drawn.iter().map(|d| d.0).collect();
        for (key, rect, look, _) in drawn {
            if let Some(s) = self.layers.get_mut(&key) {
                s.shown = Some((rect, look));
            }
        }
        Ok(outcome)
    }

    /// Takes every layer away, as when the window shows a colour instead of a frame.
    pub(crate) fn hide(
        &mut self,
        system: &mut (impl WindowSystem + ?Sized),
        window: WindowId,
    ) -> Result<(), UiError> {
        if self.shown.is_empty() {
            return Ok(());
        }
        system
            .present_surface_layers(window, &[])
            .map_err(UiError::Platform)?;
        self.shown.clear();
        for s in self.layers.values_mut() {
            s.shown = None;
            s.damage.invalidate();
        }
        Ok(())
    }
}
