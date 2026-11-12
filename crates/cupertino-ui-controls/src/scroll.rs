//! ScrollView: a viewport that clips and moves its content by the model's offset, with scroll bars beside or over it.
//! Wheel, touchpad, touch and keys move the offset, which goes back to the model as a message. A gesture belongs to
//! the view that took its first event until it ends; only a new gesture passes on to the view around it.

use crate::look::{self, paint_focus_ring};
use crate::{Handler, ScrollView, Scroller};
use cupertino_geometry::{Point, Rect, Size};
use cupertino_ui_core::kinetic::ScrollMotion;
use cupertino_ui_core::session_scroll_bars;
use cupertino_ui_core::{
    AccessibleValue, Change, ControlState, Element, Event, EventCx, MeasureCx, PaintCx, Phase,
    Propagation, Role, SemanticsCx, Widget,
};
use cupertino_ui_layout::{
    Align, Axis, Constraints, ContainerKind, Flex, Justify, LayoutDirection, LayoutSpec, Measured,
    Overflow, ScrollState, SizeSpec,
};
use cupertino_ui_platform::{
    Keysym, Mods, MouseButton, PointerEvent, ScrollBars, ScrollSource, Timestamp, TouchEvent,
};
use cupertino_ui_theme::ColorRole;
use std::cell::Cell;
use std::rc::Rc;

/// A view is scrolled once its offset passes this many points.
const SCROLLED_AT: f64 = 0.5;

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
/// A scroll bar's frame across its axis, in either style: the strip an overlay bar takes the pointer in and the
/// gutter an always-shown track takes from the content.
const BAR: f64 = 17.0;
/// An overlay thumb's width: thinner than the strip it is grabbed in.
const OVERLAY_THUMB: f64 = 6.0;
/// Between an overlay thumb and the outer edge of its strip.
const OVERLAY_EDGE: f64 = 3.0;
/// An always-shown track's thumb, centred in the track.
const TRACK_THUMB: f64 = 11.0;
const THUMB_MIN: f64 = 24.0;
/// Between an overlay thumb and the ends of its strip.
const INSET: f64 = 2.0;
/// An overlay bar stays shown this long after the content last moved, then fades for [`FADE_NS`].
const SHOWN_NS: u64 = 500_000_000;
/// How long a hide that was held back, by the pointer on the thumb or a drag of it, waits once the hold ends.
const RESTART_SHOWN_NS: u64 = 300_000_000;
/// How long the flash an overlay bar makes when its content first has something to scroll stays shown.
const FLASH_NS: u64 = 1_000_000_000;
/// How long a hiding overlay bar takes to fade out.
const FADE_NS: u64 = 250_000_000;
/// The pointer on a shown overlay thumb widens it after this delay...
const HOVER_DELAY_NS: u64 = 50_000_000;
/// ...over this long.
const HOVER_GROW_NS: u64 = 125_000_000;
/// An overlay thumb's width with the pointer on it.
const OVERLAY_THUMB_HOVER: f64 = 10.0;

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

/// The bars a view shows: the session's style when it said one, else the view's own.
fn style(own: Scroller) -> Scroller {
    match session_scroll_bars() {
        Some(ScrollBars::Always) => Scroller::Always,
        Some(ScrollBars::WhenScrolling) => Scroller::Overlay,
        None => own,
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

/// As [`frame`], for a view whose band under raised nodes holds text outside glass, a table's pinned header: the
/// scroll edge there is the solid one in every appearance, so the text over it stays legible.
pub(crate) fn pinned<M: 'static>(
    content: Element<M>,
    state: ScrollState,
    axes: (bool, bool),
    scroller: Scroller,
    on_scroll: ScrollHandler<M>,
) -> Element<M> {
    edged(content, state, axes, scroller, on_scroll, true, true)
}

/// As [`frame`], with the bars' tracks disabled, as a disabled text box shows them, unless `enabled`.
pub(crate) fn tracked<M: 'static>(
    content: Element<M>,
    state: ScrollState,
    axes: (bool, bool),
    scroller: Scroller,
    on_scroll: ScrollHandler<M>,
    enabled: bool,
) -> Element<M> {
    edged(content, state, axes, scroller, on_scroll, enabled, false)
}

