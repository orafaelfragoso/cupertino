use crate::lens::{paint_lens, Lens, LensKind};
use crate::look::{self, paint_focus_ring};
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
        let track_color = track_color(&theme, self.on, &look_state);
        look::fill(cx, track, radius, track_color);
        let thumb = thumb_at(track, self.on, direction, shape, state.travel());
        match Lens::of(LensKind::Switch, thumb, state.held() && self.enabled) {
            // held, the thumb is a lens: the track shows through it, shrunk about its centre
            Some(lens) => paint_lens(cx, &lens, |cx, lens| {
                look::fill(cx, lens.through(track), radius * lens.minify, track_color);
            }),
            None => {
                look::fill(
                    cx,
                    thumb,
                    thumb.height() / 2.0,
                    look::color(&theme, ColorRole::SwitchThumb, &look_state),
                );
            }
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
    let Size { width, height } = shape.thumb;
    let at_right = on == (direction == LayoutDirection::Ltr);
    let (start, end) = (
        track.min_x() + shape.inset,
        track.max_x() - shape.inset - width,
    );
    let x = if at_right { end } else { start };
    let x = (x + travel.unwrap_or(0.0)).clamp(start, end.max(start));
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

/// The node state of a switch: how it is armed, and the drag of its thumb. Opaque.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SwitchState {
    press: Press,
    drag: Option<Drag>,
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
        SwitchState::default()
    }

    fn update(&self, old: &Self, _: &mut SwitchState) -> Change {
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
    fn drag_event(
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
        SwitchState::default()
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
        SwitchState::default()
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
