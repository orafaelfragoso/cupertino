//! The lens a switch's thumb or a slider's knob becomes while it is held or dragged: a larger, darker optical body
//! with a fine bright rim, through which the track shows, shrunk about the lens's centre. At rest the thumb is a plain
//! white capsule; ordinary buttons, check boxes and fields never take a lens.
//!
//! The model ([`Lens::lifted`]) is pure: the control's state says how far the lens shows and the thumb's
//! rectangle where it is. The lens is painted over the track by the control itself ([`GlassRole::ThumbLens`]): the
//! compositor's glass lies under the window's content and could not show the track drawn above it. It reaches past
//! the thumb, and past the control's node, but never changes the layout: only paint grows, never the node or its
//! target.
//!
//! The accessibility options change the lens, never take it away: Increase Contrast darkens its outline and
//! brightens its rim, Reduce Transparency makes its body nearly opaque, and Reduce Motion leaves it as it is (it
//! appears and goes at once, with no motion of its own).
//!
//! The lens swells out of the thumb as it is taken hold of and sinks back into it when let go
//! ([`Lens::lifted`]): the control times that lift and fades the lens in over the plain thumb.

use crate::glass::GlassRole;
use cupertino_geometry::{Insets, Point, Rect};
use cupertino_ui_core::PaintCx;
use cupertino_ui_render::{Effect, GradientStop, Paint, PaintOp, Shape};
use cupertino_ui_theme::{Color, ThemeTokens};

/// Which thumb the lens takes the place of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LensKind {
    /// A switch's thumb: the lens grows wide, past the track's ends.
    Switch,
    /// A slider's knob.
    Slider,
}

/// How much larger than the thumb the lens is, on each axis.
const SWITCH_GROWTH: f64 = 1.6;
const SLIDER_GROWTH: f64 = 1.33;
/// How much the lens shrinks what lies under it, about its centre.
const SWITCH_MINIFY: f64 = 0.75;
const SLIDER_MINIFY: f64 = 0.85;
/// The slider's lens is a rounded rectangle: its corner radius as a share of its height.
const SLIDER_CORNER: f64 = 0.42;
/// The shade under the lens: its offset down, its blur and its strength in the dark and the light appearance.
const SHADE_OFFSET: f64 = 2.0;
const SHADE_BLUR: f64 = 3.0;
const SHADE_DARK: f32 = 0.35;
const SHADE_LIGHT: f32 = 0.18;
/// The body's tint over what the lens shows: a share of black, and with Reduce Transparency a nearly opaque body of
/// the appearance's control colour.
const BODY_DARK: f32 = 0.10;
const BODY_LIGHT: f32 = 0.04;
const BODY_OPAQUE: f32 = 0.85;
/// The body's light at its top and bottom edges, fading to none across its middle: a share of white.
const EDGE_LIGHT_DARK: f32 = 0.16;
const EDGE_LIGHT_LIGHT: f32 = 0.45;
/// The bright rim just inside the edge, strongest at the top and the bottom; with Increase Contrast it is brighter.
const RIM: f32 = 0.42;
const RIM_HIGH_CONTRAST: f32 = 0.75;
const RIM_WIDTH: f64 = 1.0;
/// The dark outline at the edge; with Increase Contrast it is darker.
const OUTLINE_DARK: f32 = 0.70;
const OUTLINE_LIGHT: f32 = 0.22;
const OUTLINE_HIGH_CONTRAST: f32 = 0.95;
const OUTLINE_WIDTH: f64 = 0.75;

/// A lens: where it is, its corners and how it shrinks what it shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Lens {
    pub rect: Rect,
    pub radius: f64,
    /// What lies under the lens is drawn at this scale about its centre.
    pub minify: f64,
}

impl Lens {
    /// The lens over `thumb` while `held`; none at rest. A disabled control is never held.
    #[cfg(test)]
    pub fn of(kind: LensKind, thumb: Rect, held: bool) -> Option<Lens> {
        Lens::lifted(kind, thumb, if held { 1.0 } else { 0.0 })
    }

