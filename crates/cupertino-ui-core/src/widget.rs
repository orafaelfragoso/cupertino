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
    /// before them, with their materials under the window as any other's, and scroll containers outside them clip
    /// what they scroll to the part the raised nodes leave, so nothing scrolls over those materials. Their paint draws
    /// after the rest of the window then, and so do their materials: they lie over any the rest of the window asks
    /// for, wherever the raised nodes are in the tree. Raised nodes take a point before the
    /// nodes they lie over, wherever they are in the tree, so a toolbar row can come first for the keyboard and
    /// assistive technology. A widget whose answer changes returns [`Change::Layout`] from its update.
    fn raised(&self, _state: &Self::State) -> bool {
        false
    }
    /// Whether a raised node ([`raised`](Self::raised)) covers what lies under it for the rest of the window. One that
    /// floats over the window's middle for a moment (an alert) answers no: it draws in the raised layer, with its glass
    /// over the window's content, but obscures nothing ([`PaintCx::obscured`]), so scroll containers keep their edges
    /// and bars where they are and, without a raised layer, clip nothing for it.
    fn obscures(&self, _state: &Self::State) -> bool {
        true
    }
    /// Whether the window whose root this widget is grows out of what it opened from: a popup that is a popover
    /// starts small at the point of its anchor nearest to it, springs to its full size, and shrinks back into that
    /// point as it closes, its glass with it. Under Reduce Motion it shows and goes at once.
    fn grows_from_anchor(&self) -> bool {
        false
    }
    /// Whether the popup whose root this widget is is a menu that draws its own material (one with a pointer, say):
    /// it fades out as it closes, as a menu on the menu surface does, taking no input from the start of the fade.
    fn closes_as_menu(&self) -> bool {
        false
    }
}

/// An immutable description of one widget and its children.
pub struct Element<M> {
    pub(crate) key: Option<Key>,
    pub(crate) widget: Box<dyn AnyWidget<M>>,
    pub(crate) width: Option<f64>,
    pub(crate) height: Option<f64>,
    pub(crate) span: Option<GridSpan>,
    pub(crate) fill: bool,
}

impl<M: 'static> Element<M> {
    pub fn new(widget: impl Widget<M>) -> Self {
        Self {
            key: None,
            widget: Box::new(widget),
            width: None,
            height: None,
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

    /// Holds the element at `points` high.
    pub fn height(mut self, points: f64) -> Self {
        self.height = Some(points);
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

    /// The element with its widget replaced by `f` of it when that widget is a `W`, keeping its key and size; any
    /// other element unchanged. A container uses it to tell a child of a known kind where it stands among the others.
    pub fn map_widget<W: Widget<M>, V: Widget<M>>(self, f: impl FnOnce(W) -> V) -> Self {
        if !self.widget.as_any().is::<W>() {
            return self;
        }
        let Self {
            key,
            widget,
            width,
            height,
            span,
            fill,
        } = self;
        let Ok(widget) = widget.into_any().downcast::<W>() else {
            unreachable!("the widget was checked to be a W");
        };
        Self {
            key,
            widget: Box::new(f(*widget)),
            width,
            height,
            span,
            fill,
        }
    }

    /// An element made only when its place needs it: when the node at its place among its siblings (found by `key`)
    /// was made by `kept` with the same `mark`, that node and its subtree stay as they are, nothing of them is compared
    /// or laid out again, and `build` is not called. `mark` stands for everything `build` reads: when what it would make
    /// changes, so must the mark. A long list keeps its rows this way while it only scrolls, and makes only those
    /// that come into view.
    pub fn kept(
        key: impl Into<Key>,
        mark: u64,
        build: impl FnOnce() -> Element<M> + 'static,
    ) -> Self {
        Element::new(Kept {
            mark,
            build: Box::new(build),
        })
        .key(key)
    }

    /// The mark of an element made by [`Element::kept`], `None` for any other.
    pub fn kept_mark(&self) -> Option<u64> {
        self.widget
            .as_any()
            .downcast_ref::<Kept<M>>()
            .map(|kept| kept.mark)
    }

    /// The element itself, or the one an element made by [`Element::kept`] builds, at its key, with its mark: what the
    /// tree makes of it when the node at its place cannot be kept.
    pub fn built(self) -> (Self, Option<u64>) {
        if !self.widget.as_any().is::<Kept<M>>() {
            return (self, None);
        }
        let key = self.key;
        let Ok(kept) = self.widget.into_any().downcast::<Kept<M>>() else {
            unreachable!("the widget was checked to be kept");
        };
        let mut element = (kept.build)();
        element.key = key;
        (element, Some(kept.mark))
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
        if let Some(height) = self.height {
            spec.height = SizeSpec::Fixed(height);
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
            .field("height", &self.height)
            .field("span", &self.span)
            .field("fill", &self.fill)
            .finish()
    }
}

/// What [`Element::kept`] makes: the mark and how to build the element, which the tree takes apart before the element
/// reaches a node. It never is a node's widget.
struct Kept<M> {
    mark: u64,
    build: Box<dyn FnOnce() -> Element<M>>,
}

impl<M: 'static> Widget<M> for Kept<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(cupertino_ui_layout::ContainerKind::Leaf)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::Layout
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// A widget with its type erased, so nodes of every kind live in one tree. Core only pairs a widget with the state
/// it created and with an old widget of the same type, so the downcasts below cannot fail.
pub(crate) trait AnyWidget<M> {
    fn type_name(&self) -> &'static str;
    fn widget_type(&self) -> TypeId;
    fn as_any(&self) -> &dyn Any;
    fn into_any(self: Box<Self>) -> Box<dyn Any>;
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
    fn obscures(&self, state: &dyn Any) -> bool;
    fn grows_from_anchor(&self) -> bool;
    fn closes_as_menu(&self) -> bool;
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

    fn into_any(self: Box<Self>) -> Box<dyn Any> {
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

    fn obscures(&self, state: &dyn Any) -> bool {
        Widget::obscures(self, state.downcast_ref().expect(STATE))
    }

    fn grows_from_anchor(&self) -> bool {
        Widget::grows_from_anchor(self)
    }

    fn closes_as_menu(&self) -> bool {
        Widget::closes_as_menu(self)
    }
}
