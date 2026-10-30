use crate::{look, Progress};
use cupertino_geometry::{Rect, Size};
use cupertino_ui_core::{
    AccessibleStates, AccessibleValue, Change, ControlState, Event, EventCx, MeasureCx, PaintCx,
    Propagation, Role, SemanticsCx, Widget,
};
use cupertino_ui_layout::{
    Constraints, ContainerKind, LayoutDirection, LayoutSpec, Measured, SizeSpec,
};
use cupertino_ui_render::{PaintOp, Shape};
use cupertino_ui_theme::ColorRole;

const TRACK: f64 = 6.0;
/// The moving segment of an indeterminate bar, as a share of the track.
const SEGMENT: f64 = 0.3;
/// One sweep from the leading edge to the trailing one.
const SWEEP_NS: u64 = 1_200_000_000;

impl<M: 'static> Widget<M> for Progress {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self != old {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), constraints: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(constraints.width.ideal(), TRACK),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        if let Event::AnimationFrame(_) = event {
            cx.request_paint();
        }
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let theme = cx.env().theme.clone();
        let direction = cx.env().direction;
        let state = ControlState {
            enabled: true,
            window_active: cx.window_active(),
            ..ControlState::default()
        };
        let filled = ControlState {
            selected: true,
            ..state
        };
        let width = cx.size().width;
        let track = Rect::new(0.0, 0.0, width, TRACK);
        let radius = TRACK / 2.0;
        look::fill(
            cx,
            track,
            radius,
            look::color(&theme, ColorRole::SwitchTrack, &state),
        );
        // the fill as a span from the leading edge, in the track's own direction
        let (start, length) = match self.value {
            Some(value) => (0.0, width * value.clamp(0.0, 1.0)),
            None if theme.motion.reduce => {
                let still = look::color(&theme, ColorRole::SecondarySelectedControlFill, &state);
                look::fill(cx, track, radius, still);
                return;
            }
            None => {
                cx.request_animation_frame();
                let segment = width * SEGMENT;
                let phase = (cx.now().0 % SWEEP_NS) as f64 / SWEEP_NS as f64;
                (phase * (width + segment) - segment, segment)
            }
        };
        let x = match direction {
            LayoutDirection::Ltr => start,
            LayoutDirection::Rtl => width - start - length,
        };
        cx.push(PaintOp::PushClip(Shape::RoundedRect {
            rect: track,
            radius,
        }));
        look::fill(
            cx,
            Rect::new(x, 0.0, length, TRACK),
            radius,
            look::color(&theme, ColorRole::ControlFill, &filled),
        );
        cx.push(PaintOp::PopClip);
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::ProgressIndicator);
        cx.name(self.label.clone());
        if let Some(value) = self.value {
            cx.value(AccessibleValue::Number {
                value,
                min: 0.0,
                max: 1.0,
                step: None,
            });
        }
        cx.states(AccessibleStates {
            busy: self.value.is_none(),
            ..AccessibleStates::default()
        });
    }
}