    /// The lens over `thumb` `lift` of the way (0 to 1) from the plain thumb to the held lens: as large as the thumb
    /// and showing what is under it as it is at 0, where there is none, and the whole lens at 1.
    pub fn lifted(kind: LensKind, thumb: Rect, lift: f64) -> Option<Lens> {
        if lift <= 0.0 {
            return None;
        }
        let lift = lift.min(1.0);
        let (growth, minify) = match kind {
            LensKind::Switch => (SWITCH_GROWTH, SWITCH_MINIFY),
            LensKind::Slider => (SLIDER_GROWTH, SLIDER_MINIFY),
        };
        let (growth, minify) = (1.0 + (growth - 1.0) * lift, 1.0 + (minify - 1.0) * lift);
        let (w, h) = (thumb.width() * growth, thumb.height() * growth);
        let centre = thumb.center();
        let rect = Rect::new(centre.x - w / 2.0, centre.y - h / 2.0, w, h);
        let radius = match kind {
            LensKind::Switch => h / 2.0,
            LensKind::Slider => h * SLIDER_CORNER,
        };
        Some(Lens {
            rect,
            radius,
            minify,
        })
    }

    pub fn centre(&self) -> Point {
        self.rect.center()
    }

    /// `rect` as the lens shows it: shrunk by [`Lens::minify`] about the lens's centre.
    pub fn through(&self, rect: Rect) -> Rect {
        let c = self.centre();
        let k = self.minify;
        Rect::new(
            c.x + (rect.min_x() - c.x) * k,
            c.y + (rect.min_y() - c.y) * k,
            rect.width() * k,
            rect.height() * k,
        )
    }

    fn shape(&self) -> Shape {
        Shape::RoundedRect {
            rect: self.rect,
            radius: self.radius,
        }
    }
}

fn white(a: f32) -> Color {
    Color::new(1.0, 1.0, 1.0, a)
}

fn black(a: f32) -> Color {
    Color::new(0.0, 0.0, 0.0, a)
}

fn stop(offset: f32, color: Color) -> GradientStop {
    GradientStop { offset, color }
}

/// The lens's colours in `theme`'s appearance and accessibility options.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LensLook {
    pub shade: Color,
    pub body: Color,
    pub edge_light: Color,
    pub rim: Color,
    pub outline: Color,
}

impl LensLook {
    pub fn of(theme: &ThemeTokens) -> LensLook {
        let dark = theme.preferences.appearance.is_dark();
        let contrast = theme.preferences.appearance.is_high_contrast();
        let body = if theme.preferences.reduce_transparency {
            let tone = if dark { 0.20 } else { 0.96 };
            Color::new(tone, tone, tone + 0.01, BODY_OPAQUE)
        } else if dark {
            black(BODY_DARK)
        } else {
            black(BODY_LIGHT)
        };
        LensLook {
            shade: black(if dark { SHADE_DARK } else { SHADE_LIGHT }),
            body,
            edge_light: white(if dark {
                EDGE_LIGHT_DARK
            } else {
                EDGE_LIGHT_LIGHT
            }),
            rim: white(if contrast { RIM_HIGH_CONTRAST } else { RIM }),
            outline: black(if contrast {
                OUTLINE_HIGH_CONTRAST
            } else if dark {
                OUTLINE_DARK
            } else {
                OUTLINE_LIGHT
            }),
        }
    }
}

