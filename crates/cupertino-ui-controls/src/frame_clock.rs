//! A content's frame clock: while it holds a message, the window's every frame callback sends that message once, so
//! what an application moves by itself advances with the display's frames instead of a timer of its own.

use crate::FrameClock;
use cupertino_geometry::Size;
use cupertino_ui_core::{
    Change, Element, Event, EventCx, MeasureCx, PaintCx, Propagation, SemanticsCx, Widget,
};
use cupertino_ui_layout::{Constraints, ContainerKind, LayoutSpec, Measured};

impl<M: Clone + 'static> From<FrameClock<M>> for Element<M> {
    fn from(clock: FrameClock<M>) -> Self {
        Element::new(Clock {
            children: vec![clock.content],
            on_frame: clock.on_frame,
        })
    }
}

/// The content, the size of the content, and the message each frame sends.
struct Clock<M> {
    children: Vec<Element<M>>,
    on_frame: Option<M>,
}

impl<M: Clone + 'static> Widget<M> for Clock<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Overlay)
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    /// Starting to ask paints the node, which asks for the first frame.
    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.on_frame.is_some() && old.on_frame.is_none() {
            Change::Paint
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

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        if let (Event::AnimationFrame(_), Some(message)) = (event, &self.on_frame) {
            if cx.node() == cx.target() {
                cx.emit(message.clone());
                cx.request_animation_frame();
            }
        }
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        if self.on_frame.is_some() {
            cx.request_animation_frame();
        }
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}
