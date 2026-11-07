//! Page dots: one dot per page in a row, the shown page's dot opaque. Up to [`MAX_TABS`] pages each dot is a tab of
//! one tab list and the Tab stop sits on the shown page; with more, the row is one slider of page numbers.

use crate::look::{self, paint_focus_ring};
use crate::press::{Keys, Outcome, Press};
use crate::PageDots;
use cupertino_geometry::{Rect, Size};
use cupertino_ui_core::{
    AccessibleStates, AccessibleValue, Action, ActionData, Change, ControlState, Element,
    Environment, Event, EventCx, FocusTarget, MeasureCx, PaintCx, Propagation, Role, SemanticsCx,
    Widget,
};
use cupertino_ui_layout::{
    Align, Axis, Constraints, ContainerKind, Justify, LayoutDirection, LayoutSpec, Measured,
};
use cupertino_ui_platform::Keysym;
use cupertino_ui_theme::ColorRole;
use std::rc::Rc;

const LEFT: Keysym = Keysym(0xff51);
const RIGHT: Keysym = Keysym(0xff53);

const DOT: f64 = 8.0;
/// From one dot's centre to the next.
const PITCH: f64 = 20.0;
/// Above this many pages the dots are a slider to assistive technology. Approximate design value.
const MAX_TABS: usize = 10;

/// The page Left or Right asks for, mirrored in right-to-left layouts; `None` past either end.
fn neighbour(
    current: usize,
    count: usize,
    keysym: Keysym,
    direction: LayoutDirection,
) -> Option<usize> {
    let (forward, back) = match direction {
        LayoutDirection::Ltr => (RIGHT, LEFT),
        LayoutDirection::Rtl => (LEFT, RIGHT),
    };
    match keysym {
        k if k == forward => (current + 1 < count).then_some(current + 1),
        k if k == back => current.checked_sub(1),
        _ => None,
    }
}

/// One page's dot. Every dot shares the row's messages, so each can choose its neighbours.
struct Dot<M> {
    index: usize,
    count: usize,
    current: usize,
    enabled: bool,
    /// The message choosing each page, in order; empty without a handler.
    messages: Rc<[M]>,
}

impl<M> Dot<M> {
    fn tabs(&self) -> bool {
        self.count <= MAX_TABS
    }
}

impl<M: Clone + 'static> Dot<M> {
    fn choose(&self, index: usize, cx: &mut EventCx<'_, M>) {
        if let Some(message) = self.messages.get(index) {
            cx.emit(message.clone());
        }
    }
}

impl<M: Clone + 'static> Widget<M> for Dot<M> {
    type State = Press;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) -> Press {
        Press::default()
    }

    fn update(&self, old: &Self, _: &mut Press) -> Change {
        if self.current != old.current || self.enabled != old.enabled {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &Press, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(PITCH, PITCH),
            baseline: None,
        }
    }

    fn target(&self, _: &Press, env: &Environment) -> Size {
        env.theme.metrics.min_target
    }

    fn event(&self, press: &mut Press, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        if let Event::Key(key, _) = event {
            if self.enabled && self.tabs() && key.pressed && cx.node() == cx.target() {
                let direction = cx.env().direction;
                if let Some(index) = neighbour(self.index, self.count, key.keysym, direction) {
                    self.choose(index, cx);
                    cx.focus_item(FocusTarget::Index(index));
                    return Propagation::Stop;
                }
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
                if self.index != self.current {
                    self.choose(self.index, cx);
                }
                Propagation::Stop
            }
        }
    }

    fn paint(&self, press: &Press, cx: &mut PaintCx<'_>) {
        let theme = cx.env().theme.clone();
        let state = ControlState {
            enabled: self.enabled,
            pressed: press.pressed(),
            window_active: cx.window_active(),
            ..ControlState::default()
        };
        let size = cx.size();
        let dot = Rect::new(
            (size.width - DOT) / 2.0,
            (size.height - DOT) / 2.0,
            DOT,
            DOT,
        );
        let role = if self.index == self.current {
            ColorRole::Text
        } else {
            ColorRole::PageDot
        };
        look::fill(cx, dot, DOT / 2.0, look::color(&theme, role, &state));
        if self.tabs() {
            paint_focus_ring(cx, dot, DOT / 2.0);
        }
    }

    fn semantics(&self, _: &Press, cx: &mut SemanticsCx<'_>) {
        if !self.tabs() {
            return;
        }
        cx.role(Role::Tab);
        cx.name((self.index + 1).to_string());
        cx.item_index(self.index);
        cx.states(AccessibleStates {
            disabled: !self.enabled,
            selected: self.index == self.current,
            ..AccessibleStates::default()
        });
        cx.roving_item();
        if self.enabled && self.index == self.current {
            cx.focusable();
        }
        if self.enabled {
            cx.action(Action::Click);
        }
    }
}

impl<M: Clone + 'static> PageDots<M> {
    fn choose(&self, index: usize, cx: &mut EventCx<'_, M>) {
        if index != self.current && index < self.count {
            if let Some(on_select) = &self.on_select {
                cx.emit(on_select(index));
            }
        }
    }
}

impl<M: Clone + 'static> Widget<M> for PageDots<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Stack {
            axis: Axis::Horizontal,
            spacing: 0.0,
            align: Align::Center,
            justify: Justify::Center,
        })
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        let messages: Rc<[M]> = self
            .on_select
            .as_ref()
            .map(|f| (0..self.count).map(f).collect())
            .unwrap_or_default();
        (0..self.count)
            .map(|index| {
                Element::new(Dot {
                    index,
                    count: self.count,
                    current: self.current,
                    enabled: self.enabled,
                    messages: Rc::clone(&messages),
                })
            })
            .collect()
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.current != old.current || self.enabled != old.enabled || self.label != old.label {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        if self.count <= MAX_TABS || !self.enabled {
            return Propagation::Continue;
        }
        match event {
            Event::Key(key, _) if key.pressed && cx.node() == cx.target() => {
                let direction = cx.env().direction;
                match neighbour(self.current, self.count, key.keysym, direction) {
                    Some(index) => {
                        self.choose(index, cx);
                        Propagation::Stop
                    }
                    None => Propagation::Continue,
                }
            }
            Event::Action(request) => {
                let index = match (request.action, &request.data) {
                    (Action::Increment, _) => self.current + 1,
                    (Action::Decrement, _) => self.current.saturating_sub(1),
                    (Action::SetValue, Some(ActionData::Number(page))) => {
                        (page.round().max(1.0) as usize) - 1
                    }
                    (Action::Focus, _) => {
                        cx.request_focus();
                        return Propagation::Stop;
                    }
                    _ => return Propagation::Continue,
                };
                self.choose(index.min(self.count.saturating_sub(1)), cx);
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        if self.count > MAX_TABS {
            let size = cx.size();
            let row = Rect::new(0.0, (size.height - PITCH) / 2.0, size.width, PITCH);
            paint_focus_ring(cx, row, PITCH / 2.0);
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.name(self.label.clone());
        if self.count <= MAX_TABS {
            cx.role(Role::TabList);
            cx.item_count(self.count);
            cx.roving_group();
            return;
        }
        cx.role(Role::Slider);
        cx.value(AccessibleValue::Number {
            value: (self.current + 1) as f64,
            min: 1.0,
            max: self.count as f64,
            step: Some(1.0),
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
