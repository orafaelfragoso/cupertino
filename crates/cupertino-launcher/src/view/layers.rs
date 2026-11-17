//! The panel drawn as compositor layers, so the compositor runs its close without a frame of the panel's.
//!
//! While the panel rests (and through a close the compositor runs), and the window system shows surface layers, its
//! parts draw into layers of their own ([`PanelView::layered`]) with the same pixels as when they paint in place:
//!
//! * the enclosure's glass, a layer with the glass in its look and nothing of its own ([`GLASS_LAYER`]), which carries
//!   the thin dark edge just outside the enclosure, drawn by the compositor along the glass ([`glass_edge`]);
//! * the content, unscaled, in two layers: the query row ([`ROW_LAYER`]) and the body below it ([`BODY_LAYER`]),
//!   which a fold fades on its own curve while the row does not, both from the content's top ([`ContentPart`]);
//! * the browser's view options button at the row's end, in a layer around it ([`OPTIONS_LAYER`]), which keeps to
//!   the row's end as the enclosure's width changes, where the rest of the row keeps to its start;
//! * each mode button's glass, in the field's glass group, and its face (its focus ring and symbol) in a layer around
//!   it ([`button_glass_layer`], [`button_face_layer`]);
//! * while the compositor runs the search field's open, the query field's caret, over the query row's layer and at its
//!   rect ([`CARET_LAYER`]), its opacity the field's own blink ([`caret_opacity`]).
//!
//! A layer that draws strokes or content starts on whole device pixels at every quarter-step backing scale
//! ([`LAYER_GRID`]), as the window's frame does, so its pixels are the ones the window would draw. The glass layers
//! carry no pixels of their own: their glass is drawn by the compositor in their bounds, wherever those lie.
//!
//! The edge is the compositor's at rest and through every transition the compositor runs, as no fixed frame of an
//! edge can follow them: while the search field opens with its mode buttons ([`field_open_changes`]) the shapes join
//! and part every frame, and the compositor draws the edge round each shape of the glass group cut by the others, as
//! the panel cuts its own edges; while a scope folds into the search field or the field unfolds into a scope
//! ([`fold_changes`]) the outline changes between a scope's continuous corners and the field's capsule. The panel
//! strokes its own edge along the glass's continuous outline, as the compositor draws it ([`super::edge_outside`]).
//!
//! [`close_changes`] are the layers' changes for a close from rest: on the close's ease-out over
//! [`CLOSE_DURATION`], the glass grows to the enclosure's closing shape, its edge following it, the content and the
//! glass's edge blur to the close's blur, everything fades out, and the mode buttons move out with the group.

use super::modes::{JOIN_CONTAINER, JOIN_SPACING};
use super::{
    edge_alpha, edge_outside, enclosure_radius, enclosure_rect, mode_rects_of, Input, EDGE_WIDTH,
};
use crate::motion::{
    Frame, Header, Motion, CLOSE_BLUR, CLOSE_DURATION, OPEN_DAMPING, OPEN_FADE, OPEN_RESPONSE,
};
use cupertino_kit::controls::CaretBlink;
use cupertino_kit::core::{
    Change, Element, Event, EventCx, MeasureCx, PaintCx, Propagation, SemanticsCx,
    SurfaceLayerMark, Widget,
};
use cupertino_kit::geometry::{Point, Rect, Size};
use cupertino_kit::layout::{Constraints, ContainerKind, LayoutSpec, Measured, SizeSpec};
use cupertino_kit::platform::{
    CubicBezier, GlassStyle, LayerChange, LayerCurve, LayerEdge, LayerProperty, SurfaceLayerLook,
};
use std::cell::Cell;
use std::sync::Arc;

/// The keys of the panel's layers: the enclosure's glass, and its content's query row and body.
pub const GLASS_LAYER: u64 = 1;
pub const ROW_LAYER: u64 = 3;
pub const BODY_LAYER: u64 = 4;
/// The key of the layer of the browser's view options button, at the query row's end.
pub const OPTIONS_LAYER: u64 = 5;
/// The key of the query field's caret's layer, over the query row's and at its rect, while the compositor runs the
/// search field's open ([`field_open_changes`]).
pub const CARET_LAYER: u64 = 6;
/// How long the caret's blink holds past the end of its layer's curve, in seconds: room for the compositor's word that
/// the layers came to rest, after which the field shows the blink itself.
pub const CARET_HOLD: f64 = 0.1;

/// The key of the layer of mode button `index`'s glass.
pub fn button_glass_layer(index: usize) -> u64 {
    16 + 2 * index as u64
}

/// The key of the layer of mode button `index`'s face: its edge, focus ring and symbol.
pub fn button_face_layer(index: usize) -> u64 {
    17 + 2 * index as u64
}

/// The grid, in points, the layers that draw pixels start on: a whole number of device pixels at every quarter-step
/// backing scale.
pub const LAYER_GRID: f64 = 4.0;

/// Room around a button for its edge's stroke and its antialiasing, and for its focus ring.
const EDGE_ROOM: f64 = EDGE_WIDTH + 1.5;
const FOCUS_ROOM: f64 = 4.0;

