//! How a control's state picks its colours, its label's type, and the keyboard focus ring drawn around its chrome.

use cupertino_geometry::{Point, Rect};
use cupertino_theme::metrics::{alignment_height, Control};
use cupertino_ui_core::{ControlState, Environment, PaintCx};
use cupertino_ui_render::{Paint, PaintOp, Shape};
use cupertino_ui_text::{AttributedText, FontWeight, ParagraphLayout, ParagraphStyle, TextStyle};
use cupertino_ui_theme::{
    Color, ColorRole, ControlSize, FocusRing, InteractionVariant, StateVariant, ThemeTokens,
};

/// Disabled overrides everything; pressed overrides hover. Keyboard focus is drawn as a ring, not a colour.
pub(crate) fn variant(state: &ControlState) -> StateVariant {
    let interaction = if !state.enabled {
        InteractionVariant::Disabled
    } else if state.pressed {
        InteractionVariant::Pressed
    } else if state.hovered {
        InteractionVariant::Hover
    } else {
        InteractionVariant::Normal
    };
    StateVariant {
        interaction,
        selected: state.selected,
        window_active: state.window_active,
    }
}

/// A theme without the role paints nothing rather than a guessed colour.
pub(crate) fn color(theme: &ThemeTokens, role: ColorRole, state: &ControlState) -> Option<Color> {
    theme.colors.get(role, variant(state))
}

/// The height of `control` at the theme's control size, else at the regular size.
pub(crate) fn metric(control: Control, theme: &ThemeTokens) -> f64 {
    alignment_height(control, theme.metrics.size)
        .or_else(|| alignment_height(control, ControlSize::Regular))
        .unwrap_or_default()
}

/// The ring's stroke: centred `outset + width / 2` outside the chrome, with the chrome's radius plus the theme's
/// extra, so it never overlaps the chrome or changes layout.
pub(crate) fn focus_ring(chrome: Rect, radius: f64, ring: &FocusRing) -> (Rect, f64) {
    (
        chrome.outset(ring.outset + ring.width / 2.0),
        radius + ring.radius_extra,
    )
}

pub(crate) fn fill(cx: &mut PaintCx<'_>, rect: Rect, radius: f64, color: Option<Color>) {
    if let Some(color) = color {
        cx.push(PaintOp::Fill {
            shape: Shape::RoundedRect { rect, radius },
            paint: Paint::Solid(color),
        });
    }
}

pub(crate) fn stroke(
    cx: &mut PaintCx<'_>,
    rect: Rect,
    radius: f64,
    width: f64,
    color: Option<Color>,
) {
    if let Some(color) = color {
        cx.push(PaintOp::Stroke {
            shape: Shape::RoundedRect { rect, radius },
            width,
            paint: Paint::Solid(color),
        });
    }
}

/// The label laid out on one line in the theme's body type. An empty label, or one the fonts cannot shape, has none.
pub(crate) fn label(env: &Environment, text: &str) -> Option<ParagraphLayout> {
    if text.is_empty() {
        return None;
    }
    paragraph(env, text)
}

/// `text` laid out on one line in the theme's body type, empty text included; `None` if the fonts cannot shape it.
pub(crate) fn paragraph(env: &Environment, text: &str) -> Option<ParagraphLayout> {
    let body = env.theme.type_scale.body;
    let style = TextStyle {
        weight: FontWeight(body.weight),
        ..TextStyle::new(body.size)
    };
    let paragraph = env
        .text
        .paragraph(
            &AttributedText::plain(text, style),
            &ParagraphStyle::default(),
        )
        .ok()?;
    Some(paragraph.layout(None))
}

/// The baseline's distance below the chrome's centre, as a share of the text size; the shell's fields use the same.
const BASELINE_SHARE: f64 = 0.36;

/// The top-left corner that centres a one-line `layout` horizontally in `chrome` and puts its baseline at the shared
/// optical centre.
pub(crate) fn centred(layout: &ParagraphLayout, chrome: Rect, env: &Environment) -> Point {
    Point::new(
        chrome.min_x() + (chrome.width() - layout.size().width) / 2.0,
        top(layout, chrome, env),
    )
}

/// The baseline of body text centred in `chrome`.
pub(crate) fn baseline(chrome: Rect, env: &Environment) -> f64 {
    let size = f64::from(env.theme.type_scale.body.size);
    chrome.min_y() + chrome.height() / 2.0 + size * BASELINE_SHARE
}

/// The y of `layout`'s top that puts its first baseline at [`baseline`].
pub(crate) fn top(layout: &ParagraphLayout, chrome: Rect, env: &Environment) -> f64 {
    let first = layout.lines().first().map_or(0.0, |line| line.baseline);
    baseline(chrome, env) - first
}

/// Paints `layout` with its top-left corner at `origin`.
pub(crate) fn glyphs(
    cx: &mut PaintCx<'_>,
    layout: &ParagraphLayout,
    origin: Point,
    color: Option<Color>,
) {
    let Some(color) = color else {
        return;
    };
    for line in layout.lines() {
        for run in &line.runs {
            cx.push(PaintOp::Glyphs {
                origin: origin + run.origin,
                run: run.run.clone(),
                color,
            });
        }
    }
}

/// Draws the keyboard focus ring around `chrome` when the focus is keyboard-visible and the window is active; an
/// inactive window keeps its focus but shows no ring until it is active again.
pub(crate) fn paint_focus_ring(cx: &mut PaintCx<'_>, chrome: Rect, radius: f64) {
    if !cx.focus_visible() || !cx.window_active() {
        return;
    }
    let theme = cx.env().theme.clone();
    let (rect, radius) = focus_ring(chrome, radius, &theme.focus);
    let color = theme
        .colors
        .get(ColorRole::KeyboardFocusIndicator, StateVariant::NORMAL);
    stroke(cx, rect, radius, theme.focus.width, color);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(enabled: bool, hovered: bool, pressed: bool) -> ControlState {
        ControlState {
            enabled,
            hovered,
            pressed,
            window_active: true,
            ..ControlState::default()
        }
    }

    #[test]
    fn disabled_overrides_pressed_and_pressed_overrides_hover() {
        let interaction = |s| variant(&s).interaction;
        assert_eq!(
            interaction(state(true, false, false)),
            InteractionVariant::Normal
        );
        assert_eq!(
            interaction(state(true, true, false)),
            InteractionVariant::Hover
        );
        assert_eq!(
            interaction(state(true, true, true)),
            InteractionVariant::Pressed
        );
        assert_eq!(
            interaction(state(false, true, true)),
            InteractionVariant::Disabled
        );
    }

    #[test]
    fn the_ring_sits_outside_the_chrome_with_its_stroke_centred_and_a_larger_radius() {
        let ring = FocusRing {
            width: 2.0,
            outset: 2.0,
            radius_extra: 3.0,
        };
        let (rect, radius) = focus_ring(Rect::new(10.0, 10.0, 80.0, 20.0), 5.0, &ring);
        assert_eq!(rect, Rect::new(7.0, 7.0, 86.0, 26.0));
        assert_eq!(radius, 8.0);
    }
}
