//! Radio groups, segmented controls and tab rows: one item per choice, one Tab stop at the selected item (the first when none
//! is), arrows that choose the next or previous item and move the focus with the choice.

use crate::look::{self, paint_focus_ring};
use crate::press::{Keys, Outcome, Press};
use crate::{FullWidthSegmentedControl, RadioGroup, SegmentedControl};
use cupertino_geometry::{Insets, Point, Rect, Size};
use cupertino_ui_core::{
    AccessibleStates, AccessibleValue, Action, Change, Checked, ControlState, Element, Environment,
    Event, EventCx, FocusTarget, MeasureCx, PaintCx, Propagation, Relation, Role, SemanticsCx,
    Widget,
};
use cupertino_ui_layout::{
    Align, Axis, Constraints, ContainerKind, Justify, LayoutDirection, LayoutSpec, Measured,
};
use cupertino_ui_platform::Keysym;
use cupertino_ui_platform::{MouseButton, PointerEvent};
use cupertino_ui_render::{Paint, PaintOp, Shape};
use cupertino_ui_theme::{choice_mark, ColorRole, ControlFamily, StateVariant, ThemeTokens};
use std::rc::Rc;

const LEFT: Keysym = Keysym(0xff51);
const UP: Keysym = Keysym(0xff52);
const RIGHT: Keysym = Keysym(0xff53);
const DOWN: Keysym = Keysym(0xff54);
const HOME: Keysym = Keysym(0xff50);
const END: Keysym = Keysym(0xff57);

/// Between a radio button's circle and its label.
const LABEL_GAP: f64 = 6.0;
/// A chosen radio button's dot, as a share of its circle.
const DOT_SHARE: f64 = 0.375;
pub(crate) const SEGMENT_RADIUS: f64 = 5.0;
const SEGMENT_INSET: f64 = 10.0;
/// How far a divider between segments stays from the control's top and bottom edges.
const DIVIDER_INSET: f64 = 6.0;

/// The corner radius of a segmented control's chrome at the theme's size.
pub(crate) fn segment_radius(theme: &ThemeTokens) -> f64 {
    let g = look::geometry(theme, ControlFamily::Segmented);
    g.corners.radius(g.height)
}

/// The node state of one choice: how it is armed and, for a segment held by the pointer, the segment the pointer is
/// over now, which shows pressed while the chosen segment stays marked. Opaque.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChoiceState {
    press: Press,
    /// The segment under the pointer while this segment is held, counted over the whole control.
    hot: Option<usize>,
}

/// The segment under window x `x` of a row of `count` segments `segment` points wide, the segment `index` of which
/// spans `bounds`; `None` past either end of the row. Right to left, the first segment is on the right.
pub fn segment_at(
    x: f64,
    bounds: Rect,
    index: usize,
    count: usize,
    segment: f64,
    rtl: bool,
) -> Option<usize> {
    if segment <= 0.0 {
        return None;
    }
    let steps = ((x - bounds.min_x()) / segment).floor();
    let at = if rtl {
        index as f64 - steps
    } else {
        index as f64 + steps
    };
    (at >= 0.0 && at < count as f64).then_some(at as usize)
}

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
    /// The item grows to share the room of a control that fills its row.
    fill: bool,
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
    items_sized(look, labels, selected, axis, enabled, on_select, false)
}

/// [`items`], with each item growing to share the room its row gives equally when `fill`.
pub(crate) fn items_sized<M>(
    look: Look,
    labels: &[String],
    selected: Option<usize>,
    axis: Axis,
    enabled: bool,
    on_select: Option<&crate::Handler<usize, M>>,
    fill: bool,
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
                fill,
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
    /// A segment held by the pointer follows it across the control: the segment under the pointer shows pressed while
    /// the chosen one stays marked, and the release chooses the segment it ends over, once, if that is another;
    /// a release past the control's ends chooses nothing. `None` leaves the event to the press.
    fn track_segments(
        &self,
        state: &mut ChoiceState,
        event: &Event,
        cx: &mut EventCx<'_, M>,
    ) -> Option<Propagation> {
        // the segment under the pointer; none past the row's ends, above it or below it
        let hot_at = |at: cupertino_geometry::Point, cx: &EventCx<'_, M>| {
            let bounds = cx.bounds()?;
            if at.y < bounds.min_y() || at.y >= bounds.max_y() {
                return None;
            }
            let segment = if self.fill {
                bounds.width()
            } else {
                segment_width(cx.env(), &self.labels)
            };
            let rtl = cx.env().direction == LayoutDirection::Rtl;
            segment_at(at.x, bounds, self.index, self.labels.len(), segment, rtl)
        };
        match *event {
            Event::Pointer(PointerEvent::Motion { position }, _) if state.press.pointer_armed() => {
                let hot = hot_at(position, cx);
                if hot != state.hot {
                    state.hot = hot;
                    cx.request_paint();
                }
                None
            }
            Event::Pointer(
                PointerEvent::Button {
                    position,
                    button: MouseButton::Left,
                    pressed: false,
                    ..
                },
                _,
            ) if state.press.pointer_armed() => {
                let hot = hot_at(position, cx);
                state.hot = None;
                if let Some(hot) = hot.filter(|&h| h != self.index) {
                    // the release lands on another segment: this one's press ends without activating
                    state.press.disarm();
                    cx.release_pointer();
                    cx.request_paint();
                    if self.enabled && self.selected != Some(hot) {
                        self.choose(hot, cx);
                    }
                    return Some(Propagation::Stop);
                }
                None
            }
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left,
                    pressed: true,
                    ..
                },
                _,
            ) if self.enabled && !state.press.pointer_armed() => {
                state.hot = Some(self.index);
                None
            }
            Event::Cancel => {
                state.hot = None;
                None
            }
            _ => None,
        }
    }

    fn choose(&self, index: usize, cx: &mut EventCx<'_, M>) {
        if let Some(message) = self.messages.get(index) {
            cx.emit(message.clone());
        }
    }
}

