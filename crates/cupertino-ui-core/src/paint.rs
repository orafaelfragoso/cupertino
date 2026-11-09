//! The paint pass. Each node paints in its own coordinates into its own [`PaintNode`], whose transform places it in
//! the window. A node's clip is the one it puts on its descendants, in its coordinates; the renderer intersects the
//! clips along the parent chain. Bounds cover what the node and its whole subtree draw, in window coordinates. A
//! node repaints only when it changed or its size did, and keeps its paint revision otherwise.

use crate::cx::{OverlayCx, PaintCx};
use crate::route::Geometry;
use crate::tree::Tree;
use crate::{Environment, NodeId, WindowBackground};
use cupertino_geometry::{Insets, Point, Rect, Size};
use cupertino_ui_platform::{GlassRegion, GlassStyle, GlassTone, MaterialRegion, Timestamp};
use cupertino_ui_render::{
    DisplayList, Effect, Group, ImageResource, Paint, PaintNode, PaintNodeId, PaintOp,
    PaintResources, PaintRevision, PathVerb, ResourceId, Shape, Transform,
};
use cupertino_ui_theme::FaceTone;
use cupertino_ui_theme::{Color, ColorRole, StateVariant};
use std::collections::{HashMap, HashSet};
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
}

/// The glass a node paints on, which picks the colours of its text and symbols: the tone of the glass it lies on, if
/// any, and, for a node that asked for a material, the tone of the region it would get.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Foreground {
    on: Option<GlassTone>,
    own: GlassTone,
}

/// The tone of each glass region of a window ([`WindowEvent::GlassTone`](cupertino_ui_platform::WindowEvent)): the
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

/// `env` with the theme of text on glass of `tone` ([`ThemeTokens::on_glass`](cupertino_ui_theme::ThemeTokens)).
fn glass_environment(env: &Environment, tone: GlassTone) -> Environment {
    Environment {
        theme: Arc::new(env.theme.on_glass(face(tone))),
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
    pub(crate) alpha: f64,
    pub(crate) glass: Option<GlassRegion>,
}

/// How far a blur reaches beyond the content it blurs, per unit of radius.
const BLUR_REACH: f64 = 3.0;

/// How far past its clip, in points, a node's content may still change a pixel: the clip's anti-aliased edge covers
/// part of the pixel it falls in, and the renderer scales the content's own coverage of that pixel by it.
const CLIP_EDGE: f64 = 1.0;

/// Everything `ops` may touch, in their own coordinates: strokes reach half their width outside the shape, effects
/// reach as far as their blur and offset, and a glyph is taken to fill its em square around the pen with room for
/// ascenders, descenders and overhang.
/// `ops` of a node at `origin` in the window, with those at `pixels` on the window's device pixels at `scale`: a fill
/// at the nearest pixel edges, a clip widened to the pixels it touches. Each stays on the pixels its rect touches but a
/// fill narrower than a pixel, which grows to one ([`pixel_extent`]).
fn on_pixels(ops: &[PaintOp], pixels: &[usize], origin: Point, scale: f64) -> Vec<PaintOp> {
    let mut ops = ops.to_vec();
    let snap = |rect: Rect, to: fn(Rect, f64) -> Rect| {
        to(rect.translate(origin.x, origin.y), scale).translate(-origin.x, -origin.y)
    };
    for &i in pixels {
        match ops.get_mut(i) {
            Some(PaintOp::Fill {
                shape: Shape::Rect(rect),
                ..
            }) => *rect = snap(*rect, crate::cx::pixel_fill),
            Some(PaintOp::PushClip(Shape::Rect(rect))) => {
                *rect = snap(*rect, crate::cx::pixel_clip)
            }
            _ => {}
        }
    }
    ops
}

/// The extent of `ops`, those at `pixels` to be put on device pixels at `scale` ([`on_pixels`]): a fill there reaches
/// at least one device pixel each way, which is as far as it can grow.
fn pixel_extent(ops: &[PaintOp], pixels: &[usize], scale: f64) -> Option<Rect> {
    if pixels.is_empty() {
        return extent(ops);
    }
    let pixel = 1.0 / crate::cx::usable_scale(scale);
    let mut ops = ops.to_vec();
    for &i in pixels {
        if let Some(PaintOp::Fill {
            shape: Shape::Rect(rect),
            ..
        }) = ops.get_mut(i)
        {
            *rect = Rect::new(
                rect.min_x(),
                rect.min_y(),
                rect.width().max(pixel),
                rect.height().max(pixel),
            );
        }
    }
    extent(&ops)
}

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
    /// The environment the kept paint was made in; any other repaints every node.
    painted_in: Option<Environment>,
    /// The tone of the window's glass regions; a change repaints what lies on the region.
    pub(crate) tones: GlassTones,
    /// The environments of text on glass of each tone, made from `painted_in`.
    glass_envs: Option<[Environment; 3]>,
    /// What each node's overlay drew last, and the revision it drew with, while the node asks for one: a caret that
    /// shows again where it showed before is the same paint.
    overlays: HashMap<NodeId, (Vec<PaintOp>, PaintRevision)>,
    overlay_revision: u64,
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

/// How far the raised nodes' rects (`raised`, window coordinates) cover `rect` from its top: a raised rect that lies
/// across it and reaches its top edge covers it down to the raised rect's bottom. The raised layer runs from the
/// window's top edge, so nothing raised covers a node from another edge.
pub(crate) fn obscured_insets(rect: Rect, raised: &[(NodeId, Rect)]) -> Insets {
    const SLACK: f64 = 0.5;
    let top = raised
        .iter()
        .filter(|(_, r)| {
            r.min_y() <= rect.min_y() + SLACK
                && r.max_y() > rect.min_y()
                && r.min_x().max(rect.min_x()) < r.max_x().min(rect.max_x())
        })
        .map(|(_, r)| r.max_y().min(rect.max_y()) - rect.min_y())
        .fold(0.0_f64, f64::max);
    Insets {
        top,
        left: 0.0,
        bottom: 0.0,
        right: 0.0,
    }
}

/// The raised nodes ([`Widget::raised`](crate::Widget::raised)) under `root` with their rects in window coordinates,
/// in paint order; a raised node's subtree is part of it, so none under it is listed.
pub(crate) fn raised_nodes<M: 'static>(
    tree: &Tree<M>,
    root: NodeId,
    geometry: &Geometry,
) -> Vec<(NodeId, Rect)> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        let Some(node) = tree.get(id) else { continue };
        if node.widget.raised(&*node.state) {
            if let Some(placement) = geometry.get(&id) {
                out.push((id, placement.rect));
            }
            continue;
        }
        stack.extend(node.children.iter().rev());
    }
    out
}

