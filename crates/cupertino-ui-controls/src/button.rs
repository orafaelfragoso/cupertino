use crate::look::{self, paint_focus_ring};
use crate::press::{Keys, Outcome, Press};
use crate::{Button, ButtonRole, ButtonStyle};
use cupertino_geometry::{Rect, Size};
use cupertino_ui_core::{
    AccessibleStates, Action, Change, ControlState, Event, EventCx, MeasureCx, PaintCx,
    Propagation, Role, SemanticsCx, Widget,
};
use cupertino_ui_layout::{Constraints, ContainerKind, LayoutSpec, Measured};
use cupertino_ui_theme::{ColorRole, ThemeTokens};

impl<M> Button<M> {
    /// The fill and text roles: the role picks the colour family, a borderless button fills only while pressed.
    fn roles(&self) -> (ColorRole, ColorRole) {
        match self.role {
            ButtonRole::Normal => (ColorRole::ControlFill, ColorRole::ControlText),
            ButtonRole::Default => (ColorRole::DefaultButtonFill, ColorRole::DefaultButtonText),
            ButtonRole::Destructive => (ColorRole::DestructiveFill, ColorRole::DestructiveText),
        }
    }

    fn radius(&self, chrome: Rect, theme: &ThemeTokens) -> f64 {
        match self.style {
            ButtonStyle::Bordered | ButtonStyle::Borderless => theme.metrics.button_radius,
            ButtonStyle::Pill | ButtonStyle::RoundIcon => chrome.height() / 2.0,
        }
    }

    fn active(&self) -> bool {
        self.enabled && !self.busy
    }
}

/// The chrome: the button's height, centred in a node padded out to the minimum target. A round icon button is
/// square.
fn chrome(size: Size, style: ButtonStyle, theme: &ThemeTokens) -> Rect {
    let height = theme.metrics.button_height;
    let width = match style {
        ButtonStyle::RoundIcon => height,
        _ => size.width,
    };
    Rect::new(
        (size.width - width) / 2.0,
        (size.height - height) / 2.0,
        width,
        height,
    )
}

impl<M: Clone + 'static> Widget<M> for Button<M> {
    type State = Press;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) -> Press {
        Press::default()
    }

    fn update(&self, old: &Self, _: &mut Press) -> Change {
        if self.label != old.label || self.style != old.style {
            Change::Layout
        } else if self.role != old.role || self.enabled != old.enabled || self.busy != old.busy {
            Change::Paint
        } else {
            Change::None
        }
    }

    /// The label's width comes with text shaping; until then the chrome holds only its insets.
    fn measure(&self, _: &Press, _: Constraints, cx: &mut MeasureCx<'_>) -> Measured {
        let metrics = cx.env().theme.metrics;
        let width = match self.style {
            ButtonStyle::RoundIcon => metrics.button_height,
            _ => 2.0 * metrics.button_inset,
        };
        Measured {
            size: Size::new(
                width.max(metrics.min_target.width),
                metrics.button_height.max(metrics.min_target.height),
            ),
            baseline: None,
        }
    }

    fn event(&self, press: &mut Press, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match press.event(event, self.active(), Keys::SpaceAndReturn, cx) {
            Outcome::Ignored => Propagation::Continue,
            Outcome::Changed => {
                cx.request_paint();
                Propagation::Stop
            }
            Outcome::Activated(_) => {
                cx.request_paint();
                if let Some(message) = &self.on_press {
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
            busy: self.busy,
            ..ControlState::default()
        };
        let chrome = chrome(cx.size(), self.style, &theme);
        let radius = self.radius(chrome, &theme);
        let (fill, _) = self.roles();
        if self.style != ButtonStyle::Borderless || state.pressed {
            look::fill(cx, chrome, radius, look::color(&theme, fill, &state));
        }
        if self.style == ButtonStyle::Bordered && self.role == ButtonRole::Normal {
            look::stroke(
                cx,
                chrome.inset(cupertino_geometry::Insets::uniform(0.5)),
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
        cx.states(AccessibleStates {
            disabled: !self.enabled,
            busy: self.busy,
            ..AccessibleStates::default()
        });
        if self.enabled {
            cx.focusable();
        }
        if self.active() {
            cx.action(Action::Click);
        }
    }
}
