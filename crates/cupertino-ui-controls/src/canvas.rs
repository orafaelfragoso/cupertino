use crate::Canvas;
use cupertino_geometry::Size;
use cupertino_ui_core::{
    Change, Element, Event, EventCx, MeasureCx, PaintCx, Propagation, Role, SemanticsCx, Widget,
};
use cupertino_ui_layout::{Constraints, ContainerKind, LayoutSpec, Measured, SizeSpec};

impl<M: 'static> Widget<M> for Canvas<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        let (width, height) = match self.size {
            Some(size) => (SizeSpec::Fixed(size.width), SizeSpec::Fixed(size.height)),
            None => (SizeSpec::Fill, SizeSpec::Fill),
        };
        LayoutSpec {
            width,
            height,
            ..LayoutSpec::new(ContainerKind::Overlay)
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    /// The drawing is a closure, which cannot be compared, so every update repaints.
    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.size != old.size {
            Change::Layout
        } else {
            Change::Paint
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        (self.paint)(cx);
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Group);
        cx.name(self.label.clone());
    }
}
