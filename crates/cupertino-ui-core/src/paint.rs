//! The paint pass. Each node paints in its own coordinates into its own [`PaintNode`], whose transform places it in
//! the window. A node's clip is the one it puts on its descendants, in its coordinates; the renderer intersects the
//! clips along the parent chain. Bounds cover what the node and its whole subtree draw, in window coordinates. A
//! node repaints only when it changed or its size did, and keeps its paint revision otherwise.

use crate::cx::{OverlayCx, PaintCx};
use crate::layout::{ContainerKind, LayoutDirection};
use crate::layout::{IdMap, IdSet};
use crate::platform::{BlurRamp, GlassRegion, GlassStyle, GlassTone, MaterialRegion, Timestamp};
use crate::route::{Geometry, Placement};
use crate::tree::Tree;
use crate::{Environment, NodeId, WindowBackground};
use cupertino_geometry::{Insets, Point, Rect, Size};
use cupertino_theme::ui::{Color, ColorRole, StateVariant};
use cupertino_theme::ui::{FaceTone, ThemeTokens};
use cupertino_ui_render::{
    DisplayList, Effect, Group, ImageResource, Layer, Paint, PaintNode, PaintNodeId, PaintOp,
    PaintResources, PaintRevision, PathVerb, ResourceId, Shape, Transform,
};
use std::sync::Arc;

struct Painted {
    source: u64,
    /// The window background painted under the node's own ops.
    background: Option<WindowBackground>,
    size: Size,
    revision: PaintRevision,
    ops: Vec<PaintOp>,
    group: Option<Group>,
    /// What the ops may touch, in node coordinates.
    extent: Option<Rect>,
    /// The images the ops draw.
    images: Vec<(ResourceId, ImageResource)>,
    /// The part of the node that showed, when its paint depended on it.
    visible: Option<Rect>,
    /// Whether the window could show one more material when the node painted, if the node asked for one.
    materials: Option<bool>,
    /// The material the window shows under the node.
    material: Option<NodeMaterial>,
    /// The glass the node painted on ([`Foreground`]).
    foreground: Foreground,
    /// The node draws an overlay on every frame ([`PaintCx::overlay`]).
    overlay: bool,
    /// The node lay on a material when it painted ([`PaintCx::on_material`]).
    on_material: bool,
    /// The node asked for glass or a translucent surface's material, which its subtree lies on.
    lays_material: bool,
    /// The ops snapped to the window's device pixels where a frame draws the node ([`PaintCx::fill_pixels`]), by
    /// index.
    pixels: Vec<usize>,
    /// How far raised nodes covered the node's edges when it painted, when its paint depended on it.
    obscured: Option<Insets>,
    /// The appearance the node painted in, pinned by an ancestor ([`PaintCx::pin_appearance`]).
    pinned: Option<bool>,
    /// The appearance the node pinned its subtree to.
    pins: Option<bool>,
    /// What the node read of the window's marks when it painted, when its paint depended on them.
    marks: Option<WindowMarks>,
}

/// What nodes of one window tell each other while they paint: whether a scroll view's page is scrolled under the
/// window's raised nodes, and whether a raised node shows a band over it. Each is kept per node, so two windows of one
/// process, or two scroll views of one window, never read each other's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct WindowMarks {
    pub(crate) page_scrolled: bool,
    pub(crate) band_shown: bool,
}

/// The glass a node paints on, which picks the colours of its text and symbols: the tone of the glass it lies on, if
/// any, and, for a node that asked for a material, the tone of the region it would get.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Foreground {
    on: Option<GlassTone>,
    own: GlassTone,
}

/// The tone of each glass region of a window ([`WindowEvent::GlassTone`](crate::platform::WindowEvent)): the
/// window's own material, and each region of the last materials asked for, by index.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct GlassTones {
    pub(crate) window: GlassTone,
    pub(crate) regions: Vec<GlassTone>,
}

impl GlassTones {
    fn region(&self, i: usize) -> GlassTone {
        self.regions.get(i).copied().unwrap_or_default()
    }
}

/// The face tone a glass tone draws text with: none for the appearance's.
fn face(tone: GlassTone) -> Option<FaceTone> {
    match tone {
        GlassTone::Appearance => None,
        GlassTone::Dark => Some(FaceTone::Dark),
        GlassTone::Light => Some(FaceTone::Light),
    }
}

/// `env` with the theme of text on glass of `tone` ([`ThemeTokens::on_glass`](cupertino_theme::ui::ThemeTokens)).
fn glass_environment(env: &Environment, tone: GlassTone) -> Environment {
    Environment {
        theme: Arc::new(env.theme.on_glass(face(tone))),
        ..env.clone()
    }
}

/// `env` with its colours `progress` of the way from the window's inactive colours to its active ones.
fn crossed(env: &Environment, progress: f32) -> Environment {
    let mut theme = ThemeTokens::clone(&env.theme);
    theme.colors.set_activation(progress);
    Environment {
        theme: Arc::new(theme),
        ..env.clone()
    }
}

/// What a node lies on, which its children inherit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Under {
    /// The window's own content.
    Window,
    /// A material other than glass: its text keeps the window's colours.
    Material,
    /// Glass with the tone of its face.
    Glass(GlassTone),
}

/// Whether `outer` holds `inner` whole, to within the rounding of a pixel edge.
fn holds(outer: &Rect, inner: &Rect) -> bool {
    const SLACK: f64 = 0.5;
    inner.min_x() >= outer.min_x() - SLACK
        && inner.min_y() >= outer.min_y() - SLACK
        && inner.max_x() <= outer.max_x() + SLACK
        && inner.max_y() <= outer.max_y() + SLACK
}

/// The index of an appearance a subtree is pinned to in the painter's environments: none (the window's), light, dark.
fn pin_index(pin: Option<bool>) -> usize {
    match pin {
        None => 0,
        Some(false) => 1,
        Some(true) => 2,
    }
}

