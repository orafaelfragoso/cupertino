use crate::lens::{paint_lens_faded, Lens, LensKind};
use crate::look::{self, paint_focus_ring};
use crate::motion::{Curve, Tween};
use crate::press::{Keys, Outcome, Press};
use crate::{HeaderToggle, SmallToggle, Toggle};
use cupertino_geometry::{Rect, Size};
use cupertino_ui_core::Phase;
use cupertino_ui_core::{
    AccessibleStates, AccessibleValue, Action, Change, Checked, ControlState, Environment, Event,
    EventCx, MeasureCx, PaintCx, Propagation, Role, SemanticsCx, Widget,
};
use cupertino_ui_layout::{Constraints, ContainerKind, LayoutDirection, LayoutSpec, Measured};
use cupertino_ui_platform::{MouseButton, PointerEvent};
use cupertino_ui_theme::{switch_geometry, Color, ColorRole, ControlMetrics, ThemeTokens};

/// The track: the accent when on; off, a quiet well that neither darkens under the pointer nor while held, or the
/// theme's stronger track under increased contrast.
fn track_color(theme: &ThemeTokens, on: bool, state: &ControlState) -> Option<Color> {
    if on || theme.preferences.appearance.is_high_contrast() {
        return look::color(theme, ColorRole::SwitchTrack, state);
    }
    let resting = ControlState {
        hovered: false,
        pressed: false,
        ..*state
    };
    Some(look::quiet(theme, look::WELL, &resting))
}

impl<M: 'static> Toggle<M> {
    fn paint_shaped(&self, state: &SwitchState, cx: &mut PaintCx<'_>, shape: &SwitchShape) {
        let theme = cx.env().theme.clone();
        let direction = cx.env().direction;
        let look_state = self.control_state(
            &state.press,
            cx.is_focused(),
            cx.focus_visible(),
            cx.window_active(),
        );
        let track = track(cx.size(), shape);
        let radius = track.height() / 2.0;
        // how far the thumb is on its way to the on end: the track takes the on colour as it goes
        let on = state.on.sample(cx);
        let (off_color, on_color) = (
            track_color(&theme, false, &look_state),
            track_color(&theme, true, &look_state),
        );
        let fill_track = |cx: &mut PaintCx<'_>, rect: Rect, radius: f64| {
            if on < 1.0 {
                look::fill(cx, rect, radius, off_color);
            }
            if on >= 1.0 {
                look::fill(cx, rect, radius, on_color);
            } else if on > 0.0 {
                look::fill(cx, rect, radius, look::faded(on_color, on as f32));
            }
        };
        fill_track(cx, track, radius);
        let thumb = thumb_at_fraction(track, on, direction, shape, state.travel());
        let lift = if self.enabled {
            state.lift.sample(cx)
        } else {
            0.0
        };
        // the plain thumb gives way to the lens as the lens swells out of it
        let lens_opacity = (lift / LENS_OPAQUE_AT).min(1.0);
        if lens_opacity < 1.0 {
            look::fill(
                cx,
                thumb,
                thumb.height() / 2.0,
                look::faded(
                    look::color(&theme, ColorRole::SwitchThumb, &look_state),
                    1.0 - lens_opacity as f32,
                ),
            );
        }
        if let Some(lens) = Lens::lifted(LensKind::Switch, thumb, lift) {
            // held, the thumb is a lens: the track shows through it, shrunk about its centre
            paint_lens_faded(cx, &lens, lens_opacity as f32, |cx, lens| {
                fill_track(cx, lens.through(track), radius * lens.minify);
            });
        }
        paint_focus_ring(cx, track, radius);
    }

    fn control_state(
        &self,
        press: &Press,
        focused: bool,
        focus_visible: bool,
        window_active: bool,
    ) -> ControlState {
        ControlState {
            enabled: self.enabled,
            hovered: press.hovered(),
            pressed: press.pressed(),
            focused,
            focus_visible,
            selected: self.on,
            window_active,
            ..ControlState::default()
        }
    }
}

/// The small switch's track, as in a settings form's rows.
const SMALL_TRACK: Size = Size::new(36.0, 16.0);
/// The small switch's thumb, a capsule wider than it is tall.
const SMALL_THUMB: Size = Size::new(21.0, 13.0);
/// Space between the small switch's thumb and the edge of its track.
const SMALL_THUMB_INSET: f64 = 1.5;

