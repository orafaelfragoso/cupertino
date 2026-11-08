use crate::look::{self, paint_focus_ring};
use crate::press::{Keys, Outcome, Press};
use crate::{SmallToggle, Toggle};
use cupertino_geometry::{Rect, Size};
use cupertino_ui_core::{
    AccessibleStates, AccessibleValue, Action, Change, Checked, ControlState, Environment, Event,
    EventCx, MeasureCx, PaintCx, Propagation, Role, SemanticsCx, Widget,
};
use cupertino_ui_layout::{Constraints, ContainerKind, LayoutDirection, LayoutSpec, Measured};
use cupertino_ui_theme::{ColorRole, ControlMetrics};

impl<M: 'static> Toggle<M> {
    fn paint_shaped(&self, press: &Press, cx: &mut PaintCx<'_>, shape: &SwitchShape) {
        let theme = cx.env().theme.clone();
        let direction = cx.env().direction;
        let state = self.control_state(
            press,
            cx.is_focused(),
            cx.focus_visible(),
            cx.window_active(),
        );
        let track = track(cx.size(), shape);
        let radius = track.height() / 2.0;
        look::fill(
            cx,
            track,
            radius,
            look::color(&theme, ColorRole::SwitchTrack, &state),
        );
        let thumb = thumb(track, self.on, direction, shape);
        look::fill(
            cx,
            thumb,
            thumb.height() / 2.0,
            look::color(&theme, ColorRole::SwitchThumb, &state),
        );
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
    fn regular(metrics: &ControlMetrics) -> SwitchShape {
        SwitchShape {
            track: metrics.switch,
            thumb: Size::new(metrics.switch_thumb, metrics.switch_thumb),
            inset: metrics.switch_thumb_inset,
        }
    }

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
fn thumb(track: Rect, on: bool, direction: LayoutDirection, shape: &SwitchShape) -> Rect {
    let Size { width, height } = shape.thumb;
    let at_right = on == (direction == LayoutDirection::Ltr);
    let x = if at_right {
        track.max_x() - shape.inset - width
    } else {
        track.min_x() + shape.inset
    };
    Rect::new(
        x,
        track.min_y() + (track.height() - height) / 2.0,
        width,
        height,
    )
}

impl<M: 'static> Toggle<M> {
    /// The same switch at the small size of a settings form's rows: a 36 × 16 pt track and a capsule thumb.
    pub fn small(self) -> SmallToggle<M> {
        SmallToggle { toggle: self }
    }
}

impl<M: 'static> Widget<M> for Toggle<M> {
    type State = Press;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) -> Press {
        Press::default()
    }

    fn update(&self, old: &Self, _: &mut Press) -> Change {
        if self.on != old.on || self.enabled != old.enabled || self.label != old.label {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &Press, _: Constraints, cx: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: SwitchShape::regular(&cx.env().theme.metrics).track,
            baseline: None,
        }
    }

    fn target(&self, _: &Press, env: &Environment) -> Size {
        env.theme.metrics.min_target
    }

    fn event(&self, press: &mut Press, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match press.event(event, self.enabled, Keys::Space, cx) {
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

    fn paint(&self, press: &Press, cx: &mut PaintCx<'_>) {
        let shape = SwitchShape::regular(&cx.env().theme.metrics);
        self.paint_shaped(press, cx, &shape);
    }

    fn semantics(&self, _: &Press, cx: &mut SemanticsCx<'_>) {
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
    type State = Press;

    fn layout(&self) -> LayoutSpec {
        self.toggle.layout()
    }

    fn state(&self) -> Press {
        Press::default()
    }

    fn update(&self, old: &Self, state: &mut Press) -> Change {
        self.toggle.update(&old.toggle, state)
    }

    fn measure(&self, _: &Press, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: SwitchShape::SMALL.track,
            baseline: None,
        }
    }

    fn target(&self, press: &Press, env: &Environment) -> Size {
        self.toggle.target(press, env)
    }

    fn event(&self, press: &mut Press, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        self.toggle.event(press, event, cx)
    }

    fn paint(&self, press: &Press, cx: &mut PaintCx<'_>) {
        self.toggle.paint_shaped(press, cx, &SwitchShape::SMALL);
    }

    fn semantics(&self, press: &Press, cx: &mut SemanticsCx<'_>) {
        self.toggle.semantics(press, cx)
    }
}
