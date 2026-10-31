use crate::look::{self, paint_focus_ring};
use crate::{Handler, SplitLayout, SplitPane, SplitView, Stack};
use cupertino_geometry::{Insets, Point, Rect, Size};
use cupertino_ui_core::{
    AccessibleStates, AccessibleValue, Action, ActionData, Change, ControlState, Element, Event,
    EventCx, MeasureCx, PaintCx, Propagation, Role, SemanticsCx, Widget,
};
use cupertino_ui_layout::{
    Align, Axis, Constraints, ContainerKind, Flex, Justify, LayoutDirection, LayoutSpec, Measured,
    Overflow, SizeSpec,
};
use cupertino_ui_platform::{CursorShape, Keysym, MouseButton, PointerEvent};
use cupertino_ui_theme::ColorRole;
use std::cell::Cell;

const RETURN: Keysym = Keysym(0xff0d);
const KP_ENTER: Keysym = Keysym(0xff8d);
const HOME: Keysym = Keysym(0xff50);
const LEFT: Keysym = Keysym(0xff51);
const UP: Keysym = Keysym(0xff52);
const RIGHT: Keysym = Keysym(0xff53);
const DOWN: Keysym = Keysym(0xff54);
const END: Keysym = Keysym(0xff57);

/// The divider line's thickness.
const DIVIDER: f64 = 1.0;
/// The strip centred on the line that takes the pointer; narrower than the 24 pt target so the panes' edge controls
/// stay reachable. Approximate design value.
const ZONE: f64 = 6.0;
/// How far an arrow key moves the divider. Approximate design value.
const KEY_STEP: f64 = 10.0;

fn no_measure() -> Measured {
    Measured {
        size: Size::ZERO,
        baseline: None,
    }
}

impl<M: 'static> From<SplitView<M>> for Element<M> {
    fn from(view: SplitView<M>) -> Self {
        let collapsed = view.layout.collapsed;
        let pane = |content, extent| {
            Element::new(Pane {
                axis: view.axis,
                extent,
                content: Some(content),
            })
        };
        let mut panes = Vec::with_capacity(2);
        if collapsed != Some(SplitPane::Leading) {
            let extent = (collapsed.is_none()).then_some(view.layout.position.max(0.0));
            panes.push(pane(view.leading, extent));
        }
        if collapsed != Some(SplitPane::Trailing) {
            panes.push(pane(view.trailing, None));
        }
        let spacing = if collapsed.is_none() { DIVIDER } else { 0.0 };
        Element::new(Split {
            children: vec![
                Element::new(Stack {
                    axis: view.axis,
                    padding: Insets::ZERO,
                    spacing,
                    align: Align::Stretch,
                    children: panes,
                }),
                Element::new(Divider {
                    label: view.label,
                    axis: view.axis,
                    layout: view.layout,
                    min_leading: view.min_leading,
                    min_trailing: view.min_trailing,
                    on_change: view.on_change,
                }),
            ],
        })
    }
}

