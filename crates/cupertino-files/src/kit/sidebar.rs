//! The sidebar's look and its keyboard: the window's sidebar material behind the split view, the folder's pane opaque
//! over it, and a watch that tells the window while the keyboard is in the sidebar.
//!
//! The window paints no background of its own. [`Backdrop`] asks the compositor for the theme's sidebar material under
//! the whole split view, or paints the sidebar surface's opaque fill where the window cannot show it (no desktop
//! effects, Reduce Transparency, Increase Contrast). [`Opaque`] paints the window background under the folder's pane,
//! so the material shows through the sidebar and the divider only.

use cupertino_kit::core::{
    Change, Element, Event, EventCx, MeasureCx, PaintCx, Propagation, SemanticsCx, Widget,
};
use cupertino_kit::geometry::{Rect, Size};
use cupertino_kit::layout::{
    Align, Axis, Constraints, ContainerKind, Flex, Justify, LayoutSpec, Measured, SizeSpec,
};
use cupertino_kit::render::{Paint, PaintOp, Shape};
use cupertino_kit::theme::{ColorRole, StateVariant, Surface};

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

/// The sidebar material under its content, or the sidebar surface's fill.
pub(super) struct Backdrop {
    pub children: Vec<Element<Message>>,
}

impl Widget<Message> for Backdrop {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        filling()
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

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        if cx.material(Surface::Sidebar, 0.0, 1.0) {
            return;
        }
        let size = cx.size();
        if let Some(fill) = cx.env().theme.surface(Surface::Sidebar).fill {
            cx.push(PaintOp::Fill {
                shape: Shape::Rect(Rect::new(0.0, 0.0, size.width, size.height)),
                paint: Paint::Solid(fill),
            });
        }
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// The window background under its content.
pub(super) struct Opaque {
    pub children: Vec<Element<Message>>,
}

impl Widget<Message> for Opaque {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        filling()
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

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let state = StateVariant {
            window_active: cx.window_active(),
            ..StateVariant::NORMAL
        };
        let size = cx.size();
        let theme = &cx.env().theme;
        if let Some(color) = theme.colors.get(ColorRole::WindowBackground, state) {
            cx.push(PaintOp::Fill {
                shape: Shape::Rect(Rect::new(0.0, 0.0, size.width, size.height)),
                paint: Paint::Solid(color),
            });
        }
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
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
