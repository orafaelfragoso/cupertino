use crate::look::{self, paint_focus_ring};
use crate::{Handler, SplitLayout, SplitPane, SplitView, Stack};
use cupertino_geometry::{Insets, Point, Rect, Size};
use cupertino_ui_core::{
    AccessibleStates, AccessibleValue, Action, ActionData, Change, ControlState, Element, Event,
    EventCx, MeasureCx, PaintCx, Propagation, Role, SemanticsCx, Widget,
};
use cupertino_ui_layout::{
    Align, Axis, Constraints, ContainerKind, Flex, Justify, LayoutDirection, LayoutSpec, Measured,
    Overflow, SizeSpec, SplitSpec,
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
pub(crate) const DIVIDER: f64 = 1.0;
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

/// How the divider line looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DividerLook {
    /// The separator colour.
    Separator,
    /// Where a sidebar's glass meets the content: the content's own background, which the trailing pane lays under the
    /// line, with a rim one device pixel wide on the leading pane's side.
    SidebarRim,
}

/// The rim on the sidebar's side of a [`DividerLook::SidebarRim`] line: a share of white in the dark appearance, of
/// black in the light appearance.
const SIDEBAR_RIM_DARK: f32 = 0.13;
const SIDEBAR_RIM_LIGHT: f32 = 0.09;

impl<M: 'static> From<SplitView<M>> for Element<M> {
    fn from(view: SplitView<M>) -> Self {
        split_element(view, DividerLook::Separator)
    }
}

/// The element of `view` with its divider drawn as `look`.
pub(crate) fn split_element<M: 'static>(view: SplitView<M>, look: DividerLook) -> Element<M> {
    let pane = |content| {
        Element::new(Pane {
            axis: view.axis,
            content: Some(Element::fill(content)),
        })
    };
    let alone = |content| {
        Element::new(Stack {
            axis: view.axis,
            padding: Insets::ZERO,
            spacing: 0.0,
            align: Align::Stretch,
            children: vec![pane(content)],
        })
    };
    // beside a sidebar the trailing pane reaches under the line and keeps the line's width clear at its leading edge
    // (`SidebarWindow`'s content pane): the window background it lays then meets the sidebar's glass at one edge, so no
    // rounding of either to device pixels leaves a column between them with only the line over the desktop. The
    // leading pane's extent is the same either way.
    let under = match look {
        DividerLook::Separator => 0.0,
        DividerLook::SidebarRim => DIVIDER,
    };
    let panes = match view.layout.collapsed {
        None => Element::new(Panes {
            spec: SplitSpec {
                axis: view.axis,
                leading: view.layout.position.max(0.0),
                gap: DIVIDER - under,
                min_leading: view.min_leading,
                min_trailing: view.min_trailing + under,
            },
            children: vec![pane(view.leading), pane(view.trailing)],
        }),
        Some(SplitPane::Leading) => alone(view.trailing),
        Some(SplitPane::Trailing) => alone(view.leading),
    };
    Element::new(Split {
        children: vec![
            panes,
            Element::new(Divider {
                label: view.label,
                axis: view.axis,
                layout: view.layout,
                min_leading: view.min_leading,
                min_trailing: view.min_trailing,
                on_change: view.on_change,
                look,
            }),
        ],
    })
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

