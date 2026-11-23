//! The body of a menu's or a panel's popup: the menu variant's glass over the whole window with the popup's corners,
//! without the variant's drop shadow, as the popup draws the menu's window shadow apart; where the desktop shows no
//! glass, the theme's opaque fill of the menu surface in the same shape.

use super::*;
use cupertino_kit::core::Glass;
use cupertino_kit::core::GlassVariant;

/// The menu's glass with corners of `radius`: the menu variant at the menu's own size, its shadow left out.
pub fn menu_glass(radius: f64) -> Glass {
    Glass::menu()
        .variant(GlassVariant::Menu)
        .exclude_shadow(true)
        .radius(radius)
}

/// Paints the popup's body under the node, the whole node with corners of `radius`; what the node and its subtree
/// paint after lies on it.
pub fn paint(cx: &mut PaintCx<'_>, radius: f64) {
    let theme = cx.env().theme.surface(Surface::Menu);
    if theme.material.is_some() && cx.glass(menu_glass(radius)) {
        return;
    }
    if let Some(fill) = theme.fill {
        let size = cx.size();
        cx.push(PaintOp::Fill {
            shape: Shape::RoundedRect {
                rect: Rect::new(0.0, 0.0, size.width, size.height),
                radius,
            },
            paint: Paint::Solid(fill),
        });
    }
}

/// A popup's content laid over its body, the whole window.
pub struct Body<M> {
    pub radius: f64,
    pub children: Vec<Element<M>>,
}

impl<M: Clone + 'static> Widget<M> for Body<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Overlay)
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.radius != old.radius {
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

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        paint(cx, self.radius);
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}
