//! The pieces of a parent surface's layers (`cupertino_layers_v1`, [`crate::layers_v1`]): each layer's content where
//! its animated values put it, the placeholder where its content has not arrived, and its glass below it, moving,
//! scaling and fading with it. Raised layers come in front of the flat ones and lift their glass with a soft shadow; a
//! vibrant layer draws its coverage blended with the glass behind it ([`SurfaceRole::Vibrant`]). The glass of the layers
//! of one glass group is one container of their shapes as shown, drawn below the content of its first layer
//! ([`glass_anchors`]), and the edge of that layer (`set_glass_edge`) runs along the outside of the container's outline at
//! its width in points, whatever the layers' scale, softened as that layer's content blurs ([`glass_edge`]). A layer
//! of the normal role whose content blurs is drawn blurred ([`SurfaceRole::Blurred`], `super::content_blur`). A layer is drawn only inside its parent's window, its glass and placeholder too, wherever its
//! values place it ([`masked_clip`], [`clear_of_edges`]).

use cupertino_scene::{Clip, ContentBlur, SceneKind, SurfaceRole, VibrantBlend};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Physical, Point, Rectangle};

use super::compose::{corner_style, logical_rect, output_rect, Piece, Tree};
use super::scene::{rrect_item, SceneParams};
use super::shader_rect::Sig;
use crate::app_effects_v1::{BackdropArea, GlassSpec};
use crate::layers_v1::{present, BlendRole, LayerDraw, VibrantWeight};

/// The drop shadow of a raised layer's glass: offset down in points, strength in percent, blur radius in points.
pub const RAISED_SHADOW: ([f32; 2], f32, f32) = ([0.0, 3.0], 10.0, 12.0);
/// Opacity of a vibrant label over a dark and over a light face, by weight: primary, secondary, fill.
pub const VIBRANT_WEIGHT_DARK: [f32; 3] = [0.847, 0.549, 0.098];
pub const VIBRANT_WEIGHT_LIGHT: [f32; 3] = [0.847, 0.498, 0.098];
/// Bit set in the backdrop keys of layers' glass, apart from the client's backdrops.
const LAYER_GLASS_KEY: u64 = 1 << 62;

/// Where a parent's layers are drawn: the parent's origin and the rescale of its window.
pub struct LayerPlace {
    /// The parent surface's origin, output-local points, before the window's rescale.
    pub origin: Point<f64, Logical>,
    /// The window's rescale: about `pivot` (output-local points) by `scale`.
    pub pivot: Point<f64, Logical>,
    pub scale: (f64, f64),
    pub opacity: f32,
    /// The parent's window: the layers are drawn inside it.
    pub clip: Clip,
    pub kind: SceneKind,
}

impl LayerPlace {
    /// `q` (output-local points) moved with the window's rescale.
    fn map(&self, q: Point<f64, Logical>) -> Point<f64, Logical> {
        Point::from((
            self.pivot.x + (q.x - self.pivot.x) * self.scale.0,
            self.pivot.y + (q.y - self.pivot.y) * self.scale.1,
        ))
    }

    fn map_rect(&self, r: Rectangle<f64, Logical>) -> Rectangle<f64, Logical> {
        Rectangle::new(
            self.map(r.loc),
            (r.size.w * self.scale.0, r.size.h * self.scale.1).into(),
        )
    }
}

/// A layer's bounds scaled about their centre, output-local points before the window's rescale.
fn scaled_bounds(l: &LayerDraw, origin: Point<f64, Logical>) -> Rectangle<f64, Logical> {
    let c = centre(l, origin);
    Rectangle::new(
        (
            c.x - 0.5 * l.width * l.scale,
            c.y - 0.5 * l.height * l.scale,
        )
            .into(),
        (l.width * l.scale, l.height * l.scale).into(),
    )
}

fn centre(l: &LayerDraw, origin: Point<f64, Logical>) -> Point<f64, Logical> {
    Point::from((
        origin.x + l.x + 0.5 * l.width,
        origin.y + l.y + 0.5 * l.height,
    ))
}

/// The window a parent's layers are drawn inside, from their place's clip: its rectangle (output-local points), its
/// corner radius and corner style; `None` for a parent drawn without a clip.
fn window_of(
    clip: Clip,
) -> Option<(
    Rectangle<f64, Logical>,
    f64,
    cupertino_geometry::CornerStyle,
)> {
    match clip {
        Clip::None => None,
        Clip::Rect(r) => Some((logical_rect(r), 0.0, corner_style(false))),
        Clip::Rounded {
            rect,
            radius,
            style,
            ..
        } => Some((logical_rect(rect), radius, style)),
    }
}