/// [`paint_lens`] at `opacity`, as a lens fading in over its thumb or out of it.
pub(crate) fn paint_lens_faded(
    cx: &mut PaintCx<'_>,
    lens: &Lens,
    opacity: f32,
    under: impl FnOnce(&mut PaintCx<'_>, &Lens),
) {
    if opacity >= 1.0 {
        return paint_lens(cx, lens, under);
    }
    cx.push(PaintOp::PushLayer {
        opacity,
        effects: Vec::new(),
    });
    paint_lens(cx, lens, under);
    cx.push(PaintOp::PopLayer);
}

/// Paints `lens`: its shade, then what `under` draws (in the lens's shrunk coordinates, see [`Lens::through`]),
/// clipped to the lens, then its body, its edge light, its rim and its outline.
pub(crate) fn paint_lens(
    cx: &mut PaintCx<'_>,
    lens: &Lens,
    under: impl FnOnce(&mut PaintCx<'_>, &Lens),
) {
    debug_assert_eq!(
        GlassRole::ThumbLens.material(),
        crate::GlassMaterial::Painted
    );
    let look = LensLook::of(&cx.env().theme);
    let shape = lens.shape();
    // the shade: a blurred copy of the lens, a little below it
    cx.push(PaintOp::PushLayer {
        opacity: 1.0,
        effects: vec![Effect::Blur { radius: SHADE_BLUR }],
    });
    cx.push(PaintOp::Fill {
        shape: Shape::RoundedRect {
            rect: lens.rect.translate(0.0, SHADE_OFFSET),
            radius: lens.radius,
        },
        paint: Paint::Solid(look.shade),
    });
    cx.push(PaintOp::PopLayer);
    cx.push(PaintOp::PushClip(shape.clone()));
    under(cx, lens);
    cx.push(PaintOp::Fill {
        shape: shape.clone(),
        paint: Paint::Solid(look.body),
    });
    let (top, bottom) = (lens.rect.min_y(), lens.rect.max_y());
    let clear = Color {
        a: 0.0,
        ..look.edge_light
    };
    cx.push(PaintOp::Fill {
        shape: shape.clone(),
        paint: Paint::Linear {
            start: Point::new(lens.rect.min_x(), top),
            end: Point::new(lens.rect.min_x(), bottom),
            stops: vec![
                stop(0.0, look.edge_light),
                stop(0.38, clear),
                stop(0.62, clear),
                stop(1.0, look.edge_light),
            ],
        },
    });
    cx.push(PaintOp::PopClip);
    let rim_rect = lens
        .rect
        .inset(Insets::uniform(RIM_WIDTH / 2.0 + OUTLINE_WIDTH));
    let faint = Color {
        a: look.rim.a * 0.25,
        ..look.rim
    };
    cx.push(PaintOp::Stroke {
        shape: Shape::RoundedRect {
            rect: rim_rect,
            radius: (lens.radius - RIM_WIDTH / 2.0 - OUTLINE_WIDTH).max(0.0),
        },
        width: RIM_WIDTH,
        paint: Paint::Linear {
            start: Point::new(lens.rect.min_x(), top),
            end: Point::new(lens.rect.min_x(), bottom),
            stops: vec![stop(0.0, look.rim), stop(0.5, faint), stop(1.0, look.rim)],
        },
    });
    cx.push(PaintOp::Stroke {
        shape: Shape::RoundedRect {
            rect: lens.rect.inset(Insets::uniform(OUTLINE_WIDTH / 2.0)),
            radius: (lens.radius - OUTLINE_WIDTH / 2.0).max(0.0),
        },
        width: OUTLINE_WIDTH,
        paint: Paint::Solid(look.outline),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use cupertino_ui_theme::{Appearance, ControlMetrics, ThemePreferences};

    fn theme(appearance: Appearance, reduce_transparency: bool) -> ThemeTokens {
        let preferences = ThemePreferences {
            appearance,
            reduce_transparency,
            reduce_motion: false,
        };
        let mut theme = ThemeTokens::neutral(preferences).unwrap();
        theme.metrics = ControlMetrics::regular();
        theme
    }

    #[test]
    fn a_thumb_at_rest_has_no_lens() {
        let thumb = Rect::new(10.0, 2.0, 32.0, 20.0);
        assert_eq!(Lens::of(LensKind::Switch, thumb, false), None);
        assert_eq!(Lens::of(LensKind::Slider, thumb, false), None);
    }

    #[test]
    fn a_held_switch_thumb_becomes_a_wider_capsule_about_its_centre() {
        let thumb = Rect::new(10.0, 2.0, 32.0, 20.0);
        let lens = Lens::of(LensKind::Switch, thumb, true).unwrap();
        assert_eq!(lens.centre(), thumb.center());
        assert!((lens.rect.width() - 51.2).abs() < 1e-9);
        assert!((lens.rect.height() - 32.0).abs() < 1e-9);
        assert_eq!(lens.radius, lens.rect.height() / 2.0, "a capsule");
        assert!(lens.rect.width() > thumb.width() && lens.rect.height() > thumb.height());
    }

    #[test]
    fn a_held_slider_knob_becomes_a_rounded_lens_a_third_larger() {
        let knob = Rect::new(40.0, 0.0, 20.0, 16.0);
        let lens = Lens::of(LensKind::Slider, knob, true).unwrap();
        assert!((lens.rect.width() - 26.6).abs() < 1e-9);
        assert!(
            lens.radius < lens.rect.height() / 2.0,
            "a rounded rectangle, not a capsule"
        );
    }

    #[test]
    fn a_lifting_lens_swells_from_the_thumb_to_its_held_size() {
        let thumb = Rect::new(10.0, 2.0, 32.0, 20.0);
        assert_eq!(Lens::lifted(LensKind::Switch, thumb, 0.0), None);
        let start = Lens::lifted(LensKind::Switch, thumb, 1e-9).unwrap();
        assert!(
            (start.rect.width() - thumb.width()).abs() < 1e-6,
            "as large as the thumb"
        );
        assert!(
            (start.minify - 1.0).abs() < 1e-6,
            "showing what is under it as it is"
        );
        let half = Lens::lifted(LensKind::Switch, thumb, 0.5).unwrap();
        let held = Lens::of(LensKind::Switch, thumb, true).unwrap();
        assert!(half.rect.width() > thumb.width() && half.rect.width() < held.rect.width());
        assert_eq!(half.centre(), thumb.center());
        assert_eq!(Lens::lifted(LensKind::Switch, thumb, 1.0), Some(held));
        assert_eq!(
            Lens::lifted(LensKind::Switch, thumb, 3.0),
            Some(held),
            "never past it"
        );
    }

    #[test]
    fn the_lens_shrinks_what_it_shows_about_its_centre() {
        let thumb = Rect::new(0.0, 0.0, 20.0, 20.0);
        let lens = Lens::of(LensKind::Switch, thumb, true).unwrap();
        let track = Rect::new(-20.0, 5.0, 60.0, 10.0);
        let seen = lens.through(track);
        assert_eq!(seen.center(), Point::new(10.0 + (10.0 - 10.0) * 0.75, 10.0));
        assert!((seen.height() - 7.5).abs() < 1e-9);
        assert!((seen.width() - 45.0).abs() < 1e-9);
        assert_eq!(
            lens.through(Rect::new(10.0, 10.0, 0.0, 0.0)).origin,
            lens.centre()
        );
    }

    #[test]
    fn contrast_and_transparency_change_the_lens_without_taking_it_away() {
        let plain = LensLook::of(&theme(Appearance::Dark, false));
        let contrast = LensLook::of(&theme(Appearance::DarkHighContrast, false));
        let opaque = LensLook::of(&theme(Appearance::Dark, true));
        assert!(contrast.outline.a > plain.outline.a);
        assert!(contrast.rim.a > plain.rim.a);
        assert!(opaque.body.a > plain.body.a);
        assert_eq!(opaque.outline, plain.outline);
        let light = LensLook::of(&theme(Appearance::Light, false));
        assert!(light.shade.a < plain.shade.a);
        assert!(light.edge_light.a > plain.edge_light.a);
    }
}
