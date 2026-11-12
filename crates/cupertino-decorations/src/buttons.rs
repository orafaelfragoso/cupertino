//! Window control buttons: states, colours and the vector glyphs.
//!
//! The glyphs are authored here as strokes (a cross for close, a dash for minimize, a plus for
//! zoom, two triangles on hover; the shapes are as designed only as "×, −, + appear on all three buttons while the group is
//! hovered"), their proportions are approximate.
//!
//! States: normal (coloured), hover-group (glyphs on all three), pressed (its own fill, which is lighter than the
//! normal one in the dark appearance),
//! inactive window (a translucent grey disc without a ring, colour returns while the group is hovered), disabled (the
//! same disc, no glyph).

use crate::metrics::{ButtonKind, DecorationMetrics};
use crate::palette::{Palette, Rgba};
use cupertino_geometry::Point;
use tiny_skia::{
    Color, FillRule, GradientStop, LineCap, LinearGradient, Paint, PathBuilder, Pixmap,
    Point as SkPoint, SpreadMode, Stroke, Transform,
};

/// What the pointer and the window do to the buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ButtonState {
    pub window_active: bool,
    /// Pointer over the button group: glyphs on all enabled buttons.
    pub group_hovered: bool,
    pub pressed: bool,
    pub enabled: bool,
}

