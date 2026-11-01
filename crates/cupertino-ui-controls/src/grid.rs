use crate::Grid;
use cupertino_geometry::Size;
use cupertino_ui_core::{
    Change, Element, Event, EventCx, MeasureCx, PaintCx, Propagation, SemanticsCx, Widget,
};
use cupertino_ui_layout::{Constraints, ContainerKind, LayoutSpec, Measured};

impl<M: 'static> Widget<M> for Grid<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            insets: self.padding,
            ..LayoutSpec::new(ContainerKind::Grid {
                columns: self.columns.clone(),
                rows: self.rows.clone(),
                gaps: self.spacing,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.columns != old.columns
            || self.rows != old.rows
            || self.padding != old.padding
            || self.spacing != old.spacing
        {
            Change::Layout
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

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}
