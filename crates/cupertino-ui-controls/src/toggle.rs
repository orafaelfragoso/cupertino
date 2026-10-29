use crate::look::{self, paint_focus_ring};
use crate::press::{Keys, Outcome, Press};
use crate::Toggle;
use cupertino_geometry::{Rect, Size};
use cupertino_ui_core::{
    AccessibleStates, AccessibleValue, Action, Change, Checked, ControlState, Event, EventCx,
    MeasureCx, PaintCx, Propagation, Role, SemanticsCx, Widget,
};
use cupertino_ui_layout::{Constraints, ContainerKind, LayoutDirection, LayoutSpec, Measured};
use cupertino_ui_theme::{ColorRole, ControlMetrics};

impl<M: 'static> Toggle<M> {
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

/// The pill, centred in a node padded out to the minimum target.
fn track(size: Size, metrics: &ControlMetrics) -> Rect {
    Rect::new(
        (size.width - metrics.switch.width) / 2.0,
        (size.height - metrics.switch.height) / 2.0,
        metrics.switch.width,
        metrics.switch.height,
    )
}

/// The thumb sits at the end when on and at the start when off; the end is on the left in right-to-left layouts.
pub(crate) fn thumb(
    track: Rect,
    on: bool,
    direction: LayoutDirection,
    metrics: &ControlMetrics,
) -> Rect {
    let d = metrics.switch_thumb;
    let inset = metrics.switch_thumb_inset;
    let at_right = on == (direction == LayoutDirection::Ltr);
    let x = if at_right {
        track.max_x() - inset - d
    } else {
        track.min_x() + inset
    };
    Rect::new(x, track.min_y() + (track.height() - d) / 2.0, d, d)
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
        let metrics = cx.env().theme.metrics;
        Measured {
            size: Size::new(
                metrics.switch.width.max(metrics.min_target.width),
                metrics.switch.height.max(metrics.min_target.height),
            ),
            baseline: None,
        }
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
        let theme = cx.env().theme.clone();
        let direction = cx.env().direction;
        let state = self.control_state(
            press,
            cx.is_focused(),
            cx.focus_visible(),
            cx.window_active(),
        );
        let track = track(cx.size(), &theme.metrics);
        let radius = track.height() / 2.0;
        look::fill(
            cx,
            track,
            radius,
            look::color(&theme, ColorRole::SwitchTrack, &state),
        );
        let thumb = thumb(track, self.on, direction, &theme.metrics);
        look::fill(
            cx,
            thumb,
            thumb.height() / 2.0,
            look::color(&theme, ColorRole::SwitchThumb, &state),
        );
        paint_focus_ring(cx, track, radius);
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
