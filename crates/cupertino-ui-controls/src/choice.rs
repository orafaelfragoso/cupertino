//! Radio groups, segmented controls and tab rows: one item per choice, one Tab stop at the selected item (the first when none
//! is), arrows that choose the next or previous item and move the focus with the choice.

use crate::look::{self, paint_focus_ring};
use crate::press::{Keys, Outcome, Press};
use crate::{RadioGroup, SegmentedControl};
use cupertino_geometry::{Insets, Point, Rect, Size};
use cupertino_theme::metrics::Control;
use cupertino_ui_core::{
    AccessibleStates, AccessibleValue, Action, Change, Checked, ControlState, Element, Environment,
    Event, EventCx, FocusTarget, MeasureCx, PaintCx, Propagation, Relation, Role, SemanticsCx,
    Widget,
};
use cupertino_ui_layout::{
    Align, Axis, Constraints, ContainerKind, Justify, LayoutDirection, LayoutSpec, Measured,
};
use cupertino_ui_platform::Keysym;
use cupertino_ui_render::{Paint, PaintOp, Shape};
use cupertino_ui_theme::{ColorRole, StateVariant};
use std::rc::Rc;

const LEFT: Keysym = Keysym(0xff51);
const UP: Keysym = Keysym(0xff52);
const RIGHT: Keysym = Keysym(0xff53);
const DOWN: Keysym = Keysym(0xff54);
const HOME: Keysym = Keysym(0xff50);
const END: Keysym = Keysym(0xff57);

/// Between a radio button's circle and its label.
const LABEL_GAP: f64 = 6.0;
const DOT: f64 = 6.0;
pub(crate) const SEGMENT_RADIUS: f64 = 5.0;
const SEGMENT_INSET: f64 = 10.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Look {
    Radio,
    Segment,
    /// A segment that is a tab: Home and End reach the ends, and Return selects too.
    Tab,
}

/// A segment's width: the widest label between the insets, at least the minimum target.
pub(crate) fn segment_width(env: &Environment, labels: &[String]) -> f64 {
    let widest = labels
        .iter()
        .filter_map(|l| look::label(env, l))
        .map(|l| l.size().width)
        .fold(0.0, f64::max);
    (widest + 2.0 * SEGMENT_INSET).max(env.theme.metrics.min_target.width)
}

/// One choice. Every item of a group shares the group's labels and messages, so each can choose its neighbours.
struct Item<M> {
    look: Look,
    index: usize,
    labels: Rc<[String]>,
    selected: Option<usize>,
    axis: Axis,
    enabled: bool,
    /// The message choosing each item, in order; empty without a handler.
    messages: Rc<[M]>,
}

pub(crate) fn items<M>(
    look: Look,
    labels: &[String],
    selected: Option<usize>,
    axis: Axis,
    enabled: bool,
    on_select: Option<&crate::Handler<usize, M>>,
) -> Vec<Element<M>>
where
    M: Clone + 'static,
{
    let labels: Rc<[String]> = labels.into();
    let messages: Rc<[M]> = on_select
        .map(|f| (0..labels.len()).map(f).collect())
        .unwrap_or_default();
    (0..labels.len())
        .map(|index| {
            Element::new(Item {
                look,
                index,
                labels: Rc::clone(&labels),
                selected,
                axis,
                enabled,
                messages: Rc::clone(&messages),
            })
        })
        .collect()
}

impl<M> Item<M> {
    fn checked(&self) -> bool {
        self.selected == Some(self.index)
    }

    /// The group's one Tab stop.
    fn tab_stop(&self) -> bool {
        self.enabled && self.selected.unwrap_or(0) == self.index
    }

    fn label(&self) -> &str {
        &self.labels[self.index]
    }
}

impl<M: Clone + 'static> Item<M> {
    fn choose(&self, index: usize, cx: &mut EventCx<'_, M>) {
        if let Some(message) = self.messages.get(index) {
            cx.emit(message.clone());
        }
    }
}