/// Both panes when neither is collapsed: the leading one at the model's position within the minimums, or hidden
/// when the minimums do not fit, the trailing one taking the rest.
struct Panes<M> {
    spec: SplitSpec,
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for Panes<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Split(self.spec))
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.spec != old.spec {
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

/// One pane, clipped. A pane laid out with no room along the axis is hidden, its content out of the Tab order.
struct Pane<M> {
    axis: Axis,
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
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            flex: Flex {
                grow: 1.0,
                shrink: 1.0,
                basis: Some(0.0),
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
        if self.axis != old.axis {
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
        let size = cx.size();
        let along = match self.axis {
            Axis::Horizontal => size.width,
            Axis::Vertical => size.height,
        };
        if along == 0.0 {
            cx.hidden();
        }
    }
}

/// The divider: as large as the split view, so it knows the panes' room, but hit only on its strip.
struct Divider<M> {
    label: String,
    axis: Axis,
    layout: SplitLayout,
    min_leading: f64,
    min_trailing: f64,
    on_change: Option<Handler<SplitLayout, M>>,
    look: DividerLook,
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

    /// Where the line starts, from the leading edge: after the leading pane as shown, or at the edge of a collapsed
    /// pane.
    fn line(&self, extent: f64) -> f64 {
        match self.layout.collapsed {
            Some(SplitPane::Leading) => 0.0,
            Some(SplitPane::Trailing) => (extent - DIVIDER).max(0.0),
            None => self.spec().shown(extent).unwrap_or(0.0),
        }
    }

    fn spec(&self) -> SplitSpec {
        SplitSpec {
            axis: self.axis,
            leading: self.layout.position.max(0.0),
            gap: DIVIDER,
            min_leading: self.min_leading,
            min_trailing: self.min_trailing,
        }
    }

    /// Whether the leading pane is hidden for want of room, which takes the divider with it: no line, no strip, no
    /// Tab stop. A pane the person collapsed keeps the divider at its edge.
    fn gone(&self, extent: f64) -> bool {
        self.layout.collapsed.is_none() && self.spec().shown(extent).is_none()
    }

    /// Where the keys and actions step from: the shown position, or the model's while a pane is collapsed.
    fn step_base(&self, extent: f64) -> f64 {
        match self.layout.collapsed {
            Some(_) => self.layout.position,
            None => self.line(extent),
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
        let position = self.step_base(extent);
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
            || self.look != old.look
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
        state.painted.get().is_some_and(|(size, mirrored)| {
            !self.gone(self.extent(size)) && self.zone(size, mirrored).contains(point)
        })
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
                let position = self.step_base(extent);
                match (request.action, &request.data) {
                    (Action::Increment, _) => self.place(position + KEY_STEP, extent, cx),
                    (Action::Decrement, _) => self.place(position - KEY_STEP, extent, cx),
                    (Action::SetValue, Some(ActionData::Number(value))) => {
                        self.set(self.moved(*value, extent), cx)
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
        if self.gone(self.extent(size)) {
            return;
        }
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
            match self.look {
                DividerLook::Separator => look::fill(
                    cx,
                    rect,
                    0.0,
                    look::color(&theme, ColorRole::Separator, &state),
                ),
                DividerLook::SidebarRim => {
                    // the line paints only its rim: the trailing pane lays its background under the line itself
                    // (`SidebarWindow`'s content pane), so the strip shows the content's own material, tinted as the
                    // content is, and an opaque fill here would stand out from it as a plain grey column
                    let px = (1.0 / cx.env().scale.max(1.0)).min(DIVIDER);
                    // the rim lies on the leading pane's side, which is the far side when mirrored
                    let rim = match (self.axis, mirrored) {
                        (Axis::Horizontal, false) => Rect::new(start, 0.0, px, size.height),
                        (Axis::Horizontal, true) => {
                            Rect::new(start + DIVIDER - px, 0.0, px, size.height)
                        }
                        (Axis::Vertical, false) => Rect::new(0.0, start, size.width, px),
                        (Axis::Vertical, true) => {
                            Rect::new(0.0, start + DIVIDER - px, size.width, px)
                        }
                    };
                    let color = if theme.preferences.appearance.is_dark() {
                        cupertino_ui_theme::Color::new(1.0, 1.0, 1.0, SIDEBAR_RIM_DARK)
                    } else {
                        cupertino_ui_theme::Color::new(0.0, 0.0, 0.0, SIDEBAR_RIM_LIGHT)
                    };
                    // one whole device pixel: at a fractional scale a rim between two pixels would show as a soft line two
                    // pixels wide, half over the sidebar's glass
                    cx.fill_pixels(rim, color);
                }
            }
        }
        paint_focus_ring(cx, zone, 0.0);
    }

    fn semantics(&self, _: &DividerState, cx: &mut SemanticsCx<'_>) {
        let size = cx.size();
        let extent = self.extent(size);
        if self.gone(extent) {
            cx.hidden();
            return;
        }
        cx.role(Role::Splitter);
        cx.name(self.label.clone());
        // the whole span the line can stand on, collapsed ends included, so the value never leaves it
        cx.value(AccessibleValue::Number {
            value: self.line(extent),
            min: 0.0,
            max: (extent - DIVIDER).max(0.0),
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