/// The frame of [`tracked`], its scroll edge solid when `firm` ([`pinned`]).
fn edged<M: 'static>(
    content: Element<M>,
    state: ScrollState,
    (horizontal, vertical): (bool, bool),
    scroller: Scroller,
    on_scroll: ScrollHandler<M>,
    enabled: bool,
    firm: bool,
) -> Element<M> {
    let scroller = style(scroller);
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
                    edge: false,
                    scrolled: false,
                    firm: false,
                    children: vec![inner, bars.next().expect("one bar per scrolled axis")],
                });
            }
            let mut children = vec![inner];
            children.extend(bars);
            return Element::new(Frame {
                kind: Some(Axis::Vertical),
                fit: false,
                edge: vertical,
                scrolled: state.offset.y > SCROLLED_AT,
                firm,
                children,
            });
        }
    };
    Element::new(Frame {
        kind: None,
        fit: false,
        edge: vertical,
        scrolled: state.offset.y > SCROLLED_AT,
        firm,
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
        edge: true,
        scrolled: state.offset.y > SCROLLED_AT,
        firm: false,
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
    /// The frame of a view that scrolls vertically: where raised nodes cover its top, it draws the scroll edge there
    /// ([`crate::scroll_edge`]).
    pub(crate) edge: bool,
    /// The content is scrolled: something lies under the covered band, so the edge shows.
    pub(crate) scrolled: bool,
    /// The covered band holds text outside glass: the edge there is the solid one ([`pinned`]).
    pub(crate) firm: bool,
    pub(crate) children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for Frame<M> {
    type State = Cell<Option<crate::scroll_edge::EdgeLook>>;

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

    fn state(&self) -> Self::State {
        Cell::new(None)
    }

    fn update(&self, old: &Self, _: &mut Self::State) -> Change {
        if self.kind != old.kind || self.fit != old.fit {
            Change::Layout
        } else if self.edge != old.edge || self.scrolled != old.scrolled || self.firm != old.firm {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &Self::State, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(&self, _: &mut Self::State, _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, edge: &Self::State, cx: &mut PaintCx<'_>) {
        let top = cx.obscured().top;
        let look = (self.edge && top > 0.0)
            .then(|| crate::scroll_edge::EdgeLook::of(cx, top, self.firm, self.scrolled))
            .flatten();
        edge.set(look);
        if look.is_some_and(|l| l.is_soft()) {
            crate::scroll_edge::set_page_scrolled(self.scrolled);
        }
        if look.is_some() {
            cx.overlay();
        }
    }

    fn semantics(&self, _: &Self::State, _: &mut SemanticsCx<'_>) {}

    /// The scroll edge, over the content and its bars where raised nodes cover them.
    fn overlay(&self, edge: &Self::State, cx: &mut cupertino_ui_core::OverlayCx<'_>) {
        if let Some(look) = edge.get() {
            cx.reach_obscured();
            look.paint(cx);
        }
    }
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
    /// How far the content is drawn past its edges, on top of the model's offset.
    stretch: Point,
    /// Whether this view owns the gesture in progress, from its first event.
    latch: Option<bool>,
    /// When the wheel burst in progress ends.
    wheel_until: Option<Timestamp>,
    /// The content's motion while a finger or touch drags it, it coasts or a stretch returns.
    motion: Option<ScrollMotion>,
    touch: Option<(i32, Point)>,
    /// The time of the last input that moved the content, in seconds: the next one is never earlier.
    input_at: f64,
    /// The time the content was last drawn at, in seconds.
    drawn_at: f64,
    /// How far raised nodes covered the viewport's top at its last paint: a page is what shows below them.
    obscured: Cell<f64>,
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

    /// Draws the content without a stretch, as after a key or a reveal: any motion stops.
    fn settle(&self, state: &mut ViewportState, cx: &mut EventCx<'_, M>) {
        state.motion = None;
        if state.stretch != Point::ZERO {
            state.stretch = Point::ZERO;
            cx.request_layout();
        }
    }

    /// Content at rest at `offset`.
    fn at_rest(&self, offset: Point) -> ScrollMotion {
        ScrollMotion::new(
            offset,
            self.state.max_offset(),
            self.state.viewport,
            (self.horizontal, self.vertical),
        )
    }

    /// The motion in progress, or content at rest at the offset last sent.
    fn motion<'s>(&self, state: &'s mut ViewportState) -> &'s mut ScrollMotion {
        let at = state.offset;
        state.motion.get_or_insert_with(|| self.at_rest(at))
    }

    /// When an input event stamped `at` moves the content, in seconds: when it happened, not when it is handled, so
    /// movements delivered together keep their spacing and the speed a lift leaves is measured as the finger moved.
    /// Kept in order after the last input, and never before the content was last drawn when it catches moving
    /// content, which is caught where it shows.
    fn input_seconds(&self, state: &mut ViewportState, at: Timestamp, cx: &EventCx<'_, M>) -> f64 {
        let mut t = seconds(cx.input_time(at)).max(state.input_at);
        if state.motion.as_ref().is_some_and(|m| !m.is_dragging()) {
            t = t.max(state.drawn_at);
        }
        state.input_at = t;
        t
    }

    /// Sends the motion's offset at `now` and draws its stretch; the motion ends once it is at rest.
    fn show(&self, state: &mut ViewportState, now: f64, cx: &mut EventCx<'_, M>) {
        let Some(motion) = &state.motion else {
            return;
        };
        state.drawn_at = state.drawn_at.max(now);
        let (offset, stretch) = (motion.offset(now), motion.stretch(now));
        let animating = motion.is_animating(now);
        let dragging = motion.is_dragging();
        self.scroll_to(state, offset, cx);
        if stretch != state.stretch {
            state.stretch = stretch;
            cx.request_layout();
        }
        if animating {
            cx.request_animation_frame();
        } else if !dragging {
            state.motion = None;
        }
    }

    /// Whether a gesture whose first movement is `delta` is this view's: it moves the content, or, for a finger,
    /// it pulls along an axis this view scrolls past an edge, which stretches it.
    fn claims(&self, state: &mut ViewportState, delta: Point, finger: bool, now: f64) -> bool {
        let moves = match &state.motion {
            Some(motion) => motion.can_move(delta, now),
            None => self.at_rest(state.offset).can_move(delta, now),
        };
        if moves {
            return true;
        }
        let max = self.state.max_offset();
        finger
            && if delta.y.abs() >= delta.x.abs() {
                self.vertical && max.y > 0.0
            } else {
                self.horizontal && max.x > 0.0
            }
    }

    /// The fingers lifted: the content coasts on animation frames, without kinetic motion under Reduce Motion, and
    /// a stretch returns to its edge.
    fn lift(&self, state: &mut ViewportState, now: f64, cx: &mut EventCx<'_, M>) {
        let inertia = !cx.env().theme.motion.reduce;
        if let Some(motion) = &mut state.motion {
            motion.release(now, inertia);
        }
        self.show(state, now, cx);
        if state.motion.is_none() {
            state.latch = None;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn wheel(
        &self,
        state: &mut ViewportState,
        delta: Point,
        value120: Option<(i32, i32)>,
        source: ScrollSource,
        stop: bool,
        at: Timestamp,
        cx: &mut EventCx<'_, M>,
    ) -> Propagation {
        // the burst's end is timed on the clock its wake comes on; the movement at the time it happened
        let now = cx.now();
        let t = self.input_seconds(state, at, cx);
        let direction = cx.env().direction;
        let delta = match (delta, value120) {
            (d, Some((x, y))) if d == Point::ZERO => {
                Point::new(f64::from(x) / 120.0 * LINE, f64::from(y) / 120.0 * LINE)
            }
            (d, _) => d,
        };
        let delta = Point::new(self.along_x(direction, delta.x), delta.y);
        let wheel = matches!(source, ScrollSource::Wheel | ScrollSource::WheelTilt);
        let finger = source == ScrollSource::Finger;
        if wheel && state.wheel_until.is_some_and(|until| now >= until) {
            state.latch = None;
        }
        let mine = match state.latch {
            Some(mine) => mine,
            // a gesture's owner is the first view its first movement can move, or for a finger stretch
            None if delta == Point::ZERO => return Propagation::Continue,
            None => {
                let mine = self.claims(state, delta, finger, t);
                state.latch = Some(mine);
                mine
            }
        };
        if wheel {
            let until = Timestamp(now.0 + WHEEL_IDLE_NS);
            state.wheel_until = Some(until);
            cx.wake_at(until);
        } else if stop {
            if mine && finger {
                self.lift(state, t, cx);
            } else {
                state.latch = None;
            }
            return if mine {
                Propagation::Stop
            } else {
                Propagation::Continue
            };
        }
        if !mine {
            return Propagation::Continue;
        }
        if finger {
            self.motion(state).drag(delta, t);
        } else {
            // a wheel's or a continuous source's movement is complete as it arrives: no stretch and no inertia
            self.motion(state).step(delta, t);
        }
        self.show(state, t, cx);
        Propagation::Stop
    }

    fn key(
        &self,
        state: &mut ViewportState,
        keysym: Keysym,
        mods: Mods,
        cx: &mut EventCx<'_, M>,
    ) -> bool {
        // a key held with a command modifier is a shortcut, for the window
        if [Mods::PRIMARY, Mods::CTRL, Mods::ALT]
            .into_iter()
            .any(|m| mods.contains(m))
        {
            return false;
        }
        let page = (self.state.viewport.height - state.obscured.get() - LINE).max(LINE);
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
        self.settle(state, cx);
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
        let t = seconds(now);
        match *event {
            Event::ScrollGeometry { viewport, content } => {
                let next = ScrollState {
                    offset: state.offset,
                    viewport,
                    content,
                }
                .clamped();
                if let Some(motion) = &mut state.motion {
                    motion.set_bounds(next.max_offset(), viewport, t);
                }
                if next != self.state {
                    state.offset = next.offset;
                    cx.emit((self.on_scroll)(next));
                }
            }
            Event::Reveal(offset) => {
                self.settle(state, cx);
                self.scroll_to(state, offset, cx);
            }
            Event::Pointer(
                PointerEvent::Scroll {
                    delta,
                    value120,
                    source,
                    stop,
                    ..
                },
                at,
            ) => return self.wheel(state, delta, value120, source, stop, at, cx),
            Event::Wake(_) => {
                if state.wheel_until.is_some_and(|until| now >= until) {
                    state.wheel_until = None;
                    state.latch = None;
                }
            }
            Event::AnimationFrame(_) => {
                if state.motion.is_none() {
                    return Propagation::Continue;
                }
                self.show(state, t, cx);
                // the gesture this view owned ends with its motion
                if state.motion.is_none() && state.wheel_until.is_none() && state.touch.is_none() {
                    state.latch = None;
                }
            }
            Event::Touch(TouchEvent::Down { id, position }, _) if state.touch.is_none() => {
                state.touch = Some((id, position));
                state.latch = Some(true);
            }
            Event::Touch(TouchEvent::Motion { id, position }, at) => {
                let Some((touch, last)) = state.touch.filter(|(t, _)| *t == id) else {
                    return Propagation::Continue;
                };
                let t = self.input_seconds(state, at, cx);
                let direction = cx.env().direction;
                let delta = Point::new(
                    self.along_x(direction, last.x - position.x),
                    last.y - position.y,
                );
                state.touch = Some((touch, position));
                self.motion(state).drag(delta, t);
                self.show(state, t, cx);
            }
            Event::Touch(TouchEvent::Up { id }, at)
                if state.touch.is_some_and(|(t, _)| t == id) =>
            {
                state.touch = None;
                let t = self.input_seconds(state, at, cx);
                self.lift(state, t, cx);
            }
            Event::Touch(TouchEvent::Cancel, _) | Event::Cancel => {
                // a touch cancel happened when it is stamped; a cancel of the runtime's own, now
                let t = match *event {
                    Event::Touch(_, at) => self.input_seconds(state, at, cx),
                    _ => t,
                };
                state.touch = None;
                state.latch = None;
                // the drag stops where it is; a stretch still returns
                if let Some(motion) = &mut state.motion {
                    motion.cancel(t);
                }
                self.show(state, t, cx);
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

    fn paint(&self, state: &ViewportState, cx: &mut PaintCx<'_>) {
        let top = cx.obscured().top;
        state.obscured.set(top);
        if cx.is_focused() {
            let size = cx.size();
            // the ring goes round what shows below raised nodes
            let top = top.min(size.height);
            paint_focus_ring(cx, Rect::new(0.0, top, size.width, size.height - top), 0.0);
        }
    }

    fn semantics(&self, _: &ViewportState, cx: &mut SemanticsCx<'_>) {
        cx.role(Role::ScrollView);
        if cx.can_scroll() {
            cx.focus_fallback();
        }
    }

    /// The model's offset with the stretch drawn past an edge.
    fn scroll_offset(&self, state: &ViewportState) -> Point {
        Point::new(
            self.state.offset.x + state.stretch.x,
            self.state.offset.y + state.stretch.y,
        )
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
    /// The pointer is on the shown thumb: an overlay bar stays shown.
    hover: bool,
    /// How long the bar stays shown from `shown_from`, before it fades.
    hold_ns: Cell<u64>,
    /// The bar had something to scroll at its last paint: when it gains something, it flashes.
    scrollable: Cell<bool>,
    /// The thumb's widening: when it last turned, how wide it was then (0 to 1) and where it heads.
    widen: Cell<(Timestamp, f32, bool)>,
    /// The bar's size and direction at its last paint, for hit-testing.
    painted: Cell<Option<(Size, bool)>>,
    /// How far raised nodes cover the top of a vertical bar at its last paint: its track starts below them.
    obscured: Cell<f64>,
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

    /// Whether the content passes the viewport along the bar's axis.
    fn scrolls(&self) -> bool {
        let (viewport, content, _) = self.lengths();
        content > viewport && viewport > 0.0
    }

    /// The frame across the axis; an always-shown track that has nothing to scroll gives its gutter back.
    fn thickness(&self) -> f64 {
        match self.scroller {
            Scroller::Always if !self.scrolls() => 0.0,
            _ => BAR,
        }
    }

    fn thumb_width(&self) -> f64 {
        match self.scroller {
            Scroller::Overlay => OVERLAY_THUMB,
            Scroller::Always => TRACK_THUMB,
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

    /// The thumb as drawn, with a horizontal bar's start on the right in right-to-left: centred in an always-shown
    /// track, near the outer edge of an overlay strip (the trailing edge of a vertical bar, the bottom of a horizontal
    /// one).
    fn thumb_rect(&self, size: Size, rtl: bool) -> Option<Rect> {
        let strip = self.strip(size, rtl);
        let (start, len) = self.thumb(size)?;
        // from the strip's low edge across it, of `thickness`, with the outer edge at its high end or not
        let across = |low: f64, thickness: f64, outer_high: bool| {
            let width = self.thumb_width().min(thickness);
            let edge = OVERLAY_EDGE.min(thickness - width);
            let at = match (self.scroller, outer_high) {
                (Scroller::Always, _) => (thickness - width) / 2.0,
                (Scroller::Overlay, true) => thickness - edge - width,
                (Scroller::Overlay, false) => edge,
            };
            (low + at, width)
        };
        Some(match self.axis {
            Axis::Vertical => {
                let (x, width) = across(strip.min_x(), strip.width(), !rtl);
                Rect::new(x, start, width, len)
            }
            Axis::Horizontal => {
                let (y, width) = across(strip.min_y(), strip.height(), true);
                let x = if rtl { size.width - start - len } else { start };
                Rect::new(x, y, len, width)
            }
        })
    }

    /// Where the thumb takes the pointer: its length along the bar and the whole strip across, wider than drawn.
    fn target_rect(&self, size: Size, rtl: bool) -> Option<Rect> {
        let strip = self.strip(size, rtl);
        let thumb = self.thumb_rect(size, rtl)?;
        Some(match self.axis {
            Axis::Vertical => {
                Rect::new(strip.min_x(), thumb.min_y(), strip.width(), thumb.height())
            }
            Axis::Horizontal => {
                Rect::new(thumb.min_x(), strip.min_y(), thumb.width(), strip.height())
            }
        })
    }

    /// The pointer left the thumb or let go of it: an overlay bar shown meanwhile stays a while longer, then fades.
    fn release_hold(&self, state: &BarState, cx: &mut EventCx<'_, M>) {
        if state.shown_from.get().is_some() {
            let now = cx.now();
            state.shown_from.set(Some(now));
            state.hold_ns.set(RESTART_SHOWN_NS);
            cx.wake_at(Timestamp(now.0 + RESTART_SHOWN_NS));
            cx.request_paint();
        }
    }

    fn shown(&self, state: &BarState, now: Timestamp, reduce: bool) -> Shown {
        let Some(from) = state.shown_from.get() else {
            return Shown::Hidden;
        };
        // held by the pointer on it or dragging it
        if state.hover || state.grab.is_some() {
            return Shown::Until(Timestamp(now.0 + SHOWN_NS));
        }
        let until = from.0 + state.hold_ns.get();
        if now.0 < until {
            Shown::Until(Timestamp(until))
        } else if !reduce && now.0 < until + FADE_NS {
            Shown::Fading(1.0 - (now.0 - until) as f32 / FADE_NS as f32)
        } else {
            Shown::Hidden
        }
    }

    /// How wide the thumb is, from 0 (resting) to 1 (widened by the pointer), at `now`, and whether it is still moving.
    fn widening(state: &BarState, now: Timestamp, reduce: bool) -> (f32, bool) {
        let (from_time, from, to) = state.widen.get();
        let target = if to { 1.0 } else { 0.0 };
        if reduce {
            return (target, false);
        }
        let start = from_time.0 + if to { HOVER_DELAY_NS } else { 0 };
        let progress = (now.0.saturating_sub(start) as f32 / HOVER_GROW_NS as f32).min(1.0);
        (from + (target - from) * progress, progress < 1.0)
    }

    /// The pointer came onto or left the thumb: the widening turns from where it is.
    fn turn_widening(state: &BarState, now: Timestamp, reduce: bool, to: bool) {
        let (value, _) = Self::widening(state, now, reduce);
        state.widen.set((now, value, to));
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
        // an always-shown track that starts or stops having something to scroll takes or gives back its gutter
        if self.fit != old.fit
            || self.scroller != old.scroller
            || (self.scroller == Scroller::Always && self.scrolls() != old.scrolls())
        {
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
        // a vertical bar under raised nodes lies below them: its geometry is that of the part they leave
        let top = state.obscured.get();
        let size = Size::new(bounds.size.width, (bounds.size.height - top).max(0.0));
        let local = |p: Point| Point::new(p.x - bounds.origin.x, p.y - bounds.origin.y - top);
        let rtl = cx.env().direction == LayoutDirection::Rtl;
        match *event {
            Event::AnimationFrame(now) | Event::Wake(now) if self.scroller == Scroller::Overlay => {
                match self.shown(state, now, cx.env().theme.motion.reduce) {
                    Shown::Until(until) => {
                        cx.wake_at(until);
                        if Self::widening(state, now, cx.env().theme.motion.reduce).1 {
                            cx.request_animation_frame();
                            cx.request_paint();
                        }
                    }
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
                let Some(thumb) = self.target_rect(size, rtl) else {
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
                    if cupertino_ui_core::personal().scroll_click
                        == cupertino_ui_core::ScrollClick::JumpToSpot
                    {
                        // the knob's middle goes to the click, and goes on with the pointer while it is held
                        let half = match self.axis {
                            Axis::Horizontal => thumb.size.width,
                            Axis::Vertical => thumb.size.height,
                        } / 2.0;
                        let at = match self.axis {
                            Axis::Horizontal if rtl => size.width - local.x,
                            _ => self.along(local),
                        };
                        state.grab = Some(half);
                        cx.capture_pointer();
                        self.scroll_to(self.offset_for(size, at - half), cx);
                        return Propagation::Stop;
                    }
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
                if !state.hover {
                    self.release_hold(state, cx);
                }
            }
            Event::Pointer(PointerEvent::Enter { .. }, _) if cx.node() == cx.target() => {
                state.hover = true;
                let reduce = cx.env().theme.motion.reduce;
                Self::turn_widening(state, cx.now(), reduce, true);
                cx.request_animation_frame();
            }
            Event::Pointer(PointerEvent::Leave, _) if cx.node() == cx.target() => {
                state.hover = false;
                let reduce = cx.env().theme.motion.reduce;
                Self::turn_widening(state, cx.now(), reduce, false);
                cx.request_animation_frame();
                if state.grab.is_none() {
                    self.release_hold(state, cx);
                }
            }
            Event::Cancel => {
                state.grab = None;
                state.hover = false;
                let reduce = cx.env().theme.motion.reduce;
                Self::turn_widening(state, cx.now(), reduce, false);
                self.release_hold(state, cx);
            }
            _ => return Propagation::Continue,
        }
        Propagation::Stop
    }

    fn paint(&self, state: &BarState, cx: &mut PaintCx<'_>) {
        let theme = cx.env().theme.clone();
        let whole = cx.size();
        let top = match self.axis {
            Axis::Vertical => cx.obscured().top.clamp(0.0, whole.height),
            Axis::Horizontal => 0.0,
        };
        state.obscured.set(top);
        // drawn in the part raised nodes leave, moved down past them
        let size = Size::new(whole.width, whole.height - top);
        let below = |r: Rect| r.translate(0.0, top);
        let rtl = cx.env().direction == LayoutDirection::Rtl;
        state.painted.set(Some((size, rtl)));
        let normal = ControlState {
            enabled: true,
            window_active: cx.window_active(),
            ..ControlState::default()
        };
        let alpha = match self.scroller {
            Scroller::Always if !self.scrolls() => return,
            Scroller::Always => {
                look::fill(
                    cx,
                    below(self.strip(size, rtl)),
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
                let scrolls = self.thumb(size).is_some();
                let moved = state.moved.replace(false) && scrolls;
                let appeared = scrolls && !state.scrollable.replace(scrolls);
                if !scrolls {
                    state.scrollable.set(false);
                }
                if moved || appeared {
                    state.shown_from.set(Some(cx.now()));
                    state.hold_ns.set(if moved { SHOWN_NS } else { FLASH_NS });
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
        let mut thumb = thumb;
        if self.scroller == Scroller::Overlay {
            let (grow, busy) = Self::widening(state, cx.now(), theme.motion.reduce);
            if busy {
                cx.request_animation_frame();
            }
            let extra = (OVERLAY_THUMB_HOVER - OVERLAY_THUMB) * f64::from(grow);
            // the thumb widens inward, away from the outer edge it rests against
            thumb = match self.axis {
                Axis::Vertical if rtl => Rect::new(
                    thumb.min_x(),
                    thumb.min_y(),
                    thumb.width() + extra,
                    thumb.height(),
                ),
                Axis::Vertical => Rect::new(
                    thumb.min_x() - extra,
                    thumb.min_y(),
                    thumb.width() + extra,
                    thumb.height(),
                ),
                Axis::Horizontal => Rect::new(
                    thumb.min_x(),
                    thumb.min_y() - extra,
                    thumb.width(),
                    thumb.height() + extra,
                ),
            };
        }
        let radius = thumb.width().min(thumb.height()) / 2.0;
        look::fill(cx, below(thumb), radius, color);
    }

    fn semantics(&self, _: &BarState, cx: &mut SemanticsCx<'_>) {
        if self.scroller == Scroller::Always && self.scrolls() {
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
        let point = Point::new(point.x, point.y - state.obscured.get());
        match self.scroller {
            Scroller::Always => state
                .painted
                .get()
                .is_some_and(|(size, rtl)| self.strip(size, rtl).contains(point)),
            // only a shown thumb takes the pointer; the content under the rest of the strip stays reachable
            Scroller::Overlay => {
                state.shown_from.get().is_some()
                    && state.painted.get().is_some_and(|(size, rtl)| {
                        self.target_rect(size, rtl)
                            .is_some_and(|t| t.contains(point))
                    })
            }
        }
    }
}
