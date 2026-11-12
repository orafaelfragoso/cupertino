//! The pointer of a callout: the small soft triangle under a panel or a label that points at what it belongs to. As glass
//! it is a few stacked layers, each narrower than the one above, that the container's smooth union melts into one tapering
//! shape with curved shoulders flowing into the panel; without glass it is one filled outline of the same silhouette.

use cupertino_geometry::{Point, Size};
use cupertino_ui_core::{
    Change, Element, Event, EventCx, Glass, MeasureCx, PaintCx, Propagation, SemanticsCx, Widget,
};
use cupertino_ui_layout::{Align, Axis, Constraints, ContainerKind, Justify, LayoutSpec, Measured};
use cupertino_ui_render::{FillRule, Paint, PaintOp, PathVerb, Shape};
use cupertino_ui_theme::Surface;

/// How wide each layer is, from the one at the panel to the tip, as a share of the pointer's width: they narrow in even
/// steps, so the union's outline is a straight taper with a rounded tip.
const LAYER_WIDTH: [f64; 6] = [1.0, 0.8, 0.62, 0.44, 0.28, 0.16];

/// A callout's pointer `width` wide where it leaves the panel and `depth` deep, as glass of the container `glass` names
/// or, where `surface` shows no material, a filled outline of `surface`'s fill.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CalloutPointer {
    pub width: f64,
    pub depth: f64,
    pub glass: Glass,
    pub surface: Surface,
    /// Whether the pointer fills its outline where the session shows no material; a callout whose body paints the whole
    /// outline itself leaves it off.
    pub fills_without_glass: bool,
}

impl CalloutPointer {
    pub fn element<M: Clone + 'static>(self) -> Element<M> {
        Element::new(self)
    }
}

/// The outline of the pointer: from the panel's edge the sides curve in to a rounded tip.
fn outline(w: f64, d: f64) -> Vec<PathVerb> {
    let (mid, half, tip) = (w / 2.0, w / 2.0, d);
    let rim = 0.2 * w;
    vec![
        PathVerb::MoveTo(Point::new(mid - half, -0.5)),
        PathVerb::LineTo(Point::new(mid + half, -0.5)),
        PathVerb::CubicTo(
            Point::new(mid + half - rim, 0.0),
            Point::new(mid + 0.18 * w, tip),
            Point::new(mid, tip),
        ),
        PathVerb::CubicTo(
            Point::new(mid - 0.18 * w, tip),
            Point::new(mid - half + rim, 0.0),
            Point::new(mid - half, -0.5),
        ),
        PathVerb::Close,
    ]
}

impl<M: Clone + 'static> Widget<M> for CalloutPointer {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Stack {
            axis: Axis::Vertical,
            spacing: 0.0,
            align: Align::Center,
            justify: Justify::Start,
        })
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        LAYER_WIDTH
            .iter()
            .enumerate()
            .map(|(i, share)| {
                Element::new(Layer {
                    size: Size::new(self.width * share, self.depth / LAYER_WIDTH.len() as f64),
                    glass: self.glass,
                })
                .key(format!("layer-{i}"))
            })
            .collect()
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self != old {
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

    fn hit(&self, _: &(), _: Point) -> bool {
        false
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        if !self.fills_without_glass {
            return;
        }
        let surface = cx.env().theme.surface(self.surface);
        if surface.material.is_some() {
            return;
        }
        let Some(fill) = surface.fill else {
            return;
        };
        let size = cx.size();
        cx.push(PaintOp::Fill {
            shape: Shape::Path {
                verbs: outline(size.width, size.height),
                fill_rule: FillRule::NonZero,
            },
            paint: Paint::Solid(fill),
        });
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.hidden();
    }
}

/// One layer of the glass.
struct Layer {
    size: Size,
    glass: Glass,
}

impl<M: Clone + 'static> Widget<M> for Layer {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.size != old.size || self.glass != old.glass {
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

    fn hit(&self, _: &(), _: Point) -> bool {
        false
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        cx.glass(self.glass.capsule());
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.hidden();
    }
}
