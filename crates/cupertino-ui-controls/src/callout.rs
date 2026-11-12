//! The pointer of a callout: the small soft triangle under a panel or a label that points at what it belongs to. As glass
//! it is a few stacked layers, each narrower than the one above, that the container's smooth union melts into one tapering
//! shape with curved shoulders flowing into the panel; without glass it is one filled outline of the same silhouette.

use cupertino_geometry::{Point, Size};
use cupertino_ui_core::{
    Change, Element, Event, EventCx, Glass, MeasureCx, PaintCx, Propagation, SemanticsCx, Widget,
};
use cupertino_ui_layout::{
    Align, Axis, Constraints, ContainerKind, Justify, LayoutSpec, Measured, SizeSpec,
};
use cupertino_ui_render::{FillRule, Paint, PaintOp, PathVerb, Shape};
use cupertino_ui_theme::Surface;

/// How wide each layer is, from the one at the panel to the tip, as a share of the pointer's width: they narrow a little
/// faster towards the tip, so the union's outline is a taper with soft shoulders and a small rounded tip.
const LAYER_WIDTH: [f64; 6] = [1.0, 0.787, 0.587, 0.402, 0.235, 0.092];

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
    /// Whether the pointer points up, from a panel under it (a popover below what it belongs to), rather than down.
    pub up: bool,
}

impl CalloutPointer {
    pub fn element<M: Clone + 'static>(self) -> Element<M> {
        Element::new(self)
    }
}

/// How far a smooth union of blend `spacing` reaches past two shapes where they meet: a quarter of the blend.
fn union_swell(spacing: f64) -> f64 {
    spacing.max(0.0) / 4.0
}

/// The outline of the pointer: from the panel's edge the sides curve in to a rounded tip.
fn outline(w: f64, d: f64, up: bool) -> Vec<PathVerb> {
    let (mid, half) = (w / 2.0, w / 2.0);
    // the panel's edge and the tip, in the pointer's own height: down from the top edge, or up from the bottom edge
    let (edge, tip) = if up { (d + 0.5, 0.0) } else { (-0.5, d) };
    let (rim, inner) = (0.2 * w, if up { d } else { 0.0 });
    vec![
        PathVerb::MoveTo(Point::new(mid - half, edge)),
        PathVerb::LineTo(Point::new(mid + half, edge)),
        PathVerb::CubicTo(
            Point::new(mid + half - rim, inner),
            Point::new(mid + 0.18 * w, tip),
            Point::new(mid, tip),
        ),
        PathVerb::CubicTo(
            Point::new(mid - 0.18 * w, tip),
            Point::new(mid - half + rim, inner),
            Point::new(mid - half, edge),
        ),
        PathVerb::Close,
    ]
}

impl<M: Clone + 'static> Widget<M> for CalloutPointer {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            // the whole depth, though the layers stop short of it
            height: SizeSpec::Fixed(self.depth),
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Vertical,
                spacing: 0.0,
                align: Align::Center,
                // the layers stay against the panel; the room the union's swell takes is left at the tip's end
                justify: if self.up {
                    Justify::End
                } else {
                    Justify::Start
                },
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        let mut layers: Vec<(usize, f64)> = LAYER_WIDTH.iter().copied().enumerate().collect();
        // an upward pointer has its tip first
        if self.up {
            layers.reverse();
        }
        // the smooth union swells the outline past the last layer by up to a quarter of its blend: the layers stop that
        // short of `depth`, so the tip lands at it
        let stack = (self.depth - union_swell(self.glass.spacing)).max(self.depth * 0.5);
        layers
            .into_iter()
            .map(|(i, share)| {
                Element::new(Layer {
                    size: Size::new(self.width * share, stack / LAYER_WIDTH.len() as f64),
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
                verbs: outline(size.width, size.height, self.up),
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
