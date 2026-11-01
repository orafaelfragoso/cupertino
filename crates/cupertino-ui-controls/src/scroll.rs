//! ScrollView: a viewport that clips and moves its content by the model's offset, with scroll bars beside or over it.
//! Wheel, touchpad, touch and keys move the offset, which goes back to the model as a message. A gesture belongs to
//! the view that took its first event until it ends; only a new gesture passes on to the view around it.

use crate::look::{self, paint_focus_ring};
use crate::{Handler, ScrollView, Scroller};
use cupertino_animation::{KineticScroller, ScrollConfig};
use cupertino_geometry::{Point, Rect, Size};
use cupertino_ui_core::{
    AccessibleValue, Change, ControlState, Element, Event, EventCx, MeasureCx, PaintCx, Phase,
    Propagation, Role, SemanticsCx, Widget,
};
use cupertino_ui_layout::{
    Align, Axis, Constraints, ContainerKind, Flex, Justify, LayoutDirection, LayoutSpec, Measured,
    Overflow, ScrollState, SizeSpec,
};
use cupertino_ui_platform::{
    Keysym, Mods, MouseButton, PointerEvent, ScrollSource, Timestamp, TouchEvent,
};
use cupertino_ui_theme::ColorRole;
use std::cell::Cell;
use std::rc::Rc;

const HOME: Keysym = Keysym(0xff50);
const LEFT: Keysym = Keysym(0xff51);
const UP: Keysym = Keysym(0xff52);
const RIGHT: Keysym = Keysym(0xff53);
const DOWN: Keysym = Keysym(0xff54);
const PAGE_UP: Keysym = Keysym(0xff55);
const PAGE_DOWN: Keysym = Keysym(0xff56);
const END: Keysym = Keysym(0xff57);
const SPACE: Keysym = Keysym(0x20);

/// An arrow key's step, a wheel detent's when the device reports only detents, and what a page step leaves in view.
const LINE: f64 = 40.0;
/// A wheel burst ends after this long without an event.
const WHEEL_IDLE_NS: u64 = 300_000_000;
const THUMB: f64 = 12.0;
const THUMB_MIN: f64 = 24.0;
/// Between an overlay thumb and the view's edges.
const INSET: f64 = 2.0;
/// An always-visible scroll bar's track.
const TRACK: f64 = 15.0;
const SHOWN_NS: u64 = 1_000_000_000;
const FADE_NS: u64 = 300_000_000;

/// A fitted frame keeps its content's size until the room runs out, then gives way.
const SHRINK: Flex = Flex {
    shrink: 1.0,
    ..Flex::NONE
};

pub(crate) type ScrollHandler<M> = Rc<Handler<ScrollState, M>>;

impl<M: 'static> From<ScrollView<M>> for Element<M> {
    fn from(view: ScrollView<M>) -> Self {
        frame(
            view.content,
            view.state,
            (view.horizontal, view.vertical),
            view.scroller,
            Rc::new(view.on_scroll),
        )
    }
}

/// A viewport around `content` with its scroll bars; ListView builds on it too.
pub(crate) fn frame<M: 'static>(
    content: Element<M>,
    state: ScrollState,
    axes: (bool, bool),
    scroller: Scroller,
    on_scroll: ScrollHandler<M>,
) -> Element<M> {
    tracked(content, state, axes, scroller, on_scroll, true)
}

/// As [`frame`], with the bars' tracks disabled, as a disabled text box shows them, unless `enabled`.
pub(crate) fn tracked<M: 'static>(
    content: Element<M>,
    state: ScrollState,
    (horizontal, vertical): (bool, bool),
    scroller: Scroller,
    on_scroll: ScrollHandler<M>,
    enabled: bool,
) -> Element<M> {
    let viewport = Element::new(Viewport {
        content: Some(content),
        state,
        horizontal,
        vertical,
        fit: false,
        on_scroll: on_scroll.clone(),
    });
    let bar = |axis| {
        Element::new(Bar {
            axis,
            scroller,
            state,
            fit: false,
            on_scroll: on_scroll.clone(),
            enabled,
        })
    };
    let bars: Vec<Element<M>> = [(vertical, Axis::Vertical), (horizontal, Axis::Horizontal)]
        .into_iter()
        .filter(|&(on, _)| on)
        .map(|(_, axis)| bar(axis))
        .collect();
    let children = match scroller {
        // over the content, the vertical bar along the trailing edge and the horizontal one along the bottom
        Scroller::Overlay => std::iter::once(viewport).chain(bars).collect(),
        // beside the content: each bar takes its own track
        Scroller::Always => {
            let mut bars = bars.into_iter();
            let mut inner = viewport;
            if vertical {
                inner = Element::new(Frame {
                    kind: Some(Axis::Horizontal),
                    fit: false,
                    children: vec![inner, bars.next().expect("one bar per scrolled axis")],
                });
            }
            let mut children = vec![inner];
            children.extend(bars);
            return Element::new(Frame {
                kind: Some(Axis::Vertical),
                fit: false,
                children,
            });
        }
    };
    Element::new(Frame {
        kind: None,
        fit: false,
        children,
    })
}

