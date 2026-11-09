//! A colour well: the colour it holds in a capsule set in a quiet rim, which opens a colour picker when pressed.

use crate::look::{self, paint_focus_ring};
use crate::press::{Keys, Outcome, Press};
use crate::ColorWell;
use cupertino_geometry::{Insets, Rect, Size};
use cupertino_ui_core::{
    AccessibleStates, AccessibleValue, Action, Change, ControlState, Event, EventCx, MeasureCx,
    PaintCx, Propagation, Role, SemanticsCx, Widget,
};
use cupertino_ui_layout::{Constraints, ContainerKind, LayoutSpec, Measured};
use cupertino_ui_theme::{Color, ColorRole, ControlFamily};

/// The well's width as a share of its height: a capsule about twice as wide as it is high.
const ASPECT: f64 = 2.0;
/// How far the colour sits inside the rim.
const RIM: f64 = 3.0;
/// The fine edge round the colour: white at this opacity in a dark appearance, black at this one in a light one.
const EDGE_DARK: f32 = 0.2;
const EDGE_LIGHT: f32 = 0.14;
/// A disabled well shows its colour at this share of its opacity.
const DISABLED_COLOR: f32 = 0.45;

impl<M> ColorWell<M> {
    /// The capsule, as high as a button, centred in the node.
    fn chrome(size: Size, height: f64) -> Rect {
        let width = height * ASPECT;
        Rect::new(
            (size.width - width) / 2.0,
            (size.height - height) / 2.0,
            width,
            height,
        )
    }
}

impl<M: Clone + 'static> Widget<M> for ColorWell<M> {
    type State = Press;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) -> Press {
        Press::default()
    }

    fn update(&self, old: &Self, _: &mut Press) -> Change {
        if self.color != old.color || self.enabled != old.enabled || self.label != old.label {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &Press, _: Constraints, cx: &mut MeasureCx<'_>) -> Measured {
        let theme = &cx.env().theme;
        let height = look::geometry(theme, ControlFamily::PushButton).height;
        let target = theme.metrics.min_target;
        Measured {
            size: Size::new(
                (height * ASPECT).max(target.width),
                height.max(target.height),
            ),
            baseline: None,
        }
    }

    fn event(&self, press: &mut Press, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        let active = self.enabled && self.on_open.is_some();
        match press.event(event, active, Keys::SpaceAndReturn, cx) {
            Outcome::Ignored => Propagation::Continue,
            Outcome::Changed => {
                cx.request_paint();
                Propagation::Stop
            }
            Outcome::Activated(_) => {
                cx.request_paint();
                if let Some(message) = &self.on_open {
                    cx.emit(message.clone());
                }
                Propagation::Stop
            }
        }
    }

    fn paint(&self, press: &Press, cx: &mut PaintCx<'_>) {
        let theme = cx.env().theme.clone();
        let state = ControlState {
            enabled: self.enabled,
            hovered: press.hovered(),
            pressed: press.pressed(),
            focused: cx.is_focused(),
            focus_visible: cx.focus_visible(),
            window_active: cx.window_active(),
            ..ControlState::default()
        };
        let height = look::geometry(&theme, ControlFamily::PushButton).height;
        let chrome = Self::chrome(cx.size(), height);
        let radius = height / 2.0;
        let appearance = theme.preferences.appearance;
        // the rim: an ordinary button's quiet bezel, which a press darkens
        look::fill(
            cx,
            chrome,
            radius,
            Some(look::quiet(&theme, look::BEZEL, &state)),
        );
        let inner = chrome.inset(Insets::uniform(RIM));
        let inner_radius = radius - RIM;
        let payload = if self.enabled {
            self.color
        } else {
            Color {
                a: self.color.a * DISABLED_COLOR,
                ..self.color
            }
        };
        look::fill(cx, inner, inner_radius, Some(payload));
        // the colour's own edge, so a colour like the rim's still reads as the payload; the theme's border, stronger,
        // under increased contrast
        let edge = if appearance.is_high_contrast() {
            look::color(&theme, ColorRole::ControlBorder, &state)
        } else if appearance.is_dark() {
            Some(Color::new(1.0, 1.0, 1.0, EDGE_DARK))
        } else {
            Some(Color::new(0.0, 0.0, 0.0, EDGE_LIGHT))
        };
        look::stroke(
            cx,
            inner.inset(Insets::uniform(0.5)),
            inner_radius - 0.5,
            1.0,
            edge,
        );
        if appearance.is_high_contrast() {
            look::stroke(
                cx,
                chrome.inset(Insets::uniform(0.5)),
                radius - 0.5,
                1.0,
                look::color(&theme, ColorRole::ControlBorder, &state),
            );
        }
        paint_focus_ring(cx, chrome, radius);
    }

    fn semantics(&self, _: &Press, cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Button);
        cx.name(self.label.clone());
        cx.value(AccessibleValue::Text {
            value: self.color.hex(),
            editing: None,
        });
        cx.states(AccessibleStates {
            disabled: !self.enabled,
            ..AccessibleStates::default()
        });
        if self.enabled {
            cx.focusable();
            if self.on_open.is_some() {
                cx.action(Action::Click);
            }
        }
    }
}
