use crate::look::{self, paint_focus_ring};
use crate::press::{Keys, Outcome, Press};
use crate::CheckBox;
use cupertino_geometry::{Insets, Point, Rect, Size};
use cupertino_ui_core::{
    AccessibleStates, AccessibleValue, Action, Change, Checked, ControlState, Environment, Event,
    EventCx, MeasureCx, PaintCx, Propagation, Role, SemanticsCx, Widget,
};
use cupertino_ui_layout::{Constraints, ContainerKind, LayoutDirection, LayoutSpec, Measured};
use cupertino_ui_render::{Paint, PaintOp, PathVerb, Shape};
use cupertino_ui_text::ParagraphLayout;
use cupertino_ui_theme::{choice_mark, ColorRole, ControlFamily};

/// Between the box and its label.
const LABEL_GAP: f64 = 6.0;
const MARK_WIDTH: f64 = 2.0;
/// The mixed state's bar.
const MIXED: Size = Size::new(8.0, 2.0);

impl<M: 'static> CheckBox<M> {
    /// The label for a check box `width` wide with a box `side` wide: on one line when it fits beside the box, else
    /// wrapped at word breaks into the room beside it, so a long label in a narrow column stays inside it.
    fn label_layout(&self, env: &Environment, width: f64, side: f64) -> Option<ParagraphLayout> {
        let line = look::label(env, &self.label)?;
        let room = width - side - LABEL_GAP;
        if !room.is_finite() || room <= 0.0 || line.size().width <= room {
            return Some(line);
        }
        look::wrapped(env, &self.label, room).or(Some(line))
    }

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

/// The box at the leading edge, centred in the node's height, or on the first line of a label that wraps (`first`).
fn check_box(size: Size, side: f64, direction: LayoutDirection, first: Option<Rect>) -> Rect {
    let x = match direction {
        LayoutDirection::Ltr => 0.0,
        LayoutDirection::Rtl => size.width - side,
    };
    let y = match first {
        Some(line) => (line.center().y - side / 2.0).max(0.0),
        None => (size.height - side) / 2.0,
    };
    Rect::new(x, y, side, side)
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

    fn measure(&self, _: &Press, constraints: Constraints, cx: &mut MeasureCx<'_>) -> Measured {
        let theme = &cx.env().theme;
        let side = choice_mark(theme.metrics.size, theme.metrics.compact);
        let label = self
            .label_layout(cx.env(), constraints.width.max(), side)
            .map(|l| l.size());
        let width = side + label.map_or(0.0, |l| LABEL_GAP + l.width);
        let height = label.map_or(side, |l| side.max(l.height));
        Measured {
            size: Size::new(width, height),
            baseline: None,
        }
    }

    fn target(&self, _: &Press, env: &Environment) -> Size {
        env.theme.metrics.min_target
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
        let side = choice_mark(theme.metrics.size, theme.metrics.compact);
        let radius = look::geometry(theme, ControlFamily::CheckBox)
            .corners
            .radius(side);
        let size = cx.size();
        let label = self.label_layout(&env, size.width, side);
        let first = label
            .as_ref()
            .filter(|l| l.lines().len() > 1)
            .and_then(|l| l.lines().first().map(|line| line.rect));
        let b = check_box(size, side, env.direction, first);
        // unchecked a quiet well, checked (or mixed) the accent, subdued in a window that is not active
        let fill = if self.checked == Checked::False {
            Some(look::quiet(theme, look::WELL, &state))
        } else {
            look::color(theme, ColorRole::ControlFill, &state)
        };
        look::fill(cx, b, radius, fill);
        if theme.preferences.appearance.is_high_contrast() {
            look::stroke(
                cx,
                b.inset(Insets::uniform(0.5)),
                radius - 0.5,
                1.0,
                look::color(theme, ColorRole::ControlBorder, &state),
            );
        }
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
        if let Some(label) = label {
            let width = label.size().width;
            let x = match env.direction {
                LayoutDirection::Ltr => b.max_x() + LABEL_GAP,
                LayoutDirection::Rtl => b.min_x() - LABEL_GAP - width,
            };
            let row = Rect::new(0.0, 0.0, size.width, size.height);
            // a wrapped label hangs from the top, its first line beside the box
            let top = if first.is_some() {
                0.0
            } else {
                look::top(&label, row, &env)
            };
            let origin = Point::new(x, top);
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
        paint_focus_ring(cx, b, radius);
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
