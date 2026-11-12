use crate::{Separator, Spacer, Stack, Wrap};
use cupertino_geometry::{Rect, Size};
use cupertino_ui_core::{
    Change, Element, Event, EventCx, MeasureCx, PaintCx, Propagation, SemanticsCx, Widget,
};
use cupertino_ui_layout::{
    Axis, Constraints, ContainerKind, Flex, Justify, LayoutSpec, Measured, SizeSpec,
};
use cupertino_ui_render::{Paint, PaintOp, Shape};
use cupertino_ui_theme::{ColorRole, StateVariant};

/// A container's leaf callbacks: layout sizes it and it neither paints nor takes part in accessibility.
fn no_measure() -> Measured {
    Measured {
        size: Size::ZERO,
        baseline: None,
    }
}

impl<M: 'static> Widget<M> for Stack<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            insets: self.padding,
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: self.axis,
                spacing: self.spacing,
                align: self.align,
                justify: Justify::Start,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.axis != old.axis
            || self.padding != old.padding
            || self.spacing != old.spacing
            || self.align != old.align
        {
            Change::Layout
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

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

impl<M: 'static> Widget<M> for Spacer {
    type State = ();

    /// Grows along its stack's axis from `min`.
    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            flex: Flex {
                grow: 1.0,
                shrink: 0.0,
                basis: Some(self.min),
            },
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.min != old.min {
            Change::Layout
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

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// The separator's thickness in logical points.
const HAIRLINE: f64 = 1.0;

impl<M: 'static> Widget<M> for Separator {
    type State = ();

    /// Spans its stack across `axis`, one hairline thick.
    fn layout(&self) -> LayoutSpec {
        let (width, height) = match self.axis {
            Axis::Horizontal => (SizeSpec::Fill, SizeSpec::Fixed(HAIRLINE)),
            Axis::Vertical => (SizeSpec::Fixed(HAIRLINE), SizeSpec::Fill),
        };
        LayoutSpec {
            width,
            height,
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.axis != old.axis {
            Change::Layout
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
        let color = cx
            .env()
            .theme
            .colors
            .get(ColorRole::Separator, StateVariant::NORMAL);
        if let Some(color) = color {
            let size = cx.size();
            cx.push(PaintOp::Fill {
                shape: Shape::Rect(Rect::new(0.0, 0.0, size.width, size.height)),
                paint: Paint::Solid(color),
            });
        }
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

impl<M: 'static> Widget<M> for Wrap<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Wrap { gaps: self.gaps })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.gaps != old.gaps {
            Change::Layout
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

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}
