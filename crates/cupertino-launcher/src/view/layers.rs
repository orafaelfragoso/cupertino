//! The panel drawn as compositor layers, so the compositor runs its close without a frame of the panel's.
//!
//! While the panel rests (and through a close the compositor runs), and the window system shows surface layers, its
//! parts draw into layers of their own ([`PanelView::layered`]) with the same pixels as when they paint in place:
//!
//! * the enclosure's glass, a layer with the glass in its look and nothing of its own ([`GLASS_LAYER`]);
//! * the thin dark edge just outside the enclosure, in a layer around it whose centre is the point the enclosure grows
//!   about, so the layer's scale grows the edge with the glass ([`EDGE_LAYER`]);
//! * the content, unscaled ([`CONTENT_LAYER`]);
//! * each mode button's glass, in the field's glass group, and its face (its edge, focus ring and symbol) in a layer
//!   around it ([`button_glass_layer`], [`button_face_layer`]).
//!
//! A layer that draws strokes or content starts on whole device pixels at every quarter-step backing scale
//! ([`LAYER_GRID`]), as the window's frame does, so its pixels are the ones the window would draw. The glass layers
//! carry no pixels of their own: their glass is drawn by the compositor in their bounds, wherever those lie.
//!
//! While the compositor runs the search field's open with its mode buttons ([`field_open_changes`]) the shapes join and
//! part every frame, which a fixed frame of an edge cannot follow: the field's glass layer then carries the edge in its
//! look ([`glass_edge`]), which the compositor draws round each shape of the glass group cut by the others, as the panel
//! cuts its own edges, and neither the edge layer nor the buttons' faces draw one.
//!
//! [`close_changes`] are the layers' changes for a close from rest: on the close's ease-out over
//! [`CLOSE_DURATION`], the glass grows to the enclosure's closing shape, the edge scales with it and the content and
//! the edge blur to the close's blur, everything fades out, and the mode buttons move out with the group.

use super::modes::{JOIN_CONTAINER, JOIN_SPACING};
use super::{
    edge_alpha, edge_outside, enclosure_radius, enclosure_rect, mode_rects_of, Input, EDGE_WIDTH,
};
use crate::motion::{
    Frame, Motion, CLOSE_BLUR, CLOSE_DURATION, OPEN_DAMPING, OPEN_FADE, OPEN_RESPONSE,
};
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

/// The keys of the panel's layers: the enclosure's glass, its edge and its content.
pub const GLASS_LAYER: u64 = 1;
pub const EDGE_LAYER: u64 = 2;
pub const CONTENT_LAYER: u64 = 3;

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

/// Room around an edge for its stroke and its antialiasing, and around a button for its focus ring.
const EDGE_ROOM: f64 = EDGE_WIDTH + 1.5;
const FOCUS_ROOM: f64 = 4.0;

/// The close's blur as the deviation of a Gaussian, in points: half the blur's radius.
pub const CLOSE_DEVIATION: f64 = CLOSE_BLUR / 2.0;

/// How many values the edge's blur is carried through over the close: its deviation in the edge layer's points is
/// the close's blur over the layer's scale, which no single curve draws.
pub const EDGE_BLUR_SAMPLES: usize = 25;

fn grid_floor(v: f64) -> f64 {
    (v / LAYER_GRID).floor() * LAYER_GRID
}

fn grid_ceil(v: f64) -> f64 {
    (v / LAYER_GRID).ceil() * LAYER_GRID
}