/// A switch's geometry: its track, its thumb and the space between the thumb and the track's edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SwitchShape {
    track: Size,
    thumb: Size,
    inset: f64,
}

impl SwitchShape {
    /// The switch of `metrics`: its track, and a capsule thumb as high as theirs, as much wider than high as the
    /// control size table's thumb.
    fn regular(metrics: &ControlMetrics) -> SwitchShape {
        let table = switch_geometry(metrics.compact);
        let aspect = table.thumb.0 / table.thumb.1;
        SwitchShape {
            track: metrics.switch,
            thumb: Size::new(metrics.switch_thumb * aspect, metrics.switch_thumb),
            inset: metrics.switch_thumb_inset,
        }
    }

    /// The switch at the head of a settings pane: a little larger than a row's.
    const HEADER: SwitchShape = SwitchShape {
        track: Size::new(44.5, 20.0),
        thumb: Size::new(25.5, 15.0),
        inset: 2.5,
    };

    const SMALL: SwitchShape = SwitchShape {
        track: SMALL_TRACK,
        thumb: SMALL_THUMB,
        inset: SMALL_THUMB_INSET,
    };
}

/// The pill, centred in a node padded out to the minimum target.
fn track(size: Size, shape: &SwitchShape) -> Rect {
    Rect::new(
        (size.width - shape.track.width) / 2.0,
        (size.height - shape.track.height) / 2.0,
        shape.track.width,
        shape.track.height,
    )
}

/// The thumb sits at the end when on and at the start when off; the end is on the left in right-to-left layouts.
#[cfg(test)]
fn thumb(track: Rect, on: bool, direction: LayoutDirection, shape: &SwitchShape) -> Rect {
    thumb_at(track, on, direction, shape, None)
}

/// [`thumb`], moved `travel` points along the track while it is dragged, never past either end.
fn thumb_at(
    track: Rect,
    on: bool,
    direction: LayoutDirection,
    shape: &SwitchShape,
    travel: Option<f64>,
) -> Rect {
    thumb_at_fraction(track, f64::from(u8::from(on)), direction, shape, travel)
}

/// The thumb's left and right resting x along the track.
fn thumb_run(track: Rect, shape: &SwitchShape) -> (f64, f64) {
    let start = track.min_x() + shape.inset;
    (
        start,
        (track.max_x() - shape.inset - shape.thumb.width).max(start),
    )
}

/// How far a thumb at `thumb` is from the off end to the on end, 0 to 1.
fn on_fraction(track: Rect, thumb: Rect, direction: LayoutDirection, shape: &SwitchShape) -> f64 {
    let (left, right) = thumb_run(track, shape);
    let right_share = if right > left {
        ((thumb.min_x() - left) / (right - left)).clamp(0.0, 1.0)
    } else {
        0.0
    };
    match direction {
        LayoutDirection::Ltr => right_share,
        LayoutDirection::Rtl => 1.0 - right_share,
    }
}

/// [`thumb_at`] with the thumb `on` of the way (0 to 1) from the off end to the on end, as it travels between them.
fn thumb_at_fraction(
    track: Rect,
    on: f64,
    direction: LayoutDirection,
    shape: &SwitchShape,
    travel: Option<f64>,
) -> Rect {
    let Size { width, height } = shape.thumb;
    let (start, end) = thumb_run(track, shape);
    let right_share = match direction {
        LayoutDirection::Ltr => on,
        LayoutDirection::Rtl => 1.0 - on,
    };
    let x = start + (end - start) * right_share.clamp(0.0, 1.0);
    let x = (x + travel.unwrap_or(0.0)).clamp(start, end);
    Rect::new(
        x,
        track.min_y() + (track.height() - height) / 2.0,
        width,
        height,
    )
}

/// Whether a thumb dragged to `thumb` turns the switch on: its centre past the track's, towards the end.
fn dragged_on(track: Rect, thumb: Rect, direction: LayoutDirection) -> bool {
    let past = thumb.center().x > track.center().x;
    past == (direction == LayoutDirection::Ltr)
}

/// How far the pointer travels before a press on the switch becomes a drag of its thumb.
const DRAG_SLOP: f64 = 3.0;
/// How long, in seconds, the thumb takes to swell into its lens when it is taken hold of.
const SWITCH_LIFT: f64 = 0.2;
/// How long the thumb takes to travel from one end of the track to the other, in seconds.
const SWITCH_TRAVEL: f64 = 0.2;
/// How long the lens takes to sink back into the plain thumb once it is let go (after its travel when it travels).
const SWITCH_SETTLE: f64 = 0.2;
/// The share of its lift by which the lens has fully covered the plain thumb: the white thumb goes early.
const LENS_OPAQUE_AT: f64 = 0.3;

