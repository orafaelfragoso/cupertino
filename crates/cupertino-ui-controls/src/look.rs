//! How a control's state picks its colours, and the keyboard focus ring drawn around its chrome.

use cupertino_geometry::Rect;
use cupertino_ui_core::{ControlState, PaintCx};
use cupertino_ui_render::{Paint, PaintOp, Shape};
use cupertino_ui_theme::{
    Color, ColorRole, FocusRing, InteractionVariant, StateVariant, ThemeTokens,
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

/// Draws the keyboard focus ring around `chrome` when the focus is keyboard-visible.
pub(crate) fn paint_focus_ring(cx: &mut PaintCx<'_>, chrome: Rect, radius: f64) {
    if !cx.focus_visible() {
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
