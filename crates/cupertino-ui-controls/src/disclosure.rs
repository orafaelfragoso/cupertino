use crate::look::{self, paint_focus_ring};
use crate::press::{Keys, Outcome, Press};
use crate::{DisclosureButton, DisclosureStyle};
use cupertino_geometry::{Insets, Point, Rect, Size};
use cupertino_ui_core::{
    AccessibleStates, Action, Change, ControlState, Environment, Event, EventCx, MeasureCx,
    PaintCx, Propagation, Role, SemanticsCx, Widget,
};
use cupertino_ui_layout::{Constraints, ContainerKind, LayoutDirection, LayoutSpec, Measured};
use cupertino_ui_render::{Paint, PaintOp, PathVerb, Shape};
use cupertino_ui_theme::{ColorRole, ControlFamily};

/// The square a triangle is drawn in.
const TRIANGLE: f64 = 13.0;
/// A bordered button's chevron width; it is half as high. Approximate design value.
const CHEVRON: f64 = 7.0;
const CHEVRON_STROKE: f64 = 1.5;

/// `points` in a unit square turned to `area`: rotated a quarter turn clockwise when open, mirrored in right-to-left
/// layouts while closed, so a closed shape points along the reading direction and an open one down.
pub(crate) fn place(points: [(f64, f64); 3], area: Rect, open: bool, rtl: bool) -> Vec<Point> {
    points
        .into_iter()
        .map(|(x, y)| match (open, rtl) {
            (true, _) => (1.0 - y, x),
            (false, true) => (1.0 - x, y),
            (false, false) => (x, y),
        })
        .map(|(x, y)| {
            Point::new(
                area.min_x() + x * area.width(),
                area.min_y() + y * area.height(),
            )
        })
        .collect()
}

pub(crate) fn path(points: &[Point], close: bool) -> Shape {
    let mut verbs = vec![PathVerb::MoveTo(points[0])];
    verbs.extend(points[1..].iter().map(|&p| PathVerb::LineTo(p)));
    if close {
        verbs.push(PathVerb::Close);
    }
    Shape::Path {
        verbs,
        fill_rule: Default::default(),
    }
}

/// A square of `side` centred in the node.
fn centred(size: Size, side: f64) -> Rect {
    Rect::new(
        (size.width - side) / 2.0,
        (size.height - side) / 2.0,
        side,
        side,
    )
}

impl<M: Clone + 'static> DisclosureButton<M> {
    fn toggle(&self, cx: &mut EventCx<'_, M>) {
        if let Some(on_toggle) = &self.on_toggle {
            cx.emit(on_toggle(!self.expanded));
        }
    }
}

impl<M: Clone + 'static> Widget<M> for DisclosureButton<M> {
    type State = Press;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) -> Press {
        Press::default()
    }

    fn update(&self, old: &Self, _: &mut Press) -> Change {
        if self.style != old.style {
            Change::Layout
        } else if self.expanded != old.expanded
            || self.enabled != old.enabled
            || self.label != old.label
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &Press, _: Constraints, cx: &mut MeasureCx<'_>) -> Measured {
        let theme = &cx.env().theme;
        let side = look::geometry(theme, ControlFamily::Disclosure).height;
        Measured {
            size: Size::new(side, side),
            baseline: None,
        }
    }

    fn target(&self, _: &Press, env: &Environment) -> Size {
        env.theme.metrics.min_target
    }

    fn event(&self, press: &mut Press, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        if let Event::Action(request) = event {
            let wanted = match request.action {
                Action::Expand => Some(true),
                Action::Collapse => Some(false),
                _ => None,
            };
            if let Some(wanted) = wanted {
                if self.enabled && wanted != self.expanded {
                    self.toggle(cx);
                }
                return Propagation::Stop;
            }
        }
        match press.event(event, self.enabled, Keys::SpaceAndReturn, cx) {
            Outcome::Ignored => Propagation::Continue,
            Outcome::Changed => {
                cx.request_paint();
                Propagation::Stop
            }
            Outcome::Activated(_) => {
                cx.request_paint();
                self.toggle(cx);
                Propagation::Stop
            }
        }
    }

    fn paint(&self, press: &Press, cx: &mut PaintCx<'_>) {
        let theme = cx.env().theme.clone();
        let rtl = cx.env().direction == LayoutDirection::Rtl;
        let state = ControlState {
            enabled: self.enabled,
            hovered: press.hovered(),
            pressed: press.pressed(),
            focused: cx.is_focused(),
            focus_visible: cx.focus_visible(),
            window_active: cx.window_active(),
            ..ControlState::default()
        };
        let size = cx.size();
        match self.style {
            DisclosureStyle::Triangle => {
                let area = centred(size, TRIANGLE);
                let points = place(
                    [(0.25, 0.15), (0.8, 0.5), (0.25, 0.85)],
                    area,
                    self.expanded,
                    rtl,
                );
                if let Some(color) = look::color(&theme, ColorRole::SecondaryText, &state) {
                    cx.push(PaintOp::Fill {
                        shape: path(&points, true),
                        paint: Paint::Solid(color),
                    });
                }
                paint_focus_ring(cx, area, TRIANGLE / 2.0);
            }
            DisclosureStyle::Bordered => {
                let chrome = centred(
                    size,
                    look::geometry(&theme, ControlFamily::Disclosure).height,
                );
                let radius = theme.metrics.button_radius;
                look::fill(
                    cx,
                    chrome,
                    radius,
                    look::color(&theme, ColorRole::ControlFill, &state),
                );
                look::stroke(
                    cx,
                    chrome.inset(Insets::uniform(0.5)),
                    radius - 0.5,
                    1.0,
                    look::color(&theme, ColorRole::ControlBorder, &state),
                );
                // a chevron that points the way the triangle would, in a square as wide as the chevron
                let side = CHEVRON;
                let area = centred(size, side);
                let points = place(
                    [(0.25, 0.0), (0.75, 0.5), (0.25, 1.0)],
                    area,
                    self.expanded,
                    rtl,
                );
                if let Some(color) = look::color(&theme, ColorRole::ControlText, &state) {
                    cx.push(PaintOp::Stroke {
                        shape: path(&points, false),
                        width: CHEVRON_STROKE,
                        paint: Paint::Solid(color),
                    });
                }
                paint_focus_ring(cx, chrome, radius);
            }
        }
    }

    fn semantics(&self, _: &Press, cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Button);
        cx.name(self.label.clone());
        cx.states(AccessibleStates {
            disabled: !self.enabled,
            expanded: Some(self.expanded),
            ..AccessibleStates::default()
        });
        if self.enabled {
            cx.focusable();
            cx.action(Action::Click);
            cx.action(if self.expanded {
                Action::Collapse
            } else {
                Action::Expand
            });
        }
    }
}