/// The index of a tone in the painter's glass environments.
fn tone_index(tone: GlassTone) -> usize {
    match tone {
        GlassTone::Appearance => 0,
        GlassTone::Dark => 1,
        GlassTone::Light => 2,
    }
}

/// The most compositor materials one window shows under its nodes ([`PaintCx::material`]); a node past them paints its
/// own fill.
pub const MAX_MATERIALS: usize = 32;

/// The material a node asked for under itself.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct NodeMaterial {
    pub(crate) name: &'static str,
    pub(crate) radius: f64,
    /// The corners are half the node's shorter side, whatever its size ([`crate::GlassCorners::Capsule`]).
    pub(crate) capsule: bool,
    /// A surface layer draws it ([`crate::PaintCx::layer_glass`]): the window shows no region for it.
    pub(crate) in_layer: bool,
    pub(crate) alpha: f64,
    pub(crate) glass: Option<GlassRegion>,
    /// How its blur varies from its top to its bottom ([`crate::PaintCx::set_blur_ramp`]).
    pub(crate) blur_ramp: BlurRamp,
}

mod geometry;
use geometry::*;
pub(crate) use geometry::{extent, obscured_insets, raised_nodes, unobscured};

/// One window's paint, kept between frames.
#[derive(Default)]
pub(crate) struct Painter {
    nodes: IdMap<NodeId, Painted>,
    revision: u64,
    /// The environment the kept paint was made in; any other repaints every node.
    painted_in: Option<Environment>,
    /// The tone of the window's glass regions; a change repaints what lies on the region.
    pub(crate) tones: GlassTones,
    /// The pinned themes the kept paint was made with ([`PaintInput::pinned`]); others repaint every node.
    pinned_in: [Option<Arc<ThemeTokens>>; 2],
    /// The environments of text on glass of each tone, made from `painted_in`, then from the light and the dark pinned
    /// themes ([`pin_index`]).
    glass_envs: Option<[[Environment; 3]; 3]>,
    /// The environments of the window's appearance, the light one and the dark one, made from `painted_in`.
    pin_envs: Option<[Environment; 3]>,
    /// What each node's overlay drew last, and the revision it drew with, while the node asks for one: a caret that
    /// shows again where it showed before is the same paint.
    overlays: IdMap<NodeId, (Vec<PaintOp>, PaintRevision)>,
    overlay_revision: u64,
    /// The scroll views that told they are scrolled, by node ([`PaintCx::set_page_scrolled`]).
    scrolled_pages: IdMap<NodeId, bool>,
    /// The raised nodes that told they show a band, by node ([`PaintCx::set_band_shown`]).
    bands: IdMap<NodeId, bool>,
    /// The kept subtrees ([`Element::kept`](crate::Element::kept)) of the last list, by their root, which the next
    /// paint lays in as they were while nothing in them or under them changed.
    segments: IdMap<NodeId, Segment>,
    /// What the last paint did, node by node.
    pub(crate) stats: PaintStats,
    /// Per-frame scratch arena for temporary paint-pass buffers.
    scratch: bumpalo::Bump,
}

/// How many nodes a paint went through one by one, and how many it laid in with their kept subtree, unwalked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PaintStats {
    pub(crate) walked: usize,
    pub(crate) kept: usize,
}

/// What the root of a kept subtree lay on, and the frame's flags every node's paint depends on: a subtree painted on
/// the same ground is painted as it was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Ground {
    under: Under,
    inherited: Option<GlassTone>,
    pinned: Option<bool>,
    on_material: bool,
    backdrop: bool,
    window_active: bool,
    focus_visible: bool,
    layer: bool,
    surface_layers: bool,
    glass: GlassSupport,
    materials: bool,
}

/// A kept subtree's nodes as the last list laid them, in paint order from its root: none raised, focused or with an
/// overlay, a material, a pinned appearance, a scroll layer or a window mark, so what they draw follows from their
/// own paint, their placement and their root's ground alone.
struct Segment {
    /// The root's subtree revision when it was laid.
    revision: u64,
    ground: Ground,
    nodes: Vec<NodeId>,
}

/// A kept subtree being laid by the walk, until a node outside it comes.
struct Recording {
    root: NodeId,
    ground: Ground,
    revision: u64,
    nodes: Vec<NodeId>,
    /// The path from the root down to the last node laid, which the next node of the subtree is a child of.
    open: Vec<NodeId>,
    /// Every node so far can be laid again as it is.
    keeps: bool,
}

/// The paint node of `node`'s overlay ([`Widget::overlay`](crate::Widget::overlay)): it comes after the node's
/// subtree, as the node's last child, with its operations in window coordinates. Its id is apart from every node's.
pub fn overlay_id(node: NodeId) -> PaintNodeId {
    // a node's own id has a non-zero generation in its upper half; an overlay's has none
    PaintNodeId(u64::from(node.index()))
}

/// An overlay waiting for the end of its node's subtree.
struct Pending {
    owner: NodeId,
    node: PaintNode,
    /// The clip of the node's ancestors.
    clip: Option<Rect>,
    /// It draws in the raised layer.
    layered: bool,
}

/// The id of the node that clips the `n`th raised subtree of a layer to the clip of its ancestors: apart from every
/// node's (a generation in the upper half) and every overlay's (none, and an index below `u32::MAX`).
fn layer_clip_id(n: usize) -> PaintNodeId {
    PaintNodeId(u64::MAX - n as u64)
}

/// Whether paint made in `a` is paint made in `b`: the same theme, text system, icons and scale, the same direction.
/// Compares the shared parts by identity, not `Environment`'s `==`, so an environment's owner keeps its `Arc`s across
/// frames; one rebuilt every frame repaints every node every frame.
pub(crate) fn same_environment(a: &Environment, b: &Environment) -> bool {
    Arc::ptr_eq(&a.theme, &b.theme)
        && Arc::ptr_eq(&a.text, &b.text)
        && Arc::ptr_eq(&a.icons, &b.icons)
        && a.scale == b.scale
        && a.direction == b.direction
}

