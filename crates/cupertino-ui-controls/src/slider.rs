use crate::icon::symbol;
use crate::lens::{paint_lens_faded, Lens, LensKind};
use crate::look::{self, paint_focus_ring};
use crate::motion::{Curve, Tween};
use crate::{Slider, SliderStyle};
use cupertino_geometry::{Insets, Rect, Size};
use cupertino_ui_core::{
    AccessibleStates, AccessibleValue, Action, ActionData, Change, ControlState, Environment,
    Event, EventCx, MeasureCx, PaintCx, Propagation, Role, SemanticsCx, Widget,
};
use cupertino_ui_layout::{
    Constraints, ContainerKind, LayoutDirection, LayoutSpec, Measured, SizeSpec,
};
use cupertino_ui_platform::{Keysym, MouseButton, PointerEvent};
use cupertino_ui_render::{Effect, PaintOp};
use cupertino_ui_theme::{slider_geometry, Color, ColorRole, ThemeTokens};

const HOME: Keysym = Keysym(0xff50);
const LEFT: Keysym = Keysym(0xff51);
const UP: Keysym = Keysym(0xff52);
const RIGHT: Keysym = Keysym(0xff53);
const DOWN: Keysym = Keysym(0xff54);
const PAGE_UP: Keysym = Keysym(0xff55);
const PAGE_DOWN: Keysym = Keysym(0xff56);
const END: Keysym = Keysym(0xff57);

/// A level slider's track and knob.
const LEVEL: f64 = 22.0;
/// The symbol inside a level slider's minimum end.
const LEVEL_SYMBOL: f64 = 14.0;
const PANEL_TRACK_BORDER: f64 = 0.5;
/// A tick mark: a round dot under the track at each step position.
const TICK_DOT: f64 = 2.0;
/// Between the track's bottom and the dots under it.
const TICK_GAP: f64 = 3.0;
/// How much of the secondary text colour a tick dot keeps: lighter than the text.
const TICK_SHARE: f32 = 0.6;
/// The ring around the knob, and its width with Increase Contrast, where the knob takes the colour of the fill it sits
/// on and only the ring tells them apart.
const KNOB_RING: f64 = 1.0;
const KNOB_RING_HIGH_CONTRAST: f64 = 2.0;

/// How long, in seconds, a regular knob takes to swell into its lens when it is taken hold of, and to sink back
/// into the plain knob when it is let go.
const LENS_IN: f64 = 0.15;
const LENS_OUT: f64 = 0.12;
/// How long the knob takes to reach a press on the track away from it, or the next stop of a slider with stops
/// moved by a key, in seconds. A slider without stops moved by a key steps at once.
const KNOB_TRAVEL: f64 = 0.2;

/// The node state of a [`Slider`]. Opaque.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SliderState {
    hovered: bool,
    dragging: bool,
    /// The value under the pointer while it drags: what the knob shows and the drag ends at, even before the
    /// application's next view carries it.
    held: Option<f64>,
    /// The value the knob is drawn at, on its way to the one it shows.
    knob: Tween,
    /// How far the knob has swelled into its lens, 0 to 1.
    lift: Tween,
}

/// The soft shade under a regular slider's white knob at rest.
const KNOB_SHADE: f32 = 0.28;
const KNOB_SHADE_BLUR: f64 = 1.5;
const KNOB_SHADE_OFFSET: f64 = 0.5;

/// The track's height and the knob's width and height: a regular slider's from the control size table (a capsule
/// knob wider than it is high), a level slider's a round knob as high as its wide track.
fn sizes(style: SliderStyle, theme: &ThemeTokens) -> (f64, Size) {
    match style {
        SliderStyle::Regular => {
            let g = slider_geometry(theme.metrics.size);
            (g.track, Size::new(g.knob.0, g.knob.1))
        }
        SliderStyle::Level | SliderStyle::PanelLevel(_) => (LEVEL, Size::new(LEVEL, LEVEL)),
    }
}

