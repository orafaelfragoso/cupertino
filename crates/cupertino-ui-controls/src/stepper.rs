use crate::look::{self, paint_focus_ring};
use crate::Stepper;
use cupertino_geometry::{Insets, Point, Rect, Size};
use cupertino_theme::metrics::{control_metric, Control};
use cupertino_ui_core::{
    AccessibleStates, AccessibleValue, Action, ActionData, Change, ControlState, Environment,
    Event, EventCx, MeasureCx, PaintCx, Propagation, Role, SemanticsCx, Widget,
};
use cupertino_ui_layout::{Constraints, ContainerKind, LayoutSpec, Measured};
use cupertino_ui_platform::{Keysym, MouseButton, PointerEvent};
use cupertino_ui_render::{Paint, PaintOp, PathVerb, Shape};
use cupertino_ui_theme::ColorRole;

const HOME: Keysym = Keysym(0xff50);
const UP: Keysym = Keysym(0xff52);
const DOWN: Keysym = Keysym(0xff54);
const PAGE_UP: Keysym = Keysym(0xff55);
const PAGE_DOWN: Keysym = Keysym(0xff56);
const END: Keysym = Keysym(0xff57);

/// A chevron's width; it is half as high. Approximate design value.
const CHEVRON: f64 = 7.0;
const CHEVRON_STROKE: f64 = 1.5;
/// The page keys move this many steps. Approximate design value.
const PAGE: f64 = 10.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Half {
    Up,
    Down,
}

/// The node state of a [`Stepper`]. Opaque.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StepperState {
    hovered: bool,
    pressed: Option<Half>,
}

/// The chrome: the regular image's size, centred in the node.
fn chrome(size: Size) -> Rect {
    let (width, height) = control_metric(Control::Stepper)
        .image_regular
        .unwrap_or_default();
    Rect::new(
        (size.width - width) / 2.0,
        (size.height - height) / 2.0,
        width,
        height,
    )
}

fn half_rect(chrome: Rect, half: Half) -> Rect {
    let height = chrome.height() / 2.0;
    let y = match half {
        Half::Up => chrome.min_y(),
        Half::Down => chrome.min_y() + height,
    };
    Rect::new(chrome.min_x(), y, chrome.width(), height)
}

/// A chevron centred in `half`, pointing up or down.
fn chevron(half: Half, area: Rect) -> Shape {
    let centre = Point::new(
        area.min_x() + area.width() / 2.0,
        area.min_y() + area.height() / 2.0,
    );
    let rise = match half {
        Half::Up => -CHEVRON / 4.0,
        Half::Down => CHEVRON / 4.0,
    };
    Shape::Path {
        verbs: vec![
            PathVerb::MoveTo(Point::new(centre.x - CHEVRON / 2.0, centre.y - rise)),
            PathVerb::LineTo(Point::new(centre.x, centre.y + rise)),
            PathVerb::LineTo(Point::new(centre.x + CHEVRON / 2.0, centre.y - rise)),
        ],
        fill_rule: Default::default(),
    }
}

impl<M> Stepper<M> {
    /// `value` clamped to the range and snapped to the step from `min`.
    fn fit(&self, value: f64) -> f64 {
        let value = value.clamp(self.min, self.max.max(self.min));
        if self.step > 0.0 {
            (self.min + ((value - self.min) / self.step).round() * self.step).min(self.max)
        } else {
            value
        }
    }

    /// The half can still move the value.
    fn can(&self, half: Half) -> bool {
        self.enabled
            && match half {
                Half::Up => self.value < self.max,
                Half::Down => self.value > self.min,
            }
    }
}

impl<M: 'static> Stepper<M> {
    fn set(&self, value: f64, cx: &mut EventCx<'_, M>) {
        let value = self.fit(value);
        if value != self.value {
            if let Some(on_change) = &self.on_change {
                cx.emit(on_change(value));
            }
        }
    }
}