pub(crate) struct Painting {
    pub(crate) list: DisplayList,
    /// What each node itself draws, in window coordinates, in the list's order; damage is tracked per node. A node
    /// with a group draws its whole subtree, and a node under a group with effects draws that group's whole reach.
    pub(crate) own: Vec<Rect>,
    /// Whether each node, in the list's order, draws nothing itself: no operation and no group (whose opacity reaches
    /// its children). Such a node changes no pixel when it paints again, moves or goes away.
    pub(crate) blank: Vec<bool>,
    /// The nodes that asked for the next frame while they painted.
    pub(crate) animating: Vec<NodeId>,
    /// The nodes that asked for the window's idle time while they painted ([`PaintCx::request_idle`]).
    pub(crate) idle: Vec<NodeId>,
    /// The images the list draws.
    pub(crate) resources: PaintResources,
    /// The materials the nodes asked for, in window coordinates, in paint order.
    pub(crate) materials: Vec<MaterialRegion>,
    /// What the raised nodes draw, when the window system shows them in a layer of their own.
    pub(crate) layer: Option<LayerPainting>,
    /// A node pinned its subtree to an appearance ([`PaintCx::pin_appearance`]).
    pub(crate) pins: bool,
    /// The kept subtrees laid in as the last paint laid them, unwalked: each one's root with the number of nodes it
    /// lays from it on, in the list's order. The damage tracker compares them with the last frame's in one go.
    pub(crate) kept_runs: IdMap<PaintNodeId, usize>,
}

/// The raised layer of a [`Painting`]: its own list, damage boxes and blank flags, in window coordinates.
pub(crate) struct LayerPainting {
    pub(crate) list: DisplayList,
    pub(crate) own: Vec<Rect>,
    pub(crate) blank: Vec<bool>,
    /// How far down the window the layer's nodes draw, in points.
    pub(crate) bottom: f64,
}

/// What a window's glass takes beyond its style: a system variant, a tint and a scale
/// ([`Capabilities::glass_variants`](crate::platform::Capabilities::glass_variants), `glass_tint`, `glass_scale`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct GlassSupport {
    pub(crate) variants: bool,
    pub(crate) tint: bool,
    pub(crate) scale: bool,
}

impl GlassSupport {
    pub(crate) fn of(capabilities: &crate::platform::Capabilities) -> Self {
        Self {
            variants: capabilities.glass_variants,
            tint: capabilities.glass_tint,
            scale: capabilities.glass_scale,
        }
    }
}

pub(crate) struct PaintInput<'a> {
    pub(crate) env: &'a Environment,
    pub(crate) focus: Option<NodeId>,
    pub(crate) focus_visible: bool,
    pub(crate) window_active: bool,
    pub(crate) background: WindowBackground,
    /// The window system shows the material of a [`WindowBackground::Surface`], so nothing stands in for it.
    pub(crate) backdrop: bool,
    /// The window system can show materials under nodes.
    pub(crate) materials: bool,
    pub(crate) now: Timestamp,
    /// The window's raised nodes ([`Widget::raised`](crate::Widget::raised)) with their rects, in window coordinates.
    pub(crate) raised: &'a [(NodeId, Rect)],
    /// Those of them that cover what lies under them ([`Widget::obscures`](crate::Widget::obscures)).
    pub(crate) obscuring: &'a [(NodeId, Rect)],
    /// The window system shows a raised layer, which the raised nodes draw in.
    pub(crate) layer: bool,
    /// The window system shows surface layers ([`PaintCx::surface_layers`]).
    pub(crate) surface_layers: bool,
    /// What the window system's glass takes beyond its style ([`PaintCx::glass_variants`]).
    pub(crate) glass: GlassSupport,
    /// The window's theme in the light and the dark appearance, for subtrees pinned to one
    /// ([`PaintCx::pin_appearance`]); `None` keeps such a subtree in the window's.
    pub(crate) pinned: &'a [Option<Arc<ThemeTokens>>; 2],
    /// How far the window's colours have crossed from its inactive ones to its active ones, 0 to 1
    /// ([`ColorTokens::set_activation`](cupertino_theme::ui::ColorTokens::set_activation)); 1 when they do not cross.
    /// `env` and `pinned` are the themes before the crossing, so what the painter makes from them outlives it.
    pub(crate) activation: f32,
}

/// The paint node a UI node draws into, so tests and the inspector can find a node's operations.
impl From<NodeId> for PaintNodeId {
    fn from(id: NodeId) -> Self {
        PaintNodeId(id.0.get())
    }
}

impl Painter {
    /// The marks nodes have set so far.
    fn marks(&self) -> WindowMarks {
        WindowMarks {
            page_scrolled: self.scrolled_pages.values().any(|&v| v),
            band_shown: self.bands.values().any(|&v| v),
        }
    }

    /// Drops every node's kept paint, so the next paint draws each node again.
    pub(crate) fn forget_nodes(&mut self) {
        self.nodes.clear();
        self.segments.clear();
    }

