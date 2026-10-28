use crate::{
    DisplayList, Effect, Paint, PaintNode, PaintNodeId, PaintOp, PaintResources, PathVerb,
    RenderError, Shape, Transform,
};
use cupertino_geometry::{Point, Rect};

/// Physical pixels per logical point beyond which a request is refused.
const MAX_SCALE: f64 = 64.0;
/// Largest effect radius or offset, in logical points.
const MAX_EFFECT_EXTENT: f64 = 4096.0;

pub(crate) fn check(
    list: &DisplayList,
    resources: &PaintResources,
    scale: f64,
    width: u32,
    height: u32,
) -> Result<(), RenderError> {
    if !(scale.is_finite() && scale > 0.0 && scale <= MAX_SCALE) {
        return Err(RenderError::InvalidScale);
    }
    if width == 0 || height == 0 {
        return Err(RenderError::InvalidSize);
    }
    list.nodes
        .iter()
        .try_for_each(|node| node_ok(node, resources))
}

#[derive(PartialEq)]
enum Scope {
    Clip,
    Layer,
}

fn node_ok(node: &PaintNode, resources: &PaintResources) -> Result<(), RenderError> {
    let id = node.id;
    let finite = |ok: bool| {
        if ok {
            Ok(())
        } else {
            Err(RenderError::NonFiniteValue(id))
        }
    };
    finite(transform_ok(&node.transform) && rect_ok(&node.bounds) && rect_ok(&node.hit))?;
    if let Some(clip) = &node.clip {
        finite(shape_ok(clip))?;
    }
    let mut scopes = Vec::new();
    for op in &node.ops {
        match op {
            PaintOp::Fill { shape, paint } => finite(shape_ok(shape) && paint_ok(paint))?,
            PaintOp::Stroke {
                shape,
                width,
                paint,
            } => finite(shape_ok(shape) && paint_ok(paint) && width.is_finite() && *width >= 0.0)?,
            PaintOp::Glyphs { origin, run, .. } => finite(
                point_ok(origin)
                    && run.size.is_finite()
                    && run
                        .glyphs
                        .iter()
                        .all(|g| g.x.is_finite() && g.y.is_finite()),
            )?,
            PaintOp::Image {
                image, src, dst, ..
            } => {
                finite(rect_ok(src) && rect_ok(dst))?;
                let res = resources
                    .images
                    .get(&image.id)
                    .ok_or(RenderError::MissingResource(image.id))?;
                if res.revision != image.revision {
                    return Err(RenderError::StaleResource(image.id));
                }
                let row = u64::from(res.width) * 4;
                let need = u64::from(res.stride) * u64::from(res.height.saturating_sub(1)) + row;
                if u64::from(res.stride) < row || (res.pixels.len() as u64) < need {
                    return Err(RenderError::InvalidImage(image.id));
                }
            }
            PaintOp::PushClip(shape) => {
                finite(shape_ok(shape))?;
                scopes.push(Scope::Clip);
            }
            PaintOp::PopClip => pop(&mut scopes, Scope::Clip, id)?,
            PaintOp::PushLayer { opacity, effects } => {
                finite(
                    opacity.is_finite()
                        && (0.0..=1.0).contains(opacity)
                        && effects.iter().all(effect_ok),
                )?;
                if !effects.iter().all(effect_bounded) {
                    return Err(RenderError::ResourceLimit);
                }
                scopes.push(Scope::Layer);
            }
            PaintOp::PopLayer => pop(&mut scopes, Scope::Layer, id)?,
        }
    }
    if scopes.is_empty() {
        Ok(())
    } else {
        Err(RenderError::UnbalancedScope(id))
    }
}

fn pop(scopes: &mut Vec<Scope>, want: Scope, id: PaintNodeId) -> Result<(), RenderError> {
    match scopes.pop() {
        Some(top) if top == want => Ok(()),
        _ => Err(RenderError::UnbalancedScope(id)),
    }
}

fn transform_ok(t: &Transform) -> bool {
    t.0.iter().all(|v| v.is_finite())
}

fn point_ok(p: &Point) -> bool {
    p.x.is_finite() && p.y.is_finite()
}

fn rect_ok(r: &Rect) -> bool {
    point_ok(&r.origin) && r.size.width.is_finite() && r.size.height.is_finite()
}

fn shape_ok(shape: &Shape) -> bool {
    match shape {
        Shape::Rect(r) => rect_ok(r),
        Shape::RoundedRect { rect, radius } => {
            rect_ok(rect) && radius.is_finite() && *radius >= 0.0
        }
        Shape::Path { verbs, .. } => verbs.iter().all(|v| match v {
            PathVerb::MoveTo(p) | PathVerb::LineTo(p) => point_ok(p),
            PathVerb::QuadTo(a, b) => point_ok(a) && point_ok(b),
            PathVerb::CubicTo(a, b, c) => point_ok(a) && point_ok(b) && point_ok(c),
            PathVerb::Close => true,
        }),
    }
}

fn paint_ok(paint: &Paint) -> bool {
    match paint {
        Paint::Solid(_) => true,
        Paint::Linear { start, end, stops } => point_ok(start) && point_ok(end) && stops_ok(stops),
        Paint::Radial {
            center,
            radius,
            stops,
        } => point_ok(center) && radius.is_finite() && *radius >= 0.0 && stops_ok(stops),
    }
}

fn stops_ok(stops: &[crate::GradientStop]) -> bool {
    stops.iter().all(|s| s.offset.is_finite())
}

fn effect_ok(effect: &Effect) -> bool {
    match effect {
        Effect::Blur { radius } => radius.is_finite() && *radius >= 0.0,
        Effect::DropShadow { offset, blur, .. } | Effect::InnerShadow { offset, blur, .. } => {
            point_ok(offset) && blur.is_finite() && *blur >= 0.0
        }
    }
}

fn effect_bounded(effect: &Effect) -> bool {
    match effect {
        Effect::Blur { radius } => *radius <= MAX_EFFECT_EXTENT,
        Effect::DropShadow { offset, blur, .. } | Effect::InnerShadow { offset, blur, .. } => {
            *blur <= MAX_EFFECT_EXTENT
                && offset.x.abs() <= MAX_EFFECT_EXTENT
                && offset.y.abs() <= MAX_EFFECT_EXTENT
        }
    }
}