/// The panes and the divider over them, filling the space it is given.
struct Split<M> {
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for Split<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            flex: Flex::grow(1.0),
            overflow: Overflow::Clip,
            ..LayoutSpec::new(ContainerKind::Overlay)
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

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// One pane, clipped: the leading one at its extent, the trailing one taking what is left.
struct Pane<M> {
    axis: Axis,
    extent: Option<f64>,
    content: Option<Element<M>>,
}

impl<M: 'static> Widget<M> for Pane<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        let kind = ContainerKind::Stack {
            axis: Axis::Vertical,
            spacing: 0.0,
            align: Align::Stretch,
            justify: Justify::Start,
        };
        let along = self.extent.map_or(SizeSpec::Fill, SizeSpec::Fixed);
        let (width, height) = match self.axis {
            Axis::Horizontal => (along, SizeSpec::Fill),
            Axis::Vertical => (SizeSpec::Fill, along),
        };
        LayoutSpec {
            width,
            height,
            flex: match self.extent {
                Some(_) => Flex::NONE,
                None => Flex {
                    grow: 1.0,
                    shrink: 1.0,
                    basis: Some(0.0),
                },
            },
            overflow: Overflow::Clip,
            ..LayoutSpec::new(kind)
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        self.content.take().into_iter().collect()
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.axis != old.axis || self.extent != old.extent {
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

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// The divider: as large as the split view, so it knows the panes' room, but hit only on its strip.
struct Divider<M> {
    label: String,
    axis: Axis,
    layout: SplitLayout,
    min_leading: f64,
    min_trailing: f64,
    on_change: Option<Handler<SplitLayout, M>>,
}

#[derive(Debug, Default)]
pub struct DividerState {
    dragging: bool,
    /// The size and direction it last painted with, for hit testing, which has neither.
    painted: Cell<Option<(Size, bool)>>,
}

impl<M> Divider<M> {
    fn along(&self, point: Point) -> f64 {
        match self.axis {
            Axis::Horizontal => point.x,
            Axis::Vertical => point.y,
        }
    }

    fn extent(&self, size: Size) -> f64 {
        self.along(Point::new(size.width, size.height))
    }

    /// Whether the leading pane is on the right.
    fn mirrored(&self, direction: LayoutDirection) -> bool {
        self.axis == Axis::Horizontal && direction == LayoutDirection::Rtl
    }

    /// The farthest the leading pane can reach in `extent` and leave the trailing pane its minimum.
    fn max(&self, extent: f64) -> f64 {
        (extent - DIVIDER - self.min_trailing).max(self.min_leading)
    }

    /// Where the line starts, from the leading edge.
    fn line(&self, extent: f64) -> f64 {
        let end = (extent - DIVIDER).max(0.0);
        match self.layout.collapsed {
            Some(SplitPane::Leading) => 0.0,
            Some(SplitPane::Trailing) => end,
            None => self.layout.position.clamp(0.0, end),
        }
    }

    /// The pointer strip, in node coordinates.
    fn zone(&self, size: Size, mirrored: bool) -> Rect {
        let extent = self.extent(size);
        let line = self.line(extent);
        let start = if mirrored {
            extent - DIVIDER - line
        } else {
            line
        };
        let from = (start + DIVIDER / 2.0 - ZONE / 2.0).clamp(0.0, (extent - ZONE).max(0.0));
        match self.axis {
            Axis::Horizontal => Rect::new(from, 0.0, ZONE.min(extent), size.height),
            Axis::Vertical => Rect::new(0.0, from, size.width, ZONE.min(extent)),
        }
    }

    /// The layout a divider moved to `raw` (the leading pane's extent) in `extent` asks for.
    fn moved(&self, raw: f64, extent: f64) -> SplitLayout {
        let collapsed = if raw < self.min_leading / 2.0 {
            Some(SplitPane::Leading)
        } else if extent - DIVIDER - raw < self.min_trailing / 2.0 {
            Some(SplitPane::Trailing)
        } else {
            None
        };
        SplitLayout {
            position: match collapsed {
                Some(_) => self.layout.position,
                None => raw.clamp(self.min_leading, self.max(extent)),
            },
            collapsed,
        }
    }

    fn cursor(&self) -> CursorShape {
        match self.axis {
            Axis::Horizontal => CursorShape::ResizeEw,
            Axis::Vertical => CursorShape::ResizeNs,
        }
    }
}

impl<M: 'static> Divider<M> {
    fn set(&self, layout: SplitLayout, cx: &mut EventCx<'_, M>) {
        if layout != self.layout {
            if let Some(on_change) = &self.on_change {
                cx.emit(on_change(layout));
            }
        }
    }

    /// Moves the divider to `position`, restoring a collapsed pane.
    fn place(&self, position: f64, extent: f64, cx: &mut EventCx<'_, M>) {
        self.set(
            SplitLayout {
                position: position.clamp(self.min_leading, self.max(extent)),
                collapsed: None,
            },
            cx,
        );
    }

    fn key(&self, keysym: Keysym, extent: f64, mirrored: bool, cx: &mut EventCx<'_, M>) -> bool {
        let (grow, shrink) = match (self.axis, mirrored) {
            (Axis::Vertical, _) => (DOWN, UP),
            (Axis::Horizontal, false) => (RIGHT, LEFT),
            (Axis::Horizontal, true) => (LEFT, RIGHT),
        };
        let position = self.layout.position;
        match keysym {
            k if k == grow => self.place(position + KEY_STEP, extent, cx),
            k if k == shrink => self.place(position - KEY_STEP, extent, cx),
            HOME => self.place(self.min_leading, extent, cx),
            END => self.place(self.max(extent), extent, cx),
            RETURN | KP_ENTER => self.set(
                SplitLayout {
                    position,
                    collapsed: match self.layout.collapsed {
                        Some(_) => None,
                        None => Some(SplitPane::Leading),
                    },
                },
                cx,
            ),
            _ => return false,
        }
        true
    }
}

impl<M: 'static> Widget<M> for Divider<M> {
    type State = DividerState;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) -> DividerState {
        DividerState::default()
    }

    fn update(&self, old: &Self, _: &mut DividerState) -> Change {
        if self.axis != old.axis
            || self.layout != old.layout
            || self.min_leading != old.min_leading
            || self.min_trailing != old.min_trailing
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &DividerState, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn hit(&self, state: &DividerState, point: Point) -> bool {
        state
            .painted
            .get()
            .is_some_and(|(size, mirrored)| self.zone(size, mirrored).contains(point))
    }

    fn event(
        &self,
        state: &mut DividerState,
        event: &Event,
        cx: &mut EventCx<'_, M>,
    ) -> Propagation {
        let Some(bounds) = cx.bounds() else {
            return Propagation::Continue;
        };
        let extent = self.extent(bounds.size);
        let mirrored = self.mirrored(cx.env().direction);
        let leading_distance = |position: Point| {
            let along = self.along(position) - self.along(bounds.origin);
            let from_leading = if mirrored { extent - along } else { along };
            from_leading - DIVIDER / 2.0
        };
        match *event {
            Event::Pointer(PointerEvent::Enter { .. }, _) if cx.node() == cx.target() => {
                cx.set_cursor(Some(self.cursor()));
                Propagation::Continue
            }
            Event::Pointer(PointerEvent::Leave, _)
                if cx.node() == cx.target() && !state.dragging =>
            {
                cx.set_cursor(Some(CursorShape::Default));
                Propagation::Continue
            }
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left,
                    pressed: true,
                    ..
                },
                _,
            ) if cx.node() == cx.target() => {
                state.dragging = true;
                cx.capture_pointer();
                cx.request_focus();
                cx.request_paint();
                Propagation::Stop
            }
            Event::Pointer(PointerEvent::Motion { position }, _) if state.dragging => {
                self.set(self.moved(leading_distance(position), extent), cx);
                Propagation::Stop
            }
            Event::Pointer(PointerEvent::Button { pressed: false, .. }, _) if state.dragging => {
                state.dragging = false;
                cx.release_pointer();
                cx.request_paint();
                Propagation::Stop
            }
            Event::Cancel if state.dragging => {
                state.dragging = false;
                cx.request_paint();
                Propagation::Continue
            }
            Event::Key(key, _) if key.pressed && cx.node() == cx.target() => {
                if self.key(key.keysym, extent, mirrored, cx) {
                    Propagation::Stop
                } else {
                    Propagation::Continue
                }
            }
            Event::Action(ref request) => {
                let position = self.layout.position;
                match (request.action, &request.data) {
                    (Action::Increment, _) => self.place(position + KEY_STEP, extent, cx),
                    (Action::Decrement, _) => self.place(position - KEY_STEP, extent, cx),
                    (Action::SetValue, Some(ActionData::Number(value))) => {
                        self.place(*value, extent, cx)
                    }
                    (Action::Focus, _) => cx.request_focus(),
                    _ => return Propagation::Continue,
                }
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, state: &DividerState, cx: &mut PaintCx<'_>) {
        let size = cx.size();
        let mirrored = self.mirrored(cx.env().direction);
        state.painted.set(Some((size, mirrored)));
        let theme = cx.env().theme.clone();
        let zone = self.zone(size, mirrored);
        if self.layout.collapsed.is_none() {
            let extent = self.extent(size);
            let line = self.line(extent);
            let start = if mirrored {
                extent - DIVIDER - line
            } else {
                line
            };
            let rect = match self.axis {
                Axis::Horizontal => Rect::new(start, 0.0, DIVIDER, size.height),
                Axis::Vertical => Rect::new(0.0, start, size.width, DIVIDER),
            };
            let state = ControlState {
                enabled: true,
                window_active: cx.window_active(),
                ..ControlState::default()
            };
            look::fill(
                cx,
                rect,
                0.0,
                look::color(&theme, ColorRole::Separator, &state),
            );
        }
        paint_focus_ring(cx, zone, 0.0);
    }

    fn semantics(&self, _: &DividerState, cx: &mut SemanticsCx<'_>) {
        let size = cx.size();
        let extent = self.extent(size);
        cx.role(Role::Splitter);
        cx.name(self.label.clone());
        cx.value(AccessibleValue::Number {
            value: match self.layout.collapsed {
                Some(SplitPane::Leading) => 0.0,
                _ => self.line(extent),
            },
            min: self.min_leading.min(self.max(extent)),
            max: self.max(extent),
            step: Some(KEY_STEP),
        });
        cx.states(AccessibleStates::default());
        cx.bounds(self.zone(size, self.mirrored(cx.direction())));
        cx.focusable();
        cx.action(Action::Focus);
        cx.action(Action::Increment);
        cx.action(Action::Decrement);
        cx.action(Action::SetValue);
    }
}