    pub(crate) fn paint<M: 'static>(
        &mut self,
        tree: &Tree<M>,
        root: NodeId,
        geometry: &Geometry,
        input: &PaintInput<'_>,
    ) -> Painting {
        self.scratch.reset();
        // a node paints in its environment's colours, type and pixels, none of which its own revision covers
        let same_pins =
            self.pinned_in
                .iter()
                .zip(input.pinned)
                .all(|(kept, now)| match (kept, now) {
                    (Some(kept), Some(now)) => Arc::ptr_eq(kept, now),
                    (None, None) => true,
                    _ => false,
                });
        if !same_pins
            || !self
                .painted_in
                .as_ref()
                .is_some_and(|env| same_environment(env, input.env))
        {
            self.nodes.clear();
            self.segments.clear();
            self.painted_in = Some(input.env.clone());
            self.pinned_in = input.pinned.clone();
            self.glass_envs = None;
            self.pin_envs = None;
        }
        let pin_envs = self
            .pin_envs
            .get_or_insert_with(|| {
                let pinned = |theme: &Option<Arc<ThemeTokens>>| Environment {
                    theme: theme
                        .clone()
                        .unwrap_or_else(|| Arc::clone(&input.env.theme)),
                    ..input.env.clone()
                };
                [
                    input.env.clone(),
                    pinned(&input.pinned[0]),
                    pinned(&input.pinned[1]),
                ]
            })
            .clone();
        let glass_envs = self
            .glass_envs
            .get_or_insert_with(|| {
                pin_envs.clone().map(|env| {
                    [GlassTone::Appearance, GlassTone::Dark, GlassTone::Light]
                        .map(|tone| glass_environment(&env, tone))
                })
            })
            .clone();
        let (pin_envs, glass_envs) = if input.activation < 1.0 {
            // the crossing changes no colour of the tables, only how they are read, so the copies share them
            (
                pin_envs.map(|env| crossed(&env, input.activation)),
                glass_envs.map(|envs| envs.map(|env| crossed(&env, input.activation))),
            )
        } else {
            (pin_envs, glass_envs)
        };
        // the appearance each node pinned its subtree to, as its children inherit it
        let mut pinned_below: IdMap<NodeId, Option<bool>> = IdMap::default();
        let mut pins = false;
        // What each node lies on, as its children inherit it: the window's material when it is glass and shown, below
        // a node that shows a material that node's region. A node with nothing under it from its ancestors lies on
        // the glass painted before it that holds it whole: a panel's content is often drawn beside the node of its
        // glass, after it.
        let mut on_glass: IdMap<NodeId, Under> = IdMap::default();
        let mut glass_shown: Vec<(Rect, GlassTone)> = Vec::new();
        let window_glass = match input.background {
            WindowBackground::Surface { surface, .. } if input.backdrop => input
                .env
                .theme
                .surface(surface)
                .material
                .and_then(GlassStyle::of_material)
                .map_or(Under::Window, |_| Under::Glass(self.tones.window)),
            _ => Under::Window,
        };
        let mut list = DisplayList::default();
        let mut animating = Vec::new();
        let mut idle_nodes = Vec::new();
        // the nodes that read the window's marks this pass, and the overlays with the band they saw
        let mut mark_readers: Vec<NodeId> = Vec::new();
        let mut band_readers: Vec<(NodeId, bool)> = Vec::new();
        let mut resources = PaintResources::default();
        let mut seen = IdSet::with_capacity_and_hasher(self.nodes.len(), Default::default());
        let mut clips = Vec::new();
        let mut materials = Vec::new();
        let mut blank = Vec::new();
        let window = geometry.get(&root).map(|p| p.rect);
        let mut pending: Vec<Pending> = Vec::new();
        // whether each node of the list is raised: in the raised layer where there is one, else over the rest
        let mut layered: Vec<bool> = Vec::new();
        let flush = |pending: &mut Vec<Pending>,
                     until: Option<NodeId>,
                     list: &mut DisplayList,
                     clips: &mut Vec<Option<Rect>>,
                     blank: &mut Vec<bool>,
                     layered: &mut Vec<bool>| {
            while let Some(top) = pending.last() {
                if until.is_some_and(|id| within(tree, top.owner, id)) {
                    break;
                }
                let done = pending.pop().expect("checked above");
                list.nodes.push(done.node);
                clips.push(done.clip);
                blank.push(false);
                layered.push(done.layered);
            }
        };
        let raised_ids: IdSet<NodeId> = input.raised.iter().map(|(id, _)| *id).collect();
        // whether each node draws in the raised layer, as its children inherit it
        let mut in_layer: IdMap<NodeId, bool> = IdMap::default();
        let mut overlaid = IdSet::default();
        // whether each node's children lie on a material, from the window's background down
        let mut material_below: IdMap<NodeId, bool> = IdMap::default();
        let window_material = matches!(
            input.background,
            WindowBackground::Surface { surface, .. } if crate::cx::translucent(surface)
        );
        let order = paint_order(tree, root, &raised_ids, input.layer);
        // the focused node and its ancestors: a kept subtree that holds the focus paints it, so it is walked
        let focus_path: IdSet<NodeId> = input
            .focus
            .filter(|&f| tree.contains(f))
            .map(|f| tree.path(f))
            .unwrap_or_default()
            .into_iter()
            .collect();
        let mut segments: IdMap<NodeId, Segment> =
            IdMap::with_capacity_and_hasher(self.segments.len(), Default::default());
        let mut recording: Option<Recording> = None;
        let mut stats = PaintStats::default();
        let mut kept_runs: IdMap<PaintNodeId, usize> = IdMap::default();
        let mut at = 0;
        while at < order.len() {
            let id = order[at];
            at += 1;
            let (Some(node), Some(placement)) = (tree.get(id), geometry.get(&id)) else {
                continue;
            };
            // a node outside the kept subtree being laid ends it
            let ended = recording.as_mut().is_some_and(|rec| {
                while rec.open.last().is_some_and(|&top| Some(top) != node.parent) {
                    rec.open.pop();
                }
                rec.open.is_empty()
            });
            if ended {
                let rec = recording.take().expect("checked above");
                if rec.keeps {
                    segments.insert(rec.root, rec.segment());
                }
            }
            flush(
                &mut pending,
                Some(id),
                &mut list,
                &mut clips,
                &mut blank,
                &mut layered,
            );
            let raised = raised_ids.contains(&id)
                || node
                    .parent
                    .is_some_and(|p| in_layer.get(&p).copied().unwrap_or(false));
            in_layer.insert(id, raised);
            let obscured = if raised {
                Insets::ZERO
            } else {
                obscured_insets(placement.rect, input.obscuring)
            };
            let size = placement.rect.size;
            let focused = input.focus == Some(id);
            let background = (id == root
                && match input.background {
                    WindowBackground::Theme => true,
                    WindowBackground::Surface { .. } => !input.backdrop,
                    WindowBackground::Solid(color) => color.a > 0.0,
                    WindowBackground::None => false,
                })
            .then_some(input.background);
            // Focus, the window's active state, its background and whether the window system shows that background
            // change how a node paints, though nothing in the node changed.
            let source = node.paint_revision << 4
                | u64::from(input.backdrop) << 3
                | u64::from(input.window_active) << 2
                | u64::from(focused) << 1
                | u64::from(focused && input.focus_visible);
            let origin = placement.rect.origin;
            let visible = [placement.clip, window]
                .into_iter()
                .flatten()
                .try_fold(placement.rect, |shown, clip| shown.intersection(&clip))
                .map_or(Rect::new(0.0, 0.0, 0.0, 0.0), |shown| {
                    shown.translate(-origin.x, -origin.y)
                });
            // without a layer a raised node's material lies under the window, as any other: what scrolls under it is
            // clipped away (`placements`), so it shows, and it comes after the rest's (`paint_order`), so it lies over
            // them
            let can_show = input.materials && materials.len() < MAX_MATERIALS;
            let under = match node.parent {
                Some(parent) => on_glass.get(&parent).copied().unwrap_or(Under::Window),
                None => window_glass,
            };
            let inherited = match under {
                Under::Glass(tone) => Some(tone),
                Under::Material => None,
                Under::Window => {
                    let shown = visible.translate(origin.x, origin.y);
                    glass_shown
                        .iter()
                        .rev()
                        .find(|(glass, _)| !shown.is_empty() && holds(glass, &shown))
                        .map(|(_, tone)| *tone)
                }
            };
            let foreground = Foreground {
                on: inherited,
                own: self.tones.region(materials.len()),
            };
            let pinned = node
                .parent
                .and_then(|parent| pinned_below.get(&parent).copied().flatten());
            let (base, glass) = (&pin_envs[pin_index(pinned)], &glass_envs[pin_index(pinned)]);
            let on_material = inherited.is_some()
                || match node.parent {
                    Some(parent) => material_below.get(&parent).copied().unwrap_or(false),
                    None => window_material,
                };
            let ground = Ground {
                under,
                inherited,
                pinned,
                on_material,
                backdrop: input.backdrop,
                window_active: input.window_active,
                focus_visible: input.focus_visible,
                layer: input.layer,
                surface_layers: input.surface_layers,
                glass: input.glass,
                materials: input.materials,
            };
            // a kept subtree that changed in nothing and lies on the same ground is laid in as it was, unwalked
            let candidate =
                node.kept.is_some() && id != root && !raised && !focus_path.contains(&id);
            if let Some(segment) = candidate
                .then(|| self.segments.remove(&id))
                .flatten()
                .filter(|s| {
                    s.revision == node.subtree_revision
                        && s.ground == ground
                        && order.get(at - 1..at - 1 + s.nodes.len()) == Some(&s.nodes[..])
                })
            {
                let lengths = (list.nodes.len(), clips.len(), blank.len(), layered.len());
                let mut laid = true;
                let mut reach: Option<Rect> = None;
                for &d in &segment.nodes {
                    let (Some(n), Some(p), Some(painted)) =
                        (tree.get(d), geometry.get(&d), self.nodes.get(&d))
                    else {
                        laid = false;
                        break;
                    };
                    // what a node's paint read of its place must be as it was
                    if painted.size != p.rect.size
                        || painted.visible.is_some_and(|v| v != shown_part(p, window))
                        || painted
                            .obscured
                            .is_some_and(|o| o != obscured_insets(p.rect, input.obscuring))
                    {
                        laid = false;
                        break;
                    }
                    reach = Some(reach.map_or(p.rect, |r| r.union(&p.rect)));
                    add_images(&mut resources, painted);
                    blank.push(painted.ops.is_empty() && painted.group.is_none());
                    list.nodes.push(paint_node(
                        d,
                        n.parent,
                        p,
                        painted,
                        group_layer(painted),
                        input.env.scale,
                    ));
                    clips.push(p.clip);
                    layered.push(false);
                    seen.insert(d);
                }
                // on the window, each node of it looked for glass painted before it that holds it, and found none
                if laid && under == Under::Window {
                    laid = reach.is_none_or(|reach| {
                        !glass_shown
                            .iter()
                            .any(|(glass, _)| glass.outset(1.0).intersects(&reach))
                    });
                }
                if laid {
                    stats.kept += segment.nodes.len();
                    kept_runs.insert(id.into(), segment.nodes.len());
                    at += segment.nodes.len() - 1;
                    if let Some(rec) = &mut recording {
                        rec.nodes.extend(&segment.nodes);
                        rec.open.push(id);
                    }
                    segments.insert(id, segment);
                    continue;
                }
                list.nodes.truncate(lengths.0);
                clips.truncate(lengths.1);
                blank.truncate(lengths.2);
                layered.truncate(lengths.3);
            }
            stats.walked += 1;
            let fresh = self.nodes.get(&id).is_some_and(|p| {
                p.source == source
                    && p.background == background
                    && p.size == size
                    && p.visible.is_none_or(|v| v == visible)
                    && p.materials.is_none_or(|m| m == can_show)
                    && p.foreground.on == foreground.on
                    && p.on_material == on_material
                    && (p.materials.is_none() || p.foreground.own == foreground.own)
                    && p.obscured.is_none_or(|o| o == obscured)
                    && p.marks.is_none_or(|m| m == self.marks())
                    && p.pinned == pinned
            });
            if !fresh {
                let mut ops = Vec::new();
                // the window's own background, under everything its content paints
                if let Some(background) = background {
                    let bounds = Rect::new(0.0, 0.0, size.width, size.height);
                    let surface_fill = |surface, alpha| {
                        pin_envs[0]
                            .theme
                            .surface(surface)
                            .fill
                            .map(|fill: Color| Color {
                                a: fill.a * crate::surface_alpha(alpha) as f32,
                                ..fill
                            })
                    };
                    let (shape, color) = match background {
                        WindowBackground::Surface {
                            surface,
                            radius,
                            alpha,
                            ..
                        } => (
                            Shape::RoundedRect {
                                rect: bounds,
                                radius,
                            },
                            surface_fill(surface, alpha),
                        ),
                        WindowBackground::Solid(color) => (Shape::Rect(bounds), Some(color)),
                        _ => {
                            let state = StateVariant {
                                window_active: input.window_active,
                                ..StateVariant::NORMAL
                            };
                            let theme = &pin_envs[0].theme.colors;
                            (
                                Shape::Rect(bounds),
                                theme.get(ColorRole::WindowBackground, state),
                            )
                        }
                    };
                    ops.extend(color.map(|color| PaintOp::Fill {
                        shape,
                        paint: Paint::Solid(color),
                    }));
                }
                let mut group = None;
                let mut animate = false;
                let mut images = Vec::new();
                let mut reads_visible = false;
                let mut material = None;
                let mut asked_material = false;
                let mut overlay = false;
                let mut idle = false;
                let mut lays_material = false;
                let mut pixels = Vec::new();
                let mut reads_obscured = false;
                let marks = self.marks();
                let mut reads_marks = false;
                let mut set_scrolled = None;
                let mut set_band = None;
                let mut pin = None;
                node.widget.paint(
                    &*node.state,
                    &mut PaintCx {
                        node: id,
                        size,
                        env: match inherited {
                            Some(tone) => &glass[tone_index(tone)],
                            None => base,
                        },
                        plain_env: base,
                        glass_env: &glass[tone_index(foreground.own)],
                        focused,
                        focus_visible: focused && input.focus_visible,
                        window_active: input.window_active,
                        on_glass: inherited.is_some(),
                        backdrop: input.backdrop,
                        now: input.now,
                        ops: &mut ops,
                        group: &mut group,
                        animate: &mut animate,
                        images: &mut images,
                        visible,
                        reads_visible: &mut reads_visible,
                        materials: can_show,
                        material: &mut material,
                        asked_material: &mut asked_material,
                        overlay: &mut overlay,
                        idle: &mut idle,
                        on_material,
                        lays_material: &mut lays_material,
                        pixels: &mut pixels,
                        obscured,
                        reads_obscured: &mut reads_obscured,
                        layer: input.layer,
                        in_layer: raised && input.layer,
                        surface_layers: input.surface_layers,
                        glass_support: input.glass,
                        marks,
                        reads_marks: &mut reads_marks,
                        set_page_scrolled: &mut set_scrolled,
                        set_band_shown: &mut set_band,
                        pin: &mut pin,
                    },
                );
                for (kept, set) in [
                    (&mut self.scrolled_pages, set_scrolled),
                    (&mut self.bands, set_band),
                ] {
                    match set {
                        Some(v) => kept.insert(id, v),
                        None => kept.remove(&id),
                    };
                }
                if reads_marks {
                    mark_readers.push(id);
                }
                if animate {
                    animating.push(id);
                }
                if idle {
                    idle_nodes.push(id);
                }
                self.revision += 1;
                self.nodes.insert(
                    id,
                    Painted {
                        source,
                        background,
                        size,
                        revision: PaintRevision(self.revision),
                        extent: pixel_extent(&ops, &pixels, input.env.scale),
                        ops,
                        group,
                        images,
                        visible: reads_visible.then_some(visible),
                        materials: asked_material.then_some(can_show),
                        material,
                        foreground,
                        overlay,
                        on_material,
                        lays_material,
                        pixels,
                        obscured: reads_obscured.then_some(obscured),
                        marks: reads_marks.then_some(marks),
                        pinned,
                        pins: pin,
                    },
                );
            }
            let painted = &self.nodes[&id];
            pinned_below.insert(id, painted.pins.or(pinned));
            pins |= painted.pins.is_some();
            material_below.insert(id, on_material || painted.lays_material);
            on_glass.insert(
                id,
                match (painted.material, inherited) {
                    (Some(NodeMaterial { glass: Some(_), .. }), _) => Under::Glass(foreground.own),
                    (Some(_), _) => Under::Material,
                    (None, Some(tone)) => Under::Glass(tone),
                    (None, None) => under,
                },
            );
            blank.push(painted.ops.is_empty() && painted.group.is_none());
            if let Some(material) = painted.material {
                let shown = visible.translate(origin.x, origin.y);
                if shown.width() > 0.0 && shown.height() > 0.0 && material.glass.is_some() {
                    glass_shown.push((shown, foreground.own));
                }
                // a surface layer's glass is drawn with the layer, not under the window
                if shown.width() > 0.0 && shown.height() > 0.0 && !material.in_layer {
                    let radius = if material.capsule {
                        placement.rect.size.width.min(placement.rect.size.height) / 2.0
                    } else {
                        material.radius
                    };
                    materials.push(MaterialRegion {
                        material: material.name.to_owned(),
                        rect: shown,
                        radius,
                        alpha: material.alpha,
                        glass: material.glass,
                        raised: raised && input.layer,
                        blur_ramp: material.blur_ramp,
                    });
                }
            }
            add_images(&mut resources, painted);
            // a scroll container that clips keeps its content as a layer the renderer moves by the drawn offset
            let layer = match node.spec.kind {
                ContainerKind::Scroll {
                    horizontal,
                    vertical,
                } if placement.own_clip.is_some() => {
                    let drawn = crate::layout_view::drawn_offset(
                        node.widget.scroll_offset(&*node.state),
                        input.env.scale,
                    );
                    let x = match (horizontal, input.env.direction) {
                        (false, _) => 0.0,
                        (true, LayoutDirection::Ltr) => drawn.x,
                        (true, LayoutDirection::Rtl) => -drawn.x,
                    };
                    let y = if vertical { drawn.y } else { 0.0 };
                    Some(Layer::Scroll {
                        offset: Point::new(x, y),
                    })
                }
                _ => group_layer(painted),
            };
            if recording.is_none() && candidate {
                recording = Some(Recording {
                    root: id,
                    ground,
                    revision: node.subtree_revision,
                    nodes: Vec::new(),
                    open: Vec::new(),
                    keeps: true,
                });
            }
            if let Some(rec) = &mut recording {
                rec.nodes.push(id);
                rec.open.push(id);
                rec.keeps &= !raised
                    && !focused
                    && !painted.overlay
                    && painted.material.is_none()
                    && painted.materials.is_none()
                    && painted.pins.is_none()
                    && painted.marks.is_none()
                    && !matches!(layer, Some(Layer::Scroll { .. }))
                    && !self.scrolled_pages.contains_key(&id)
                    && !self.bands.contains_key(&id)
                    && !(under == Under::Window && inherited.is_some())
                    && animating.last() != Some(&id);
            }
            list.nodes.push(paint_node(
                id,
                node.parent,
                placement,
                painted,
                layer,
                input.env.scale,
            ));
            clips.push(placement.clip);
            layered.push(raised);
            seen.insert(id);
            if painted.overlay {
                overlaid.insert(id);
                let mut fills = Vec::new();
                let mut clip = None;
                let mut reach_obscured = false;
                let band_shown = self.marks().band_shown;
                let mut reads_band = false;
                node.widget.overlay(
                    &*node.state,
                    &mut OverlayCx {
                        size,
                        env: match inherited {
                            Some(tone) => &glass[tone_index(tone)],
                            None => base,
                        },
                        now: input.now,
                        origin,
                        ops: &mut fills,
                        clip: &mut clip,
                        obscured,
                        reach_obscured: &mut reach_obscured,
                        band_shown,
                        reads_band: &mut reads_band,
                    },
                );
                if reads_band {
                    band_readers.push((id, band_shown));
                }
                // a caret and the like are cut from what raised nodes cover, as what they mark lies under them
                let cut = match (raised || reach_obscured, placement.clip.or(window)) {
                    (false, Some(around)) if !input.obscuring.is_empty() => {
                        Some(unobscured(around, input.obscuring))
                    }
                    _ => None,
                };
                let clip = match (clip, cut) {
                    (Some(clip), Some(cut)) => Some(clip.intersection(&cut).unwrap_or(Rect::new(
                        cut.min_x(),
                        cut.min_y(),
                        0.0,
                        0.0,
                    ))),
                    (clip, cut) => clip.or(cut),
                };
                if let Some(mut overlay) = self.overlay(id, fills, clip, placement.clip) {
                    // a scroll container's own overlay (its scroll bars) stays where it is as the content moves: it
                    // lies beside the content's layer, over it, not in it
                    if matches!(layer, Some(Layer::Scroll { .. })) {
                        overlay.parent = node.parent.map(PaintNodeId::from);
                    }
                    pending.push(Pending {
                        owner: id,
                        node: overlay,
                        clip: placement.clip,
                        layered: raised,
                    });
                }
            }
        }
        flush(
            &mut pending,
            None,
            &mut list,
            &mut clips,
            &mut blank,
            &mut layered,
        );
        if let Some(rec) = recording.take().filter(|rec| rec.keeps) {
            segments.insert(rec.root, rec.segment());
        }
        self.segments = segments;
        self.stats = stats;
        self.nodes.retain(|id, _| seen.contains(id));
        self.scrolled_pages.retain(|id, _| seen.contains(id));
        self.bands.retain(|id, _| seen.contains(id));
        // a node that painted before the mark it read was set paints again on the next frame
        let marks = self.marks();
        for id in mark_readers {
            if self
                .nodes
                .get(&id)
                .is_some_and(|p| p.marks.is_some_and(|m| m != marks))
            {
                self.nodes.remove(&id);
                animating.push(id);
            }
        }
        for (id, saw) in band_readers {
            if saw != marks.band_shown {
                animating.push(id);
            }
        }
        self.overlays.retain(|id, _| overlaid.contains(id));
        let any_raised = layered.iter().any(|&l| l);
        if any_raised && !input.layer {
            // in place, the raised subtrees draw after the rest of the window, over it
            let mut main = (Vec::new(), Vec::new(), Vec::new());
            let mut over = (Vec::new(), Vec::new(), Vec::new());
            for ((node, clip), (blank, raised)) in std::mem::take(&mut list.nodes)
                .into_iter()
                .zip(std::mem::take(&mut clips))
                .zip(std::mem::take(&mut blank).into_iter().zip(&layered))
            {
                let to = if *raised { &mut over } else { &mut main };
                to.0.push(node);
                to.1.push(clip);
                to.2.push(blank);
            }
            main.0.append(&mut over.0);
            main.1.append(&mut over.1);
            main.2.append(&mut over.2);
            (list.nodes, clips, blank) = main;
        }
        let layer = (any_raised && input.layer).then(|| {
            // the raised subtrees move to a list of their own; a subtree's root keeps the clip of its ancestors
            let mut main = (DisplayList::default(), Vec::new(), Vec::new());
            let mut raised = (DisplayList::default(), Vec::new(), Vec::new());
            let ids: IdSet<PaintNodeId> = list
                .nodes
                .iter()
                .zip(&layered)
                .filter(|(_, &l)| l)
                .map(|(n, _)| n.id)
                .collect();
            let mut roots = 0;
            for ((mut node, clip), (blank, layered)) in std::mem::take(&mut list.nodes)
                .into_iter()
                .zip(std::mem::take(&mut clips))
                .zip(std::mem::take(&mut blank).into_iter().zip(&layered))
            {
                let to = if *layered { &mut raised } else { &mut main };
                if *layered && !node.parent.is_some_and(|p| ids.contains(&p)) {
                    node.parent = clip.map(|clip| {
                        let id = layer_clip_id(roots);
                        roots += 1;
                        to.0.nodes.push(PaintNode {
                            layer: None,
                            id,
                            parent: None,
                            revision: PaintRevision(0),
                            transform: Transform::IDENTITY,
                            clip: Some(Shape::Rect(clip)),
                            bounds: clip,
                            hit: Rect::new(clip.min_x(), clip.min_y(), 0.0, 0.0),
                            group: None,
                            ops: Vec::new(),
                        });
                        to.1.push(None);
                        to.2.push(true);
                        id
                    });
                }
                to.0.nodes.push(node);
                to.1.push(clip);
                to.2.push(blank);
            }
            (list, clips, blank) = main;
            let (list, own, blank) = finish(raised.0, &raised.1, raised.2, &self.scratch);
            let bottom = list
                .nodes
                .iter()
                .filter(|n| n.parent.is_none() || !n.ops.is_empty())
                .map(|n| n.bounds.max_y())
                .fold(0.0_f64, f64::max);
            LayerPainting {
                list,
                own,
                blank,
                bottom,
            }
        });
        let (list, own, blank) = finish(list, &clips, blank, &self.scratch);
        Painting {
            list,
            own,
            blank,
            animating,
            idle: idle_nodes,
            resources,
            materials,
            layer,
            pins,
            kept_runs,
        }
    }
}