/// Whether `r` (output-local points) lies inside the window of `clip` and clear of its rounded corners, so the window's
/// clip leaves all of it as it is. True for a parent drawn without a clip.
pub fn clear_of_edges(r: Rectangle<f64, Logical>, clip: Clip) -> bool {
    let Some((w, radius, style)) = window_of(clip) else {
        return true;
    };
    let (right, bottom) = (r.loc.x + r.size.w, r.loc.y + r.size.h);
    let (w_right, w_bottom) = (w.loc.x + w.size.w, w.loc.y + w.size.h);
    let inside = r.loc.x >= w.loc.x && r.loc.y >= w.loc.y && right <= w_right && bottom <= w_bottom;
    inside && !corners_in_rounding(r, w, radius * style.expansion())
}

/// Whether a corner of `r` lies where the corners of `w`, rounded over `reach` points, bend away from its edges.
fn corners_in_rounding(r: Rectangle<f64, Logical>, w: Rectangle<f64, Logical>, reach: f64) -> bool {
    if reach <= 0.0 {
        return false;
    }
    let (right, bottom) = (r.loc.x + r.size.w, r.loc.y + r.size.h);
    let (w_right, w_bottom) = (w.loc.x + w.size.w, w.loc.y + w.size.h);
    let near_left = r.loc.x < w.loc.x + reach;
    let near_right = right > w_right - reach;
    let near_top = r.loc.y < w.loc.y + reach;
    let near_bottom = bottom > w_bottom - reach;
    (near_left || near_right) && (near_top || near_bottom)
}

/// The clip of a layer that masks its content to its rounded bounds `shown` (output-local points, `radius` points)
/// inside the window of `clip`: its own bounds when the window leaves them as they are ([`clear_of_edges`]), else the
/// part of them inside the window, rounded by the larger of the two radii where it reaches the window's rounded corners,
/// which lies inside both outlines. `None` when no part of the bounds lies in the window: the layer draws nothing.
pub fn masked_clip(shown: Rectangle<f64, Logical>, radius: f64, clip: Clip) -> Option<Clip> {
    let own = Clip::Rounded {
        rect: output_rect(shown),
        radius,
        style: corner_style(true),
        exact: false,
    };
    if clear_of_edges(shown, clip) {
        return Some(own);
    }
    let (w, w_radius, w_style) = window_of(clip)?;
    let part = shown
        .intersection(w)
        .filter(|r| r.size.w > 0.0 && r.size.h > 0.0)?;
    let (radius, style) =
        if corners_in_rounding(part, w, w_radius * w_style.expansion()) && w_radius > radius {
            (w_radius, w_style)
        } else {
            (radius, corner_style(true))
        };
    Some(Clip::Rounded {
        rect: output_rect(part),
        radius,
        style,
        exact: false,
    })
}

/// Whether `r` (output-local points), grown by `margin` on every side, meets the window of `clip`: what lies farther out
/// is not drawn. True for a parent drawn without a clip.
fn meets_window(r: Rectangle<f64, Logical>, margin: f64, clip: Clip) -> bool {
    let Some((w, ..)) = window_of(clip) else {
        return true;
    };
    r.loc.x - margin < w.loc.x + w.size.w
        && r.loc.y - margin < w.loc.y + w.size.h
        && r.loc.x + r.size.w + margin > w.loc.x
        && r.loc.y + r.size.h + margin > w.loc.y
}

/// The parts of a layer's bounds its content does not cover, in the layer's points: right of it and below it.
pub fn uncovered((cw, ch): (f64, f64), width: f64, height: f64) -> Vec<[f64; 4]> {
    let mut out = Vec::new();
    if cw < width {
        out.push([cw.max(0.0), 0.0, width - cw.max(0.0), height]);
    }
    if ch < height {
        let w = cw.clamp(0.0, width);
        if w > 0.0 {
            out.push([0.0, ch.max(0.0), w, height - ch.max(0.0)]);
        }
    }
    out
}

/// The angle (radians, counter-clockwise from the right, up is π/2) the rim of glass centred at `c` is lit from by a
/// pointer at `pointer` (both output-local points); `None` when they meet.
pub fn pointer_light(c: Point<f64, Logical>, pointer: Point<f64, Logical>) -> Option<f32> {
    let (dx, dy) = (pointer.x - c.x, pointer.y - c.y);
    (dx * dx + dy * dy > 1e-6).then(|| (-dy).atan2(dx) as f32)
}