/// The node state of a switch: how it is armed, the drag of its thumb, and the thumb's travel and lift. Opaque.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SwitchState {
    press: Press,
    drag: Option<Drag>,
    /// How far the thumb is from the off end to the on end, 0 to 1.
    on: Tween,
    /// How far the thumb has swelled into its lens, 0 to 1.
    lift: Tween,
    /// The event at hand sends the thumb across: the lens sinks once it has arrived.
    travels: bool,
}

/// A pointer press that may drag the thumb.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Drag {
    /// Where the press began, in window points.
    from: f64,
    /// How far the pointer has gone along the track since.
    travel: f64,
    /// The pointer went past the slop: the thumb follows it and the release sets the state it lands on.
    moved: bool,
}

impl SwitchState {
    /// A switch at rest, on or off.
    fn of(on: bool) -> SwitchState {
        SwitchState {
            on: Tween::at(f64::from(u8::from(on))),
            ..SwitchState::default()
        }
    }

    /// The thumb's travel while it is dragged with the pointer over the switch; away from it the thumb rests.
    fn travel(&self) -> Option<f64> {
        self.drag
            .filter(|d| d.moved && self.press.pressed())
            .map(|d| d.travel)
    }

    /// Held by a pointer or a touch over the switch, or by a key: the thumb shows its lens.
    fn held(&self) -> bool {
        self.press.pressed()
    }
}

impl<M: 'static> Toggle<M> {
    /// The same switch at the size of a settings pane's header: a 44.5 × 20 pt track and a capsule thumb.
    pub fn header(self) -> HeaderToggle<M> {
        HeaderToggle { toggle: self }
    }

    /// The same switch at the small size of a settings form's rows: a 36 × 16 pt track and a capsule thumb.
    pub fn small(self) -> SmallToggle<M> {
        SmallToggle { toggle: self }
    }
}

impl<M: 'static> Widget<M> for Toggle<M> {
    type State = SwitchState;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) -> SwitchState {
        SwitchState::of(self.on)
    }

    fn update(&self, old: &Self, state: &mut SwitchState) -> Change {
        if self.on != old.on {
            let to = f64::from(u8::from(self.on));
            state.on.go(to, SWITCH_TRAVEL, 0.0, Curve::EaseInOut);
        }
        if self.on != old.on || self.enabled != old.enabled || self.label != old.label {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &SwitchState, _: Constraints, cx: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: SwitchShape::regular(&cx.env().theme.metrics).track,
            baseline: None,
        }
    }

    fn target(&self, _: &SwitchState, env: &Environment) -> Size {
        env.theme.metrics.min_target
    }

    fn event(
        &self,
        state: &mut SwitchState,
        event: &Event,
        cx: &mut EventCx<'_, M>,
    ) -> Propagation {
        let shape = SwitchShape::regular(&cx.env().theme.metrics);
        self.drag_event(state, event, cx, &shape)
    }

    fn paint(&self, state: &SwitchState, cx: &mut PaintCx<'_>) {
        let shape = SwitchShape::regular(&cx.env().theme.metrics);
        self.paint_shaped(state, cx, &shape);
    }

    fn semantics(&self, _: &SwitchState, cx: &mut SemanticsCx<'_>) {
        self.semantics_of(cx)
    }
}

