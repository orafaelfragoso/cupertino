//! Window control buttons: states, colours and the vector glyphs.
//!
//! The glyphs are authored here as strokes (a cross for close, a dash for minimize, a plus for
//! zoom; the shapes are as designed only as "×, −, + appear on all three buttons while the group is
//! hovered"), their proportions are approximate.
//!
//! States: normal (coloured), hover-group (glyphs on all three), pressed (darker),
//! inactive window (a translucent grey disc without a ring, colour returns while the group is hovered), disabled (the
//! same disc, no glyph).

use crate::metrics::{ButtonKind, DecorationMetrics};
use crate::palette::{Palette, Rgba};
use cupertino_geometry::Point;
use tiny_skia::{Color, FillRule, LineCap, Paint, PathBuilder, Pixmap, Stroke, Transform};

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

/// Darkening factor of a pressed button (approximate).
pub const PRESSED_DARKEN: f32 = 0.78;
/// Darkening factor of the ring around a button (approximate).
pub const RING_DARKEN: f32 = 0.84;

fn darken(c: Rgba, f: f32) -> Rgba {
    [c[0] * f, c[1] * f, c[2] * f, c[3]]
}

pub fn button_fill(palette: &Palette, kind: ButtonKind, v: ButtonVisual) -> Rgba {
    let base = if v.colored {
        palette.button_color(kind)
    } else {
        palette.button_inactive
    };
    if v.pressed {
        darken(base, PRESSED_DARKEN)
    } else {
        base
    }
}

pub fn button_ring(palette: &Palette, kind: ButtonKind, v: ButtonVisual) -> Rgba {
    darken(button_fill(palette, kind, v), RING_DARKEN)
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
    let ring_w = (metrics.button_diameter * metrics.button_ring * scale).max(0.5) as f32;
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
    paint.set_color(color(button_ring(palette, kind, v)));
    pm.fill_path(
        &disc,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
    let mut pb = PathBuilder::new();
    pb.push_circle(cx, cy, (r - ring_w).max(0.0));
    if let Some(inner) = pb.finish() {
        paint.set_color(color(button_fill(palette, kind, v)));
        pm.fill_path(
            &inner,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    if v.glyph {
        paint.set_color(color(palette.glyph));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visual_states() {
        let normal = ButtonState::default().visual();
        assert!(normal.colored && !normal.glyph && !normal.pressed);
        let hover = ButtonState {
            group_hovered: true,
            ..Default::default()
        }
        .visual();
        assert!(hover.colored && hover.glyph);
        let inactive = ButtonState {
            window_active: false,
            ..Default::default()
        }
        .visual();
        assert!(!inactive.colored && !inactive.glyph);
        let inactive_hover = ButtonState {
            window_active: false,
            group_hovered: true,
            ..Default::default()
        }
        .visual();
        assert!(
            inactive_hover.colored && inactive_hover.glyph,
            "colour and glyphs return on hover"
        );
        let disabled = ButtonState {
            enabled: false,
            group_hovered: true,
            pressed: true,
            ..Default::default()
        }
        .visual();
        assert!(!disabled.colored && !disabled.glyph && !disabled.pressed);
        let pressed = ButtonState {
            pressed: true,
            ..Default::default()
        }
        .visual();
        assert!(pressed.pressed && pressed.glyph);
    }

    #[test]
    fn pressed_is_darker_and_inactive_is_grey() {
        let p = Palette::builtin(false);
        let n = button_fill(&p, ButtonKind::Close, ButtonState::default().visual());
        let d = button_fill(
            &p,
            ButtonKind::Close,
            ButtonState {
                pressed: true,
                ..Default::default()
            }
            .visual(),
        );
        assert!(d[0] < n[0] && d[1] < n[1]);
        let g = button_fill(
            &p,
            ButtonKind::Close,
            ButtonState {
                window_active: false,
                ..Default::default()
            }
            .visual(),
        );
        assert!(
            (g[0] - g[1]).abs() < 1e-6 && (g[1] - g[2]).abs() < 1e-6,
            "inactive buttons are grey"
        );
        assert!(
            g[3] > 0.0 && g[3] < 0.5,
            "inactive buttons are translucent, so what is under them shows through"
        );
        assert_eq!(
            g,
            button_fill(
                &p,
                ButtonKind::Zoom,
                ButtonState {
                    window_active: false,
                    ..Default::default()
                }
                .visual()
            ),
            "all three are the same grey"
        );
    }

    #[test]
    fn glyphs_are_centred_and_symmetric() {
        for kind in ButtonKind::ALL {
            let strokes = glyph_strokes(kind, 2.4);
            assert!(!strokes.is_empty());
            let (sx, sy): (f64, f64) = strokes.iter().fold((0.0, 0.0), |a, s| {
                (a.0 + s.0 .0 + s.1 .0, a.1 + s.0 .1 + s.1 .1)
            });
            assert!(sx.abs() < 1e-12 && sy.abs() < 1e-12, "{kind:?}");
        }
        assert_eq!(glyph_strokes(ButtonKind::Close, 2.0).len(), 2);
        assert_eq!(glyph_strokes(ButtonKind::Minimize, 2.0).len(), 1);
        assert_eq!(glyph_strokes(ButtonKind::Zoom, 2.0).len(), 2);
    }
}