/// The vibrant blend of a role in the appearance, `None` for the normal role.
pub fn vibrant_blend(role: BlendRole, dark: bool, plain: bool) -> Option<VibrantBlend> {
    let BlendRole::Vibrant(w) = role else {
        return None;
    };
    let i = match w {
        VibrantWeight::Primary => 0,
        VibrantWeight::Secondary => 1,
        VibrantWeight::Fill => 2,
    };
    let weight = if dark {
        VIBRANT_WEIGHT_DARK[i]
    } else {
        VIBRANT_WEIGHT_LIGHT[i]
    };
    Some(VibrantBlend {
        dark,
        plain,
        weight: (weight * 1000.0).round() as u16,
    })
}

/// The role of a layer's content of the normal blend: blurred by `sigma` points, or as it is when it does not blur.
pub fn content_role(sigma: f64) -> SurfaceRole {
    let blur = ContentBlur::of_points(sigma);
    if blur.sigma_milli == 0 {
        SurfaceRole::WindowContent
    } else {
        SurfaceRole::Blurred(blur)
    }
}

/// The edge a layer's glass carries, in points: drawn by the glass at the output's scale and not the layer's, so its
/// width stays what the client asked as the layer scales. An edge that softens ([`crate::layers_v1::GlassEdge::soft`])
/// is blurred by the layer's content blur as the content's own Gaussian blurs at `scale` device pixels a point: by its
/// deviation as the layer's scale draws it, and not at all where the content is drawn sharp
/// ([`super::content_blur::sigma_px`]).
pub fn glass_edge(l: &LayerDraw, scale: f64) -> cupertino_glass::glass::GlassEdge {
    let soft = if l.edge.soft {
        edge_softness(l.content_blur, l.scale, scale)
    } else {
        0.0
    };
    cupertino_glass::glass::GlassEdge {
        width: l.edge.width as f32,
        color: l.edge.color,
        soft: 0.0,
    }
    .soft(soft)
}

/// The deviation in output points of the Gaussian a layer's content blurred by `content_blur` (its points) is drawn
/// with, the layer at `layer_scale` and the output at `scale` device pixels a point: the content blurs before it
/// scales, so the deviation scales with it; 0 where the content is drawn sharp.
pub fn edge_softness(content_blur: f64, layer_scale: f64, scale: f64) -> f32 {
    super::content_blur::sigma_px(ContentBlur::of_points(content_blur), scale)
        .map_or(0.0, |px| (px / scale * layer_scale) as f32)
}

/// Whether a layer drawn at `opacity` and `scale` shows at all.
fn shows(opacity: f32, l: &LayerDraw) -> bool {
    opacity > 0.0 && l.scale > 0.0
}

/// For each layer of `layers` (back to front), whether its glass is drawn with it: alone, or as the first shown layer
/// of its glass group, which draws the glass of the whole group below its content.
pub fn glass_anchors(layers: &[LayerDraw], opacity: f32) -> Vec<bool> {
    let mut seen = std::collections::HashSet::new();
    layers
        .iter()
        .map(|l| {
            let drawn = l.glass.is_some()
                && shows(opacity * l.opacity as f32, l)
                && l.width * l.scale > 0.0
                && l.height * l.scale > 0.0;
            drawn && (l.group == 0 || seen.insert(l.group))
        })
        .collect()
}

/// The glass shapes of the layers of `group` that show, back to front: their bounds as shown, output-local points before
/// the window's rescale, their corner radii and opacities.
fn group_areas(
    layers: &[LayerDraw],
    group: u32,
    opacity: f32,
    origin: Point<f64, Logical>,
) -> Vec<(Rectangle<f64, Logical>, f64, f64)> {
    layers
        .iter()
        .filter(|l| l.group == group && l.glass.is_some() && shows(opacity * l.opacity as f32, l))
        .map(|l| (scaled_bounds(l, origin), l.radius * l.scale, l.opacity))
        .filter(|(b, _, _)| b.size.w > 0.0 && b.size.h > 0.0)
        .collect()
}

/// How a layer makes its glass of the glass its backdrop gives, as [`layer_pieces`] draws it: the lens, blur ramp and
/// edge it asks for, a raised layer's shadow and its rim lit from the pointer. The appearance the glass takes and the
/// pointer are read from the frame it is drawn in, so the glass of a held surface (D19) follows them as the live one does.
#[derive(Clone, Debug)]
pub struct LayerGlass {
    lens: f64,
    blur_ramp: [f32; 2],
    edge: cupertino_glass::glass::GlassEdge,
    raised: bool,
    pointer_lit: bool,
    /// The layer's centre as shown, output-local points: its rim is lit from the pointer's side of it.
    centre: Point<f64, Logical>,
    /// The layer's opacity, which the glass of a layer alone carries (a group's shapes carry their layers' already).
    opacity: Option<f32>,
}