/// The close's blur as the deviation of a Gaussian, in points: half the blur's radius.
pub const CLOSE_DEVIATION: f64 = CLOSE_BLUR / 2.0;

fn grid_floor(v: f64) -> f64 {
    (v / LAYER_GRID).floor() * LAYER_GRID
}

fn grid_ceil(v: f64) -> f64 {
    (v / LAYER_GRID).ceil() * LAYER_GRID
}

/// The smallest rect on the grid that holds `rect`.
fn on_grid(rect: Rect) -> Rect {
    let (x, y) = (grid_floor(rect.x()), grid_floor(rect.y()));
    Rect::new(
        x,
        y,
        grid_ceil(rect.max_x()) - x,
        grid_ceil(rect.max_y()) - y,
    )
}

/// A mode button's face layer in the window, around the button at `button`.
pub fn face_layer_rect(button: Rect) -> Rect {
    on_grid(button.outset(EDGE_ROOM.max(FOCUS_ROOM)))
}

/// The look of the enclosure's glass layer: the large panel's glass in the enclosure's shape, joined with the mode
/// buttons' when they stand beside it.
pub fn glass_look(frame: &Frame) -> SurfaceLayerLook {
    SurfaceLayerLook {
        glass: Some(GlassStyle::LargePanel),
        group: if frame.modes.count > 0 {
            (JOIN_CONTAINER, JOIN_SPACING)
        } else {
            (0, 0.0)
        },
        radius: enclosure_radius(frame),
        masks: false,
        edge: LayerEdge::NONE,
    }
}

/// The look of a mode button's glass layer, `side` points across: a capsule of the large panel's glass in the field's
/// glass group.
pub fn button_glass_look(side: f64) -> SurfaceLayerLook {
    SurfaceLayerLook {
        glass: Some(GlassStyle::LargePanel),
        group: (JOIN_CONTAINER, JOIN_SPACING),
        radius: side / 2.0,
        masks: false,
        edge: LayerEdge::NONE,
    }
}

/// The thin dark edge outside the panel's glass as the compositor draws it along a glass layer's outline (the look's
/// [`SurfaceLayerLook::edge`]): [`EDGE_WIDTH`] wide in black at the opacity of the edge the panel draws over glass, in
/// the `dark` or light appearance. In a glass group it runs along the outline of the joined shape, as the panel's own
/// edges are left out where its shapes join.
pub fn glass_edge(dark: bool) -> LayerEdge {
    LayerEdge {
        width: EDGE_WIDTH,
        color: [0.0, 0.0, 0.0, edge_alpha(dark, true)],
    }
}

/// A layer that carries only the glass of its look: it draws nothing, the compositor draws the glass in its bounds.
pub struct GlassLayer {
    pub rect: Rect,
    pub look: SurfaceLayerLook,
    pub key: u64,
    /// Its glass carries the panel's edge in the appearance it is laid out in ([`glass_edge`]).
    pub edge: bool,
}