/// A vertical viewport as tall as `content` up to the room it is given, then scrolling, with an overlay bar: nothing
/// in it fills the room, so the frame takes the content's size.
pub(crate) fn fitted<M: 'static>(
    content: Element<M>,
    state: ScrollState,
    on_scroll: ScrollHandler<M>,
) -> Element<M> {
    Element::new(Frame {
        kind: None,
        fit: true,
        children: vec![
            Element::new(Viewport {
                content: Some(content),
                state,
                horizontal: false,
                vertical: true,
                fit: true,
                on_scroll: on_scroll.clone(),
            }),
            Element::new(Bar {
                axis: Axis::Vertical,
                scroller: Scroller::Overlay,
                state,
                fit: true,
                on_scroll,
                enabled: true,
            }),
        ],
    })
}

/// Lays out a viewport and its bars: layered (`None`) or in a row or column.
pub(crate) struct Frame<M> {
    pub(crate) kind: Option<Axis>,
    /// Sized by the viewport's content instead of filling the room.
    pub(crate) fit: bool,
    pub(crate) children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for Frame<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        let kind = match self.kind {
            None => ContainerKind::Overlay,
            Some(axis) => ContainerKind::Stack {
                axis,
                spacing: 0.0,
                align: Align::Stretch,
                justify: Justify::Start,
            },
        };
        if self.fit {
            return LayoutSpec {
                flex: SHRINK,
                ..LayoutSpec::new(kind)
            };
        }
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            flex: Flex::grow(1.0),
            ..LayoutSpec::new(kind)
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.kind != old.kind || self.fit != old.fit {
            Change::Layout
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

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

struct Viewport<M> {
    content: Option<Element<M>>,
    state: ScrollState,
    horizontal: bool,
    vertical: bool,
    fit: bool,
    on_scroll: ScrollHandler<M>,
}

/// The node state of a scroll view's viewport. Opaque.
#[derive(Default)]
pub struct ViewportState {
    /// The offset last sent, ahead of the model until the next view.
    offset: Point,
    /// Whether this view owns the gesture in progress, from its first event.
    latch: Option<bool>,
    /// When the wheel burst in progress ends.
    wheel_until: Option<Timestamp>,
    /// Finger and touch drags: one scroller per axis, x then y, coasting after the lift.
    kinetic: Option<[KineticScroller; 2]>,
    touch: Option<(i32, Point)>,
}

fn seconds(t: Timestamp) -> f64 {
    t.0 as f64 / 1e9
}

impl<M: 'static> Viewport<M> {
    /// How far the content moves toward its end along x for a pointer or finger moving by `dx`: the offset counts
    /// from the start edge, which is the right in right-to-left.
    fn along_x(&self, direction: LayoutDirection, dx: f64) -> f64 {
        match direction {
            LayoutDirection::Ltr => dx,
            LayoutDirection::Rtl => -dx,
        }
    }

    fn scroll_to(&self, state: &mut ViewportState, offset: Point, cx: &mut EventCx<'_, M>) {
        let next = ScrollState {
            offset: Point::new(
                if self.horizontal { offset.x } else { 0.0 },
                if self.vertical { offset.y } else { 0.0 },
            ),
            ..self.state
        }
        .clamped();
        if next.offset != state.offset {
            state.offset = next.offset;
            cx.emit((self.on_scroll)(next));
        }
    }

    /// Whether moving by `delta` along the scrolled axes changes the offset at all.
    fn can_move(&self, state: &ViewportState, delta: Point) -> bool {
        let max = self.state.max_offset();
        let room = |on: bool, d: f64, at: f64, max: f64| {
            on && ((d > 0.0 && at < max) || (d < 0.0 && at > 0.0))
        };
        room(self.horizontal, delta.x, state.offset.x, max.x)
            || room(self.vertical, delta.y, state.offset.y, max.y)
    }

    fn kinetic(&self, state: &ViewportState, now: f64) -> [KineticScroller; 2] {
        let max = self.state.max_offset();
        let axis = |len: f64, max: f64, at: f64| {
            let mut k = KineticScroller::new(
                ScrollConfig {
                    dimension: len,
                    ..ScrollConfig::default()
                },
                0.0,
                max,
                at,
            );
            k.begin_drag(now);
            k
        };
        [
            axis(self.state.viewport.width, max.x, state.offset.x),
            axis(self.state.viewport.height, max.y, state.offset.y),
        ]
    }

    fn drag(&self, state: &mut ViewportState, delta: Point, now: f64, cx: &mut EventCx<'_, M>) {
        let kinetic = match state.kinetic.take() {
            Some(k) => k,
            None => self.kinetic(state, now),
        };
        let mut kinetic = kinetic;
        kinetic[0].drag(delta.x, now);
        kinetic[1].drag(delta.y, now);
        let offset = Point::new(kinetic[0].position(now), kinetic[1].position(now));
        state.kinetic = Some(kinetic);
        self.scroll_to(state, offset, cx);
    }

    /// The fingers lifted: the content coasts on animation frames, or stops at once under Reduce Motion.
    fn lift(&self, state: &mut ViewportState, now: f64, cx: &mut EventCx<'_, M>) {
        match &mut state.kinetic {
            Some(kinetic) if !cx.env().theme.motion.reduce => {
                kinetic[0].end_drag(now);
                kinetic[1].end_drag(now);
                cx.request_animation_frame();
            }
            _ => {
                state.kinetic = None;
                state.latch = None;
            }
        }
    }

    fn wheel(
        &self,
        state: &mut ViewportState,
        delta: Point,
        value120: Option<(i32, i32)>,
        source: ScrollSource,
        stop: bool,
        cx: &mut EventCx<'_, M>,
    ) -> Propagation {
        let now = cx.now();
        let direction = cx.env().direction;
        let delta = match (delta, value120) {
            (d, Some((x, y))) if d == Point::ZERO => {
                Point::new(f64::from(x) / 120.0 * LINE, f64::from(y) / 120.0 * LINE)
            }
            (d, _) => d,
        };
        let delta = Point::new(self.along_x(direction, delta.x), delta.y);
        if source == ScrollSource::Wheel && state.wheel_until.is_some_and(|until| now >= until) {
            state.latch = None;
        }
        let mine = match state.latch {
            Some(mine) => mine,
            // a gesture's owner is the first view its first movement can move
            None if delta == Point::ZERO => return Propagation::Continue,
            None => {
                let mine = self.can_move(state, delta);
                state.latch = Some(mine);
                mine
            }
        };
        match source {
            ScrollSource::Wheel => {
                let until = Timestamp(now.0 + WHEEL_IDLE_NS);
                state.wheel_until = Some(until);
                cx.wake_at(until);
            }
            ScrollSource::Finger | ScrollSource::Continuous if stop => {
                if mine {
                    self.lift(state, seconds(now), cx);
                } else {
                    state.latch = None;
                }
                return if mine {
                    Propagation::Stop
                } else {
                    Propagation::Continue
                };
            }
            ScrollSource::Finger | ScrollSource::Continuous => {}
        }
        if !mine {
            return Propagation::Continue;
        }
        if source == ScrollSource::Wheel {
            let offset = Point::new(state.offset.x + delta.x, state.offset.y + delta.y);
            self.scroll_to(state, offset, cx);
        } else {
            self.drag(state, delta, seconds(now), cx);
        }
        Propagation::Stop
    }

    fn key(
        &self,
        state: &mut ViewportState,
        keysym: Keysym,
        mods: Mods,
        cx: &mut EventCx<'_, M>,
    ) -> bool {
        let page = (self.state.viewport.height - LINE).max(LINE);
        let at = state.offset;
        let rtl = cx.env().direction == LayoutDirection::Rtl;
        let (forward, back) = if rtl { (LEFT, RIGHT) } else { (RIGHT, LEFT) };
        let offset = match keysym {
            UP if self.vertical => Point::new(at.x, at.y - LINE),
            DOWN if self.vertical => Point::new(at.x, at.y + LINE),
            k if k == forward && self.horizontal => Point::new(at.x + LINE, at.y),
            k if k == back && self.horizontal => Point::new(at.x - LINE, at.y),
            PAGE_UP if self.vertical => Point::new(at.x, at.y - page),
            PAGE_DOWN if self.vertical => Point::new(at.x, at.y + page),
            SPACE if self.vertical && mods.contains(Mods::SHIFT) => Point::new(at.x, at.y - page),
            SPACE if self.vertical => Point::new(at.x, at.y + page),
            HOME => Point::ZERO,
            END => self.state.max_offset(),
            _ => return false,
        };
        self.scroll_to(state, offset, cx);
        true
    }
}

impl<M: 'static> Widget<M> for Viewport<M> {
    type State = ViewportState;