impl<M: 'static> Widget<M> for Stepper<M> {
    type State = StepperState;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) -> StepperState {
        StepperState::default()
    }

    fn update(&self, old: &Self, _: &mut StepperState) -> Change {
        if self.value != old.value
            || self.min != old.min
            || self.max != old.max
            || self.step != old.step
            || self.enabled != old.enabled
            || self.label != old.label
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &StepperState, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: chrome(Size::ZERO).size,
            baseline: None,
        }
    }

    fn target(&self, _: &StepperState, env: &Environment) -> Size {
        env.theme.metrics.min_target
    }

    fn event(
        &self,
        state: &mut StepperState,
        event: &Event,
        cx: &mut EventCx<'_, M>,
    ) -> Propagation {
        match *event {
            Event::Pointer(PointerEvent::Enter { .. }, _) if cx.node() == cx.target() => {
                state.hovered = true;
                cx.request_paint();
                Propagation::Continue
            }
            Event::Pointer(PointerEvent::Leave, _) if cx.node() == cx.target() => {
                state.hovered = false;
                cx.request_paint();
                Propagation::Continue
            }
            Event::Pointer(
                PointerEvent::Button {
                    position,
                    button: MouseButton::Left,
                    pressed: true,
                    ..
                },
                _,
            ) if self.enabled && cx.node() == cx.target() => {
                let Some(bounds) = cx.bounds() else {
                    return Propagation::Continue;
                };
                let middle = bounds.min_y() + bounds.height() / 2.0;
                let half = if position.y < middle {
                    Half::Up
                } else {
                    Half::Down
                };
                cx.request_focus();
                if self.can(half) {
                    state.pressed = Some(half);
                    cx.capture_pointer();
                    let step = match half {
                        Half::Up => self.step,
                        Half::Down => -self.step,
                    };
                    self.set(self.value + step, cx);
                    cx.request_paint();
                }
                Propagation::Stop
            }
            Event::Pointer(PointerEvent::Button { pressed: false, .. }, _)
                if state.pressed.is_some() =>
            {
                state.pressed = None;
                cx.release_pointer();
                cx.request_paint();
                Propagation::Stop
            }
            Event::Cancel if state.pressed.is_some() => {
                state.pressed = None;
                cx.request_paint();
                Propagation::Continue
            }
            Event::Key(key, _) if self.enabled && key.pressed && cx.node() == cx.target() => {
                let value = match key.keysym {
                    UP => self.value + self.step,
                    DOWN => self.value - self.step,
                    PAGE_UP => self.value + PAGE * self.step,
                    PAGE_DOWN => self.value - PAGE * self.step,
                    HOME => self.min,
                    END => self.max,
                    _ => return Propagation::Continue,
                };
                self.set(value, cx);
                Propagation::Stop
            }
            Event::Action(ref request) if self.enabled => {
                let value = match (request.action, &request.data) {
                    (Action::Increment, _) => self.value + self.step,
                    (Action::Decrement, _) => self.value - self.step,
                    (Action::SetValue, Some(ActionData::Number(value))) => *value,
                    (Action::Focus, _) => {
                        cx.request_focus();
                        return Propagation::Stop;
                    }
                    _ => return Propagation::Continue,
                };
                self.set(value, cx);
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, state: &StepperState, cx: &mut PaintCx<'_>) {
        let theme = cx.env().theme.clone();
        let base = ControlState {
            enabled: self.enabled,
            hovered: state.hovered,
            focused: cx.is_focused(),
            focus_visible: cx.focus_visible(),
            window_active: cx.window_active(),
            ..ControlState::default()
        };
        let chrome = chrome(cx.size());
        let radius = theme.metrics.button_radius;
        look::fill(
            cx,
            chrome,
            radius,
            look::color(&theme, ColorRole::ControlFill, &base),
        );
        for half in [Half::Up, Half::Down] {
            let area = half_rect(chrome, half);
            let own = ControlState {
                enabled: self.can(half),
                pressed: state.pressed == Some(half),
                ..base
            };
            if own.pressed {
                look::fill(
                    cx,
                    area.inset(Insets::uniform(1.0)),
                    radius - 1.0,
                    look::color(&theme, ColorRole::ControlFill, &own),
                );
            }
            if let Some(color) = look::color(&theme, ColorRole::ControlText, &own) {
                cx.push(PaintOp::Stroke {
                    shape: chevron(half, area),
                    width: CHEVRON_STROKE,
                    paint: Paint::Solid(color),
                });
            }
        }
        look::stroke(
            cx,
            chrome.inset(Insets::uniform(0.5)),
            radius - 0.5,
            1.0,
            look::color(&theme, ColorRole::ControlBorder, &base),
        );
        paint_focus_ring(cx, chrome, radius);
    }

    fn semantics(&self, _: &StepperState, cx: &mut SemanticsCx<'_>) {
        cx.role(Role::SpinButton);
        cx.name(self.label.clone());
        cx.value(AccessibleValue::Number {
            value: self.value,
            min: self.min,
            max: self.max,
            step: Some(self.step),
        });
        cx.states(AccessibleStates {
            disabled: !self.enabled,
            ..AccessibleStates::default()
        });
        if self.enabled {
            cx.focusable();
            cx.action(Action::Focus);
            cx.action(Action::Increment);
            cx.action(Action::Decrement);
            cx.action(Action::SetValue);
        }
    }
}
