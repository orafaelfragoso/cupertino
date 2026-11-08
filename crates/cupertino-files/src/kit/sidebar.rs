//! A watch that tells the window while the keyboard is in the sidebar, or in the folder's items. The sidebar's glass
//! and the folder's background are the Kit's sidebar window shell's.

use cupertino_kit::core::{
    Change, Element, Event, EventCx, MeasureCx, PaintCx, Propagation, SemanticsCx, Widget,
};
use cupertino_kit::geometry::Size;
use cupertino_kit::layout::{
    Align, Axis, Constraints, ContainerKind, Flex, Justify, LayoutSpec, Measured, SizeSpec,
};

use super::Message;

fn no_measure() -> Measured {
    Measured {
        size: Size::ZERO,
        baseline: None,
    }
}

/// A container that fills its room and stacks its one child in it.
fn filling() -> LayoutSpec {
    LayoutSpec {
        width: SizeSpec::Fill,
        height: SizeSpec::Fill,
        flex: Flex::grow(1.0),
        ..LayoutSpec::new(ContainerKind::Stack {
            axis: Axis::Vertical,
            spacing: 0.0,
            align: Align::Stretch,
            justify: Justify::Start,
        })
    }
}

/// Sends `on_focus` with `true` when the keyboard comes into its content and `false` when it leaves.
pub(super) struct FocusWatch {
    pub on_focus: Box<dyn Fn(bool) -> Message>,
    pub children: Vec<Element<Message>>,
}

impl Widget<Message> for FocusWatch {
    type State = bool;

    fn layout(&self) -> LayoutSpec {
        filling()
    }

    fn take_children(&mut self) -> Vec<Element<Message>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) -> bool {
        false
    }

    fn update(&self, _: &Self, _: &mut bool) -> Change {
        Change::None
    }

    fn measure(&self, _: &bool, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(
        &self,
        within: &mut bool,
        event: &Event,
        cx: &mut EventCx<'_, Message>,
    ) -> Propagation {
        // an inactive window keeps its focus: only the focus moving in or out counts
        if let Event::FocusWithin { gained, .. } = *event {
            if cx.node() == cx.target() && gained != *within {
                *within = gained;
                cx.emit((self.on_focus)(gained));
            }
        }
        Propagation::Continue
    }

    fn paint(&self, _: &bool, _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &bool, _: &mut SemanticsCx<'_>) {}
}