impl<M: Clone + 'static> Widget<M> for Item<M> {
    type State = Press;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) -> Press {
        Press::default()
    }

    fn update(&self, old: &Self, _: &mut Press) -> Change {
        if self.labels != old.labels {
            Change::Layout
        } else if self.selected != old.selected || self.enabled != old.enabled {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &Press, _: Constraints, cx: &mut MeasureCx<'_>) -> Measured {
        let env = cx.env();
        let size = match self.look {
            Look::Radio => {
                let side = look::metric(Control::CheckBoxRadio, &env.theme);
                let label = look::label(env, self.label()).map(|l| l.size());
                Size::new(
                    side + label.map_or(0.0, |l| LABEL_GAP + l.width),
                    label.map_or(side, |l| side.max(l.height)),
                )
            }
            Look::Segment | Look::Tab => Size::new(
                segment_width(env, &self.labels),
                look::metric(Control::SegmentedControl, &env.theme),
            ),
        };
        Measured {
            size,
            baseline: None,
        }
    }

    fn target(&self, _: &Press, env: &Environment) -> Size {
        env.theme.metrics.min_target
    }

    fn event(&self, press: &mut Press, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        if let Event::Key(key, _) = event {
            if self.enabled && key.pressed && cx.node() == cx.target() {
                let rtl = cx.env().direction == LayoutDirection::Rtl;
                let (forward, back) = match (self.axis, rtl) {
                    (Axis::Horizontal, false) | (Axis::Vertical, _) => (RIGHT, LEFT),
                    (Axis::Horizontal, true) => (LEFT, RIGHT),
                };
                let count = self.labels.len();
                let tab = self.look == Look::Tab;
                let step = match key.keysym {
                    k if k == forward || k == DOWN => Some(FocusTarget::Next),
                    k if k == back || k == UP => Some(FocusTarget::Previous),
                    HOME if tab => Some(FocusTarget::First),
                    END if tab => Some(FocusTarget::Last),
                    _ => None,
                };
                if let Some(target) = step {
                    let index = match target {
                        FocusTarget::Next => (self.index + 1) % count,
                        FocusTarget::First => 0,
                        FocusTarget::Last => count - 1,
                        _ => (self.index + count - 1) % count,
                    };
                    if self.selected != Some(index) {
                        self.choose(index, cx);
                    }
                    cx.focus_item(target);
                    return Propagation::Stop;
                }
            }
        }
        let keys = match self.look {
            Look::Tab => Keys::SpaceAndReturn,
            Look::Radio | Look::Segment => Keys::Space,
        };
        match press.event(event, self.enabled, keys, cx) {
            Outcome::Ignored => Propagation::Continue,
            Outcome::Changed => {
                cx.request_paint();
                Propagation::Stop
            }
            Outcome::Activated(_) => {
                cx.request_paint();
                if !self.checked() {
                    self.choose(self.index, cx);
                }
                Propagation::Stop
            }
        }
    }

    fn paint(&self, press: &Press, cx: &mut PaintCx<'_>) {
        let env = cx.env().clone();
        let theme = &env.theme;
        let state = ControlState {
            enabled: self.enabled,
            hovered: press.hovered(),
            pressed: press.pressed(),
            focused: cx.is_focused(),
            focus_visible: cx.focus_visible(),
            selected: self.checked(),
            window_active: cx.window_active(),
            ..ControlState::default()
        };
        let size = cx.size();
        let label = look::label(&env, self.label());
        match self.look {
            Look::Radio => {
                let side = look::metric(Control::CheckBoxRadio, theme);
                let x = match env.direction {
                    LayoutDirection::Ltr => 0.0,
                    LayoutDirection::Rtl => size.width - side,
                };
                let circle = Rect::new(x, (size.height - side) / 2.0, side, side);
                look::fill(
                    cx,
                    circle,
                    side / 2.0,
                    look::color(theme, ColorRole::ControlFill, &state),
                );
                look::stroke(
                    cx,
                    circle.inset(Insets::uniform(0.5)),
                    side / 2.0 - 0.5,
                    1.0,
                    look::color(theme, ColorRole::ControlBorder, &state),
                );
                if self.checked() {
                    let dot = Rect::new(
                        circle.min_x() + (side - DOT) / 2.0,
                        circle.min_y() + (side - DOT) / 2.0,
                        DOT,
                        DOT,
                    );
                    look::fill(
                        cx,
                        dot,
                        DOT / 2.0,
                        look::color(theme, ColorRole::ControlText, &state),
                    );
                }
                if let Some(label) = label {
                    let width = label.size().width;
                    let x = match env.direction {
                        LayoutDirection::Ltr => circle.max_x() + LABEL_GAP,
                        LayoutDirection::Rtl => circle.min_x() - LABEL_GAP - width,
                    };
                    let row = Rect::new(0.0, 0.0, size.width, size.height);
                    let text = ControlState {
                        selected: false,
                        ..state
                    };
                    let origin = Point::new(x, look::top(&label, row, &env));
                    look::glyphs(
                        cx,
                        &label,
                        origin,
                        look::color(theme, ColorRole::Text, &text),
                    );
                }
                paint_focus_ring(cx, circle, side / 2.0);
            }
            Look::Segment | Look::Tab => {
                let height = look::metric(Control::SegmentedControl, theme);
                let chrome = Rect::new(0.0, (size.height - height) / 2.0, size.width, height);
                if self.checked() {
                    look::fill(
                        cx,
                        chrome.inset(Insets::uniform(1.0)),
                        SEGMENT_RADIUS - 1.0,
                        look::color(theme, ColorRole::ControlFill, &state),
                    );
                }
                if let Some(label) = label {
                    let origin = look::centred(&label, chrome, &env);
                    look::glyphs(
                        cx,
                        &label,
                        origin,
                        look::color(theme, ColorRole::ControlText, &state),
                    );
                }
                paint_focus_ring(cx, chrome, SEGMENT_RADIUS);
            }
        }
    }

    fn semantics(&self, _: &Press, cx: &mut SemanticsCx<'_>) {
        cx.name(self.label().to_owned());
        if self.look == Look::Tab {
            cx.role(Role::Tab);
            cx.item_index(self.index);
            if self.checked() {
                cx.relation_to(Relation::Controls, self.index as u64);
            }
        } else {
            cx.role(Role::RadioButton);
            cx.value(AccessibleValue::Toggle(if self.checked() {
                Checked::True
            } else {
                Checked::False
            }));
        }
        cx.states(AccessibleStates {
            disabled: !self.enabled,
            selected: self.look == Look::Tab && self.checked(),
            ..AccessibleStates::default()
        });
        cx.roving_item();
        if self.tab_stop() {
            cx.focusable();
        }
        if self.enabled {
            cx.action(Action::Click);
        }
    }
}