impl LayerGlass {
    /// The glass of layer `l` of lens `lens`, its centre as shown at `centre` (output-local points), on an output of
    /// `scale` device pixels a point.
    fn of(l: &LayerDraw, lens: f64, centre: Point<f64, Logical>, scale: f64) -> Self {
        Self {
            lens,
            blur_ramp: l.blur_ramp,
            edge: glass_edge(l, scale),
            raised: l.raised,
            pointer_lit: l.pointer_lit,
            centre,
            opacity: (l.group == 0).then_some(l.opacity as f32),
        }
    }

    /// The same glass, its layer moved by `by` points.
    pub fn moved(&self, by: Point<f64, Logical>) -> Self {
        Self {
            centre: self.centre + by,
            ..self.clone()
        }
    }

    /// `pieces`, the glass of the layer's backdrop made as its parent's own backdrops are, as the layer makes it in the
    /// frame of `p`: lit from the pointer there when the layer asks for it.
    pub fn make(&self, p: &SceneParams<'_>, pieces: Vec<Piece>) -> Vec<Piece> {
        let pointer = Point::<f64, Logical>::from((p.pointer.x - p.rect.x, p.pointer.y - p.rect.y));
        let pointer_here =
            pointer.x >= 0.0 && pointer.y >= 0.0 && pointer.x <= p.rect.w && pointer.y <= p.rect.h;
        let light = (self.pointer_lit && pointer_here)
            .then(|| pointer_light(self.centre, pointer))
            .flatten();
        let mut pieces = pieces;
        for piece in &mut pieces {
            let Piece::Glass(g) = piece else { continue };
            let mut gc = (*g.glass).clone();
            gc.container.lens = self.lens as f32;
            gc.container = gc.container.with_blur_ramp(self.blur_ramp);
            if let Some(angle) = light {
                gc.container.params.highlight.angle = angle;
            }
            gc.container.edge = self.edge;
            if self.raised {
                let s = &mut gc.container.params.shadow;
                (s.offset, s.amount, s.height) = RAISED_SHADOW;
                s.fill = [0.0, 0.0, 0.0, 1.0];
                s.opacity = 1.0;
            }
            gc.key = Sig::new()
                .u(gc.key)
                .f(self.lens)
                .f(f64::from(self.blur_ramp[0]))
                .f(f64::from(self.blur_ramp[1]))
                .f(light.map_or(-10.0, f64::from))
                .u(u64::from(self.raised))
                .f(f64::from(self.edge.width))
                .f(f64::from(self.edge.color[0]))
                .f(f64::from(self.edge.color[1]))
                .f(f64::from(self.edge.color[2]))
                .f(f64::from(self.edge.color[3]))
                .f(f64::from(self.edge.soft))
                .finish();
            g.glass = std::rc::Rc::new(gc);
            if let Some(opacity) = self.opacity {
                g.opacity *= opacity;
            }
        }
        pieces
    }
}

