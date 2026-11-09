use crate::cx::{EventCx, MeasureCx, OverlayCx, PaintCx, SemanticsCx};
use crate::{Environment, Event, Key};
use cupertino_geometry::{Point, Size};
use cupertino_ui_layout::{Constraints, Flex, GridSpan, LayoutSpec, Measured, SizeSpec};
use std::any::{type_name, Any, TypeId};
use std::fmt;

/// What an update changed in a node.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Change {
    None,
    Paint,
    Layout,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Propagation {
    Continue,
    Stop,
}

/// A widget: one kind of node in the tree. `State` is the node's own state, which lives as long as the node and
/// never in the application's model (hover, press, caret blink, scroll velocity).
pub trait Widget<M>: 'static {
    type State: 'static;

    fn layout(&self) -> LayoutSpec;
    /// Hands the widget's child elements to the tree, which mounts them as child nodes; called once per element.
    fn take_children(&mut self) -> Vec<Element<M>> {
        Vec::new()
    }
    fn state(&self) -> Self::State;
    /// Takes over a node that `old` described.
    fn update(&self, old: &Self, state: &mut Self::State) -> Change;
    /// The size of a leaf; containers are measured by layout.
    fn measure(
        &self,
        state: &Self::State,
        constraints: Constraints,
        cx: &mut MeasureCx<'_>,
    ) -> Measured;
    fn event(&self, state: &mut Self::State, event: &Event, cx: &mut EventCx<'_, M>)
        -> Propagation;
    fn paint(&self, state: &Self::State, cx: &mut PaintCx<'_>);
    fn semantics(&self, state: &Self::State, cx: &mut SemanticsCx<'_>);
    /// How far a scroll container's content is scrolled, from the start edge: `x` grows toward the end of a line in
    /// either direction. Other widgets keep the default.
    fn scroll_offset(&self, _state: &Self::State) -> Point {
        Point::new(0.0, 0.0)
    }
    /// Whether the pointer at `point` (node coordinates, inside the node) hits the node itself rather than what lies
    /// under it, such as an overlay scroll bar that shows only its thumb. Children are hit-tested either way.
    fn hit(&self, _state: &Self::State, _point: Point) -> bool {
        true
    }
    /// The least size of the node's hit target, centred on its rect. A target larger than the rect takes no layout
    /// space; it reaches past the rect only where no other node's rect holds the point.
    fn target(&self, _state: &Self::State, _env: &Environment) -> Size {
        Size::ZERO
    }
    /// Draws over everything the node painted, on every frame while its last paint asked for it
    /// ([`PaintCx::overlay`]), without painting the node again: a caret that blinks. Placed on whole device pixels of
    /// the window ([`OverlayCx`]).
    fn overlay(&self, _state: &Self::State, _cx: &mut OverlayCx<'_>) {}
    /// Whether the node and its subtree draw in the window's raised layer: over everything else of the window, with
    /// the glass they ask for between the two, so what the rest of the window draws under them shows through their
    /// glass and does not cover it (a toolbar row over a page that scrolls under it). The layer runs from the
    /// window's top edge; the part of the window a raised node covers is obscured for the rest of it
    /// ([`PaintCx::obscured`]): scroll containers reveal what they scroll to below it and overlays such as a caret are
    /// cut from it. Where the window system has no raised layer
    /// ([`Capabilities::raised_layer`](cupertino_ui_platform::Capabilities)) the nodes draw in place, over what comes
    /// before them, and paint their own fills for the materials they ask for. A widget whose answer changes returns
    /// [`Change::Layout`] from its update.
    fn raised(&self, _state: &Self::State) -> bool {
        false
    }
}

/// An immutable description of one widget and its children.
pub struct Element<M> {
    pub(crate) key: Option<Key>,
    pub(crate) widget: Box<dyn AnyWidget<M>>,
    pub(crate) width: Option<f64>,
    pub(crate) span: Option<GridSpan>,
    pub(crate) fill: bool,
}

impl<M: 'static> Element<M> {
    pub fn new(widget: impl Widget<M>) -> Self {
        Self {
            key: None,
            widget: Box::new(widget),
            width: None,
            span: None,
            fill: false,
        }
    }

    pub fn key(mut self, key: impl Into<Key>) -> Self {
        self.key = Some(key.into());
        self
    }

    /// Holds the element at `points` wide, or at its parent's limit when that is narrower.
    pub fn width(mut self, points: f64) -> Self {
        self.width = Some(points);
        self
    }

    /// Covers `columns` by `rows` tracks when its parent is a grid.
    pub fn span(mut self, columns: usize, rows: usize) -> Self {
        self.span = Some(GridSpan { columns, rows });
        self
    }

    /// Takes all the room its parent gives it, and in a stack its share of what the other children leave.
    pub fn fill(mut self) -> Self {
        self.fill = true;
        self
    }

    pub(crate) fn layout(&self) -> LayoutSpec {
        let mut spec = self.widget.layout();
        if self.fill {
            spec.width = SizeSpec::Fill;
            spec.height = SizeSpec::Fill;
            spec.flex = Flex::grow(1.0);
        }
        if let Some(width) = self.width {
            spec.width = SizeSpec::Fixed(width);
        }
        if let Some(span) = self.span {
            spec.span = span;
        }
        spec
    }
}

impl<M> fmt::Debug for Element<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Element")
            .field("key", &self.key)
            .field("widget", &self.widget.type_name())
            .field("width", &self.width)
            .field("span", &self.span)
            .field("fill", &self.fill)
            .finish()
    }
}

