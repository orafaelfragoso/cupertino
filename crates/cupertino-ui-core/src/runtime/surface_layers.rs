//! A window's surface layers ([`Widget::surface_layer`](crate::Widget::surface_layer)): the marked subtrees leave the
//! window's paint for lists of their own, each drawn at its layout size in whole points
//! ([`surface_layer_size`](crate::platform::surface_layer_size)) into a frame of its own, with its own damage,
//! and handed to the window system with the window's frame ([`Submission::surface_layers`](crate::platform::Submission),
//! or [`WindowSystem::present_surface_layers`](crate::platform::WindowSystem::present_surface_layers) where it declines
//! the frame). The
//! compositor places and animates them; the application changes their properties with
//! [`WindowRequest::Layer`](crate::platform::WindowRequest). Layout, input and semantics never see the split.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::platform::{
    surface_layer_size, LayerList, PresentOutcome, SharedRenderer, SubmittedLayer, SurfaceLayer,
    SurfaceLayerLook, WindowId, WindowSystem,
};
use cupertino_geometry::{Point, Rect, Size};
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

/// A layer's root paint node in a window (a marked node's, or the overlay's of a node whose overlay is a layer of its
/// own), the layer's rect in the window and its mark.
pub(crate) type Marked = (PaintNodeId, Rect, SurfaceLayerMark);

/// The layers under `root` in paint order, in a window `window` large: the marked nodes, a mark inside a marked
/// subtree being part of the outer layer, and the overlays that are layers of their own
/// ([`Widget::overlay_layer`](crate::Widget::overlay_layer)), each after the layer it lies in and at its own rect
/// ([`Widget::overlay_layer_rect`](crate::Widget::overlay_layer_rect)) or that layer's (the
/// window's where it lies in none). A key marked twice keeps its first.
pub(crate) fn marked<M: 'static>(
    tree: &Tree<M>,
    root: NodeId,
    geometry: &Geometry,
    window: Size,
) -> Vec<Marked> {
    let mut out: Vec<Marked> = Vec::new();
    let taken = |out: &[Marked], key: u64| out.iter().any(|(_, _, m)| m.key == key);
    // per node: the rect of the marked subtree it lies in, if any
    let mut stack = vec![(root, None::<Rect>)];
    while let Some((id, within)) = stack.pop() {
        let Some(node) = tree.get(id) else { continue };
        let mut inner = within;
        if within.is_none() {
            if let Some(mark) = node.widget.surface_layer(&*node.state) {
                let Some(placement) = geometry.get(&id) else {
                    continue;
                };
                if !taken(&out, mark.key) {
                    out.push((PaintNodeId::from(id), placement.rect, mark));
                }
                inner = Some(placement.rect);
            }
        }
        if let Some(mark) = node.widget.overlay_layer(&*node.state) {
            if let Some(placement) = geometry.get(&id).filter(|_| !taken(&out, mark.key)) {
                let origin = placement.rect.origin;
                let rect = match node.widget.overlay_layer_rect(&*node.state) {
                    Some(own) => own.translate(origin.x, origin.y),
                    None => inner.unwrap_or(Rect::from_origin_size(Point::ZERO, window)),
                };
                out.push((crate::paint::overlay_id(id), rect, mark));
            }
        }
        stack.extend(node.children.iter().rev().map(|&c| (c, inner)));
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
        .map(|(i, (id, ..))| (*id, i))
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
    pub(crate) renderer: Option<SharedRenderer>,
    pub(crate) list: Arc<DisplayList>,
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
        for r in self.layers.values().filter_map(|l| l.renderer.as_ref()) {
            super::held(r).drop_layers();
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

    /// The layers of `prepared` as a submitted frame carries them ([`Submission::surface_layers`]): each that changed
    /// with its list to draw at `scale` by its renderer, which `renderers` makes on the layer's first frame.
    ///
    /// [`Submission::surface_layers`]: crate::platform::Submission::surface_layers
    pub(crate) fn submitted(
        &mut self,
        prepared: Prepared,
        scale: f64,
        renderers: &mut dyn FnMut() -> Box<dyn UiRenderer<Frame = CpuFrame> + Send>,
    ) -> Vec<SubmittedLayer> {
        prepared
            .paints
            .into_iter()
            .map(|(paint, damage)| {
                let state = self.layers.entry(paint.key).or_default();
                state.list = Arc::new(paint.list);
                let (width, height) = surface_layer_size(paint.rect);
                let layer = damage.map(|damage| LayerList {
                    renderer: Arc::clone(
                        state
                            .renderer
                            .get_or_insert_with(|| Arc::new(Mutex::new(renderers()))),
                    ),
                    list: Arc::clone(&state.list),
                    width: super::physical(f64::from(width), scale).max(1),
                    height: super::physical(f64::from(height), scale).max(1),
                    damage,
                });
                SubmittedLayer {
                    key: paint.key,
                    rect: paint.rect,
                    look: paint.look,
                    layer,
                }
            })
            .collect()
    }

    /// Draws the frames of `layers` on this thread and hands them all to the window system for `window`'s next
    /// present, with `resources` at `scale`: the layers of a frame the window system declined.
    pub(crate) fn present(
        &mut self,
        layers: &[SubmittedLayer],
        system: &mut (impl WindowSystem + ?Sized),
        window: WindowId,
        resources: &Arc<PaintResources>,
        scale: f64,
    ) -> Result<PresentOutcome, UiError> {
        let mut drawn = Vec::with_capacity(layers.len());
        for l in layers {
            drawn.push(match &l.layer {
                Some(layer) => {
                    let mut renderer = super::held(&layer.renderer);
                    let id = renderer
                        .render(&RenderRequest {
                            list: &layer.list,
                            resources,
                            scale,
                            width: layer.width,
                            height: layer.height,
                            damage: &layer.damage,
                        })
                        .map_err(UiError::Render)?;
                    Some((renderer, id))
                }
                None => None,
            });
        }
        let mut frames = Vec::with_capacity(layers.len());
        for (l, drawn) in layers.iter().zip(&drawn) {
            let frame = match drawn {
                Some((renderer, id)) => Some(renderer.frame(*id).map_err(UiError::Render)?),
                None => None,
            };
            frames.push(SurfaceLayer {
                key: l.key,
                rect: l.rect,
                look: l.look,
                frame,
            });
        }
        let outcome = system
            .present_surface_layers(window, &frames)
            .map_err(UiError::Platform)?;
        drop(frames);
        match outcome {
            PresentOutcome::Presented => self.shown(&shows(layers)),
            PresentOutcome::NotReady => self.not_shown(),
        }
        Ok(outcome)
    }

    /// The window system shows these layers, each key with its rect and look, back to front.
    pub(crate) fn shown(&mut self, shown: &[(u64, Rect, SurfaceLayerLook)]) {
        self.shown = shown.iter().map(|s| s.0).collect();
        for &(key, rect, look) in shown {
            if let Some(s) = self.layers.get_mut(&key) {
                s.shown = Some((rect, look));
            }
        }
    }

    /// The window system showed none of the layers it was handed: the next frame hands them all again, drawn whole.
    pub(crate) fn not_shown(&mut self) {
        self.shown.clear();
        for s in self.layers.values_mut() {
            s.shown = None;
            s.damage.invalidate();
        }
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
        self.not_shown();
        Ok(())
    }
}

/// What showing `layers` shows: each key with its rect and look.
pub(crate) fn shows(layers: &[SubmittedLayer]) -> Vec<(u64, Rect, SurfaceLayerLook)> {
    layers.iter().map(|l| (l.key, l.rect, l.look)).collect()
}