impl<M: 'static> Toggle<M> {
    /// A press arms the switch; once the pointer goes past [`DRAG_SLOP`] along the track the thumb follows it, and a
    /// release over the switch sets the state the thumb lands on, once and only if it changed. A press that never
    /// moves that far is a click: release over the switch toggles it. Released away from the switch, or cancelled,
    /// neither changes anything and the thumb goes back.
    ///
    /// Taken hold of, the thumb swells into its lens over [`SWITCH_LIFT`]; let go, the lens sinks back over
    /// [`SWITCH_SETTLE`], once the thumb has travelled to its end when it travels.
    fn drag_event(
        &self,
        state: &mut SwitchState,
        event: &Event,
        cx: &mut EventCx<'_, M>,
        shape: &SwitchShape,
    ) -> Propagation {
        let held = state.held();
        state.travels = false;
        let propagation = self.drag_or_press(state, event, cx, shape);
        if state.held() != held {
            if state.held() {
                state.lift.go(1.0, SWITCH_LIFT, 0.0, Curve::EaseOut);
            } else {
                let delay = if state.travels { SWITCH_TRAVEL } else { 0.0 };
                state.lift.go(0.0, SWITCH_SETTLE, delay, Curve::EaseInOut);
            }
            cx.request_paint();
        }
        propagation
    }

    fn drag_or_press(
        &self,
        state: &mut SwitchState,
        event: &Event,
        cx: &mut EventCx<'_, M>,
        shape: &SwitchShape,
    ) -> Propagation {
        if self.enabled {
            match *event {
                Event::Pointer(
                    PointerEvent::Button {
                        position,
                        button: MouseButton::Left,
                        pressed: true,
                        ..
                    },
                    _,
                ) if !state.press.pointer_armed() && cx.phase() != Phase::Capture => {
                    state.drag = Some(Drag {
                        from: position.x,
                        travel: 0.0,
                        moved: false,
                    });
                }
                Event::Pointer(PointerEvent::Motion { position }, _)
                    if state.press.pointer_armed() =>
                {
                    if let Some(drag) = &mut state.drag {
                        drag.travel = position.x - drag.from;
                        if drag.travel.abs() > DRAG_SLOP {
                            drag.moved = true;
                        }
                        if drag.moved {
                            // the press still learns whether the pointer is over the switch
                            state.press.event(event, self.enabled, Keys::Space, cx);
                            cx.request_paint();
                            return Propagation::Stop;
                        }
                    }
                }
                Event::Pointer(
                    PointerEvent::Button {
                        position,
                        button: MouseButton::Left,
                        pressed: false,
                        ..
                    },
                    _,
                ) if state.press.pointer_armed() && state.drag.is_some_and(|d| d.moved) => {
                    let travel = state.drag.map(|d| d.travel);
                    let inside = cx.hit(position);
                    state.drag = None;
                    state.press.disarm();
                    cx.release_pointer();
                    cx.request_paint();
                    // released away from the switch, the drag is cancelled and the thumb goes back
                    if let Some(bounds) = cx.bounds() {
                        let size = Size::new(bounds.width(), bounds.height());
                        let track = track(size, shape);
                        let direction = cx.env().direction;
                        let landed = thumb_at(track, self.on, direction, shape, travel);
                        // the thumb goes on from where it was let go: to the end the view keeps, or to the other one
                        state.on.jump(on_fraction(track, landed, direction, shape));
                        let rest = f64::from(u8::from(self.on));
                        state.on.go(rest, SWITCH_TRAVEL, 0.0, Curve::EaseOut);
                        state.travels = true;
                    }
                    if let (true, Some(bounds)) = (inside, cx.bounds()) {
                        let size = Size::new(bounds.width(), bounds.height());
                        let track = track(size, shape);
                        let direction = cx.env().direction;
                        let landed = thumb_at(track, self.on, direction, shape, travel);
                        let on = dragged_on(track, landed, direction);
                        if on != self.on {
                            if let Some(on_toggle) = &self.on_toggle {
                                cx.emit(on_toggle(on));
                            }
                        }
                    }
                    return Propagation::Stop;
                }
                Event::Cancel => {
                    state.drag = None;
                }
                _ => {}
            }
        }
        if let Event::Pointer(PointerEvent::Button { pressed: false, .. }, _) = event {
            state.drag = None;
        }
        match state.press.event(event, self.enabled, Keys::Space, cx) {
            Outcome::Ignored => Propagation::Continue,
            Outcome::Changed => {
                cx.request_paint();
                Propagation::Stop
            }
            Outcome::Activated(_) => {
                cx.request_paint();
                state.travels = true;
                if let Some(on_toggle) = &self.on_toggle {
                    cx.emit(on_toggle(!self.on));
                }
                Propagation::Stop
            }
        }
    }

    fn semantics_of(&self, cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Switch);
        cx.name(self.label.clone());
        cx.value(AccessibleValue::Toggle(if self.on {
            Checked::True
        } else {
            Checked::False
        }));
        cx.states(AccessibleStates {
            disabled: !self.enabled,
            ..AccessibleStates::default()
        });
        if self.enabled {
            cx.focusable();
            cx.action(Action::Click);
        }
    }
}