impl Default for ButtonState {
    fn default() -> Self {
        Self {
            window_active: true,
            group_hovered: false,
            pressed: false,
            enabled: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ButtonVisual {
    pub colored: bool,
    pub glyph: bool,
    pub pressed: bool,
}

impl ButtonState {
    pub fn visual(self) -> ButtonVisual {
        let colored = self.enabled && (self.window_active || self.group_hovered || self.pressed);
        ButtonVisual {
            colored,
            glyph: self.enabled && (self.group_hovered || self.pressed),
            pressed: self.pressed && self.enabled,
        }
    }
}

/// How far towards white the lower edge of a coloured button's face is lightened: the face is a little deeper at the top.
pub const FACE_LIGHTEN: f32 = 0.2;

/// How wide each of a button's two edge rings is, in points: the outer one in the deepest edge colour, the inner one
/// in the ring colour.
const EDGE_RING_WIDTH: f64 = 0.5;

/// How much of the ring shows at each distance from the button's centre line, as a share of the radius and an
/// opacity: the rings are at their deepest at the two sides and fade out towards the top and the bottom.
const EDGE_FADE: [(f32, f32); 4] = [(1.0, 1.0), (0.9, 0.7), (0.7, 0.2), (0.56, 0.0)];

/// Half the side of the zoom glyph's square, as a multiple of the glyph's half extent, and the width of the gap along
/// its diagonal, in points.
const ZOOM_GLYPH_HALF_SIDE: f64 = 1.07;
const ZOOM_GLYPH_GAP: f32 = 1.0;

/// The share of the edge's opacity the inner ring keeps.
const INNER_RING_SHARE: f32 = 0.85;

pub fn button_fill(palette: &Palette, kind: ButtonKind, v: ButtonVisual) -> Rgba {
    match (v.colored, v.pressed) {
        (true, true) => palette.pressed_fill(kind),
        (true, false) => palette.button_color(kind),
        (false, _) => palette.button_inactive,
    }
}

/// The colour of a coloured button's glyph: brighter while the button is pressed.
pub fn button_glyph(palette: &Palette, kind: ButtonKind, v: ButtonVisual) -> Rgba {
    let c = palette.glyph_of(kind);
    if v.pressed {
        [(c[0] * 1.35).min(1.0), c[1] * 1.1, c[2], c[3]]
    } else {
        c
    }
}

/// One straight glyph stroke between two points relative to the button centre, in points.
pub type Stroke2 = ((f64, f64), (f64, f64));

/// Glyph strokes of `kind` for a glyph half-extent `h` (points).
pub fn glyph_strokes(kind: ButtonKind, h: f64) -> Vec<Stroke2> {
    match kind {
        ButtonKind::Close => vec![((-h, -h), (h, h)), ((-h, h), (h, -h))],
        ButtonKind::Minimize => vec![((-h * 1.25, 0.0), (h * 1.25, 0.0))],
        ButtonKind::Zoom => vec![
            ((-h * 1.25, 0.0), (h * 1.25, 0.0)),
            ((0.0, -h * 1.25), (0.0, h * 1.25)),
        ],
    }
}

fn color(c: Rgba) -> Color {
    Color::from_rgba(
        c[0].clamp(0.0, 1.0),
        c[1].clamp(0.0, 1.0),
        c[2].clamp(0.0, 1.0),
        c[3].clamp(0.0, 1.0),
    )
    .expect("clamped components")
}

/// Draw one button centred at `center` (logical points) at `scale` device pixels per point.
pub fn draw_button(
    pm: &mut Pixmap,
    kind: ButtonKind,
    center: Point,
    scale: f64,
    metrics: &DecorationMetrics,
    palette: &Palette,
    state: ButtonState,
) {
    let v = state.visual();
    let (cx, cy) = ((center.x * scale) as f32, (center.y * scale) as f32);
    let r = (metrics.button_diameter * 0.5 * scale) as f32;
    let mut pb = PathBuilder::new();
    pb.push_circle(cx, cy, r);
    let Some(disc) = pb.finish() else { return };
    let mut paint = Paint {
        anti_alias: true,
        ..Paint::default()
    };
    if !v.colored {
        // one translucent disc over what the window draws beneath: a ring under it would show through doubled
        paint.set_color(color(button_fill(palette, kind, v)));
        pm.fill_path(
            &disc,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
        return;
    }
    // the face, a little deeper at the top
    {
        let top = button_fill(palette, kind, v);
        let bottom = [
            top[0] + (1.0 - top[0]) * FACE_LIGHTEN,
            top[1] + (1.0 - top[1]) * FACE_LIGHTEN,
            top[2] + (1.0 - top[2]) * FACE_LIGHTEN,
            top[3],
        ];
        match LinearGradient::new(
            SkPoint::from_xy(cx, cy - r),
            SkPoint::from_xy(cx, cy + r),
            vec![
                GradientStop::new(0.0, color(top)),
                GradientStop::new(1.0, color(bottom)),
            ],
            SpreadMode::Pad,
            Transform::identity(),
        ) {
            Some(shader) => paint.shader = shader,
            None => paint.set_color(color(top)),
        }
        pm.fill_path(
            &disc,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    // two rings at the edge, deepest at the two sides and gone at the top and the bottom
    let width = (EDGE_RING_WIDTH * scale).max(0.5) as f32;
    let rings = [
        (r - width / 2.0, palette.edge_of(kind), 1.0),
        (r - width * 1.5, palette.rim_of(kind), INNER_RING_SHARE),
    ];
    for (radius, base, share) in rings {
        let mut stops = Vec::new();
        for &(at, opacity) in &EDGE_FADE {
            stops.push((0.5 - at / 2.0, opacity));
        }
        for &(at, opacity) in EDGE_FADE.iter().rev() {
            stops.push((0.5 + at / 2.0, opacity));
        }
        let stops: Vec<GradientStop> = stops
            .into_iter()
            .map(|(position, opacity)| {
                let mut c = base;
                c[3] *= opacity * share;
                GradientStop::new(position.clamp(0.0, 1.0), color(c))
            })
            .collect();
        let mut pb = PathBuilder::new();
        pb.push_circle(cx, cy, radius.max(0.0));
        let (Some(ring), Some(shader)) = (
            pb.finish(),
            LinearGradient::new(
                SkPoint::from_xy(cx - r, cy),
                SkPoint::from_xy(cx + r, cy),
                stops,
                SpreadMode::Pad,
                Transform::identity(),
            ),
        ) else {
            continue;
        };
        let ring_paint = Paint {
            shader,
            anti_alias: true,
            ..Paint::default()
        };
        let stroke = Stroke {
            width,
            ..Stroke::default()
        };
        pm.stroke_path(&ring, &ring_paint, &stroke, Transform::identity(), None);
    }
    if v.glyph {
        paint.shader = tiny_skia::Shader::SolidColor(color(button_glyph(palette, kind, v)));
        if kind == ButtonKind::Zoom {
            // a square cut along its diagonal into two triangles, a thin gap between them
            let s = (metrics.button_diameter * metrics.glyph_half_extent * ZOOM_GLYPH_HALF_SIDE)
                as f32
                * scale as f32;
            let d = ZOOM_GLYPH_GAP * scale as f32 / std::f32::consts::SQRT_2;
            let triangles = [
                [(-s, -s), (s - d, -s), (-s, s - d)],
                [(s, s), (-s + d, s), (s, -s + d)],
            ];
            for t in triangles {
                let mut pb = PathBuilder::new();
                pb.move_to(cx + t[0].0, cy + t[0].1);
                pb.line_to(cx + t[1].0, cy + t[1].1);
                pb.line_to(cx + t[2].0, cy + t[2].1);
                pb.close();
                if let Some(p) = pb.finish() {
                    pm.fill_path(&p, &paint, FillRule::Winding, Transform::identity(), None);
                }
            }
            return;
        }
        let stroke = Stroke {
            width: (metrics.glyph_stroke * scale) as f32,
            line_cap: LineCap::Round,
            ..Stroke::default()
        };
        let h = metrics.button_diameter * metrics.glyph_half_extent;
        for ((x0, y0), (x1, y1)) in glyph_strokes(kind, h) {
            let mut pb = PathBuilder::new();
            pb.move_to(cx + (x0 * scale) as f32, cy + (y0 * scale) as f32);
            pb.line_to(cx + (x1 * scale) as f32, cy + (y1 * scale) as f32);
            if let Some(p) = pb.finish() {
                pm.stroke_path(&p, &paint, &stroke, Transform::identity(), None);
            }
        }
    }
}
