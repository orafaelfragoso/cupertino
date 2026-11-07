use crate::look::{self, paint_focus_ring};
use crate::press::{Keys, Outcome, Press};
use crate::window_drag::DragState;
use crate::{Navigation, NavigationStack};
use cupertino_geometry::{Insets, Point, Rect, Size};
use cupertino_ui_core::{
    AccessibleStates, Action, Change, ControlState, Element, Event, EventCx, MeasureCx, PaintCx,
    Propagation, Role, SemanticsCx, Widget,
};
use cupertino_ui_layout::{
    Align, Axis, Constraints, ContainerKind, Flex, Justify, LayoutDirection, LayoutSpec, Measured,
    SizeSpec,
};
use cupertino_ui_platform::{Keysym, Mods};
use cupertino_ui_render::{Paint, PaintOp, PathVerb, Shape};
use cupertino_ui_theme::ColorRole;

const BRACKET_LEFT: Keysym = Keysym(0x5b);
const BRACKET_RIGHT: Keysym = Keysym(0x5d);

/// The back and forward group's size.
const GROUP: Size = Size {
    width: 53.0,
    height: 28.0,
};
/// The toolbar row's height, as the unified toolbar of the window above the page.
const TOOLBAR_HEIGHT: f64 = 52.0;
/// The toolbar row's inset from its leading and trailing edges.
const TOOLBAR_INSET: f64 = 8.0;
/// The space between the toolbar row's items.
const TOOLBAR_SPACING: f64 = 12.0;
const CHEVRON: f64 = 7.0;
const CHEVRON_STROKE: f64 = 1.5;

fn no_measure() -> Measured {
    Measured {
        size: Size::ZERO,
        baseline: None,
    }
}

impl<M: Clone + 'static> From<NavigationStack<M>> for Element<M> {
    fn from(stack: NavigationStack<M>) -> Self {
        let message = |to, enabled| {
            stack
                .on_navigate
                .as_ref()
                .filter(|_| enabled)
                .map(|on_navigate| on_navigate(to))
        };
        let back = message(Navigation::Back, stack.can_go_back);
        let forward = message(Navigation::Forward, stack.can_go_forward);
        let arrow = |to, message: &Option<M>| {
            Element::new(Arrow {
                to,
                message: message.clone(),
            })
        };
        let group = Element::new(Group {
            children: vec![
                arrow(Navigation::Back, &back),
                arrow(Navigation::Forward, &forward),
            ],
        });
        let header = Element::new(Slot {
            spec: LayoutSpec {
                width: SizeSpec::Fill,
                height: SizeSpec::Fixed(TOOLBAR_HEIGHT),
                insets: Insets {
                    top: 0.0,
                    left: TOOLBAR_INSET,
                    bottom: 0.0,
                    right: TOOLBAR_INSET,
                },
                ..LayoutSpec::new(ContainerKind::Stack {
                    axis: Axis::Horizontal,
                    spacing: TOOLBAR_SPACING,
                    align: Align::Center,
                    justify: Justify::Start,
                })
            },
            role: Some(Role::Toolbar),
            drag: true,
            children: std::iter::once(group).chain(stack.toolbar).collect(),
        });
        Element::new(Navigator {
            back,
            forward,
            children: vec![header, stack.content.fill()],
        })
    }
}