impl<M> Slider<M> {
    fn span(&self) -> f64 {
        self.max - self.min
    }

    /// `value` clamped to the range and snapped to the step.
    fn fit(&self, value: f64) -> f64 {
        let value = value.clamp(self.min, self.max.max(self.min));
        match self.step {
            Some(step) if step > 0.0 => {
                (self.min + ((value - self.min) / step).round() * step).min(self.max)
            }
            _ => value,
        }
    }

    /// Where `value` sits between the ends, 0 to 1.
    fn fraction(&self, value: f64) -> f64 {
        if self.span() > 0.0 {
            ((value - self.min) / self.span()).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    /// How far the arrow keys move.
    fn key_step(&self) -> f64 {
        self.step
            .filter(|s| *s > 0.0)
            .unwrap_or(self.span() / 100.0)
    }

    /// The x the knob's centre runs along, from the minimum's end to the maximum's.
    fn run(&self, width: f64, knob: f64, direction: LayoutDirection) -> (f64, f64) {
        let (start, end) = (knob / 2.0, width - knob / 2.0);
        match direction {
            LayoutDirection::Ltr => (start, end),
            LayoutDirection::Rtl => (end, start),
        }
    }

    /// The value at window x `x` within a node at `bounds`.
    fn value_at(&self, x: f64, bounds: Rect, knob: f64, direction: LayoutDirection) -> f64 {
        let (from, to) = self.run(bounds.width(), knob, direction);
        let local = x - bounds.min_x();
        let t = if to != from {
            ((local - from) / (to - from)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.fit(self.min + t * self.span())
    }
}

impl<M: 'static> Slider<M> {
    /// Move to `value` from `shown`, the value the knob shows now.
    fn set_from(&self, shown: f64, value: f64, cx: &mut EventCx<'_, M>) {
        let value = self.fit(value);
        if value != shown {
            if let Some(on_change) = &self.on_change {
                cx.emit(on_change(value));
            }
        }
    }

    fn end(&self, value: f64, cx: &mut EventCx<'_, M>) {
        if let Some(on_end) = &self.on_end {
            cx.emit(on_end(value));
        }
    }

    /// The pointer drags the knob to `value`: it shows there at once, and the application hears of it when it moved.
    fn drag_to(&self, state: &mut SliderState, value: f64, cx: &mut EventCx<'_, M>) {
        let value = self.fit(value);
        let shown = state.held.unwrap_or(self.value);
        state.held = Some(value);
        state.knob.jump(value);
        self.set_from(shown, value, cx);
        if value != shown {
            cx.request_paint();
        }
    }

    /// The drag ends: at the value the pointer left the knob at, not the one the application last showed.
    fn let_go(&self, state: &mut SliderState, cx: &mut EventCx<'_, M>) {
        state.dragging = false;
        state.lift.go(0.0, LENS_OUT, 0.0, Curve::EaseOut);
        let value = state.held.take().unwrap_or(self.value);
        cx.request_paint();
        self.end(value, cx);
    }

    /// A key or assistive technology moves the value in one step: the change also ends there.
    fn step_to(&self, value: f64, cx: &mut EventCx<'_, M>) {
        let value = self.fit(value);
        if value != self.value {
            self.set_from(self.value, value, cx);
            self.end(value, cx);
        }
    }
}

impl<M: 'static> Widget<M> for Slider<M> {
    type State = SliderState;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) -> SliderState {
        SliderState {
            knob: Tween::at(self.value),
            ..SliderState::default()
        }
    }

    fn update(&self, old: &Self, state: &mut SliderState) -> Change {
        // the knob goes where the view puts it, unless the pointer holds it
        if !state.dragging && self.value != state.knob.target() {
            match self.step {
                Some(step) if step > 0.0 => {
                    state.knob.go(self.value, KNOB_TRAVEL, 0.0, Curve::EaseOut)
                }
                _ => state.knob.jump(self.value),
            }
        }
        if self.style != old.style || self.ticks != old.ticks {
            Change::Layout
        } else if self.value != old.value
            || self.min != old.min
            || self.max != old.max
            || self.step != old.step
            || self.enabled != old.enabled
            || self.label != old.label
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(
        &self,
        _: &SliderState,
        constraints: Constraints,
        cx: &mut MeasureCx<'_>,
    ) -> Measured {
        let theme = &cx.env().theme;
        let (track, knob) = sizes(self.style, theme);
        // the dots hang under the track, inside the knob's height unless the knob is small
        let ticks = if self.ticks >= 2 && self.style == SliderStyle::Regular {
            track + 2.0 * (TICK_GAP + TICK_DOT)
        } else {
            0.0
        };
        // the held knob's lens reaches past this height, but only its paint: the layout never changes
        let height = knob.height.max(ticks);
        Measured {
            size: Size::new(constraints.width.ideal().max(knob.width), height),
            baseline: None,
        }
    }

    fn target(&self, _: &SliderState, env: &Environment) -> Size {
        env.theme.metrics.min_target
    }

    fn event(
        &self,
        state: &mut SliderState,
        event: &Event,
        cx: &mut EventCx<'_, M>,
    ) -> Propagation {
        let direction = cx.env().direction;
        let (_, knob) = sizes(self.style, &cx.env().theme);
        let knob = knob.width;
        match *event {
            Event::Pointer(PointerEvent::Enter { .. }, _) if cx.node() == cx.target() => {
                state.hovered = true;
                cx.request_paint();
                Propagation::Continue
            }
            Event::Pointer(PointerEvent::Leave, _) if cx.node() == cx.target() => {
                state.hovered = false;
                cx.request_paint();
                Propagation::Continue
            }
            Event::Pointer(
                PointerEvent::Button {
                    position,
                    button: MouseButton::Left,
                    pressed: true,
                    ..
                },
                _,
            ) if self.enabled && cx.node() == cx.target() => {
                state.dragging = true;
                state.lift.go(1.0, LENS_IN, 0.0, Curve::EaseInOut);
                cx.capture_pointer();
                cx.request_focus();
                if let Some(bounds) = cx.bounds() {
                    let from = state.knob.target();
                    let value = self.value_at(position.x, bounds, knob, direction);
                    self.drag_to(state, value, cx);
                    // pressed away from the knob, the knob glides there; the value is there at once
                    state.knob.jump(from);
                    state
                        .knob
                        .go(self.fit(value), KNOB_TRAVEL, 0.0, Curve::EaseOut);
                }
                cx.request_paint();
                Propagation::Stop
            }
            Event::Pointer(PointerEvent::Motion { position }, _) if state.dragging => {
                if let Some(bounds) = cx.bounds() {
                    self.drag_to(
                        state,
                        self.value_at(position.x, bounds, knob, direction),
                        cx,
                    );
                }
                Propagation::Stop
            }
            Event::Pointer(PointerEvent::Button { pressed: false, .. }, _) if state.dragging => {
                cx.release_pointer();
                self.let_go(state, cx);
                Propagation::Stop
            }
            Event::Cancel if state.dragging => {
                self.let_go(state, cx);
                Propagation::Continue
            }
            Event::Key(key, _) if self.enabled && key.pressed && cx.node() == cx.target() => {
                let forward = match direction {
                    LayoutDirection::Ltr => RIGHT,
                    LayoutDirection::Rtl => LEFT,
                };
                let back = if forward == RIGHT { LEFT } else { RIGHT };
                let step = self.key_step();
                let page = self.span() / 10.0;
                let value = match key.keysym {
                    k if k == forward || k == UP => self.value + step,
                    k if k == back || k == DOWN => self.value - step,
                    PAGE_UP => self.value + page,
                    PAGE_DOWN => self.value - page,
                    HOME => self.min,
                    END => self.max,
                    _ => return Propagation::Continue,
                };
                self.step_to(value, cx);
                Propagation::Stop
            }
            Event::Action(ref request) if self.enabled => {
                let value = match (request.action, &request.data) {
                    (Action::Increment, _) => self.value + self.key_step(),
                    (Action::Decrement, _) => self.value - self.key_step(),
                    (Action::SetValue, Some(ActionData::Number(value))) => *value,
                    (Action::Focus, _) => {
                        cx.request_focus();
                        return Propagation::Stop;
                    }
                    _ => return Propagation::Continue,
                };
                self.step_to(value, cx);
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, state: &SliderState, cx: &mut PaintCx<'_>) {
        let theme = cx.env().theme.clone();
        let direction = cx.env().direction;
        let look_state = ControlState {
            enabled: self.enabled,
            hovered: state.hovered,
            pressed: state.dragging,
            focused: cx.is_focused(),
            focus_visible: cx.focus_visible(),
            window_active: cx.window_active(),
            ..ControlState::default()
        };
        let filled_state = ControlState {
            selected: true,
            ..look_state
        };
        let size = cx.size();
        let (track_height, knob_size) = sizes(self.style, &theme);
        let knob = knob_size.width;
        let mid = size.height / 2.0;
        let track = Rect::new(0.0, mid - track_height / 2.0, size.width, track_height);
        let radius = track_height / 2.0;
        // a quiet well, which neither darkens under the pointer nor while dragged; a strong one under increased
        // contrast
        let resting = ControlState {
            hovered: false,
            pressed: false,
            ..look_state
        };
        let empty = match self.style {
            SliderStyle::PanelLevel(_) => look::color(&theme, ColorRole::PanelTrack, &look_state),
            _ if theme.preferences.appearance.is_high_contrast() => {
                look::color(&theme, ColorRole::SliderTrack, &look_state)
            }
            _ => Some(look::quiet(&theme, look::WELL, &resting)),
        };
        look::fill(cx, track, radius, empty);
        if let SliderStyle::PanelLevel(_) = self.style {
            look::stroke(
                cx,
                track.inset(Insets::uniform(PANEL_TRACK_BORDER / 2.0)),
                radius - PANEL_TRACK_BORDER / 2.0,
                PANEL_TRACK_BORDER,
                look::color(&theme, ColorRole::PanelTrackBorder, &look_state),
            );
        }
        let (from, to) = self.run(size.width, knob, direction);
        // dragged, the knob stays under the pointer while the application catches up
        let centre = from + (to - from) * self.fraction(state.knob.sample(cx));
        // the level style fills to the knob's far edge, the regular one to its centre
        let reach = if self.style != SliderStyle::Regular {
            knob / 2.0
        } else {
            0.0
        };
        let filled = match direction {
            LayoutDirection::Ltr => Rect::new(0.0, track.min_y(), centre + reach, track_height),
            LayoutDirection::Rtl => {
                let start = centre - reach;
                Rect::new(start, track.min_y(), size.width - start, track_height)
            }
        };
        // a disabled track shows no fill, so it never reads lighter than the empty part; the knob shows the value
        if self.enabled {
            let fill = match self.style {
                SliderStyle::PanelLevel(_) => {
                    look::color(&theme, ColorRole::SwitchThumb, &look_state)
                }
                _ => look::color(&theme, ColorRole::ControlFill, &filled_state),
            };
            look::fill(cx, filled, radius, fill);
        }
        if self.ticks >= 2 && self.style == SliderStyle::Regular {
            let dot = look::faded(
                look::color(&theme, ColorRole::SecondaryText, &look_state),
                TICK_SHARE,
            );
            for i in 0..self.ticks {
                let x = from + (to - from) * f64::from(i) / f64::from(self.ticks - 1);
                let at = Rect::new(
                    x - TICK_DOT / 2.0,
                    track.max_y() + TICK_GAP,
                    TICK_DOT,
                    TICK_DOT,
                );
                look::fill(cx, at, TICK_DOT / 2.0, dot);
            }
        }
        if let SliderStyle::PanelLevel(name) = self.style {
            let x = match direction {
                LayoutDirection::Ltr => 0.0,
                LayoutDirection::Rtl => size.width - track_height,
            };
            let area = Rect::new(x, track.min_y(), track_height, track_height);
            // the fill reaches past the symbol at every value
            let role = if self.enabled {
                ColorRole::SwitchThumbGlyph
            } else {
                ColorRole::DisabledControlText
            };
            if let Some(color) = look::color(&theme, role, &look_state) {
                symbol(cx, name, LEVEL_SYMBOL, color, false, area);
            }
        }
        let thumb = Rect::new(
            centre - knob / 2.0,
            mid - knob_size.height / 2.0,
            knob,
            knob_size.height,
        );
        let knob_radius = knob_size.height / 2.0;
        let lift = if self.enabled && self.style == SliderStyle::Regular {
            state.lift.sample(cx)
        } else {
            0.0
        };
        if lift < 1.0 {
            // the plain knob fades as the lens swells out of it
            if lift > 0.0 {
                cx.push(PaintOp::PushLayer {
                    opacity: 1.0 - lift as f32,
                    effects: Vec::new(),
                });
            }
            self.paint_knob(&theme, &look_state, thumb, knob_radius, cx);
            if lift > 0.0 {
                cx.push(PaintOp::PopLayer);
            }
        }
        if let Some(lens) = Lens::lifted(LensKind::Slider, thumb, lift) {
            // held, the knob is a lens: the track and its fill show through it, shrunk about its centre
            let fill = look::color(&theme, ColorRole::ControlFill, &filled_state);
            paint_lens_faded(cx, &lens, lift as f32, |cx, lens| {
                let r = radius * lens.minify;
                look::fill(cx, lens.through(track), r, empty);
                look::fill(cx, lens.through(filled), r, fill);
            });
        }
        paint_focus_ring(cx, thumb, knob_radius);
    }

    fn semantics(&self, _: &SliderState, cx: &mut SemanticsCx<'_>) {
        self.semantics_of(cx);
    }
}

impl<M> Slider<M> {
    /// The knob at rest: a white capsule with a soft shade, or a ringed one.
    fn paint_knob(
        &self,
        theme: &ThemeTokens,
        look_state: &ControlState,
        thumb: Rect,
        knob_radius: f64,
        cx: &mut PaintCx<'_>,
    ) {
        let contrast = theme.preferences.appearance.is_high_contrast();
        let ring = if contrast {
            KNOB_RING_HIGH_CONTRAST
        } else {
            KNOB_RING
        };
        // a regular knob stands out by a soft shade; a level knob on a fill of its own colour by its ring
        let ringed = contrast || self.style != SliderStyle::Regular;
        if !ringed && self.enabled {
            cx.push(PaintOp::PushLayer {
                opacity: 1.0,
                effects: vec![Effect::Blur {
                    radius: KNOB_SHADE_BLUR,
                }],
            });
            look::fill(
                cx,
                thumb.translate(0.0, KNOB_SHADE_OFFSET),
                knob_radius,
                Some(Color::new(0.0, 0.0, 0.0, KNOB_SHADE)),
            );
            cx.push(PaintOp::PopLayer);
        }
        look::fill(
            cx,
            thumb,
            knob_radius,
            look::color(theme, ColorRole::SwitchThumb, look_state),
        );
        if ringed {
            look::stroke(
                cx,
                thumb.inset(Insets::uniform(ring / 2.0)),
                knob_radius - ring / 2.0,
                ring,
                look::color(theme, ColorRole::ControlBorder, look_state),
            );
        }
    }

    fn semantics_of(&self, cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Slider);
        cx.name(self.label.clone());
        cx.value(AccessibleValue::Number {
            value: self.value,
            min: self.min,
            max: self.max,
            step: self.step,
        });
        cx.states(AccessibleStates {
            disabled: !self.enabled,
            ..AccessibleStates::default()
        });
        if self.enabled {
            cx.focusable();
            cx.action(Action::Focus);
            cx.action(Action::Increment);
            cx.action(Action::Decrement);
            cx.action(Action::SetValue);
        }
    }
}