impl Recording {
    fn segment(self) -> Segment {
        Segment {
            revision: self.revision,
            ground: self.ground,
            nodes: self.nodes,
        }
    }
}

/// What of a node at `placement` shows, in its own coordinates: its rect cut by its ancestors' clip and the window.
fn shown_part(placement: &Placement, window: Option<Rect>) -> Rect {
    let origin = placement.rect.origin;
    [placement.clip, window]
        .into_iter()
        .flatten()
        .try_fold(placement.rect, |shown, clip| shown.intersection(&clip))
        .map_or(Rect::new(0.0, 0.0, 0.0, 0.0), |shown| {
            shown.translate(-origin.x, -origin.y)
        })
}

/// The images a node's paint draws, added to the list's resources.
fn add_images(resources: &mut PaintResources, painted: &Painted) {
    for (image, pixels) in &painted.images {
        let kept = resources
            .images
            .entry(*image)
            .or_insert_with(|| pixels.clone());
        debug_assert!(
            kept.revision == pixels.revision && Arc::ptr_eq(&kept.pixels, &pixels.pixels),
            "resource {image:?} names two images; new pixels take a new id"
        );
    }
}

/// The layer a node's group makes: a group of opacity alone below full is kept as an image of its subtree.
fn group_layer(painted: &Painted) -> Option<Layer> {
    painted
        .group
        .as_ref()
        .filter(|g| g.effects.is_empty() && g.opacity < 1.0)
        .map(|_| Layer::Opacity)
}