/// A container with a fixed layout and nothing of its own to show; with `drag`, the window's drag region.
struct Slot<M> {
    spec: LayoutSpec,
    role: Option<Role>,
    drag: bool,
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for Slot<M> {
    type State = DragState;

    fn layout(&self) -> LayoutSpec {
        self.spec.clone()
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) -> DragState {
        DragState::default()
    }

    fn update(&self, _: &Self, _: &mut DragState) -> Change {
        Change::None
    }

    fn measure(&self, _: &DragState, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, drag: &mut DragState, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        if self.drag {
            drag.event(event, cx)
        } else {
            Propagation::Continue
        }
    }

    fn paint(&self, _: &DragState, _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &DragState, cx: &mut SemanticsCx<'_>) {
        if let Some(role) = self.role {
            cx.role(role);
        }
    }
}

/// The toolbar row over the page; it takes the shortcuts the page leaves.
struct Navigator<M> {
    back: Option<M>,
    forward: Option<M>,
    children: Vec<Element<M>>,
}

impl<M: Clone + 'static> Widget<M> for Navigator<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            flex: Flex::grow(1.0),
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Vertical,
                spacing: 0.0,
                align: Align::Stretch,
                justify: Justify::Start,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        let Event::Key(key, _) = event else {
            return Propagation::Continue;
        };
        if !key.pressed || !key.mods.contains(Mods::PRIMARY) {
            return Propagation::Continue;
        }
        let message = match key.keysym {
            BRACKET_LEFT => &self.back,
            BRACKET_RIGHT => &self.forward,
            _ => return Propagation::Continue,
        };
        if let Some(message) = message {
            cx.emit(message.clone());
        }
        Propagation::Stop
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// The two buttons in one bordered chrome.
struct Group<M> {
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for Group<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(GROUP.width),
            height: SizeSpec::Fixed(GROUP.height),
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Horizontal,
                spacing: 0.0,
                align: Align::Stretch,
                justify: Justify::Start,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let theme = cx.env().theme.clone();
        let state = ControlState {
            enabled: true,
            window_active: cx.window_active(),
            ..ControlState::default()
        };
        let size = cx.size();
        let chrome = Rect::new(0.0, 0.0, size.width, size.height);
        let radius = theme.metrics.button_radius;
        look::fill(
            cx,
            chrome,
            radius,
            look::color(&theme, ColorRole::ControlFill, &state),
        );
        let border = look::color(&theme, ColorRole::ControlBorder, &state);
        look::stroke(
            cx,
            chrome.inset(Insets::uniform(0.5)),
            radius - 0.5,
            1.0,
            border,
        );
        let middle = Rect::new((size.width - 1.0) / 2.0, 1.0, 1.0, size.height - 2.0);
        look::fill(cx, middle, 0.0, border);
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// The back or the forward button; without a message it is disabled.
struct Arrow<M> {
    to: Navigation,
    message: Option<M>,
}

impl<M> Arrow<M> {
    /// The chevron, pointing back or forward along the reading direction.
    fn chevron(&self, area: Rect, direction: LayoutDirection) -> Shape {
        let centre = Point::new(
            area.min_x() + area.width() / 2.0,
            area.min_y() + area.height() / 2.0,
        );
        let forward = (self.to == Navigation::Forward) == (direction == LayoutDirection::Ltr);
        let reach = if forward {
            CHEVRON / 4.0
        } else {
            -CHEVRON / 4.0
        };
        Shape::Path {
            verbs: vec![
                PathVerb::MoveTo(Point::new(centre.x - reach, centre.y - CHEVRON / 2.0)),
                PathVerb::LineTo(Point::new(centre.x + reach, centre.y)),
                PathVerb::LineTo(Point::new(centre.x - reach, centre.y + CHEVRON / 2.0)),
            ],
            fill_rule: Default::default(),
        }
    }
}

impl<M: Clone + 'static> Widget<M> for Arrow<M> {
    type State = Press;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            flex: Flex::grow(1.0),
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) -> Press {
        Press::default()
    }

    fn update(&self, old: &Self, _: &mut Press) -> Change {
        if self.message.is_some() != old.message.is_some() {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &Press, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, press: &mut Press, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match press.event(event, self.message.is_some(), Keys::SpaceAndReturn, cx) {
            Outcome::Ignored => Propagation::Continue,
            Outcome::Changed => {
                cx.request_paint();
                Propagation::Stop
            }
            Outcome::Activated(_) => {
                cx.request_paint();
                if let Some(message) = &self.message {
                    cx.emit(message.clone());
                }
                Propagation::Stop
            }
        }
    }

    fn paint(&self, press: &Press, cx: &mut PaintCx<'_>) {
        let theme = cx.env().theme.clone();
        let state = ControlState {
            enabled: self.message.is_some(),
            hovered: press.hovered(),
            pressed: press.pressed(),
            focused: cx.is_focused(),
            focus_visible: cx.focus_visible(),
            window_active: cx.window_active(),
            ..ControlState::default()
        };
        let size = cx.size();
        let area = Rect::new(0.0, 0.0, size.width, size.height);
        let radius = theme.metrics.button_radius;
        if state.pressed {
            look::fill(
                cx,
                area.inset(Insets::uniform(1.0)),
                radius - 1.0,
                look::color(&theme, ColorRole::ControlFill, &state),
            );
        }
        if let Some(color) = look::color(&theme, ColorRole::ControlText, &state) {
            let direction = cx.env().direction;
            cx.push(PaintOp::Stroke {
                shape: self.chevron(area, direction),
                width: CHEVRON_STROKE,
                paint: Paint::Solid(color),
            });
        }
        paint_focus_ring(cx, area, radius);
    }

    fn semantics(&self, _: &Press, cx: &mut SemanticsCx<'_>) {
        let enabled = self.message.is_some();
        cx.role(Role::Button);
        cx.name(match self.to {
            Navigation::Back => "Back",
            Navigation::Forward => "Forward",
        });
        cx.states(AccessibleStates {
            disabled: !enabled,
            ..AccessibleStates::default()
        });
        if enabled {
            cx.focusable();
            cx.action(Action::Click);
        }
    }
}
