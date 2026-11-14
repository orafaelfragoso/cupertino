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
//! [`close_changes`] are the layers' changes for a close from rest: on the close's ease-out over
//! [`CLOSE_DURATION`], the glass grows to the enclosure's closing shape, the edge scales with it and the content and
//! the edge blur to the close's blur, everything fades out, and the mode buttons move out with the group.

use super::modes::{JOIN_CONTAINER, JOIN_SPACING};
use super::{
    edge_alpha, edge_outside, enclosure_radius, enclosure_rect, mode_rects_of, Input, EDGE_WIDTH,
};
use crate::motion::{Frame, Motion, CLOSE_BLUR, CLOSE_DURATION};
use cupertino_kit::core::{
    Change, Element, Event, EventCx, MeasureCx, PaintCx, Propagation, SemanticsCx,
    SurfaceLayerMark, Widget,
};
use cupertino_kit::geometry::{Point, Rect, Size};
use cupertino_kit::layout::{Constraints, ContainerKind, LayoutSpec, Measured, SizeSpec};
use cupertino_kit::platform::{
    CubicBezier, GlassStyle, LayerChange, LayerCurve, LayerProperty, SurfaceLayerLook,
};
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
    }
}

/// A layer that carries only the glass of its look: it draws nothing, the compositor draws the glass in its bounds.
pub struct GlassLayer {
    pub rect: Rect,
    pub look: SurfaceLayerLook,
    pub key: u64,
}

impl<M: 'static> Widget<M> for GlassLayer {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        placed(self.rect)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.rect != old.rect {
            Change::Layout
        } else if (self.look, self.key) != (old.look, old.key) {
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

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}

    fn surface_layer(&self, _: &()) -> Option<SurfaceLayerMark> {
        Some(SurfaceLayerMark {
            key: self.key,
            look: self.look,
        })
    }
}

/// A layer of a shape's thin dark edge (and a focus ring around it), at `rect` in its parent, the shape at `shape` in
/// the same coordinates, its edge left out inside the `joined` shapes as the shape paints it in place; its children
/// (a button's symbol) are placed in the same coordinates.
pub struct EdgeLayer<M> {
    pub key: u64,
    pub rect: Rect,
    pub shape: Rect,
    pub radius: f64,
    pub joined: Vec<(Rect, f64)>,
    pub focus: bool,
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
        } else if (self.key, self.shape, self.radius, self.focus)
            != (old.key, old.shape, old.radius, old.focus)
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
        edge_outside(cx, shape, radius, edge_alpha(dark, true), &joined);
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

/// A box at `rect` in its parent for a button's symbol, which centres itself in it.
pub struct SymbolBox<M> {
    pub rect: Rect,
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
        if self.rect != old.rect {
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

/// The face of mode button `index` at `button` in the window: its edge, cut inside the `joined` shapes, its focus
/// ring and `symbol`, in a layer of their own.
pub fn button_face<M: From<Input> + Clone + 'static>(
    index: usize,
    button: Rect,
    joined: Vec<(Rect, f64)>,
    focus: bool,
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
        children: vec![Element::new(SymbolBox {
            rect: inner(button),
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
