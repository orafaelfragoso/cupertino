//! The toolbar's search field in a slot that tells the model when the keyboard comes and goes and drives the clock of
//! the field's width ([`crate::search_field::SearchWidth`]), and the "No Results" a search with no matches shows over
//! the folder's view.

use cupertino_kit::controls::{Text, TextRole};
use cupertino_kit::core::{
    Change, Element, Event, EventCx, MeasureCx, PaintCx, Phase, Propagation, SemanticsCx, Widget,
};
use cupertino_kit::geometry::{Insets, Point};
use cupertino_kit::layout::{
    Align, Axis, Constraints, ContainerKind, Justify, LayoutSpec, Measured, SizeSpec,
};
use cupertino_kit::platform::Timestamp;
use cupertino_kit::text::{AttributedText, TextStyle};

use super::{Event as WindowEvent, Message};
use cupertino_kit::geometry::Size;

/// What a search with no matches says.
pub(super) const NO_RESULTS: &str = "No Results";

fn seconds(t: Timestamp) -> f64 {
    t.0 as f64 / 1e9
}

fn no_measure() -> Measured {
    Measured {
        size: Size::ZERO,
        baseline: None,
    }
}

/// Holds the search field. It reports the keyboard coming into and leaving the field, and while the width changes it
/// asks for a frame after each frame and reports the frame's time.
pub(super) struct SearchSlot {
    pub window: u64,
    /// The field's width is changing.
    pub animating: bool,
    pub children: Vec<Element<Message>>,
}

impl Widget<Message> for SearchSlot {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Stack {
            axis: Axis::Horizontal,
            spacing: 0.0,
            align: Align::Center,
            justify: Justify::Start,
        })
    }

    fn take_children(&mut self) -> Vec<Element<Message>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.animating && !old.animating {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, Message>) -> Propagation {
        if cx.phase() == Phase::Capture {
            return Propagation::Continue;
        }
        let animate = !cx.env().theme.motion.reduce;
        // the model says after each tick whether the width still changes; the frame after it is asked for here,
        // and goes unused once it does not
        match *event {
            Event::FocusWithin { gained, .. } => {
                cx.emit(Message::Window(
                    self.window,
                    WindowEvent::SearchFocus {
                        focused: gained,
                        now: seconds(cx.now()),
                        animate,
                    },
                ));
                cx.request_animation_frame();
            }
            Event::AnimationFrame(now) if self.animating => {
                cx.emit(Message::Window(
                    self.window,
                    WindowEvent::SearchTick {
                        now: seconds(now),
                        animate,
                    },
                ));
                cx.request_animation_frame();
            }
            _ => {}
        }
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        if self.animating {
            cx.request_animation_frame();
        }
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// `view` with "No Results" centred over it, `top` points below its top edge (a list's header stays clear).
pub(super) fn no_results(view: Element<Message>, top: f64) -> Element<Message> {
    let message = Element::new(Text {
        content: AttributedText::plain(NO_RESULTS.to_owned(), TextStyle::new(17.0)),
        // Kit has no 17 pt regular role; the title role is the nearest (approximate)
        role: TextRole::Title,
        wrap: false,
        secondary: true,
        selected: false,
    })
    .key("no-results");
    let layer = PassThrough {
        top,
        children: vec![message],
    };
    Element::new(Layers {
        children: vec![view.fill(), Element::new(layer).fill()],
    })
    .fill()
}

/// Its children over each other, each as large as it.
struct Layers {
    children: Vec<Element<Message>>,
}

impl Widget<Message> for Layers {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Overlay)
        }
    }

    fn take_children(&mut self) -> Vec<Element<Message>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, Message>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// A layer the pointer passes through to what lies under it, except where a child of it is: its children centred,
/// `top` points below its top edge.
struct PassThrough {
    top: f64,
    children: Vec<Element<Message>>,
}

impl Widget<Message> for PassThrough {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            insets: Insets {
                top: self.top,
                left: 0.0,
                bottom: 0.0,
                right: 0.0,
            },
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Vertical,
                spacing: 0.0,
                align: Align::Center,
                justify: Justify::Center,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<Message>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, Message>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}

    fn hit(&self, _: &(), _: Point) -> bool {
        false
    }
}
