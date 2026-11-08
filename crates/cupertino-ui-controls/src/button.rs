use crate::look::{self, paint_focus_ring};
use crate::press::{Keys, Outcome, Press};
use crate::{Button, ButtonRole, ButtonStyle};
use cupertino_geometry::{Rect, Size};
use cupertino_ui_core::{
    AccessibleStates, Action, Change, ControlState, Environment, Event, EventCx, MeasureCx,
    PaintCx, Propagation, Role, SemanticsCx, Widget,
};
use cupertino_ui_layout::{Constraints, ContainerKind, LayoutSpec, Measured, SizeSpec};
use cupertino_ui_theme::{Color, ColorRole, InteractionVariant, ThemeTokens};

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
pub(crate) struct SheetButton<M> {
    pub(crate) button: Button<M>,
    /// Takes the focus when the sheet opens.
    pub(crate) initial: bool,
    /// An alert's button: a capsule as wide as its place, see [`paint_capsule`].
    pub(crate) capsule: bool,
}

/// An alert's capsule button: its height and the least room on each side of its label.
pub(crate) const CAPSULE_HEIGHT: f64 = 28.0;
pub(crate) const CAPSULE_INSET: f64 = 12.0;
/// The quiet capsule's fill: a share of white in the dark appearance and of black in the light one, more under the
/// pointer and more again while pressed.
const QUIET_DARK: f32 = 0.10;
const QUIET_LIGHT: f32 = 0.06;
const QUIET_HOVER: f32 = 0.04;
const QUIET_PRESSED: f32 = 0.10;

/// An alert's button: the default answer in the accent capsule with its light label, the others on a quiet capsule
/// with the control text, or the destructive red for a destructive one (a red capsule in high contrast). A capsule
/// fills its node's width.
fn paint_capsule<M>(button: &Button<M>, press: &Press, cx: &mut PaintCx<'_>) {
    let theme = cx.env().theme.clone();
    let state = ControlState {
        enabled: button.enabled,
        hovered: press.hovered(),
        pressed: press.pressed(),
        focused: cx.is_focused(),
        focus_visible: cx.focus_visible(),
        window_active: cx.window_active(),
        busy: button.busy,
        ..ControlState::default()
    };
    let size = cx.size();
    let height = CAPSULE_HEIGHT.min(size.height);
    let chrome = Rect::new(0.0, (size.height - height) / 2.0, size.width, height);
    let radius = height / 2.0;
    let high_contrast = theme.preferences.appearance.is_high_contrast();
    let (fill, text) = match button.role {
        ButtonRole::Default => (
            look::color(&theme, ColorRole::DefaultButtonFill, &state),
            look::color(&theme, ColorRole::DefaultButtonText, &state),
        ),
        // in high contrast a red label on a quiet capsule may not stand out: the capsule takes the red instead
        ButtonRole::Destructive if high_contrast => (
            look::color(&theme, ColorRole::DestructiveFill, &state),
            look::color(&theme, ColorRole::DestructiveText, &state),
        ),
        role => {
            let dark = theme.preferences.appearance.is_dark();
            let share = if dark { QUIET_DARK } else { QUIET_LIGHT }
                + match look::variant(&state).interaction {
                    InteractionVariant::Pressed => QUIET_PRESSED,
                    InteractionVariant::Hover => QUIET_HOVER,
                    _ => 0.0,
                };
            let tone = if dark { 1.0 } else { 0.0 };
            let text = if role == ButtonRole::Destructive {
                look::color(&theme, ColorRole::DestructiveFill, &state)
            } else {
                look::color(&theme, ColorRole::ControlText, &state)
            };
            (Some(Color::new(tone, tone, tone, share)), text)
        }
    };
    look::fill(cx, chrome, radius, fill);
    if high_contrast {
        look::stroke(
            cx,
            chrome.inset(cupertino_geometry::Insets::uniform(0.5)),
            radius - 0.5,
            1.0,
            look::color(&theme, ColorRole::ControlBorder, &state),
        );
    }
    if let Some(label) = look::label(cx.env(), &button.label) {
        let origin = look::centred(&label, chrome, cx.env());
        let text = text.map(|c| {
            if state.enabled {
                c
            } else {
                Color::new(c.r, c.g, c.b, c.a * 0.5)
            }
        });
        look::glyphs(cx, &label, origin, text);
    }
    paint_focus_ring(cx, chrome, radius);
}

impl<M: Clone + 'static> Widget<M> for SheetButton<M> {
    type State = Press;

    fn layout(&self) -> LayoutSpec {
        if self.capsule {
            LayoutSpec {
                width: SizeSpec::Fill,
                ..LayoutSpec::new(ContainerKind::Leaf)
            }
        } else {
            self.button.layout()
        }
    }

    fn state(&self) -> Press {
        Press::default()
    }

    fn update(&self, old: &Self, press: &mut Press) -> Change {
        if self.capsule != old.capsule {
            return Change::Layout;
        }
        self.button.update(&old.button, press)
    }

    fn measure(&self, press: &Press, constraints: Constraints, cx: &mut MeasureCx<'_>) -> Measured {
        if !self.capsule {
            return self.button.measure(press, constraints, cx);
        }
        let label = look::label(cx.env(), &self.button.label).map_or(0.0, |l| l.size().width);
        Measured {
            size: Size::new(label + 2.0 * CAPSULE_INSET, CAPSULE_HEIGHT),
            baseline: None,
        }
    }

    fn event(&self, press: &mut Press, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        self.button.press(press, event, Keys::Space, cx)
    }

    fn paint(&self, press: &Press, cx: &mut PaintCx<'_>) {
        if self.capsule {
            paint_capsule(&self.button, press, cx);
        } else {
            self.button.paint(press, cx)
        }
    }

    fn semantics(&self, press: &Press, cx: &mut SemanticsCx<'_>) {
        self.button.semantics(press, cx);
        if self.initial {
            cx.initial_focus();
        }
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
            size: Size::new(width, metrics.button_height),
            baseline: None,
        }
    }

    fn target(&self, _: &Press, env: &Environment) -> Size {
        env.theme.metrics.min_target
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
