use crate::{Image, ImageFit, ImageSource};
use cupertino_geometry::{Rect, Size};
use cupertino_ui_core::{
    Change, Event, EventCx, MeasureCx, PaintCx, Propagation, Role, SemanticsCx, Widget,
};
use cupertino_ui_layout::{Constraints, ContainerKind, LayoutSpec, Measured};
use cupertino_ui_render::{PaintOp, Sampling, Shape};

/// Where an image `width` by `height` pixels lands in `area` for `fit`, keeping its aspect ratio.
fn fitted(fit: ImageFit, width: f64, height: f64, area: Rect) -> Rect {
    let (sx, sy) = (area.width() / width, area.height() / height);
    let scale = match fit {
        ImageFit::Contain => sx.min(sy),
        ImageFit::Cover => sx.max(sy),
    };
    let size = Size::new(width * scale, height * scale);
    Rect::new(
        area.min_x() + (area.width() - size.width) / 2.0,
        area.min_y() + (area.height() - size.height) / 2.0,
        size.width,
        size.height,
    )
}

/// The smallest rendition at least `pixels` wide, else the widest.
fn rendition(sources: &[ImageSource], pixels: f64) -> Option<&ImageSource> {
    sources
        .iter()
        .filter(|source| f64::from(source.image.width) >= pixels)
        .min_by_key(|source| source.image.width)
        .or_else(|| sources.iter().max_by_key(|source| source.image.width))
}

impl<M: 'static> Widget<M> for Image {
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
            size: self.size,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    /// The renditions share one aspect ratio; the one drawn is picked by the device pixels it covers.
    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let Some(first) = self.sources.first() else {
            return;
        };
        if first.image.width == 0 || first.image.height == 0 {
            return;
        }
        let size = cx.size();
        let area = Rect::new(0.0, 0.0, size.width, size.height);
        let dst = fitted(
            self.fit,
            f64::from(first.image.width),
            f64::from(first.image.height),
            area,
        );
        let Some(source) = rendition(&self.sources, dst.width() * cx.env().scale) else {
            return;
        };
        let visible = dst.intersection(&area).unwrap_or(dst);
        let clip = if self.radius > 0.0 {
            Some(Shape::RoundedRect {
                rect: visible,
                radius: self.radius,
            })
        } else {
            (visible != dst).then_some(Shape::Rect(visible))
        };
        if let Some(shape) = &clip {
            cx.push(PaintOp::PushClip(shape.clone()));
        }
        cx.image(source.id, &source.image, dst, Sampling::Linear);
        if clip.is_some() {
            cx.push(PaintOp::PopClip);
        }
    }

    /// A decorative image (no label) stays out of the accessibility tree.
    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        if let Some(label) = &self.label {
            cx.role(Role::Image);
            cx.name(label.clone());
        }
    }
}
