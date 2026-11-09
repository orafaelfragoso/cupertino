//! How a control's state picks its colours, its label's type, and the keyboard focus ring drawn around its chrome.

use cupertino_geometry::{Point, Rect};
use cupertino_ui_core::{ControlState, Environment, PaintCx};
use cupertino_ui_layout::LayoutDirection;
use cupertino_ui_render::{Paint, PaintOp, PathVerb, Shape};
use cupertino_ui_text::{
    AttributedText, Direction, FontWeight, Paragraph, ParagraphLayout, ParagraphStyle, TextStyle,
};
use cupertino_ui_theme::{
    Color, ColorRole, ControlFamily, ControlGeometry, FocusRing, InteractionVariant, StateVariant,
    ThemeTokens,
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

/// How strongly a quiet fill shows: a share of white in the dark appearance and of black in the light one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Shares {
    pub normal: f32,
    pub hover: f32,
    pub pressed: f32,
    pub disabled: f32,
}

/// The bezel of an ordinary button, a menu button, a segmented control and a stepper: a quiet fill with no edge.
pub(crate) const BEZEL: Shares = Shares {
    normal: 0.075,
    hover: 0.10,
    pressed: 0.16,
    disabled: 0.035,
};
/// The well of a check box, a radio button and an empty track: a little stronger than a bezel.
pub(crate) const WELL: Shares = Shares {
    normal: 0.095,
    hover: 0.12,
    pressed: 0.18,
    disabled: 0.05,
};
/// With Increase Contrast every quiet fill is this much stronger, and the control strokes its edge too.
const CONTRAST_BOOST: f32 = 0.06;

/// The quiet fill of `shares` in `state`. Disabled is fainter than an inactive window's, which keeps the normal fill:
/// a window that is not active subdues its accents, never its controls' fills.
pub(crate) fn quiet(theme: &ThemeTokens, shares: Shares, state: &ControlState) -> Color {
    let share = match variant(state).interaction {
        InteractionVariant::Disabled => shares.disabled,
        InteractionVariant::Pressed => shares.pressed,
        InteractionVariant::Hover => shares.hover,
        InteractionVariant::Normal => shares.normal,
    } + if theme.preferences.appearance.is_high_contrast() {
        CONTRAST_BOOST
    } else {
        0.0
    };
    let tone = if theme.preferences.appearance.is_dark() {
        1.0
    } else {
        0.0
    };
    Color::new(tone, tone, tone, share)
}

/// `color` at `share` of its alpha.
pub(crate) fn faded(color: Option<Color>, share: f32) -> Option<Color> {
    color.map(|c| Color {
        a: c.a * share,
        ..c
    })
}

/// The geometry of `family` at the theme's control size.
pub(crate) fn geometry(theme: &ThemeTokens, family: ControlFamily) -> ControlGeometry {
    theme.metrics.geometry(family)
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
    paragraph(env, text, None)
}

/// The base direction text takes in `env`.
pub(crate) fn direction(env: &Environment) -> Direction {
    match env.direction {
        LayoutDirection::Ltr => Direction::Ltr,
        LayoutDirection::Rtl => Direction::Rtl,
    }
}

/// `text` laid out on one line in the theme's body type, empty text included, in `direction` or the one its first
/// strong character gives; `None` if the fonts cannot shape it.
pub(crate) fn paragraph(
    env: &Environment,
    text: &str,
    direction: Option<Direction>,
) -> Option<ParagraphLayout> {
    paragraph_sized(env, text, direction, env.theme.type_scale.body.size)
}

/// [`paragraph`] with the body type's weight at `size` points.
pub(crate) fn paragraph_sized(
    env: &Environment,
    text: &str,
    direction: Option<Direction>,
    size: f32,
) -> Option<ParagraphLayout> {
    let body = env.theme.type_scale.body;
    let style = TextStyle {
        weight: FontWeight(body.weight),
        ..TextStyle::new(size)
    };
    let paragraph = env
        .text
        .paragraph(
            &AttributedText::plain(text, style),
            &ParagraphStyle {
                direction,
                ..ParagraphStyle::default()
            },
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
    baseline_sized(chrome, env.theme.type_scale.body.size)
}

/// The baseline of text `size` points large centred in `chrome`.
pub(crate) fn baseline_sized(chrome: Rect, size: f32) -> f64 {
    chrome.min_y() + chrome.height() / 2.0 + f64::from(size) * BASELINE_SHARE
}

/// The y of `layout`'s top that puts its first baseline at [`baseline`].
pub(crate) fn top(layout: &ParagraphLayout, chrome: Rect, env: &Environment) -> f64 {
    let first = layout.lines().first().map_or(0.0, |line| line.baseline);
    baseline(chrome, env) - first
}

/// Paints `layout` with its top-left corner at `origin`.
/// `paragraph` on one line: as it is when it fits `width`, else cut to end in its style's ellipsis within `width` and
/// laid out again at the cut line's width, so its box is the line's in either direction.
pub(crate) fn one_line(paragraph: &Paragraph, width: f64) -> ParagraphLayout {
    let line = paragraph.layout(None);
    if !width.is_finite() || line.size().width <= width {
        return line;
    }
    let cut = paragraph.layout(Some(width as f32));
    paragraph.layout(Some(cut.size().width as f32))
}

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
    if cx.focus_visible() && cx.window_active() {
        paint_ring(cx, chrome, radius);
    }
}

/// Draws the keyboard focus ring around `chrome`.
pub(crate) fn paint_ring(cx: &mut PaintCx<'_>, chrome: Rect, radius: f64) {
    let theme = cx.env().theme.clone();
    let (rect, radius) = focus_ring(chrome, radius, &theme.focus);
    let color = theme
        .colors
        .get(ColorRole::KeyboardFocusIndicator, StateVariant::NORMAL);
    stroke(cx, rect, radius, theme.focus.width, color);
}

/// A chevron `width` wide and half as high, centred on `centre`, its point up or down; stroke it.
pub(crate) fn chevron(centre: Point, width: f64, up: bool) -> Shape {
    let rise = if up { -width / 4.0 } else { width / 4.0 };
    Shape::Path {
        verbs: vec![
            PathVerb::MoveTo(Point::new(centre.x - width / 2.0, centre.y - rise)),
            PathVerb::LineTo(Point::new(centre.x, centre.y + rise)),
            PathVerb::LineTo(Point::new(centre.x + width / 2.0, centre.y - rise)),
        ],
        fill_rule: Default::default(),
    }
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
    fn a_disabled_fill_is_fainter_than_an_inactive_windows() {
        let theme = ThemeTokens::neutral(cupertino_ui_theme::ThemePreferences {
            appearance: cupertino_ui_theme::Appearance::Dark,
            reduce_transparency: false,
            reduce_motion: false,
        })
        .unwrap();
        let inactive = ControlState {
            window_active: false,
            ..state(true, false, false)
        };
        let disabled = state(false, false, false);
        let active = state(true, false, false);
        assert_eq!(
            quiet(&theme, BEZEL, &inactive),
            quiet(&theme, BEZEL, &active)
        );
        assert!(quiet(&theme, BEZEL, &disabled).a < quiet(&theme, BEZEL, &inactive).a);
        assert!(quiet(&theme, BEZEL, &state(true, true, true)).a > quiet(&theme, BEZEL, &active).a);
        assert!(quiet(&theme, WELL, &active).a > quiet(&theme, BEZEL, &active).a);
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