impl<M: Clone + 'static> Widget<M> for Item<M> {
    type State = ChoiceState;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            flex: if self.fill {
                cupertino_ui_layout::Flex::grow(1.0)
            } else {
                cupertino_ui_layout::Flex::NONE
            },
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) -> ChoiceState {
        ChoiceState::default()
    }

    fn update(&self, old: &Self, _: &mut ChoiceState) -> Change {
        if self.labels != old.labels {
            Change::Layout
        } else if self.selected != old.selected || self.enabled != old.enabled {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &ChoiceState, _: Constraints, cx: &mut MeasureCx<'_>) -> Measured {
        let env = cx.env();
        let size = match self.look {
            Look::Radio => {
                let side = choice_mark(env.theme.metrics.size, env.theme.metrics.compact);
                let label = look::label(env, self.label()).map(|l| l.size());
                Size::new(
                    side + label.map_or(0.0, |l| LABEL_GAP + l.width),
                    label.map_or(side, |l| side.max(l.height)),
                )
            }
            Look::Segment | Look::Tab => Size::new(
                segment_width(env, &self.labels),
                look::geometry(&env.theme, ControlFamily::Segmented).height,
            ),
        };
        Measured {
            size,
            baseline: None,
        }
    }

    fn target(&self, _: &ChoiceState, env: &Environment) -> Size {
        env.theme.metrics.min_target
    }

    fn event(
        &self,
        state: &mut ChoiceState,
        event: &Event,
        cx: &mut EventCx<'_, M>,
    ) -> Propagation {
        if self.look == Look::Segment {
            if let Some(propagation) = self.track_segments(state, event, cx) {
                return propagation;
            }
        }
        let press = &mut state.press;
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

    fn paint(&self, choice: &ChoiceState, cx: &mut PaintCx<'_>) {
        let env = cx.env().clone();
        let theme = &env.theme;
        let press = &choice.press;
        // a held segment shows pressed only while the pointer is over it; the segment it is over shows instead
        let pressed = match choice.hot {
            Some(hot) => hot == self.index && press.pointer_armed(),
            None => press.pressed(),
        };
        let state = ControlState {
            enabled: self.enabled,
            hovered: press.hovered(),
            pressed,
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
                let side = choice_mark(theme.metrics.size, theme.metrics.compact);
                let x = match env.direction {
                    LayoutDirection::Ltr => 0.0,
                    LayoutDirection::Rtl => size.width - side,
                };
                let circle = Rect::new(x, (size.height - side) / 2.0, side, side);
                // unchosen a quiet well, chosen the accent with a light dot
                let fill = if self.checked() {
                    look::color(theme, ColorRole::ControlFill, &state)
                } else {
                    Some(look::quiet(theme, look::WELL, &state))
                };
                look::fill(cx, circle, side / 2.0, fill);
                if theme.preferences.appearance.is_high_contrast() {
                    look::stroke(
                        cx,
                        circle.inset(Insets::uniform(0.5)),
                        side / 2.0 - 0.5,
                        1.0,
                        look::color(theme, ColorRole::ControlBorder, &state),
                    );
                }
                if self.checked() {
                    let dot = side * DOT_SHARE;
                    let dot = Rect::new(
                        circle.min_x() + (side - dot) / 2.0,
                        circle.min_y() + (side - dot) / 2.0,
                        dot,
                        dot,
                    );
                    look::fill(
                        cx,
                        dot,
                        dot.height() / 2.0,
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
                let height = look::geometry(theme, ControlFamily::Segmented).height;
                let radius = segment_radius(theme);
                let chrome = Rect::new(0.0, (size.height - height) / 2.0, size.width, height);
                // the chosen segment stays marked, held or not, while another is highlighted; a new choice is marked
                // at once, at the release that makes it
                if self.checked() {
                    look::fill(
                        cx,
                        chrome,
                        radius,
                        look::color(
                            theme,
                            ColorRole::ControlFill,
                            &ControlState {
                                pressed: false,
                                selected: true,
                                ..state
                            },
                        ),
                    );
                }
                let state = ControlState {
                    selected: self.checked(),
                    ..state
                };
                // the segment under a held pointer: its own, or another of the control, a pressed bezel
                if let (Some(hot), true) = (choice.hot, press.pointer_armed()) {
                    if hot != self.index || !self.checked() {
                        let steps = hot as f64 - self.index as f64;
                        let dx = if env.direction == LayoutDirection::Rtl {
                            -steps
                        } else {
                            steps
                        } * size.width;
                        let pressed = ControlState {
                            pressed: true,
                            selected: false,
                            ..state
                        };
                        if self.selected != Some(hot) {
                            look::fill(
                                cx,
                                chrome.translate(dx, 0.0),
                                radius,
                                Some(look::quiet(theme, look::BEZEL, &pressed)),
                            );
                        }
                    }
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
                paint_focus_ring(cx, chrome, radius);
            }
        }
    }

    fn semantics(&self, _: &ChoiceState, cx: &mut SemanticsCx<'_>) {
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
        paint_segments(cx, &self.segments, self.selected, self.enabled, false);
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        group_semantics(&self.label, cx);
    }
}

/// The segmented control that fills its row: [`FullWidthSegmentedControl`] lays its segments out as equal shares of
/// the room, and behaves and reads as the control does.
impl<M: Clone + 'static> Widget<M> for FullWidthSegmentedControl<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: cupertino_ui_layout::SizeSpec::Fill,
            ..self.0.layout()
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        items_sized(
            Look::Segment,
            &self.0.segments,
            self.0.selected,
            Axis::Horizontal,
            self.0.enabled,
            self.0.on_select.as_ref(),
            true,
        )
    }

    fn state(&self) {}

    fn update(&self, old: &Self, state: &mut ()) -> Change {
        self.0.update(&old.0, state)
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        paint_segments(cx, &self.0.segments, self.0.selected, self.0.enabled, true);
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        group_semantics(&self.0.label, cx);
    }
}

/// A row of segments' shared chrome: the fill and edge around all of them and the dividers between them.
pub(crate) fn paint_segments(
    cx: &mut PaintCx<'_>,
    segments: &[String],
    selected: Option<usize>,
    enabled: bool,
    fill: bool,
) {
    let theme = cx.env().theme.clone();
    let state = ControlState {
        enabled,
        window_active: cx.window_active(),
        ..ControlState::default()
    };
    let size = cx.size();
    let height = look::geometry(&theme, ControlFamily::Segmented).height;
    let radius = segment_radius(&theme);
    let count = segments.len();
    // the segments, not the width a stretching parent gives the control
    let segment = if fill {
        size.width / segments.len().max(1) as f64
    } else {
        segment_width(cx.env(), segments)
    };
    let chrome = Rect::new(
        0.0,
        (size.height - height) / 2.0,
        segment * count as f64,
        height,
    );
    // a quiet bezel with no edge, which only Increase Contrast strokes
    look::fill(
        cx,
        chrome,
        radius,
        Some(look::quiet(&theme, look::BEZEL, &state)),
    );
    if theme.preferences.appearance.is_high_contrast() {
        look::stroke(
            cx,
            chrome.inset(Insets::uniform(0.5)),
            radius - 0.5,
            1.0,
            look::color(&theme, ColorRole::ControlBorder, &state),
        );
    }
    let divider = theme.colors.get(ColorRole::Separator, StateVariant::NORMAL);
    if let (Some(color), true) = (divider, count > 1) {
        // none beside the chosen segment, whose fill would show a seam against it
        let beside = |i: usize| selected.is_some_and(|s| s == i || s + 1 == i);
        let px = 1.0 / cx.env().scale.max(1.0);
        for i in (1..count).filter(|&i| !beside(i)) {
            cx.push(PaintOp::Fill {
                shape: Shape::Rect(Rect::new(
                    segment * i as f64 - px / 2.0,
                    chrome.min_y() + DIVIDER_INSET,
                    px,
                    height - 2.0 * DIVIDER_INSET,
                )),
                paint: Paint::Solid(color),
            });
        }
    }
}