/// The small switch: the toggle's behaviour, reading and look at [`SwitchShape::SMALL`]'s size.
impl<M: 'static> Widget<M> for SmallToggle<M> {
    type State = SwitchState;

    fn layout(&self) -> LayoutSpec {
        self.toggle.layout()
    }

    fn state(&self) -> SwitchState {
        SwitchState::of(self.toggle.on)
    }

    fn update(&self, old: &Self, state: &mut SwitchState) -> Change {
        self.toggle.update(&old.toggle, state)
    }

    fn measure(&self, _: &SwitchState, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: SwitchShape::SMALL.track,
            baseline: None,
        }
    }

    fn target(&self, state: &SwitchState, env: &Environment) -> Size {
        self.toggle.target(state, env)
    }

    fn event(
        &self,
        state: &mut SwitchState,
        event: &Event,
        cx: &mut EventCx<'_, M>,
    ) -> Propagation {
        self.toggle
            .drag_event(state, event, cx, &SwitchShape::SMALL)
    }

    fn paint(&self, state: &SwitchState, cx: &mut PaintCx<'_>) {
        self.toggle.paint_shaped(state, cx, &SwitchShape::SMALL);
    }

    fn semantics(&self, _: &SwitchState, cx: &mut SemanticsCx<'_>) {
        self.toggle.semantics_of(cx)
    }
}

/// The pane header's switch: the toggle's behaviour, reading and look at [`SwitchShape::HEADER`]'s size.
impl<M: 'static> Widget<M> for HeaderToggle<M> {
    type State = SwitchState;

    fn layout(&self) -> LayoutSpec {
        self.toggle.layout()
    }

    fn state(&self) -> SwitchState {
        SwitchState::of(self.toggle.on)
    }

    fn update(&self, old: &Self, state: &mut SwitchState) -> Change {
        self.toggle.update(&old.toggle, state)
    }

    fn measure(&self, _: &SwitchState, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: SwitchShape::HEADER.track,
            baseline: None,
        }
    }

    fn target(&self, state: &SwitchState, env: &Environment) -> Size {
        self.toggle.target(state, env)
    }

    fn event(
        &self,
        state: &mut SwitchState,
        event: &Event,
        cx: &mut EventCx<'_, M>,
    ) -> Propagation {
        self.toggle
            .drag_event(state, event, cx, &SwitchShape::HEADER)
    }

    fn paint(&self, state: &SwitchState, cx: &mut PaintCx<'_>) {
        self.toggle.paint_shaped(state, cx, &SwitchShape::HEADER);
    }

    fn semantics(&self, _: &SwitchState, cx: &mut SemanticsCx<'_>) {
        self.toggle.semantics_of(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHAPE: SwitchShape = SwitchShape {
        track: Size::new(54.0, 24.0),
        thumb: Size::new(32.0, 20.0),
        inset: 2.0,
    };

    fn track() -> Rect {
        Rect::new(0.0, 0.0, 54.0, 24.0)
    }

    #[test]
    fn a_dragged_thumb_follows_the_pointer_between_the_ends() {
        let ltr = LayoutDirection::Ltr;
        assert_eq!(thumb(track(), false, ltr, &SHAPE).min_x(), 2.0);
        assert_eq!(thumb(track(), true, ltr, &SHAPE).min_x(), 20.0);
        assert_eq!(
            thumb_at(track(), false, ltr, &SHAPE, Some(7.0)).min_x(),
            9.0
        );
        assert_eq!(
            thumb_at(track(), false, ltr, &SHAPE, Some(80.0)).min_x(),
            20.0,
            "never past the end"
        );
        assert_eq!(
            thumb_at(track(), true, ltr, &SHAPE, Some(80.0)).min_x(),
            20.0
        );
    }

    #[test]
    fn the_state_a_drag_lands_on_is_the_half_the_thumb_is_in() {
        let ltr = LayoutDirection::Ltr;
        let rtl = LayoutDirection::Rtl;
        let at = |travel, on, direction| {
            dragged_on(
                track(),
                thumb_at(track(), on, direction, &SHAPE, Some(travel)),
                direction,
            )
        };
        assert!(!at(5.0, false, ltr), "short of the middle it stays off");
        assert!(at(12.0, false, ltr), "past the middle it lands on");
        assert!(!at(-12.0, true, ltr));
        assert!(
            at(-12.0, false, rtl),
            "the end is on the left in right-to-left"
        );
    }
}