/// The list's node of `id` (whose parent is `parent`) at `placement` with what it painted.
fn paint_node(
    id: NodeId,
    parent: Option<NodeId>,
    placement: &Placement,
    painted: &Painted,
    layer: Option<Layer>,
    scale: f64,
) -> PaintNode {
    let origin = placement.rect.origin;
    PaintNode {
        layer,
        id: id.into(),
        parent: parent.map(PaintNodeId::from),
        revision: painted.revision,
        transform: Transform([1.0, 0.0, 0.0, 1.0, origin.x, origin.y]),
        clip: placement
            .own_clip
            .map(|c| Shape::Rect(c.translate(-origin.x, -origin.y))),
        bounds: bounds(placement.rect, painted.extent, placement.clip),
        hit: placement
            .clip
            .map_or(Some(placement.target), |c| {
                c.intersection(&placement.target)
            })
            .unwrap_or(Rect::new(origin.x, origin.y, 0.0, 0.0)),
        group: painted.group.clone(),
        ops: on_pixels(&painted.ops, &painted.pixels, origin, scale),
    }
}

impl Painter {
    /// The paint node of `owner`'s overlay drawing `fills` (window coordinates) inside `clip` and the clip of the
    /// owner's ancestors, `around`; none when it draws nothing. Its revision changes only when what it draws does.
    fn overlay(
        &mut self,
        owner: NodeId,
        fills: Vec<PaintOp>,
        clip: Option<Rect>,
        around: Option<Rect>,
    ) -> Option<PaintNode> {
        let reach = extent(&fills)?;
        let reach = [clip, around.map(|c| c.outset(CLIP_EDGE))]
            .into_iter()
            .flatten()
            .try_fold(reach, |reach, clip| reach.intersection(&clip))?;
        let mut ops = Vec::with_capacity(fills.len() + 2);
        ops.extend(clip.map(|c| PaintOp::PushClip(Shape::Rect(c))));
        ops.extend(fills);
        ops.extend(clip.map(|_| PaintOp::PopClip));
        let revision = match self.overlays.get(&owner) {
            Some((drawn, revision)) if *drawn == ops => *revision,
            // counted apart from the nodes' revisions, which an overlay leaves as they were
            _ => {
                self.overlay_revision += 1;
                PaintRevision(self.overlay_revision)
            }
        };
        self.overlays.insert(owner, (ops.clone(), revision));
        Some(PaintNode {
            layer: None,
            id: overlay_id(owner),
            parent: Some(owner.into()),
            revision,
            transform: Transform::IDENTITY,
            clip: None,
            bounds: reach,
            hit: Rect::new(reach.min_x(), reach.min_y(), 0.0, 0.0),
            group: None,
            ops,
        })
    }
}