/// The smallest rect on the grid that holds `rect` and whose centre is `pivot`.
fn centred_on(rect: Rect, pivot: Point) -> Rect {
    let x = grid_floor(pivot.x - (pivot.x - rect.x()).max(rect.max_x() - pivot.x));
    let y = grid_floor(pivot.y - (pivot.y - rect.y()).max(rect.max_y() - pivot.y));
    Rect::new(x, y, 2.0 * (pivot.x - x), 2.0 * (pivot.y - y))
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

/// The point the enclosure grows about as `frame` presents it in a window `extent` large: the window's middle across
/// (the group is placed about it) and the enclosure's middle down.
pub fn pivot(extent: Size, frame: &Frame) -> Point {
    let rect = enclosure_rect(extent, frame);
    Point::new(extent.width / 2.0, rect.y() + rect.height() / 2.0)
}

/// The enclosure's edge layer in the window: around the edge, centred on [`pivot`].
pub fn edge_layer_rect(extent: Size, frame: &Frame) -> Rect {
    centred_on(
        enclosure_rect(extent, frame).outset(EDGE_ROOM),
        pivot(extent, frame),
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
            edge_outside(cx, shape, radius, edge_alpha(dark, true), &joined);
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
/// move on it), so each property goes to its last value on that curve, but the edge's blur: in the edge layer's points
/// it is the close's blur over the layer's scale, carried through `motion`'s values from `start`.
pub fn close_changes(
    extent: Size,
    from: &Frame,
    to: &Frame,
    modes: usize,
    motion: &Motion,
    start: f64,
) -> Vec<(u64, Vec<LayerChange>)> {
    let curve = Some(LayerCurve::Timed {
        duration: CLOSE_DURATION,
        bezier: CubicBezier::EASE_OUT,
    });
    let go = |property, value: f64| LayerChange::new(property, value, curve);
    let fade = || go(LayerProperty::Opacity, 0.0);
    let (rest, end) = (enclosure_rect(extent, from), enclosure_rect(extent, to));
    let mut out = vec![
        (
            GLASS_LAYER,
            vec![
                go(LayerProperty::X, end.x()),
                go(LayerProperty::Y, end.y()),
                go(LayerProperty::Width, end.width()),
                go(LayerProperty::Height, end.height()),
                go(LayerProperty::CornerRadius, enclosure_radius(to)),
                fade(),
            ],
        ),
        (
            EDGE_LAYER,
            vec![
                go(LayerProperty::Scale, to.scale.max(to.scale_y)),
                LayerChange::through(
                    LayerProperty::ContentBlur,
                    CLOSE_DURATION,
                    edge_blur(motion, start),
                ),
                fade(),
            ],
        ),
    ];
    // the content keeps its size, centred across the enclosure and from its top edge
    let content_at = |frame: &Frame, rect: Rect| {
        Point::new(
            rect.x() + (rect.width() - frame.shape.width) / 2.0,
            rect.y(),
        )
    };
    let (a, b) = (content_at(from, rest), content_at(to, end));
    let mut content = vec![
        go(LayerProperty::ContentBlur, CLOSE_DEVIATION * to.blur),
        fade(),
    ];
    if a.x != b.x {
        content.push(go(LayerProperty::X, b.x));
    }
    if a.y != b.y {
        content.push(go(LayerProperty::Y, b.y));
    }
    out.push((CONTENT_LAYER, content));
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
/// compositor puts it on whole device pixels as the panel does; the glass's place, size and corner and the edge's
/// scale about the enclosure's middle are carried through `motion`'s own values ([`open_values`]): a spring of the
/// compositor comes to rest within a thousandth of its unit of its target, which for a scale is a third of a device
/// pixel at the panel's sides, and the panel's own spring comes to rest at its own time.
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
    let through = |property, value: &dyn Fn(&Frame) -> f64| {
        let (duration, values) = open_values(motion, start, value);
        LayerChange::through(property, duration, values)
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
    let mut edge = vec![through(LayerProperty::Scale, &|f| f.scale.max(f.scale_y))];
    opacity(&mut edge);
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
    vec![
        (GLASS_LAYER, glass),
        (EDGE_LAYER, edge),
        (CONTENT_LAYER, content),
    ]
}

/// The blur of a mode button's symbol `sharp` of the way to sharp, as the deviation of a Gaussian in points: half the
/// blur's radius the symbol is drawn with in place ([`super::modes::glyph`]).
pub fn glyph_deviation(sharp: f64) -> f64 {
    (1.0 - sharp.clamp(0.0, 1.0)) * super::modes::GLYPH_BLUR / 2.0
}

/// Mode button `index` of `count` as `frame` presents it in a window `extent` large, laid out even at a moment the
/// frame shows none yet (the open's first, before the buttons leave the field).
pub fn button_at(extent: Size, frame: &Frame, count: usize, index: usize) -> Rect {
    let mut f = *frame;
    f.modes.count = count;
    let enclosure = enclosure_rect(extent, &f);
    mode_rects_of(extent, &f, count, &enclosure)[index]
}

/// The changes of the panel's layers for the search field's open from hidden the compositor runs: from `start`, the
/// open's first moment, to `rest`, the panel at rest with its mode buttons beside the field that the layers are drawn
/// at, in a window `extent` large. The field widens on the open's spring while its end draws back on a spring of its
/// own, the buttons bud out of it on a third and their symbols sharpen after a delay: the field's glass is the
/// product of two springs, a button's place the product of the open's scale and its place along the split, its size a
/// clamped ramp of the split, so each value the layers show is carried through `motion`'s own values
/// ([`open_values`]), left out where it never changes; everything fades in on the linear [`OPEN_FADE`]. The field's
/// glass carries the edge ([`glass_edge`], set by the view while the open is the compositor's), so no frame of an edge
/// is drawn; each button's face (its symbol alone) fades in and sharpens through its opacity and content blur.
pub fn field_open_changes(
    extent: Size,
    first: &Frame,
    rest: &Frame,
    motion: &Motion,
    start: f64,
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
    let through = |out: &mut Vec<LayerChange>, property, value: &dyn Fn(&Frame) -> f64| {
        let (duration, values) = open_values(motion, start, value);
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
    let mut out = vec![(GLASS_LAYER, glass), (CONTENT_LAYER, content)];
    for index in 0..count {
        let at = |f: &Frame| button_at(extent, f, count, index);
        let mut button = Vec::new();
        through(&mut button, LayerProperty::X, &|f| at(f).x());
        through(&mut button, LayerProperty::Y, &|f| at(f).y());
        through(&mut button, LayerProperty::Width, &|f| at(f).width());
        through(&mut button, LayerProperty::Height, &|f| at(f).height());
        through(&mut button, LayerProperty::CornerRadius, &|f| {
            at(f).width() / 2.0
        });
        opacity(&mut button);
        out.push((button_glass_layer(index), button));
        // the face keeps its place about the button's centre, its symbol fading in and sharpening
        let resting = at(rest);
        let face = face_layer_rect(resting);
        let centre = |r: Rect| Point::new(r.x() + r.width() / 2.0, r.y() + r.height() / 2.0);
        let mut symbol = Vec::new();
        through(&mut symbol, LayerProperty::X, &|f| {
            face.x() + centre(at(f)).x - centre(resting).x
        });
        through(&mut symbol, LayerProperty::Y, &|f| {
            face.y() + centre(at(f)).y - centre(resting).y
        });
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

/// How many values a curve of the open carries: the most the protocol takes. Over an open of half a second a straight
/// line between two of them stands off the spring by about a thousandth of a point.
pub const OPEN_SAMPLES: usize = cupertino_kit::platform::MAX_LAYER_SAMPLES;

/// A value of the panel through an open from `start` as `motion` presents it, to where nothing moves: the open's
/// duration and the value at [`OPEN_SAMPLES`] even steps of it, the first where the open starts and the last at rest.
pub fn open_values(
    motion: &Motion,
    start: f64,
    value: &dyn Fn(&Frame) -> f64,
) -> (f64, Arc<[f32]>) {
    let duration = motion.moving_left(start).max(1e-3);
    let last = OPEN_SAMPLES - 1;
    let values = (0..OPEN_SAMPLES)
        .map(|i| value(&motion.frame(start + duration * i as f64 / last as f64)) as f32)
        .collect();
    (duration, values)
}

/// The edge's blur through the close in the edge layer's points: the close's deviation over the layer's scale, at
/// [`EDGE_BLUR_SAMPLES`] even steps of the close from `start`.
fn edge_blur(motion: &Motion, start: f64) -> Arc<[f32]> {
    let last = EDGE_BLUR_SAMPLES - 1;
    (0..EDGE_BLUR_SAMPLES)
        .map(|i| {
            let f = motion.frame(start + CLOSE_DURATION * i as f64 / last as f64);
            (CLOSE_DEVIATION * f.blur / f.scale.max(f.scale_y).max(1e-6)) as f32
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motion::{Header, Modes, Shape};

    fn rest(shape: Shape, modes: usize) -> Frame {
        Frame {
            shape,
            anchor: shape.width.max(crate::motion::GROUP_WIDTH),
            alpha: 1.0,
            scale: 1.0,
            scale_y: 1.0,
            content: 0.0,
            blur: 0.0,
            header: Header::Search,
            modes: Modes {
                count: modes,
                spread: if modes > 0 { 1.0 } else { 0.0 },
                sharp: 1.0,
            },
        }
    }

    #[test]
    fn the_layers_that_draw_start_on_the_grid_and_the_edge_s_centre_is_the_enclosure_s_pivot() {
        let extent = Size::new(761.0, 177.0);
        let frame = rest(Shape::new(640.0, 57.0, 28.5), 0);
        let edge = edge_layer_rect(extent, &frame);
        let p = pivot(extent, &frame);
        assert_eq!((edge.x() % LAYER_GRID, edge.y() % LAYER_GRID), (0.0, 0.0));
        assert!((edge.x() + edge.width() / 2.0 - p.x).abs() < 1e-9);
        assert!((edge.y() + edge.height() / 2.0 - p.y).abs() < 1e-9);
        assert_eq!(
            edge.union(&enclosure_rect(extent, &frame).outset(EDGE_ROOM)),
            edge
        );
        let button = Rect::new(581.3, 45.0, 55.0, 55.0);
        let face = face_layer_rect(button);
        assert_eq!((face.x() % LAYER_GRID, face.y() % LAYER_GRID), (0.0, 0.0));
        assert_eq!(face.union(&button.outset(FOCUS_ROOM)), face);
    }

    #[test]
    fn the_close_s_changes_end_where_the_motion_s_close_ends() {
        let shape = Shape::new(640.0, 57.0, 28.5);
        let extent = Size::new(760.0, 177.0);
        let mut motion = Motion::new(shape);
        motion.show(shape, false, 0.0);
        motion.settle(0.0);
        let from = motion.frame(1.0);
        motion.hide(1.0);
        let to = motion.frame(1.0 + CLOSE_DURATION);
        let changes = close_changes(extent, &from, &to, 0, &motion, 1.0);
        let of = |key: u64, p: LayerProperty| {
            changes
                .iter()
                .find(|(k, _)| *k == key)
                .and_then(|(_, c)| c.iter().find(|c| c.property == p))
                .map(|c| c.to)
        };
        let end = enclosure_rect(extent, &to);
        assert_eq!(of(GLASS_LAYER, LayerProperty::Width), Some(end.width()));
        assert_eq!(of(GLASS_LAYER, LayerProperty::X), Some(end.x()));
        assert_eq!(of(GLASS_LAYER, LayerProperty::Opacity), Some(0.0));
        // the search field widens and never grows taller; its content stays where it is
        assert_eq!(of(GLASS_LAYER, LayerProperty::Height), Some(57.0));
        assert_eq!(of(CONTENT_LAYER, LayerProperty::X), None);
        assert_eq!(of(CONTENT_LAYER, LayerProperty::Y), None);
        assert_eq!(
            of(CONTENT_LAYER, LayerProperty::ContentBlur),
            Some(CLOSE_DEVIATION)
        );
        assert_eq!(of(EDGE_LAYER, LayerProperty::Scale), Some(to.scale));
        // the edge's blur ends at the close's deviation in the scaled layer's points
        let blur = changes[1].1[1].samples.clone().unwrap();
        assert_eq!(blur.len(), EDGE_BLUR_SAMPLES);
        assert_eq!(blur[0], 0.0);
        assert!((f64::from(blur[EDGE_BLUR_SAMPLES - 1]) - CLOSE_DEVIATION / to.scale).abs() < 1e-5);
    }
}