/// The pieces of `parent`'s layers, front to back, as shown at `p.now`. `glass` makes the pieces of a layer's glass
/// from its backdrop (output-local points before the window's rescale), as the parent's own backdrops are made.
pub fn layer_pieces(
    p: &mut SceneParams<'_>,
    parent: &WlSurface,
    place: &LayerPlace,
    mut glass: impl FnMut(&mut SceneParams<'_>, Vec<BackdropArea>) -> Vec<Piece>,
) -> Vec<Piece> {
    layer_pieces_with(p, parent, place, |p, areas, made| {
        let pieces = glass(p, areas);
        made.make(p, pieces)
    })
}

/// [`layer_pieces`], `glass` making the whole of a layer's glass from its backdrop and how the layer makes its glass
/// ([`LayerGlass::make`]).
pub fn layer_pieces_with(
    p: &mut SceneParams<'_>,
    parent: &WlSurface,
    place: &LayerPlace,
    mut glass: impl FnMut(&mut SceneParams<'_>, Vec<BackdropArea>, &LayerGlass) -> Vec<Piece>,
) -> Vec<Piece> {
    let layers = present(parent, p.now, p.display.reduce_motion);
    if layers.is_empty() {
        return Vec::new();
    }
    let dark = p.config.is_dark();
    let plain = p.display.reduce_transparency || p.display.increase_contrast;
    let s_min = place.scale.0.min(place.scale.1);
    let anchors = glass_anchors(&layers, place.opacity);
    let mut out = Vec::new();
    for (l, &anchor) in layers.iter().zip(&anchors).rev() {
        let opacity = place.opacity * l.opacity as f32;
        if !shows(opacity, l) {
            continue;
        }
        let c = centre(l, place.origin);
        let mc = place.map(c);
        let bounds = scaled_bounds(l, place.origin);
        let shown = place.map_rect(bounds);
        let radius = l.radius * l.scale;
        // the content, its origin on the output's device pixels
        let origin = Point::<f64, Logical>::from((
            place.origin.x + l.x - c.x + mc.x,
            place.origin.y + l.y - c.y + mc.y,
        ));
        let drawn: Point<i32, Physical> = origin.to_physical_precise_round(p.scale);
        // inside the window always; masked, inside its own rounded bounds too, and nothing when they lie outside it.
        // The bounds move with the content onto the device pixels, as one picture, as a window's content and the
        // clips inside it do
        let clip = if l.masks {
            let snap = drawn.to_f64().to_logical(p.scale) - origin;
            let shown = Rectangle::new(shown.loc + snap, shown.size);
            match masked_clip(shown, radius * s_min, place.clip) {
                Some(clip) => clip,
                None => continue,
            }
        } else {
            place.clip
        };
        let role = vibrant_blend(l.blend, dark, plain)
            .map_or_else(|| content_role(l.content_blur), SurfaceRole::Vibrant);
        out.push(Piece::Tree(Tree {
            surface: l.surface.clone(),
            kind: place.kind,
            role,
            origin: drawn,
            pivot: mc.to_physical_precise_round(p.scale),
            scale: (l.scale * place.scale.0, l.scale * place.scale.1),
            opacity,
            clip,
            only: None,
        }));
        // a hole's source, which the server draws in the layer's bounds as shown, behind the layer's own content
        let on_shell_surface = place.kind == SceneKind::LayerContent;
        out.extend(super::scene::hole_pieces(
            p,
            &l.surface,
            shown,
            opacity,
            on_shell_surface,
        ));
        // late content: the placeholder where the content does not reach
        if l.placeholder[3] > 0.0 {
            for [x, y, w, h] in uncovered(l.content, l.width, l.height) {
                let at = Point::<f64, Logical>::from((
                    c.x + (place.origin.x + l.x + x - c.x) * l.scale,
                    c.y + (place.origin.y + l.y + y - c.y) * l.scale,
                ));
                let r = place.map_rect(Rectangle::new(at, (w * l.scale, h * l.scale).into()));
                // a fill is not clipped when drawn: it is cut to the window itself, rounded where it reaches the
                // window's rounded corners
                let Some(Clip::Rounded {
                    rect: inside,
                    radius: corner,
                    style,
                    ..
                }) = masked_clip(r, 0.0, place.clip)
                else {
                    continue;
                };
                out.push(rrect_item(
                    place.kind,
                    l.serial,
                    logical_rect(inside),
                    corner,
                    !style.is_circular(),
                    l.placeholder,
                    None,
                    opacity,
                ));
            }
        }
        let Some((style, lens)) = l.glass.filter(|_| anchor) else {
            continue;
        };
        let spec = GlassSpec {
            style,
            container: l.group,
            spacing: if l.group == 0 { 0.0 } else { l.spacing },
        };
        // a group's shapes as shown, front to back as backdrops are listed, each at its layer's opacity; the glass is cut
        // to the window, and a shape too far out of it to reach in is left out
        let mut areas: Vec<BackdropArea> = if l.group == 0 {
            vec![BackdropArea {
                key: l.serial | LAYER_GLASS_KEY,
                material: String::new(),
                area: bounds,
                radius,
                alpha: 1.0,
                glass: Some(spec),
                blur_ramp: [1.0, 1.0],
            }]
        } else {
            group_areas(&layers, l.group, place.opacity, place.origin)
                .into_iter()
                .rev()
                .map(|(area, radius, alpha)| BackdropArea {
                    key: l.serial | LAYER_GLASS_KEY,
                    material: String::new(),
                    area,
                    radius,
                    alpha,
                    glass: Some(spec),
                    blur_ramp: [1.0, 1.0],
                })
                .collect()
        };
        areas.retain(|a| meets_window(place.map_rect(a.area), spec.spacing, place.clip));
        if areas.is_empty() {
            continue;
        }
        out.extend(glass(p, areas, &LayerGlass::of(l, lens, mc, p.scale)));
    }
    out
}