impl<M: 'static> Widget<M> for GlassLayer {
    /// Whether it is laid out in the dark appearance, for its edge.
    type State = Cell<bool>;

    fn layout(&self) -> LayoutSpec {
        // measured, so it learns the appearance before its layer is marked
        LayoutSpec {
            width: SizeSpec::Fit,
            height: SizeSpec::Fit,
            position: self.rect.origin,
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) -> Cell<bool> {
        Cell::new(false)
    }

    fn update(&self, old: &Self, _: &mut Cell<bool>) -> Change {
        if self.rect != old.rect || self.edge != old.edge {
            Change::Layout
        } else if (self.look, self.key) != (old.look, old.key) {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, dark: &Cell<bool>, _: Constraints, cx: &mut MeasureCx<'_>) -> Measured {
        dark.set(cx.env().theme.preferences.appearance.is_dark());
        leaf(self.rect)
    }

    fn event(&self, _: &mut Cell<bool>, _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn hit(&self, _: &Cell<bool>, _: Point) -> bool {
        false
    }

    fn paint(&self, _: &Cell<bool>, _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &Cell<bool>, _: &mut SemanticsCx<'_>) {}

    fn surface_layer(&self, dark: &Cell<bool>) -> Option<SurfaceLayerMark> {
        let mut look = self.look;
        if self.edge {
            look.edge = glass_edge(dark.get());
        }
        Some(SurfaceLayerMark {
            key: self.key,
            look,
        })
    }
}

/// A layer of a shape's thin dark edge (and a focus ring around it), at `rect` in its parent, the shape at `shape` in
/// the same coordinates, its edge left out inside the `joined` shapes as the shape paints it in place; its children
/// (a button's symbol) are placed in the same coordinates. Without `edge` the compositor draws the edge along the
/// glass and the layer draws none.
pub struct EdgeLayer<M> {
    pub key: u64,
    pub rect: Rect,
    pub shape: Rect,
    pub radius: f64,
    pub joined: Vec<(Rect, f64)>,
    pub focus: bool,
    pub edge: bool,
    pub children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for EdgeLayer<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        placed(self.rect)
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.rect != old.rect {
            Change::Layout
        } else if (self.key, self.shape, self.radius, self.focus, self.edge)
            != (old.key, old.shape, old.radius, old.focus, old.edge)
            || self.joined != old.joined
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        leaf(self.rect)
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn hit(&self, _: &(), _: Point) -> bool {
        false
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let dark = cx.env().theme.preferences.appearance.is_dark();
        let at = |r: Rect| r.translate(-self.rect.x(), -self.rect.y());
        let shape = at(self.shape);
        let joined: Vec<(Rect, f64)> = self.joined.iter().map(|(r, q)| (at(*r), *q)).collect();
        let radius = self.radius.min(shape.height() / 2.0).max(0.0);
        if self.edge {
            edge_outside(cx, shape, radius, edge_alpha(dark, true), &joined, true);
        }
        if self.focus {
            super::browse::focus_ring(cx, shape, radius);
        }
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}

    fn surface_layer(&self, _: &()) -> Option<SurfaceLayerMark> {
        Some(SurfaceLayerMark {
            key: self.key,
            look: SurfaceLayerLook::default(),
        })
    }
}

/// A box at `rect` in its parent for a button's symbol, which centres itself in it; on whole device pixels as one
/// picture when `on_pixels`, as a layer the compositor moves is.
pub struct SymbolBox<M> {
    pub rect: Rect,
    pub on_pixels: bool,
    pub children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for SymbolBox<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        placed(self.rect)
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.rect != old.rect || self.on_pixels != old.on_pixels {
            Change::Layout
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        leaf(self.rect)
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn hit(&self, _: &(), _: Point) -> bool {
        false
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}

    fn on_pixels(&self, _: &()) -> bool {
        self.on_pixels
    }
}

/// The browser's view options button in the content `width` points wide, in the content's points: at the query row's
/// end, centred down it.
pub fn options_button(width: f64) -> Rect {
    let side = super::browse::OPTIONS_SIZE;
    Rect::new(
        width - super::SIDE_PADDING - side,
        (super::COLLAPSED_HEIGHT - side) / 2.0,
        side,
        side,
    )
}

/// How far the view options button's layer reaches in from the row's end, in points: around the button and its focus
/// ring, on the grid, so the button keeps one place in it whatever the row's width.
const OPTIONS_REACH: f64 = 56.0;

/// The layer of the view options button in the content `width` points wide, in the content's points: around the
/// button and its focus ring, a whole number of grid steps in from the row's end and down from its top.
pub fn options_layer_rect(width: f64) -> Rect {
    let button = options_button(width);
    let top = grid_floor(button.y() - FOCUS_ROOM);
    Rect::new(
        width - OPTIONS_REACH,
        top,
        OPTIONS_REACH,
        grid_ceil(button.max_y() + FOCUS_ROOM) - top,
    )
}

/// What is placed at `rect` in its parent, its children in the same coordinates, drawn at `alpha` (as one layer when
/// under 1); a compositor layer of its own (`key`) when `layer`; on whole device pixels as one picture when
/// `on_pixels`.
pub struct PlacedLayer<M> {
    pub key: u64,
    pub layer: bool,
    pub rect: Rect,
    pub alpha: f64,
    pub on_pixels: bool,
    pub children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for PlacedLayer<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        placed(self.rect)
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.rect != old.rect || self.on_pixels != old.on_pixels {
            Change::Layout
        } else if (self.key, self.layer, self.alpha) != (old.key, old.layer, old.alpha) {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        leaf(self.rect)
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn hit(&self, _: &(), _: Point) -> bool {
        false
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        if self.alpha < 1.0 {
            cx.set_group(self.alpha.clamp(0.0, 1.0) as f32, Vec::new());
        }
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}

    fn on_pixels(&self, _: &()) -> bool {
        self.on_pixels
    }

    fn surface_layer(&self, _: &()) -> Option<SurfaceLayerMark> {
        self.layer.then_some(SurfaceLayerMark {
            key: self.key,
            look: SurfaceLayerLook::default(),
        })
    }
}

/// Room of `size` in a row, which draws nothing: where a part placed apart from the row stands in it.
pub struct Room(pub Size);

impl<M: 'static> Widget<M> for Room {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.0.width),
            height: SizeSpec::Fixed(self.0.height),
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.0 != old.0 {
            Change::Layout
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: self.0,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn hit(&self, _: &(), _: Point) -> bool {
        false
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// A part of the panel's content, `size` large at the content's top left, its children one above the other from `top`
/// down, across its width; a compositor layer of its own (`key`) when `layer`, cut to its shape as shown when `masks`
/// holds the corner radius it starts with. The body's part starts at the content's top as the row's does, with room
/// for the row above its children, so both lie on the device pixels the content lies on.
pub struct ContentPart<M> {
    pub key: u64,
    pub layer: bool,
    pub size: Size,
    pub top: f64,
    pub masks: Option<f64>,
    pub on_pixels: bool,
    pub children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for ContentPart<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.size.width.max(0.0)),
            height: SizeSpec::Fixed(self.size.height.max(0.0)),
            insets: cupertino_kit::geometry::Insets {
                top: self.top,
                left: 0.0,
                bottom: 0.0,
                right: 0.0,
            },
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: cupertino_kit::layout::Axis::Vertical,
                spacing: 0.0,
                align: cupertino_kit::layout::Align::Stretch,
                justify: cupertino_kit::layout::Justify::Start,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if (self.size, self.top, self.on_pixels) != (old.size, old.top, old.on_pixels) {
            Change::Layout
        } else if (self.key, self.layer, self.masks) != (old.key, old.layer, old.masks) {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: self.size,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    /// The body's part lies over the row's: a point reaches what each holds, never the part itself.
    fn hit(&self, _: &(), _: Point) -> bool {
        false
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}

    fn on_pixels(&self, _: &()) -> bool {
        self.on_pixels
    }

    fn surface_layer(&self, _: &()) -> Option<SurfaceLayerMark> {
        self.layer.then_some(SurfaceLayerMark {
            key: self.key,
            look: SurfaceLayerLook {
                masks: self.masks.is_some(),
                radius: self.masks.unwrap_or(0.0),
                ..SurfaceLayerLook::default()
            },
        })
    }
}

fn placed(rect: Rect) -> LayoutSpec {
    LayoutSpec {
        width: SizeSpec::Fixed(rect.width().max(0.0)),
        height: SizeSpec::Fixed(rect.height().max(0.0)),
        position: rect.origin,
        ..LayoutSpec::new(ContainerKind::Absolute)
    }
}

fn leaf(rect: Rect) -> Measured {
    Measured {
        size: rect.size,
        baseline: None,
    }
}

/// The face of mode button `index` at `button` in the window: its edge, cut inside the `joined` shapes (none when
/// the compositor draws it, without `edge`), its focus ring and `symbol`, in a layer of their own.
pub fn button_face<M: From<Input> + Clone + 'static>(
    index: usize,
    button: Rect,
    joined: Vec<(Rect, f64)>,
    focus: bool,
    edge: bool,
    symbol: Element<M>,
) -> Element<M> {
    let rect = face_layer_rect(button);
    let inner = |r: Rect| r.translate(-rect.x(), -rect.y());
    Element::new(EdgeLayer {
        key: button_face_layer(index),
        rect,
        shape: button,
        radius: button.width() / 2.0,
        joined,
        focus,
        edge,
        children: vec![Element::new(SymbolBox {
            rect: inner(button),
            on_pixels: false,
            children: vec![symbol],
        })],
    })
}

/// Where mode button `index`'s symbol lies while the buttons move, drawn in place: the face layer of the button at
/// `rest` moved as its centre moves from there to `button`'s, so the symbol keeps its place in it as it does in the
/// face layer the compositor moves; and the symbol's own box in that, the button at rest.
pub fn moving_symbol(button: Rect, rest: Rect) -> (Rect, Rect) {
    let face = face_layer_rect(rest);
    let (dx, dy) = (
        button.x() + button.width() / 2.0 - (rest.x() + rest.width() / 2.0),
        button.y() + button.height() / 2.0 - (rest.y() + rest.height() / 2.0),
    );
    (face.translate(dx, dy), rest.translate(-face.x(), -face.y()))
}

/// A mode button's symbol drawn in place while the buttons move ([`moving_symbol`]): on whole device pixels as one
/// picture, as its face layer is when the compositor moves it.
pub fn symbol_in_place<M: 'static>(button: Rect, rest: Rect, symbol: Element<M>) -> Element<M> {
    let (outer, inner) = moving_symbol(button, rest);
    Element::new(SymbolBox {
        rect: outer,
        on_pixels: true,
        children: vec![Element::new(SymbolBox {
            rect: inner,
            on_pixels: false,
            children: vec![symbol],
        })],
    })
}

/// The changes of the panel's layers for a close the compositor runs, from the panel at rest as `from` presents it
/// to `to`, the motion's last frame of the close, in a window `extent` large with `modes` mode buttons laid out. Every
/// value the close draws is linear in the close's one ease-out (the enclosure's scale, its opacity and its blur all
/// move on it), so each property goes to its last value on that curve. The glass blurs as the content does, and so the
/// edge the compositor draws along it blurs with the content, as the panel's own close blurs its edge.
pub fn close_changes(
    extent: Size,
    from: &Frame,
    to: &Frame,
    modes: usize,
) -> Vec<(u64, Vec<LayerChange>)> {
    let curve = Some(LayerCurve::Timed {
        duration: CLOSE_DURATION,
        bezier: CubicBezier::EASE_OUT,
    });
    let go = |property, value: f64| LayerChange::new(property, value, curve);
    let fade = || go(LayerProperty::Opacity, 0.0);
    let (rest, end) = (enclosure_rect(extent, from), enclosure_rect(extent, to));
    let blur = || go(LayerProperty::ContentBlur, CLOSE_DEVIATION * to.blur);
    // the glass has no content of its own: its blur softens the edge the compositor draws along it, as the content's
    // blurs the content
    let mut out = vec![(
        GLASS_LAYER,
        vec![
            go(LayerProperty::X, end.x()),
            go(LayerProperty::Y, end.y()),
            go(LayerProperty::Width, end.width()),
            go(LayerProperty::Height, end.height()),
            go(LayerProperty::CornerRadius, enclosure_radius(to)),
            blur(),
            fade(),
        ],
    )];
    // the content keeps its size, centred across the enclosure and from its top edge
    let content_at = |frame: &Frame, rect: Rect| {
        Point::new(
            rect.x() + (rect.width() - frame.shape.width) / 2.0,
            rect.y(),
        )
    };
    let (a, b) = (content_at(from, rest), content_at(to, end));
    let mut content = vec![blur(), fade()];
    if a.x != b.x {
        content.push(go(LayerProperty::X, b.x));
    }
    if a.y != b.y {
        content.push(go(LayerProperty::Y, b.y));
    }
    out.push((ROW_LAYER, content.clone()));
    if from.header == Header::Browse {
        let at = options_layer_rect(from.shape.width);
        out.push((OPTIONS_LAYER, moved_by(&content, at.x(), at.y())));
    }
    out.push((BODY_LAYER, content));
    let (here, there) = (
        mode_rects_of(extent, from, modes, &rest),
        mode_rects_of(extent, to, modes, &end),
    );
    for (index, (r0, r1)) in here.iter().zip(&there).enumerate() {
        let dx = r1.x() - r0.x();
        let mut glass = vec![go(LayerProperty::X, r1.x()), fade()];
        if r1.size != r0.size {
            glass.extend([
                go(LayerProperty::Y, r1.y()),
                go(LayerProperty::Width, r1.width()),
                go(LayerProperty::Height, r1.height()),
                go(LayerProperty::CornerRadius, r1.width() / 2.0),
            ]);
        }
        out.push((button_glass_layer(index), glass));
        out.push((
            button_face_layer(index),
            vec![go(LayerProperty::X, face_layer_rect(*r0).x() + dx), fade()],
        ));
    }
    out
}

/// The changes of the panel's layers for an open from hidden the compositor runs: from `first`, `motion`'s first frame
/// of the open at `start`, to `rest`, the panel at rest that the layers are drawn at, in a window `extent` large, for a
/// panel with no mode buttons (a scope). The open only scales the enclosure about its centre (on the open's spring)
/// while everything fades in (linearly over [`OPEN_FADE`]), and every value it draws is linear in that one scale. The
/// content's place, which keeps its size, springs from where the first frame has it on the same spring, as the
/// compositor puts it on whole device pixels as the panel does; the glass's place, size and corner are carried
/// through `motion`'s own values ([`open_frames`]): a spring of the compositor comes to rest within a thousandth of its
/// unit of its target, which for a scale is a third of a device pixel at the panel's sides, and the panel's own spring
/// comes to rest at its own time. The edge is the compositor's, along the glass.
pub fn open_changes(
    extent: Size,
    first: &Frame,
    rest: &Frame,
    motion: &Motion,
    start: f64,
) -> Vec<(u64, Vec<LayerChange>)> {
    let spring = Some(LayerCurve::Spring {
        response: OPEN_RESPONSE,
        damping: OPEN_DAMPING,
    });
    let fade = Some(LayerCurve::Timed {
        duration: OPEN_FADE,
        bezier: CubicBezier::LINEAR,
    });
    // a property set where the open starts and moved to where it rests along `curve`
    let from_to = |out: &mut Vec<LayerChange>, property, a: f64, b: f64, curve| {
        if a != b {
            out.push(LayerChange::new(property, a, None));
            out.push(LayerChange::new(property, b, curve));
        }
    };
    let opacity = |out: &mut Vec<LayerChange>| {
        out.push(LayerChange::new(LayerProperty::Opacity, first.alpha, None));
        out.push(LayerChange::new(LayerProperty::Opacity, rest.alpha, fade));
    };
    let (duration, frames) = open_frames(motion, start);
    let through = |property, value: &dyn Fn(&Frame) -> f64| {
        LayerChange::through(property, duration, values_of(&frames, value))
    };
    let rect = |frame: &Frame| enclosure_rect(extent, frame);
    let mut glass = vec![
        through(LayerProperty::X, &|f| rect(f).x()),
        through(LayerProperty::Y, &|f| rect(f).y()),
        through(LayerProperty::Width, &|f| rect(f).width()),
        through(LayerProperty::Height, &|f| rect(f).height()),
        through(LayerProperty::CornerRadius, &enclosure_radius),
    ];
    opacity(&mut glass);
    // the content keeps its size, centred across the enclosure and from its top edge
    let content_at = |frame: &Frame| {
        let r = rect(frame);
        Point::new(r.x() + (r.width() - frame.shape.width) / 2.0, r.y())
    };
    let (p, q) = (content_at(first), content_at(rest));
    let mut content = Vec::new();
    from_to(&mut content, LayerProperty::X, p.x, q.x, spring);
    from_to(&mut content, LayerProperty::Y, p.y, q.y, spring);
    opacity(&mut content);
    let mut out = vec![(GLASS_LAYER, glass), (ROW_LAYER, content.clone())];
    if rest.header == Header::Browse {
        let at = options_layer_rect(rest.shape.width);
        out.push((OPTIONS_LAYER, moved_by(&content, at.x(), at.y())));
    }
    out.push((BODY_LAYER, content));
    out
}

/// `changes` of the content's place moved by `dx` across and `dy` down: the place of a part of the content that keeps
/// that offset from its top left.
fn moved_by(changes: &[LayerChange], dx: f64, dy: f64) -> Vec<LayerChange> {
    changes
        .iter()
        .map(|c| {
            let by = match c.property {
                LayerProperty::X => dx,
                LayerProperty::Y => dy,
                _ => return c.clone(),
            };
            LayerChange {
                to: c.to + by,
                samples: c
                    .samples
                    .as_ref()
                    .map(|s| s.iter().map(|v| (f64::from(*v) + by) as f32).collect()),
                ..c.clone()
            }
        })
        .collect()
}

/// The blur of a mode button's symbol `sharp` of the way to sharp, as the deviation of a Gaussian in points: half the
/// blur's radius the symbol is drawn with in place ([`super::modes::glyph`]).
pub fn glyph_deviation(sharp: f64) -> f64 {
    (1.0 - sharp.clamp(0.0, 1.0)) * super::modes::GLYPH_BLUR / 2.0
}

/// Mode button `index` of `count` as `frame` presents it in a window `extent` large, laid out even at a moment the
/// frame shows none yet (the open's first, before the buttons leave the field).
pub fn button_at(extent: Size, frame: &Frame, count: usize, index: usize) -> Rect {
    buttons_at(extent, frame, count)[index]
}

/// The `count` mode buttons as `frame` presents them in a window `extent` large ([`button_at`] of each).
fn buttons_at(extent: Size, frame: &Frame, count: usize) -> Vec<Rect> {
    let mut f = *frame;
    f.modes.count = count;
    let enclosure = enclosure_rect(extent, &f);
    mode_rects_of(extent, &f, count, &enclosure)
}

/// The changes of the panel's layers for the search field's open from hidden the compositor runs: from `start`, the
/// open's first moment, to `rest`, the panel at rest with its mode buttons beside the field that the layers are drawn
/// at, in a window `extent` large. The field widens on the open's spring while its end draws back on a spring of its
/// own, the buttons bud out of it on a third and their symbols sharpen after a delay: the field's glass is the
/// product of two springs, a button's place the product of the open's scale and its place along the split, its size a
/// clamped ramp of the split, so each value the layers show is carried through `motion`'s own values
/// ([`open_frames`]), left out where it never changes; everything fades in on the linear [`OPEN_FADE`]. The field's
/// glass carries the edge ([`glass_edge`]), so no frame of an edge is drawn; each button's face (its symbol alone) fades in and sharpens through its opacity and content blur.
/// With `caret`, the query field's caret is a layer of its own ([`CARET_LAYER`]) that moves with the query row and
/// fades in with it, blinking as the field blinks it from the open's start ([`caret_opacity`]).
pub fn field_open_changes(
    extent: Size,
    first: &Frame,
    rest: &Frame,
    motion: &Motion,
    start: f64,
    caret: Option<CaretBlink>,
) -> Vec<(u64, Vec<LayerChange>)> {
    let count = rest.modes.count;
    let fade = Some(LayerCurve::Timed {
        duration: OPEN_FADE,
        bezier: CubicBezier::LINEAR,
    });
    let opacity = |out: &mut Vec<LayerChange>| {
        out.push(LayerChange::new(LayerProperty::Opacity, first.alpha, None));
        out.push(LayerChange::new(LayerProperty::Opacity, rest.alpha, fade));
    };
    // the panel at each sample of the open, worked out once for every value it carries
    let (duration, frames) = open_frames(motion, start);
    let changing = |out: &mut Vec<LayerChange>, property, values: Arc<[f32]>| {
        if values.iter().any(|v| *v != values[0]) {
            out.push(LayerChange::through(property, duration, values));
        }
    };
    let through = |out: &mut Vec<LayerChange>, property, value: &dyn Fn(&Frame) -> f64| {
        changing(out, property, values_of(&frames, value));
    };
    let rect = |frame: &Frame| enclosure_rect(extent, frame);
    let mut glass = Vec::new();
    through(&mut glass, LayerProperty::X, &|f| rect(f).x());
    through(&mut glass, LayerProperty::Y, &|f| rect(f).y());
    through(&mut glass, LayerProperty::Width, &|f| rect(f).width());
    through(&mut glass, LayerProperty::Height, &|f| rect(f).height());
    through(&mut glass, LayerProperty::CornerRadius, &enclosure_radius);
    opacity(&mut glass);
    // the content keeps its size, centred across the enclosure and from its top edge
    let mut content = Vec::new();
    through(&mut content, LayerProperty::X, &|f| {
        let r = rect(f);
        r.x() + (r.width() - f.shape.width) / 2.0
    });
    through(&mut content, LayerProperty::Y, &|f| rect(f).y());
    opacity(&mut content);
    let mut out = vec![(GLASS_LAYER, glass), (ROW_LAYER, content.clone())];
    // the caret moves with the row and fades in with it, blinking from the open's start as the field blinks it
    if let Some(blink) = caret {
        let mut changes: Vec<LayerChange> = content
            .iter()
            .filter(|c| c.property != LayerProperty::Opacity)
            .cloned()
            .collect();
        changes.push(caret_opacity(
            first.alpha,
            rest.alpha,
            motion.moving_left(start),
            blink,
        ));
        out.push((CARET_LAYER, changes));
    }
    out.push((BODY_LAYER, content));
    // every button's place at each sample, laid out once per sample for all of them
    let buttons: Vec<Vec<Rect>> = frames
        .iter()
        .map(|f| buttons_at(extent, f, count))
        .collect();
    for index in 0..count {
        let at = |value: &dyn Fn(Rect) -> f64| -> Arc<[f32]> {
            buttons.iter().map(|b| value(b[index]) as f32).collect()
        };
        let mut button = Vec::new();
        changing(&mut button, LayerProperty::X, at(&|r| r.x()));
        changing(&mut button, LayerProperty::Y, at(&|r| r.y()));
        changing(&mut button, LayerProperty::Width, at(&|r| r.width()));
        changing(&mut button, LayerProperty::Height, at(&|r| r.height()));
        changing(
            &mut button,
            LayerProperty::CornerRadius,
            at(&|r| r.width() / 2.0),
        );
        opacity(&mut button);
        out.push((button_glass_layer(index), button));
        // the face keeps its place about the button's centre, its symbol fading in and sharpening
        let resting = button_at(extent, rest, count, index);
        let face = face_layer_rect(resting);
        let centre = |r: Rect| Point::new(r.x() + r.width() / 2.0, r.y() + r.height() / 2.0);
        let mut symbol = Vec::new();
        changing(
            &mut symbol,
            LayerProperty::X,
            at(&|r| face.x() + centre(r).x - centre(resting).x),
        );
        changing(
            &mut symbol,
            LayerProperty::Y,
            at(&|r| face.y() + centre(r).y - centre(resting).y),
        );
        through(&mut symbol, LayerProperty::Opacity, &|f| {
            f.alpha * f.modes.sharp
        });
        through(&mut symbol, LayerProperty::ContentBlur, &|f| {
            glyph_deviation(f.modes.sharp)
        });
        out.push((button_face_layer(index), symbol));
    }
    out
}

/// The opacity of the query field's caret's layer through the search field's open, `open` seconds long: the
/// content's fade in from `first` to `rest` on the linear [`OPEN_FADE`] times the caret's `blink` from the open's
/// start, at even steps of a millisecond or so to the first moment past the open from which the blink holds for
/// [`CARET_HOLD`] ([`CaretBlink::steady_from`]), so the field's own blink shows the same once the compositor's ends.
pub fn caret_opacity(first: f64, rest: f64, open: f64, blink: CaretBlink) -> LayerChange {
    let duration = blink.steady_from(open.max(0.0), CARET_HOLD).max(1e-3);
    let count = ((duration * 1000.0).ceil() as usize + 1).clamp(2, OPEN_SAMPLES);
    let last = (count - 1) as f64;
    let values: Arc<[f32]> = (0..count)
        .map(|i| {
            let t = duration * i as f64 / last;
            let fade = first + (rest - first) * (t / OPEN_FADE).min(1.0);
            (fade * f64::from(blink.opacity(t))) as f32
        })
        .collect();
    LayerChange::through(LayerProperty::Opacity, duration, values)
}

/// The enclosure's corner radius as `frame` presents it, held to half its height as the panel draws it.
pub fn held_radius(extent: Size, frame: &Frame) -> f64 {
    enclosure_radius(frame)
        .min(enclosure_rect(extent, frame).height() / 2.0)
        .max(0.0)
}

/// The changes of the panel's layers for a fold of a scope into the search field the compositor runs, or an unfold of
/// the field into a scope (`unfold`), from `start` over `duration` (the enclosure's own motion,
/// [`Motion::shaping_left`]), in a window `extent` large with `modes` mode buttons beside the field as it starts. The
/// layers stay as they are drawn: the glass at the enclosure's place, size and corner as it starts, the row and the
/// body at the content's top. The enclosure moves on the shape spring, its top edge fixed and its sides closing in or
/// opening out, its corner the shape's held to half its height; the content keeps its size from the enclosure's left
/// edge. So the glass's place, size and corner and the content's place go through `motion`'s own values, left out
/// where they never change, and the body fades through its own curve (the content's opacity times the enclosure's),
/// which the row does not. An unfold's body is drawn as the scope rests, its layer cut to the glass
/// ([`ContentPart::masks`]): its size and corner follow the glass's. Mode buttons merge into the field through the
/// panel's own values, their glass and their faces moving with their places and gone once they merged. The edge is
/// the compositor's, along the glass ([`glass_edge`]), so no frame of an edge follows the shape.
pub fn fold_changes(
    extent: Size,
    motion: &Motion,
    start: f64,
    duration: f64,
    modes: usize,
    unfold: bool,
) -> Vec<(u64, Vec<LayerChange>)> {
    // the panel at each sample of the fold, worked out once for every value it carries
    let frames = frames_over(motion, start, duration);
    let through = |out: &mut Vec<LayerChange>, property, value: &dyn Fn(&Frame) -> f64| {
        let values = values_of(&frames, value);
        if values.iter().any(|v| *v != values[0]) {
            out.push(LayerChange::through(property, duration, values));
        }
    };
    let rect = |frame: &Frame| enclosure_rect(extent, frame);
    let mut glass = Vec::new();
    through(&mut glass, LayerProperty::X, &|f| rect(f).x());
    through(&mut glass, LayerProperty::Y, &|f| rect(f).y());
    through(&mut glass, LayerProperty::Width, &|f| rect(f).width());
    through(&mut glass, LayerProperty::Height, &|f| rect(f).height());
    through(&mut glass, LayerProperty::CornerRadius, &|f| {
        held_radius(extent, f)
    });
    // the content keeps its size, centred across the enclosure and from its top edge
    let mut row = Vec::new();
    through(&mut row, LayerProperty::X, &|f| {
        let r = rect(f);
        r.x() + (r.width() - f.shape.width) / 2.0
    });
    through(&mut row, LayerProperty::Y, &|f| rect(f).y());
    let mut body = row.clone();
    through(&mut body, LayerProperty::Opacity, &|f| f.content * f.alpha);
    if unfold {
        // cut to the glass: to the content's own size as the shape moves, which is where the panel's own drawing cuts
        // the body, on the device pixels the content lies on, with the glass's corners
        through(&mut body, LayerProperty::Width, &|f| f.shape.width);
        through(&mut body, LayerProperty::Height, &|f| {
            f.shape.height.max(super::COLLAPSED_HEIGHT)
        });
        through(&mut body, LayerProperty::CornerRadius, &|f| {
            held_radius(extent, f)
        });
    }
    let first = motion.frame(start);
    let mut out = vec![(GLASS_LAYER, glass), (ROW_LAYER, row.clone())];
    if first.header == Header::Browse {
        // the button keeps to the row's end, from where it is drawn as the fold starts
        let content_x = |f: &Frame| {
            let r = rect(f);
            r.x() + (r.width() - f.shape.width) / 2.0
        };
        let at = options_layer_rect(first.shape.width);
        let mut options: Vec<LayerChange> = moved_by(&row, 0.0, at.y())
            .into_iter()
            .filter(|c| c.property != LayerProperty::X)
            .collect();
        through(&mut options, LayerProperty::X, &|f| {
            content_x(f) + f.shape.width - OPTIONS_REACH
        });
        out.push((OPTIONS_LAYER, options));
    }
    out.push((BODY_LAYER, body));
    for index in 0..modes {
        let at = |f: &Frame| button_at(extent, f, modes, index);
        // shown while the frame lays the buttons out, at the enclosure's opacity
        let shown = |f: &Frame| if f.modes.count > index { f.alpha } else { 0.0 };
        let mut button = Vec::new();
        through(&mut button, LayerProperty::X, &|f| at(f).x());
        through(&mut button, LayerProperty::Y, &|f| at(f).y());
        through(&mut button, LayerProperty::Width, &|f| at(f).width());
        through(&mut button, LayerProperty::Height, &|f| at(f).height());
        through(&mut button, LayerProperty::CornerRadius, &|f| {
            at(f).width() / 2.0
        });
        through(&mut button, LayerProperty::Opacity, &shown);
        out.push((button_glass_layer(index), button));
        // the face keeps its place about the button's centre, from where it is drawn as the unfold starts
        let drawn = at(&first);
        let face = face_layer_rect(drawn);
        let centre = |r: Rect| Point::new(r.x() + r.width() / 2.0, r.y() + r.height() / 2.0);
        let mut symbol = Vec::new();
        through(&mut symbol, LayerProperty::X, &|f| {
            face.x() + centre(at(f)).x - centre(drawn).x
        });
        through(&mut symbol, LayerProperty::Y, &|f| {
            face.y() + centre(at(f)).y - centre(drawn).y
        });
        through(&mut symbol, LayerProperty::Opacity, &|f| {
            shown(f) * f.modes.sharp
        });
        out.push((button_face_layer(index), symbol));
    }
    out
}

/// The panel as `motion` presents it from `start` over `duration`, at [`OPEN_SAMPLES`] even steps, the first where it
/// starts and the last where it ends.
pub fn frames_over(motion: &Motion, start: f64, duration: f64) -> Vec<Frame> {
    let last = OPEN_SAMPLES - 1;
    (0..OPEN_SAMPLES)
        .map(|i| motion.frame(start + duration * i as f64 / last as f64))
        .collect()
}

/// A value of the panel at each of `frames`, as a layer's samples carry it.
pub fn values_of(frames: &[Frame], value: &dyn Fn(&Frame) -> f64) -> Arc<[f32]> {
    frames.iter().map(|f| value(f) as f32).collect()
}

/// How many values a curve of the open carries: the most the protocol takes. Over an open of half a second a straight
/// line between two of them stands off the spring by about a thousandth of a point.
pub const OPEN_SAMPLES: usize = cupertino_kit::platform::MAX_LAYER_SAMPLES;

/// The panel through an open from `start` as `motion` presents it, to where nothing moves: the open's duration and the
/// panel at [`OPEN_SAMPLES`] even steps of it, the first where the open starts and the last at rest.
pub fn open_frames(motion: &Motion, start: f64) -> (f64, Vec<Frame>) {
    let duration = motion.moving_left(start).max(1e-3);
    (duration, frames_over(motion, start, duration))
}