/// `rect` less the top of it raised rects lie across, whole width and from its top edge: what of it shows below them.
pub(crate) fn unobscured(rect: Rect, raised: &[(NodeId, Rect)]) -> Rect {
    const SLACK: f64 = 0.5;
    let top = raised
        .iter()
        .filter(|(_, r)| {
            r.min_y() <= rect.min_y() + SLACK
                && r.max_y() > rect.min_y()
                && r.min_x() <= rect.min_x() + SLACK
                && r.max_x() >= rect.max_x() - SLACK
        })
        .map(|(_, r)| r.max_y())
        .fold(rect.min_y(), f64::max)
        .min(rect.max_y());
    Rect::from_ltrb(rect.min_x(), top, rect.max_x(), rect.max_y())
}

/// The id of the node that clips the `n`th raised subtree of a layer to the clip of its ancestors: apart from every
/// node's (a generation in the upper half) and every overlay's (none, and an index below `u32::MAX`).
fn layer_clip_id(n: usize) -> PaintNodeId {
    PaintNodeId(u64::MAX - n as u64)
}

/// Whether `ancestor` is `id` or holds it.
fn within<M: 'static>(tree: &Tree<M>, ancestor: NodeId, id: NodeId) -> bool {
    let mut at = Some(id);
    while let Some(node) = at {
        if node == ancestor {
            return true;
        }
        at = tree.get(node).and_then(|n| n.parent);
    }
    false
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
    /// The images the list draws.
    pub(crate) resources: PaintResources,
    /// The materials the nodes asked for, in window coordinates, in paint order.
    pub(crate) materials: Vec<MaterialRegion>,
    /// What the raised nodes draw, when the window system shows them in a layer of their own.
    pub(crate) layer: Option<LayerPainting>,
}

/// The raised layer of a [`Painting`]: its own list, damage boxes and blank flags, in window coordinates.
pub(crate) struct LayerPainting {
    pub(crate) list: DisplayList,
    pub(crate) own: Vec<Rect>,
    pub(crate) blank: Vec<bool>,
    /// How far down the window the layer's nodes draw, in points.
    pub(crate) bottom: f64,
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
    /// The window system shows a raised layer, which the raised nodes draw in.
    pub(crate) layer: bool,
}

