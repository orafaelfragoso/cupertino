use crate::{Icon, IconDirection};
use cupertino_geometry::{Rect, Size};
use cupertino_ui_core::{
    Change, Event, EventCx, MeasureCx, PaintCx, Propagation, Role, SemanticsCx, Widget,
};
use cupertino_ui_layout::{Constraints, ContainerKind, LayoutDirection, LayoutSpec, Measured};
use cupertino_ui_render::Sampling;
use cupertino_ui_theme::{Color, ColorRole, StateVariant};

/// Draws the system symbol `name` at `size` points in `color`, at the device's pixels and centred in `area`.
pub(crate) fn symbol(
    cx: &mut PaintCx<'_>,
    name: &str,
    size: f64,
    color: Color,
    mirrored: bool,
    area: Rect,
) {
    let env = cx.env();
    let scale = env.scale;
    let px = (size * scale).round().max(1.0) as u32;
    let icon = env.icons.symbol(name, px, color, mirrored);
    let (width, height) = (
        f64::from(icon.image.width) / scale,
        f64::from(icon.image.height) / scale,
    );
    let dst = Rect::new(
        area.min_x() + (area.width() - width) / 2.0,
        area.min_y() + (area.height() - height) / 2.0,
        width,
        height,
    );
    cx.image(icon.id, &icon.image, dst, Sampling::Linear);
}

impl<M: 'static> Widget<M> for Icon {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.size != old.size {
            Change::Layout
        } else if self != old {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(self.size, self.size),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    /// Drawn at the device's pixels and centred in the node, so it stays sharp at every scale; artwork that points
    /// along the reading direction is mirrored in a right-to-left layout.
    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let env = cx.env();
        let role = self.tint.unwrap_or(ColorRole::Text);
        let Some(color) = env.theme.colors.get(role, StateVariant::NORMAL) else {
            return;
        };
        let mirrored =
            self.direction == IconDirection::Semantic && env.direction == LayoutDirection::Rtl;
        let size = cx.size();
        let area = Rect::new(0.0, 0.0, size.width, size.height);
        symbol(cx, &self.name, self.size, color, mirrored, area);
    }

    /// A decorative icon (no label) stays out of the accessibility tree.
    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        if let Some(label) = &self.label {
            cx.role(Role::Image);
            cx.name(label.clone());
        }
    }
}
