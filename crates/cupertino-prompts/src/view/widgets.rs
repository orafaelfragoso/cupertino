//! The widgets the alerts are made of besides the standard controls: the alert's body on glass, the frame that takes
//! Escape and names the alert to assistive technology, and the pacer that asks for frames while the scrim fades in.

use cupertino_kit::core::{
    AccessibleStates, Change, Element, Event, EventCx, Glass, MeasureCx, PaintCx, Phase,
    Propagation, Role, SemanticsCx, Widget,
};
use cupertino_kit::geometry::{Insets, Rect, Size};
use cupertino_kit::layout::{
    Align, Axis, Constraints, ContainerKind, Justify, LayoutSpec, Measured, SizeSpec,
};
use cupertino_kit::motion::Timing;
use cupertino_kit::platform::Keysym;
use cupertino_kit::render::{Paint, PaintOp, Shape};
use cupertino_kit::theme::{system_color, Color, ColorRole, StateVariant, Surface};
use std::time::Instant;

/// Escape: the alert closes without an answer.
pub const ESCAPE: Keysym = Keysym(0xff1b);

fn no_measure() -> Measured {
    Measured {
        size: Size::ZERO,
        baseline: None,
    }
}

/// Width of the outline an alert on glass draws round itself with Increase Contrast, in points: a whole device pixel or
/// more on every display.
pub const ALERT_CONTRAST_RIM: f64 = 1.0;

/// An alert's body on glass, `width` wide and as tall as its content, which it pads by `padding`: its corners rounded
/// by `radius`, and the popover surface's fill where the window cannot show glass.
pub struct AlertBody<M> {
    pub width: f64,
    pub radius: f64,
    pub padding: Insets,
    pub children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for AlertBody<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.width),
            height: SizeSpec::Fit,
            insets: self.padding,
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Vertical,
                spacing: 0.0,
                align: Align::Stretch,
                justify: Justify::Start,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.width != old.width || self.padding != old.padding {
            Change::Layout
        } else if self.radius != old.radius {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        if cx.glass(Glass::regular().radius(self.radius)) {
            // with Increase Contrast the glass takes the contrast face, which over a window of the same appearance is
            // that window's tone: the alert marks its edge with the outline its buttons have
            let theme = cx.env().theme.clone();
            let rim = theme
                .colors
                .get(ColorRole::ControlBorder, StateVariant::NORMAL);
            if let (true, Some(rim)) = (theme.preferences.appearance.is_high_contrast(), rim) {
                let half = ALERT_CONTRAST_RIM / 2.0;
                cx.push(PaintOp::Stroke {
                    shape: Shape::RoundedRect {
                        rect: Rect::new(
                            half,
                            half,
                            size.width - ALERT_CONTRAST_RIM,
                            size.height - ALERT_CONTRAST_RIM,
                        ),
                        radius: (self.radius - half).max(0.0),
                    },
                    width: ALERT_CONTRAST_RIM,
                    paint: Paint::Solid(rim),
                });
            }
            return;
        }
        if let Some(color) = cx.env().theme.surface(Surface::Popover).fill {
            cx.push(PaintOp::Fill {
                shape: Shape::RoundedRect {
                    rect: Rect::new(0.0, 0.0, size.width, size.height),
                    radius: self.radius,
                },
                paint: Paint::Solid(color),
            });
        }
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// An alert's window content: its children centred in the window, Escape (and the keys `keys` maps) taken before the
/// focused control sees them, and the alert named to assistive technology as a modal alert dialog.
pub struct AlertFrame<M> {
    /// What a key the alert takes means; `None` passes the key on to the focus.
    pub keys: Box<dyn Fn(Keysym) -> Option<M>>,
    pub label: String,
    pub description: Option<String>,
    pub children: Vec<Element<M>>,
}

impl<M: Clone + 'static> Widget<M> for AlertFrame<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Vertical,
                spacing: 0.0,
                align: Align::Center,
                justify: Justify::Center,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.label != old.label || self.description != old.description {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        if !matches!(cx.phase(), Phase::Capture | Phase::Target) {
            return Propagation::Continue;
        }
        match event {
            Event::Key(key, _) if key.pressed => match (self.keys)(key.keysym) {
                Some(message) => {
                    cx.emit(message);
                    Propagation::Stop
                }
                None => Propagation::Continue,
            },
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::AlertDialog);
        cx.name(self.label.clone());
        if let Some(description) = &self.description {
            cx.description(description.clone());
        }
        cx.states(AccessibleStates {
            modal: true,
            ..AccessibleStates::default()
        });
    }
}

/// Its children at a fixed size, filling it: a button at its frame. `initial` gives its first control the focus when the
/// window opens.
pub struct Sized<M> {
    pub size: Size,
    pub initial: bool,
    pub children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for Sized<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.size.width),
            height: SizeSpec::Fixed(self.size.height),
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Vertical,
                spacing: 0.0,
                align: Align::Stretch,
                justify: Justify::Center,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.size != old.size {
            Change::Layout
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: self.size,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        if self.initial {
            cx.initial_focus();
        }
    }
}

/// The scrim over an output: the system's authentication scrim colour all over, fading in from `since` along
/// [`Timing::AUTHENTICATION_SCRIM_FADE_IN`] (at once under Reduce Motion, or without `since`), `frame` sent each frame
/// while it fades. Hidden from assistive technology, which hears the alert instead.
pub struct Scrim<M> {
    pub since: Option<Instant>,
    pub frame: M,
}

impl<M> Scrim<M> {
    /// Its opacity now, and whether it still fades.
    fn opacity(&self, reduce_motion: bool) -> (f64, bool) {
        match self.since.filter(|_| !reduce_motion) {
            None => (1.0, false),
            Some(since) => {
                let fade = Timing::AUTHENTICATION_SCRIM_FADE_IN;
                let elapsed = since.elapsed().as_secs_f64();
                let opacity = fade.value(elapsed).clamp(0.0, 1.0);
                (opacity, opacity < 1.0)
            }
        }
    }
}

impl<M: Clone + 'static> Widget<M> for Scrim<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        // painted again with each new view, so a fading scrim shows its next frame
        Change::Paint
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        if let Event::AnimationFrame(_) = event {
            cx.emit(self.frame.clone());
        }
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        let preferences = cx.env().theme.preferences;
        let (opacity, fading) = self.opacity(preferences.reduce_motion);
        if let Some(color) = system_color("authentication-scrim", preferences.appearance) {
            cx.push(PaintOp::Fill {
                shape: Shape::Rect(Rect::new(0.0, 0.0, size.width, size.height)),
                paint: Paint::Solid(Color::new(
                    color.r,
                    color.g,
                    color.b,
                    color.a * opacity as f32,
                )),
            });
        }
        if fading {
            cx.request_animation_frame();
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.hidden();
    }
}