/// The paint node a UI node draws into, so tests and the inspector can find a node's operations.
impl From<NodeId> for PaintNodeId {
    fn from(id: NodeId) -> Self {
        PaintNodeId(id.0.get())
    }
}

impl Painter {
    pub(crate) fn paint<M: 'static>(
        &mut self,
        tree: &Tree<M>,
        root: NodeId,
        geometry: &Geometry,
        input: &PaintInput<'_>,
    ) -> Painting {
        // a node paints in its environment's colours, type and pixels, none of which its own revision covers
        if !self
            .painted_in
            .as_ref()
            .is_some_and(|env| same_environment(env, input.env))
        {
            self.nodes.clear();
            self.painted_in = Some(input.env.clone());
            self.glass_envs = None;
        }
        let glass_envs = self
            .glass_envs
            .get_or_insert_with(|| {
                [GlassTone::Appearance, GlassTone::Dark, GlassTone::Light]
                    .map(|tone| glass_environment(input.env, tone))
            })
            .clone();
        // What each node lies on, as its children inherit it: the window's material when it is glass and shown, below
        // a node that shows a material that node's region. A node with nothing under it from its ancestors lies on
        // the glass painted before it that holds it whole: a panel's content is often drawn beside the node of its
        // glass, after it.
        let mut on_glass: HashMap<NodeId, Under> = HashMap::new();
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
        let mut resources = PaintResources::default();
        let mut seen = HashSet::with_capacity(self.nodes.len());
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
        let raised_ids: HashSet<NodeId> = input.raised.iter().map(|(id, _)| *id).collect();
        // whether each node draws in the raised layer, as its children inherit it
        let mut in_layer: HashMap<NodeId, bool> = HashMap::new();
        let mut overlaid = HashSet::new();
        // whether each node's children lie on a material, from the window's background down
        let mut material_below: HashMap<NodeId, bool> = HashMap::new();
        let window_material = matches!(
            input.background,
            WindowBackground::Surface { surface, .. } if crate::cx::translucent(surface)
        );
        for id in tree.preorder(root) {
            let (Some(node), Some(placement)) = (tree.get(id), geometry.get(&id)) else {
                continue;
            };
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
                obscured_insets(placement.rect, input.raised)
            };
            let size = placement.rect.size;
            let focused = input.focus == Some(id);
            let background = (id == root
                && match input.background {
                    WindowBackground::Theme => true,
                    WindowBackground::Surface { .. } => !input.backdrop,
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
            // clipped away (`placements`), so it shows
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
            let on_material = inherited.is_some()
                || match node.parent {
                    Some(parent) => material_below.get(&parent).copied().unwrap_or(false),
                    None => window_material,
                };
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
            });
            if !fresh {
                let mut ops = Vec::new();
                // the window's own background, under everything its content paints
                if let Some(background) = background {
                    let bounds = Rect::new(0.0, 0.0, size.width, size.height);
                    let surface_fill = |surface, alpha| {
                        input
                            .env
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
                        } => (
                            Shape::RoundedRect {
                                rect: bounds,
                                radius,
                            },
                            surface_fill(surface, alpha),
                        ),
                        _ => {
                            let state = StateVariant {
                                window_active: input.window_active,
                                ..StateVariant::NORMAL
                            };
                            let theme = &input.env.theme.colors;
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
                let mut lays_material = false;
                let mut pixels = Vec::new();
                let mut reads_obscured = false;
                node.widget.paint(
                    &*node.state,
                    &mut PaintCx {
                        node: id,
                        size,
                        env: match inherited {
                            Some(tone) => &glass_envs[tone_index(tone)],
                            None => input.env,
                        },
                        plain_env: input.env,
                        glass_env: &glass_envs[tone_index(foreground.own)],
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
                        on_material,
                        lays_material: &mut lays_material,
                        pixels: &mut pixels,
                        obscured,
                        reads_obscured: &mut reads_obscured,
                        layer: input.layer,
                    },
                );
                if animate {
                    animating.push(id);
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
                    },
                );
            }
            let painted = &self.nodes[&id];
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
                if shown.width() > 0.0 && shown.height() > 0.0 {
                    if material.glass.is_some() {
                        glass_shown.push((shown, foreground.own));
                    }
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
                    });
                }
            }
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
            list.nodes.push(PaintNode {
                id: id.into(),
                parent: node.parent.map(PaintNodeId::from),
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
                ops: on_pixels(&painted.ops, &painted.pixels, origin, input.env.scale),
            });
            clips.push(placement.clip);
            layered.push(raised);
            seen.insert(id);
            if painted.overlay {
                overlaid.insert(id);
                let mut fills = Vec::new();
                let mut clip = None;
                let mut reach_obscured = false;
                node.widget.overlay(
                    &*node.state,
                    &mut OverlayCx {
                        size,
                        env: match inherited {
                            Some(tone) => &glass_envs[tone_index(tone)],
                            None => input.env,
                        },
                        now: input.now,
                        origin,
                        ops: &mut fills,
                        clip: &mut clip,
                        obscured,
                        reach_obscured: &mut reach_obscured,
                    },
                );
                // a caret and the like are cut from what raised nodes cover, as what they mark lies under them
                let cut = match (raised || reach_obscured, placement.clip.or(window)) {
                    (false, Some(around)) if !input.raised.is_empty() => {
                        Some(unobscured(around, input.raised))
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
                if let Some(overlay) = self.overlay(id, fills, clip, placement.clip) {
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
        self.nodes.retain(|id, _| seen.contains(id));
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
            let ids: HashSet<PaintNodeId> = list
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
            let (list, own, blank) = finish(raised.0, &raised.1, raised.2);
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
        let (list, own, blank) = finish(list, &clips, blank);
        Painting {
            list,
            own,
            blank,
            animating,
            resources,
            materials,
            layer,
        }
    }
}

/// The list's boxes made whole: each node's bounds widened by its children and its group's effects, and what each
/// node damages when it changes. `clips` are the clips of each node's ancestors, in the list's order.
fn finish(
    mut list: DisplayList,
    clips: &[Option<Rect>],
    blank: Vec<bool>,
) -> (DisplayList, Vec<Rect>, Vec<bool>) {
    let mut own: Vec<Rect> = list.nodes.iter().map(|n| n.bounds).collect();
    // Children come after their parent, so walking backwards widens each parent by its finished children.
    let index: HashMap<PaintNodeId, usize> = list
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id, i))
        .collect();
    for i in (0..list.nodes.len()).rev() {
        if let Some(group) = &list.nodes[i].group {
            let widened = with_effects(list.nodes[i].bounds, &group.effects);
            list.nodes[i].bounds = match clips[i] {
                Some(clip) => widened
                    .intersection(&clip.outset(CLIP_EDGE))
                    .unwrap_or(list.nodes[i].bounds),
                None => widened,
            };
        }
        let (bounds, parent) = (list.nodes[i].bounds, list.nodes[i].parent);
        if let Some(&p) = parent.and_then(|p| index.get(&p)) {
            list.nodes[p].bounds = list.nodes[p].bounds.union(&bounds);
        }
    }
    // A group's opacity and effects reach its whole subtree, so the group damages all of it; under a group with
    // effects, which spread each pixel across their reach, any node damages the outermost such group's whole reach.
    let mut spread: Vec<Option<usize>> = Vec::with_capacity(list.nodes.len());
    for (i, (node, own)) in list.nodes.iter().zip(&mut own).enumerate() {
        let above = node
            .parent
            .and_then(|p| index.get(&p))
            .and_then(|&p| spread[p]);
        if node.group.is_some() {
            *own = node.bounds;
        }
        if let Some(group) = above {
            *own = list.nodes[group].bounds;
        }
        let effects = node.group.as_ref().is_some_and(|g| !g.effects.is_empty());
        spread.push(above.or(effects.then_some(i)));
    }
    (list, own, blank)
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

/// The node's layout rect together with what its ops touch, in window coordinates, cut by its clip.
fn bounds(rect: Rect, extent: Option<Rect>, clip: Option<Rect>) -> Rect {
    let reach = extent.map_or(rect, |e| {
        rect.union(&e.translate(rect.origin.x, rect.origin.y))
    });
    match clip {
        Some(clip) => reach
            .intersection(&clip.outset(CLIP_EDGE))
            .unwrap_or(Rect::new(rect.origin.x, rect.origin.y, 0.0, 0.0)),
        None => reach,
    }
}

#[cfg(test)]
mod tests;
