use crate::look::{self, paint_focus_ring};
use crate::press::{Keys, Outcome, Press};
use crate::CheckBox;
use cupertino_geometry::{Insets, Point, Rect, Size};
use cupertino_theme::metrics::Control;
use cupertino_ui_core::{
    AccessibleStates, AccessibleValue, Action, Change, Checked, ControlState, Event, EventCx,
    MeasureCx, PaintCx, Propagation, Role, SemanticsCx, Widget,
};
use cupertino_ui_layout::{Constraints, ContainerKind, LayoutDirection, LayoutSpec, Measured};
use cupertino_ui_render::{Paint, PaintOp, PathVerb, Shape};
use cupertino_ui_theme::ColorRole;

/// Between the box and its label.
const LABEL_GAP: f64 = 6.0;
const BOX_RADIUS: f64 = 3.0;
const MARK_WIDTH: f64 = 2.0;
/// The mixed state's bar.
const MIXED: Size = Size::new(8.0, 2.0);

impl<M: 'static> CheckBox<M> {
    fn control_state(&self, press: &Press, cx: &PaintCx<'_>) -> ControlState {
        ControlState {
            enabled: self.enabled,
            hovered: press.hovered(),
            pressed: press.pressed(),
            focused: cx.is_focused(),
            focus_visible: cx.focus_visible(),
            selected: self.checked != Checked::False,
            window_active: cx.window_active(),
            ..ControlState::default()
        }
    }
}

/// The box, centred in the node's height, at the leading edge.
fn check_box(size: Size, side: f64, direction: LayoutDirection) -> Rect {
    let x = match direction {
        LayoutDirection::Ltr => 0.0,
        LayoutDirection::Rtl => size.width - side,
    };
    Rect::new(x, (size.height - side) / 2.0, side, side)
}

/// A tick from the left of the box's middle down to its lower third and up to its upper right.
fn tick(b: Rect) -> Shape {
    let at = |x: f64, y: f64| Point::new(b.min_x() + b.width() * x, b.min_y() + b.height() * y);
    Shape::Path {
        verbs: vec![
            PathVerb::MoveTo(at(0.27, 0.52)),
            PathVerb::LineTo(at(0.43, 0.68)),
            PathVerb::LineTo(at(0.75, 0.33)),
        ],
        fill_rule: Default::default(),
    }
}

impl<M: 'static> Widget<M> for CheckBox<M> {
    type State = Press;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) -> Press {
        Press::default()
    }

    fn update(&self, old: &Self, _: &mut Press) -> Change {
        if self.label != old.label {
            Change::Layout
        } else if self.checked != old.checked || self.enabled != old.enabled {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &Press, _: Constraints, cx: &mut MeasureCx<'_>) -> Measured {
        let theme = &cx.env().theme;
        let side = look::metric(Control::CheckBoxRadio, theme);
        let label = look::label(cx.env(), &self.label).map(|l| l.size());
        let width = side + label.map_or(0.0, |l| LABEL_GAP + l.width);
        let height = label.map_or(side, |l| side.max(l.height));
        Measured {
            size: Size::new(
                width.max(theme.metrics.min_target.width),
                height.max(theme.metrics.min_target.height),
            ),
            baseline: None,
        }
    }

    fn event(&self, press: &mut Press, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match press.event(event, self.enabled, Keys::Space, cx) {
            Outcome::Ignored => Propagation::Continue,
            Outcome::Changed => {
                cx.request_paint();
                Propagation::Stop
            }
            Outcome::Activated(_) => {
                cx.request_paint();
                if let Some(on_toggle) = &self.on_toggle {
                    cx.emit(on_toggle(self.checked != Checked::True));
                }
                Propagation::Stop
            }
        }
    }

    fn paint(&self, press: &Press, cx: &mut PaintCx<'_>) {
        let env = cx.env().clone();
        let theme = &env.theme;
        let state = self.control_state(press, cx);
        let side = look::metric(Control::CheckBoxRadio, theme);
        let size = cx.size();
        let b = check_box(size, side, env.direction);
        look::fill(
            cx,
            b,
            BOX_RADIUS,
            look::color(theme, ColorRole::ControlFill, &state),
        );
        look::stroke(
            cx,
            b.inset(Insets::uniform(0.5)),
            BOX_RADIUS - 0.5,
            1.0,
            look::color(theme, ColorRole::ControlBorder, &state),
        );
        let mark = look::color(theme, ColorRole::ControlText, &state);
        match (self.checked, mark) {
            (Checked::True, Some(color)) => cx.push(PaintOp::Stroke {
                shape: tick(b),
                width: MARK_WIDTH,
                paint: Paint::Solid(color),
            }),
            (Checked::Mixed, Some(color)) => cx.push(PaintOp::Fill {
                shape: Shape::Rect(Rect::new(
                    b.min_x() + (side - MIXED.width) / 2.0,
                    b.min_y() + (side - MIXED.height) / 2.0,
                    MIXED.width,
                    MIXED.height,
                )),
                paint: Paint::Solid(color),
            }),
            _ => {}
        }
        if let Some(label) = look::label(&env, &self.label) {
            let width = label.size().width;
            let x = match env.direction {
                LayoutDirection::Ltr => b.max_x() + LABEL_GAP,
                LayoutDirection::Rtl => b.min_x() - LABEL_GAP - width,
            };
            let row = Rect::new(0.0, 0.0, size.width, size.height);
            let origin = Point::new(x, look::top(&label, row, &env));
            let text = ControlState {
                selected: false,
                ..state
            };
            look::glyphs(
                cx,
                &label,
                origin,
                look::color(theme, ColorRole::Text, &text),
            );
        }
        paint_focus_ring(cx, b, BOX_RADIUS);
    }

    fn semantics(&self, _: &Press, cx: &mut SemanticsCx<'_>) {
        cx.role(Role::CheckBox);
        cx.name(self.label.clone());
        cx.value(AccessibleValue::Toggle(self.checked));
        cx.states(AccessibleStates {
            disabled: !self.enabled,
            ..AccessibleStates::default()
        });
        if self.enabled {
            cx.focusable();
            cx.action(Action::Click);
        }
    }
}