/// A widget with its type erased, so nodes of every kind live in one tree. Core only pairs a widget with the state
/// it created and with an old widget of the same type, so the downcasts below cannot fail.
pub(crate) trait AnyWidget<M> {
    fn type_name(&self) -> &'static str;
    fn widget_type(&self) -> TypeId;
    fn as_any(&self) -> &dyn Any;
    fn take_children(&mut self) -> Vec<Element<M>>;
    fn layout(&self) -> LayoutSpec;
    fn new_state(&self) -> Box<dyn Any>;
    fn update(&self, old: &dyn AnyWidget<M>, state: &mut dyn Any) -> Change;
    fn measure(
        &self,
        state: &dyn Any,
        constraints: Constraints,
        cx: &mut MeasureCx<'_>,
    ) -> Measured;
    fn event(&self, state: &mut dyn Any, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation;
    fn paint(&self, state: &dyn Any, cx: &mut PaintCx<'_>);
    fn semantics(&self, state: &dyn Any, cx: &mut SemanticsCx<'_>);
    fn scroll_offset(&self, state: &dyn Any) -> Point;
    fn hit(&self, state: &dyn Any, point: Point) -> bool;
    fn target(&self, state: &dyn Any, env: &Environment) -> Size;
    fn overlay(&self, state: &dyn Any, cx: &mut OverlayCx<'_>);
    fn raised(&self, state: &dyn Any) -> bool;
}

const STATE: &str = "a node's state is the one its widget type created";

impl<M, W: Widget<M>> AnyWidget<M> for W {
    fn type_name(&self) -> &'static str {
        type_name::<W>()
    }

    fn widget_type(&self) -> TypeId {
        TypeId::of::<W>()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        Widget::take_children(self)
    }

    fn layout(&self) -> LayoutSpec {
        Widget::layout(self)
    }

    fn new_state(&self) -> Box<dyn Any> {
        Box::new(Widget::state(self))
    }

    fn update(&self, old: &dyn AnyWidget<M>, state: &mut dyn Any) -> Change {
        let old = old
            .as_any()
            .downcast_ref::<W>()
            .expect("a node is only updated by a widget of its own type");
        Widget::update(self, old, state.downcast_mut().expect(STATE))
    }

    fn measure(
        &self,
        state: &dyn Any,
        constraints: Constraints,
        cx: &mut MeasureCx<'_>,
    ) -> Measured {
        Widget::measure(self, state.downcast_ref().expect(STATE), constraints, cx)
    }

    fn event(&self, state: &mut dyn Any, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        Widget::event(self, state.downcast_mut().expect(STATE), event, cx)
    }

    fn paint(&self, state: &dyn Any, cx: &mut PaintCx<'_>) {
        Widget::paint(self, state.downcast_ref().expect(STATE), cx)
    }

    fn semantics(&self, state: &dyn Any, cx: &mut SemanticsCx<'_>) {
        Widget::semantics(self, state.downcast_ref().expect(STATE), cx)
    }

    fn scroll_offset(&self, state: &dyn Any) -> Point {
        Widget::scroll_offset(self, state.downcast_ref().expect(STATE))
    }

    fn hit(&self, state: &dyn Any, point: Point) -> bool {
        Widget::hit(self, state.downcast_ref().expect(STATE), point)
    }

    fn target(&self, state: &dyn Any, env: &Environment) -> Size {
        Widget::target(self, state.downcast_ref().expect(STATE), env)
    }

    fn overlay(&self, state: &dyn Any, cx: &mut OverlayCx<'_>) {
        Widget::overlay(self, state.downcast_ref().expect(STATE), cx)
    }

    fn raised(&self, state: &dyn Any) -> bool {
        Widget::raised(self, state.downcast_ref().expect(STATE))
    }
}