fn group_semantics(label: &str, cx: &mut SemanticsCx<'_>) {
    cx.role(Role::RadioGroup);
    cx.name(label.to_owned());
    cx.roving_group();
}

pub(crate) fn no_measure() -> Measured {
    Measured {
        size: Size::ZERO,
        baseline: None,
    }
}

impl<M: Clone + 'static> Widget<M> for RadioGroup<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Stack {
            axis: self.axis,
            spacing: self.spacing,
            align: Align::Start,
            justify: Justify::Start,
        })
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        items(
            Look::Radio,
            &self.options,
            self.selected,
            self.axis,
            self.enabled,
            self.on_select.as_ref(),
        )
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.axis != old.axis || self.spacing != old.spacing {
            Change::Layout
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        group_semantics(&self.label, cx);
    }
}

impl<M: Clone + 'static> Widget<M> for SegmentedControl<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Stack {
            axis: Axis::Horizontal,
            spacing: 0.0,
            align: Align::Center,
            justify: Justify::Start,
        })
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        items(
            Look::Segment,
            &self.segments,
            self.selected,
            Axis::Horizontal,
            self.enabled,
            self.on_select.as_ref(),
        )
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.selected != old.selected || self.enabled != old.enabled {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    /// The chrome around every segment and the dividers between them; each segment paints its selection and label.
    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        paint_segments(cx, &self.segments, self.selected, self.enabled);
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        group_semantics(&self.label, cx);
    }
}

/// A row of segments' shared chrome: the fill and edge around all of them and the dividers between them.
pub(crate) fn paint_segments(
    cx: &mut PaintCx<'_>,
    segments: &[String],
    selected: Option<usize>,
    enabled: bool,
) {
    let theme = cx.env().theme.clone();
    let state = ControlState {
        enabled,
        window_active: cx.window_active(),
        ..ControlState::default()
    };
    let size = cx.size();
    let height = look::metric(Control::SegmentedControl, &theme);
    let count = segments.len();
    // the segments, not the width a stretching parent gives the control
    let segment = segment_width(cx.env(), segments);
    let chrome = Rect::new(
        0.0,
        (size.height - height) / 2.0,
        segment * count as f64,
        height,
    );
    look::fill(
        cx,
        chrome,
        SEGMENT_RADIUS,
        look::color(&theme, ColorRole::ControlFill, &state),
    );
    look::stroke(
        cx,
        chrome.inset(Insets::uniform(0.5)),
        SEGMENT_RADIUS - 0.5,
        1.0,
        look::color(&theme, ColorRole::ControlBorder, &state),
    );
    let divider = theme.colors.get(ColorRole::Separator, StateVariant::NORMAL);
    if let (Some(color), true) = (divider, count > 1) {
        // none beside the chosen segment, whose fill would show a seam against it
        let beside = |i: usize| selected.is_some_and(|s| s == i || s + 1 == i);
        for i in (1..count).filter(|&i| !beside(i)) {
            cx.push(PaintOp::Fill {
                shape: Shape::Rect(Rect::new(
                    segment * i as f64,
                    chrome.min_y() + 4.0,
                    1.0,
                    height - 8.0,
                )),
                paint: Paint::Solid(color),
            });
        }
    }
}