    fn layout(&self) -> LayoutSpec {
        let (size, flex) = if self.fit {
            (SizeSpec::Fit, SHRINK)
        } else {
            (SizeSpec::Fill, Flex::grow(1.0))
        };
        LayoutSpec {
            width: size,
            height: size,
            flex,
            overflow: Overflow::Clip,
            ..LayoutSpec::new(ContainerKind::Scroll {
                horizontal: self.horizontal,
                vertical: self.vertical,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        self.content.take().into_iter().collect()
    }

    fn state(&self) -> ViewportState {
        ViewportState {
            offset: self.state.offset,
            ..ViewportState::default()
        }
    }

    fn update(&self, old: &Self, state: &mut ViewportState) -> Change {
        state.offset = self.state.offset;
        if (self.horizontal, self.vertical, self.fit) != (old.horizontal, old.vertical, old.fit)
            || self.state.offset != old.state.offset
        {
            Change::Layout
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &ViewportState, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(
        &self,
        state: &mut ViewportState,
        event: &Event,
        cx: &mut EventCx<'_, M>,
    ) -> Propagation {
        if cx.phase() == Phase::Capture {
            return Propagation::Continue;
        }
        let now = cx.now();
        match *event {
            Event::ScrollGeometry { viewport, content } => {
                let next = ScrollState {
                    offset: state.offset,
                    viewport,
                    content,
                }
                .clamped();
                if next != self.state {
                    state.offset = next.offset;
                    cx.emit((self.on_scroll)(next));
                }
            }
            Event::Reveal(offset) => self.scroll_to(state, offset, cx),
            Event::Pointer(
                PointerEvent::Scroll {
                    delta,
                    value120,
                    source,
                    stop,
                    ..
                },
                _,
            ) => return self.wheel(state, delta, value120, source, stop, cx),
            Event::Wake(_) => {
                if state.wheel_until.is_some_and(|until| now >= until) {
                    state.wheel_until = None;
                    state.latch = None;
                }
            }
            Event::AnimationFrame(_) => {
                let Some(kinetic) = &state.kinetic else {
                    return Propagation::Continue;
                };
                let t = seconds(now);
                let offset = Point::new(kinetic[0].position(t), kinetic[1].position(t));
                let moving = kinetic.iter().any(|k| k.is_animating(t));
                self.scroll_to(state, offset, cx);
                if moving {
                    cx.request_animation_frame();
                } else {
                    state.kinetic = None;
                    state.latch = None;
                }
            }
            Event::Touch(TouchEvent::Down { id, position }, _) if state.touch.is_none() => {
                state.touch = Some((id, position));
                state.kinetic = Some(self.kinetic(state, seconds(now)));
                state.latch = Some(true);
            }
            Event::Touch(TouchEvent::Motion { id, position }, _) => {
                let Some((touch, last)) = state.touch.filter(|(t, _)| *t == id) else {
                    return Propagation::Continue;
                };
                let direction = cx.env().direction;
                let delta = Point::new(
                    self.along_x(direction, last.x - position.x),
                    last.y - position.y,
                );
                state.touch = Some((touch, position));
                self.drag(state, delta, seconds(now), cx);
            }
            Event::Touch(TouchEvent::Up { id }, _) if state.touch.is_some_and(|(t, _)| t == id) => {
                state.touch = None;
                self.lift(state, seconds(now), cx);
            }
            Event::Touch(TouchEvent::Cancel, _) | Event::Cancel => {
                state.touch = None;
                state.kinetic = None;
                state.latch = None;
            }
            Event::Key(key, _) if key.pressed => {
                if !self.key(state, key.keysym, key.mods, cx) {
                    return Propagation::Continue;
                }
            }
            Event::Focus { .. } if cx.node() == cx.target() => cx.request_paint(),
            _ => return Propagation::Continue,
        }
        Propagation::Stop
    }

    fn paint(&self, _: &ViewportState, cx: &mut PaintCx<'_>) {
        if cx.is_focused() {
            let size = cx.size();
            paint_focus_ring(cx, Rect::new(0.0, 0.0, size.width, size.height), 0.0);
        }
    }

    fn semantics(&self, _: &ViewportState, cx: &mut SemanticsCx<'_>) {
        cx.role(Role::ScrollView);
        cx.focus_fallback();
    }

    fn scroll_offset(&self, _: &ViewportState) -> Point {
        self.state.offset
    }
}

struct Bar<M> {
    axis: Axis,
    scroller: Scroller,
    state: ScrollState,
    /// Covers the whole frame, so it does not size it, and draws and takes the pointer only in its strip.
    fit: bool,
    on_scroll: ScrollHandler<M>,
    enabled: bool,
}

/// The node state of a scroll bar. Opaque.
#[derive(Default)]
pub struct BarState {
    /// The offset changed since the last paint: an overlay bar shows from that paint on.
    moved: Cell<bool>,
    shown_from: Cell<Option<Timestamp>>,
    /// Where on the thumb a drag holds it, along the bar.
    grab: Option<f64>,
    /// The bar's size and direction at its last paint, for hit-testing.
    painted: Cell<Option<(Size, bool)>>,
}

/// Where an overlay bar is in its show and fade.
enum Shown {
    Hidden,
    Until(Timestamp),
    Fading(f32),
}

impl<M: 'static> Bar<M> {
    fn along(&self, p: Point) -> f64 {
        match self.axis {
            Axis::Horizontal => p.x,
            Axis::Vertical => p.y,
        }
    }

    fn lengths(&self) -> (f64, f64, f64) {
        let s = &self.state;
        match self.axis {
            Axis::Horizontal => (s.viewport.width, s.content.width, s.offset.x),
            Axis::Vertical => (s.viewport.height, s.content.height, s.offset.y),
        }
    }

    /// The track's start and length along the bar.
    fn track(&self, size: Size) -> (f64, f64) {
        let len = self.along(Point::new(size.width, size.height));
        match self.scroller {
            Scroller::Overlay => (INSET, (len - 2.0 * INSET).max(0.0)),
            Scroller::Always => (0.0, len),
        }
    }

    /// The thumb's start and length along the bar from the start edge, or `None` when everything fits.
    fn thumb(&self, size: Size) -> Option<(f64, f64)> {
        let (viewport, content, offset) = self.lengths();
        if content <= viewport || viewport <= 0.0 {
            return None;
        }
        let (start, track) = self.track(size);
        let len = (track * viewport / content).max(THUMB_MIN).min(track);
        let max = content - viewport;
        Some((start + (track - len) * (offset / max).clamp(0.0, 1.0), len))
    }

    fn thickness(&self) -> f64 {
        match self.scroller {
            Scroller::Overlay => THUMB + 2.0 * INSET,
            Scroller::Always => TRACK,
        }
    }

    /// The strip of the node's `size` that the bar draws in: all of it, or for a fitted bar the trailing or bottom
    /// edge.
    fn strip(&self, size: Size, rtl: bool) -> Rect {
        let thickness = self.thickness().min(match self.axis {
            Axis::Vertical => size.width,
            Axis::Horizontal => size.height,
        });
        match (self.fit, self.axis) {
            (false, _) => Rect::new(0.0, 0.0, size.width, size.height),
            (true, Axis::Vertical) if rtl => Rect::new(0.0, 0.0, thickness, size.height),
            (true, Axis::Vertical) => {
                Rect::new(size.width - thickness, 0.0, thickness, size.height)
            }
            (true, Axis::Horizontal) => {
                Rect::new(0.0, size.height - thickness, size.width, thickness)
            }
        }
    }

    /// The thumb's rect in the bar, with a horizontal bar's start on the right in right-to-left.
    fn thumb_rect(&self, size: Size, rtl: bool) -> Option<Rect> {
        let strip = self.strip(size, rtl);
        let (start, len) = self.thumb(size)?;
        let thickness = match self.axis {
            Axis::Horizontal => strip.height(),
            Axis::Vertical => strip.width(),
        };
        let across = (thickness - THUMB) / 2.0;
        let (x, y) = (strip.min_x(), strip.min_y());
        Some(match self.axis {
            Axis::Vertical => Rect::new(x + across, start, THUMB, len),
            Axis::Horizontal if rtl => Rect::new(size.width - start - len, y + across, len, THUMB),
            Axis::Horizontal => Rect::new(start, y + across, len, THUMB),
        })
    }

    fn shown(&self, state: &BarState, now: Timestamp, reduce: bool) -> Shown {
        let Some(from) = state.shown_from.get() else {
            return Shown::Hidden;
        };
        let until = from.0 + SHOWN_NS;
        if now.0 < until {
            Shown::Until(Timestamp(until))
        } else if !reduce && now.0 < until + FADE_NS {
            Shown::Fading(1.0 - (now.0 - until) as f32 / FADE_NS as f32)
        } else {
            Shown::Hidden
        }
    }

    /// The offset that puts the thumb's start at `thumb_start` along the bar.
    fn offset_for(&self, size: Size, thumb_start: f64) -> f64 {
        let (viewport, content, _) = self.lengths();
        let Some((_, len)) = self.thumb(size) else {
            return 0.0;
        };
        let (start, track) = self.track(size);
        let room = (track - len).max(f64::EPSILON);
        (thumb_start - start) / room * (content - viewport)
    }

    fn scroll_to(&self, along: f64, cx: &mut EventCx<'_, M>) {
        let mut offset = self.state.offset;
        match self.axis {
            Axis::Horizontal => offset.x = along,
            Axis::Vertical => offset.y = along,
        }
        let next = ScrollState {
            offset,
            ..self.state
        }
        .clamped();
        if next != self.state {
            cx.emit((self.on_scroll)(next));
        }
    }
}

impl<M: 'static> Widget<M> for Bar<M> {
    type State = BarState;

    fn layout(&self) -> LayoutSpec {
        if self.fit {
            return LayoutSpec::new(ContainerKind::Leaf);
        }
        let thickness = self.thickness();
        let (width, height) = match self.axis {
            Axis::Vertical => (SizeSpec::Fixed(thickness), SizeSpec::Fill),
            Axis::Horizontal => (SizeSpec::Fill, SizeSpec::Fixed(thickness)),
        };
        LayoutSpec {
            width,
            height,
            align_self: Some(Align::End),
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) -> BarState {
        BarState::default()
    }

    fn update(&self, old: &Self, state: &mut BarState) -> Change {
        if self.state.offset != old.state.offset {
            state.moved.set(true);
        }
        if self.fit != old.fit {
            Change::Layout
        } else if self.state != old.state
            || self.scroller != old.scroller
            || self.enabled != old.enabled
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &BarState, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(&self, state: &mut BarState, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        if cx.phase() == Phase::Capture {
            return Propagation::Continue;
        }
        let Some(bounds) = cx.bounds() else {
            return Propagation::Continue;
        };
        let size = bounds.size;
        let local = |p: Point| Point::new(p.x - bounds.origin.x, p.y - bounds.origin.y);
        let rtl = cx.env().direction == LayoutDirection::Rtl;
        match *event {
            Event::AnimationFrame(now) | Event::Wake(now) if self.scroller == Scroller::Overlay => {
                match self.shown(state, now, cx.env().theme.motion.reduce) {
                    Shown::Until(until) => cx.wake_at(until),
                    Shown::Fading(_) => {
                        cx.request_animation_frame();
                        cx.request_paint();
                    }
                    Shown::Hidden => {
                        state.shown_from.set(None);
                        cx.request_paint();
                    }
                }
                return Propagation::Continue;
            }
            Event::Pointer(
                PointerEvent::Button {
                    position,
                    button: MouseButton::Left,
                    pressed: true,
                    ..
                },
                _,
            ) => {
                let local = local(position);
                let Some(thumb) = self.thumb_rect(size, rtl) else {
                    return Propagation::Continue;
                };
                if thumb.contains(local) {
                    let start = match self.axis {
                        Axis::Horizontal if rtl => size.width - thumb.max_x(),
                        _ => self.along(thumb.origin),
                    };
                    let at = match self.axis {
                        Axis::Horizontal if rtl => size.width - local.x,
                        _ => self.along(local),
                    };
                    state.grab = Some(at - start);
                    cx.capture_pointer();
                } else if self.scroller == Scroller::Always && self.strip(size, rtl).contains(local)
                {
                    let (viewport, _, offset) = self.lengths();
                    let page = (viewport - LINE).max(LINE);
                    let before = match self.axis {
                        Axis::Horizontal if rtl => local.x > thumb.max_x(),
                        _ => self.along(local) < self.along(thumb.origin),
                    };
                    self.scroll_to(if before { offset - page } else { offset + page }, cx);
                }
            }
            Event::Pointer(PointerEvent::Motion { position }, _) => {
                let Some(grab) = state.grab else {
                    return Propagation::Continue;
                };
                let local = local(position);
                let at = match self.axis {
                    Axis::Horizontal if rtl => size.width - local.x,
                    _ => self.along(local),
                };
                self.scroll_to(self.offset_for(size, at - grab), cx);
            }
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left,
                    pressed: false,
                    ..
                },
                _,
            ) if state.grab.is_some() => {
                state.grab = None;
                cx.release_pointer();
            }
            Event::Cancel => state.grab = None,
            _ => return Propagation::Continue,
        }
        Propagation::Stop
    }

    fn paint(&self, state: &BarState, cx: &mut PaintCx<'_>) {
        let theme = cx.env().theme.clone();
        let size = cx.size();
        let rtl = cx.env().direction == LayoutDirection::Rtl;
        state.painted.set(Some((size, rtl)));
        let normal = ControlState {
            enabled: true,
            window_active: cx.window_active(),
            ..ControlState::default()
        };
        let alpha = match self.scroller {
            Scroller::Always => {
                look::fill(
                    cx,
                    self.strip(size, rtl),
                    0.0,
                    look::color(
                        &theme,
                        ColorRole::ControlFill,
                        &ControlState {
                            enabled: self.enabled,
                            ..normal
                        },
                    ),
                );
                1.0
            }
            Scroller::Overlay => {
                if state.moved.replace(false) && self.thumb(size).is_some() {
                    state.shown_from.set(Some(cx.now()));
                    cx.request_animation_frame();
                }
                match self.shown(state, cx.now(), theme.motion.reduce) {
                    Shown::Hidden => return,
                    Shown::Until(_) => 1.0,
                    Shown::Fading(alpha) => alpha,
                }
            }
        };
        let Some(thumb) = self.thumb_rect(size, rtl) else {
            return;
        };
        let color = look::color(&theme, ColorRole::ScrollerThumb, &normal)
            .map(|c| c.with_alpha(c.a * alpha));
        look::fill(cx, thumb, THUMB / 2.0, color);
    }

    fn semantics(&self, _: &BarState, cx: &mut SemanticsCx<'_>) {
        if self.scroller == Scroller::Always {
            let (viewport, content, offset) = self.lengths();
            cx.role(Role::ScrollBar);
            cx.value(AccessibleValue::Number {
                value: offset,
                min: 0.0,
                max: (content - viewport).max(0.0),
                step: Some(LINE),
            });
        }
    }

    fn hit(&self, state: &BarState, point: Point) -> bool {
        match self.scroller {
            Scroller::Always => state
                .painted
                .get()
                .is_some_and(|(size, rtl)| self.strip(size, rtl).contains(point)),
            // only a shown thumb takes the pointer; the content under the rest of the strip stays reachable
            Scroller::Overlay => {
                state.shown_from.get().is_some()
                    && state.painted.get().is_some_and(|(size, rtl)| {
                        self.thumb_rect(size, rtl)
                            .is_some_and(|t| t.contains(point))
                    })
            }
        }
    }
}
