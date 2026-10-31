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

impl<M: Clone + 'static> Button<M> {
    fn press(
        &self,
        press: &mut Press,
        event: &Event,
        keys: Keys,
        cx: &mut EventCx<'_, M>,
    ) -> Propagation {
        match press.event(event, self.active(), keys, cx) {
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
}

/// A button that Space activates but Return passes over, so a sheet's Return reaches its default button.
pub(crate) struct SheetButton<M>(pub(crate) Button<M>);

impl<M: Clone + 'static> Widget<M> for SheetButton<M> {
    type State = Press;

    fn layout(&self) -> LayoutSpec {
        self.0.layout()
    }

    fn state(&self) -> Press {
        Press::default()
    }

    fn update(&self, old: &Self, press: &mut Press) -> Change {
        self.0.update(&old.0, press)
    }

    fn measure(&self, press: &Press, constraints: Constraints, cx: &mut MeasureCx<'_>) -> Measured {
        self.0.measure(press, constraints, cx)
    }

    fn event(&self, press: &mut Press, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        self.0.press(press, event, Keys::Space, cx)
    }

    fn paint(&self, press: &Press, cx: &mut PaintCx<'_>) {
        self.0.paint(press, cx)
    }

    fn semantics(&self, press: &Press, cx: &mut SemanticsCx<'_>) {
        self.0.semantics(press, cx)
    }
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

    /// A round icon button's label is only its accessible name; the others fit the label between their insets.
    fn measure(&self, _: &Press, _: Constraints, cx: &mut MeasureCx<'_>) -> Measured {
        let metrics = cx.env().theme.metrics;
        let width = match self.style {
            ButtonStyle::RoundIcon => metrics.button_height,
            _ => {
                let label = look::label(cx.env(), &self.label).map_or(0.0, |l| l.size().width);
                label + 2.0 * metrics.button_inset
            }
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
        self.press(press, event, Keys::SpaceAndReturn, cx)
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
        let (fill, text) = self.roles();
        let fills = self.style != ButtonStyle::Borderless || state.pressed;
        if fills {
            look::fill(cx, chrome, radius, look::color(&theme, fill, &state));
        }
        // in high contrast a filled button always shows its border: a fill alone may not stand out from the window
        let high_contrast = theme.preferences.appearance.is_high_contrast();
        if self.style != ButtonStyle::Borderless || (fills && high_contrast) {
            look::stroke(
                cx,
                chrome.inset(cupertino_geometry::Insets::uniform(0.5)),
                radius - 0.5,
                1.0,
                look::color(&theme, ColorRole::ControlBorder, &state),
            );
        }
        if self.style != ButtonStyle::RoundIcon {
            if let Some(label) = look::label(cx.env(), &self.label) {
                let origin = look::centred(&label, chrome, cx.env());
                look::glyphs(cx, &label, origin, look::color(&theme, text, &state));
            }
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
