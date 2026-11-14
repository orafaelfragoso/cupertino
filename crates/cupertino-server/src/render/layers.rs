//! The pieces of a parent surface's layers (`cupertino_layers_v1`, [`crate::layers_v1`]): each layer's content where
//! its animated values put it, the placeholder where its content has not arrived, and its glass below it, moving,
//! scaling and fading with it. Raised layers come in front of the flat ones and lift their glass with a soft shadow; a
//! vibrant layer draws its coverage blended with the glass behind it ([`SurfaceRole::Vibrant`]).

use cupertino_scene::{Clip, SceneKind, SurfaceRole, VibrantBlend};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Rectangle};

use super::compose::{corner_style, output_rect, Piece, Tree};
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
        (c.x - 0.5 * l.width * l.scale, c.y - 0.5 * l.height * l.scale).into(),
        (l.width * l.scale, l.height * l.scale).into(),
    )
}

fn centre(l: &LayerDraw, origin: Point<f64, Logical>) -> Point<f64, Logical> {
    Point::from((
        origin.x + l.x + 0.5 * l.width,
        origin.y + l.y + 0.5 * l.height,
    ))
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

/// The pieces of `parent`'s layers, front to back, as shown at `p.now`. `glass` makes the pieces of a layer's glass
/// from its backdrop (output-local points before the window's rescale), as the parent's own backdrops are made.
pub fn layer_pieces(
    p: &mut SceneParams<'_>,
    parent: &WlSurface,
    place: &LayerPlace,
    mut glass: impl FnMut(&mut SceneParams<'_>, Vec<BackdropArea>) -> Vec<Piece>,
) -> Vec<Piece> {
    let layers = present(parent, p.now, p.display.reduce_motion);
    if layers.is_empty() {
        return Vec::new();
    }
    let dark = p.config.is_dark();
    let plain = p.display.reduce_transparency || p.display.increase_contrast;
    let pointer = Point::<f64, Logical>::from((p.pointer.x - p.rect.x, p.pointer.y - p.rect.y));
    let pointer_here = pointer.x >= 0.0
        && pointer.y >= 0.0
        && pointer.x <= p.rect.w
        && pointer.y <= p.rect.h;
    let s_min = place.scale.0.min(place.scale.1);
    let mut out = Vec::new();
    for l in layers.iter().rev() {
        let opacity = place.opacity * l.opacity as f32;
        if opacity <= 0.0 || l.scale <= 0.0 {
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
        let clip = if l.masks {
            Clip::Rounded {
                rect: output_rect(shown),
                radius: radius * s_min,
                style: corner_style(true),
                exact: false,
            }
        } else {
            place.clip
        };
        let role = vibrant_blend(l.blend, dark, plain)
            .map_or(SurfaceRole::WindowContent, SurfaceRole::Vibrant);
        out.push(Piece::Tree(Box::new(Tree {
            surface: l.surface.clone(),
            kind: place.kind,
            role,
            origin: origin.to_physical_precise_round(p.scale),
            pivot: mc.to_physical_precise_round(p.scale),
            scale: (l.scale * place.scale.0, l.scale * place.scale.1),
            opacity,
            clip,
            only: None,
        })));
        // late content: the placeholder where the content does not reach
        if l.placeholder[3] > 0.0 {
            for [x, y, w, h] in uncovered(l.content, l.width, l.height) {
                let at = Point::<f64, Logical>::from((
                    c.x + (place.origin.x + l.x + x - c.x) * l.scale,
                    c.y + (place.origin.y + l.y + y - c.y) * l.scale,
                ));
                let r = place.map_rect(Rectangle::new(at, (w * l.scale, h * l.scale).into()));
                out.push(rrect_item(
                    place.kind,
                    l.serial,
                    r,
                    0.0,
                    true,
                    l.placeholder,
                    None,
                    opacity,
                ));
            }
        }
        let Some((style, lens)) = l.glass else {
            continue;
        };
        if bounds.size.w <= 0.0 || bounds.size.h <= 0.0 {
            continue;
        }
        let area = BackdropArea {
            key: l.serial | LAYER_GLASS_KEY,
            material: String::new(),
            area: bounds,
            radius,
            alpha: 1.0,
            glass: Some(GlassSpec {
                style,
                container: 0,
                spacing: 0.0,
            }),
        };
        let light = (l.pointer_lit && pointer_here)
            .then(|| pointer_light(mc, pointer))
            .flatten();
        for mut piece in glass(p, vec![area]) {
            if let Piece::Glass(g) = &mut piece {
                let mut gc = (*g.glass).clone();
                gc.container.lens = lens as f32;
                gc.container.blur_ramp = l.blur_ramp;
                if let Some(angle) = light {
                    gc.container.params.highlight.angle = angle;
                }
                if l.raised {
                    let s = &mut gc.container.params.shadow;
                    (s.offset, s.amount, s.height) = RAISED_SHADOW;
                    s.fill = [0.0, 0.0, 0.0, 1.0];
                    s.opacity = 1.0;
                }
                gc.key = Sig::new()
                    .u(gc.key)
                    .f(lens)
                    .f(f64::from(l.blur_ramp[0]))
                    .f(f64::from(l.blur_ramp[1]))
                    .f(light.map_or(-10.0, f64::from))
                    .u(u64::from(l.raised))
                    .finish();
                g.glass = std::rc::Rc::new(gc);
                g.opacity *= l.opacity as f32;
            }
            out.push(piece);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_placeholder_fills_what_the_content_does_not_reach() {
        assert!(uncovered((100.0, 50.0), 100.0, 50.0).is_empty());
        assert_eq!(
            uncovered((100.0, 50.0), 100.0, 80.0),
            vec![[0.0, 50.0, 100.0, 30.0]]
        );
        assert_eq!(
            uncovered((100.0, 50.0), 120.0, 80.0),
            vec![[100.0, 0.0, 20.0, 80.0], [0.0, 50.0, 100.0, 30.0]]
        );
        // no content yet: the whole bounds
        assert_eq!(
            uncovered((0.0, 0.0), 40.0, 30.0),
            vec![[0.0, 0.0, 40.0, 30.0]]
        );
    }

    #[test]
    fn the_rim_is_lit_from_the_pointer() {
        let c = Point::from((100.0, 100.0));
        let up = pointer_light(c, Point::from((100.0, 20.0))).unwrap();
        assert!((up - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        let right = pointer_light(c, Point::from((180.0, 100.0))).unwrap();
        assert!(right.abs() < 1e-6);
        let below_left = pointer_light(c, Point::from((50.0, 150.0))).unwrap();
        assert!((below_left + 3.0 * std::f32::consts::FRAC_PI_4).abs() < 1e-6);
        assert_eq!(pointer_light(c, c), None);
    }

    #[test]
    fn vibrant_roles_take_the_label_weights() {
        assert_eq!(vibrant_blend(BlendRole::Normal, true, false), None);
        let v = vibrant_blend(BlendRole::Vibrant(VibrantWeight::Secondary), false, false).unwrap();
        assert_eq!((v.dark, v.plain, v.weight), (false, false, 498));
        let v = vibrant_blend(BlendRole::Vibrant(VibrantWeight::Primary), true, true).unwrap();
        assert_eq!((v.dark, v.plain, v.weight), (true, true, 847));
    }
}
