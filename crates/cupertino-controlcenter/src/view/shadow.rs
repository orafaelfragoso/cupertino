//! The soft shadow around the tiles: bands outside their outline and the darkening between them.

use super::*;

/// `rect` with its corners rounded by `radius`, as a path of four quarter circles.
pub(super) fn rounded_path(rect: Rect, radius: f64) -> Vec<PathVerb> {
    // a quarter circle's control points sit this far along its tangents, as a fraction of the radius
    const KAPPA: f64 = 0.552_284_75;
    let (l, t, r, b) = (rect.min_x(), rect.min_y(), rect.max_x(), rect.max_y());
    let radius = radius
        .min(rect.width() / 2.0)
        .min(rect.height() / 2.0)
        .max(0.0);
    let k = radius * KAPPA;
    vec![
        PathVerb::MoveTo(Point::new(l + radius, t)),
        PathVerb::LineTo(Point::new(r - radius, t)),
        PathVerb::CubicTo(
            Point::new(r - radius + k, t),
            Point::new(r, t + radius - k),
            Point::new(r, t + radius),
        ),
        PathVerb::LineTo(Point::new(r, b - radius)),
        PathVerb::CubicTo(
            Point::new(r, b - radius + k),
            Point::new(r - radius + k, b),
            Point::new(r - radius, b),
        ),
        PathVerb::LineTo(Point::new(l + radius, b)),
        PathVerb::CubicTo(
            Point::new(l + radius - k, b),
            Point::new(l, b - radius + k),
            Point::new(l, b - radius),
        ),
        PathVerb::LineTo(Point::new(l, t + radius)),
        PathVerb::CubicTo(
            Point::new(l, t + radius - k),
            Point::new(l + radius - k, t),
            Point::new(l + radius, t),
        ),
        PathVerb::Close,
    ]
}

/// Darkening at a distance from the modules' edge, in points: the share of black laid over the backdrop. It starts
/// at about 9 % at the edge and fades out over [`SHADOW_REACH`].
pub(super) const SHADOW_PROFILE: [(f64, f64); 5] = [
    (0.0, 0.090),
    (30.0, 0.067),
    (60.0, 0.027),
    (90.0, 0.011),
    (SHADOW_REACH, 0.0),
];
/// The shadow is drawn as bands this wide, each a ring around the modules' outline.
pub(super) const SHADOW_BAND: f64 = 5.0;

/// The darkening `distance` points from the edge.
pub(super) fn shadow_at(distance: f64) -> f64 {
    let d = distance.max(0.0);
    for pair in SHADOW_PROFILE.windows(2) {
        let ((d0, a0), (d1, a1)) = (pair[0], pair[1]);
        if d <= d1 {
            return a0 + (a1 - a0) * ((d - d0) / (d1 - d0)).clamp(0.0, 1.0);
        }
    }
    0.0
}

/// A tile's outline over the soft shadow, and how opaque its glass is: the shadow is not laid under a tile whose glass
/// shows fully, since the glass lies under everything the window paints, and is laid under a fading tile at the share
/// its glass lets through.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Cutout {
    pub(super) rect: Rect,
    pub(super) radius: f64,
    pub(super) alpha: f64,
}

/// The outline of the shadow's band `distance` points out from `rect`: as far out on the left, the right and below,
/// and `top_reach / SHADOW_REACH` as far above, so the shadow fades out within `top_reach` above the modules. Each
/// outline holds the nearer ones, so the bands between them never overlap.
pub(super) fn band_outline(rect: Rect, distance: f64, top_reach: f64) -> (Rect, f64) {
    let up = distance * (top_reach / SHADOW_REACH).clamp(0.0, 1.0);
    let outline = Rect::new(
        rect.x() - distance,
        rect.y() - up,
        rect.width() + 2.0 * distance,
        rect.height() + distance + up,
    );
    (outline, LARGE_RADIUS + distance)
}

/// A soft shadow around `rect` (rounded at [`LARGE_RADIUS`]): concentric bands outside the outline, each filled at the
/// darkening of its middle, reaching [`SHADOW_REACH`] to the sides and below and `top_reach` above.
///
/// Inside `rect` the shadow is also laid between the `cutouts`, the tiles' outlines, at the strength it has at the
/// edge, and under a fading tile at the share its glass lets through, so a tile fading out leaves the darkening the
/// gaps around it have rather than a light patch, and nothing changes the moment a motion starts or ends.
pub(super) fn shadow(
    cx: &mut PaintCx<'_>,
    rect: Rect,
    top_reach: f64,
    cutouts: &[Cutout],
    resting: bool,
) {
    let edge = shadow_at(0.0);
    let black = |alpha: f64| Paint::Solid(Color::new(0.0, 0.0, 0.0, alpha as f32));
    let around = rounded_path(rect.outset(SHADOW_REACH + 1.0), 0.0);
    let without = |holes: &mut dyn Iterator<Item = &Cutout>| {
        let mut path = around.clone();
        for c in holes {
            path.extend(rounded_path(c.rect, c.radius));
        }
        Shape::Path {
            verbs: path,
            fill_rule: FillRule::EvenOdd,
        }
    };
    let fading = cutouts.iter().any(|c| c.alpha < 1.0);
    if fading {
        cx.push(PaintOp::PushClip(Shape::RoundedRect {
            rect,
            radius: LARGE_RADIUS,
        }));
    }
    cx.push(PaintOp::PushClip(without(&mut cutouts.iter())));
    fill(
        cx,
        rect,
        LARGE_RADIUS,
        Color::new(0.0, 0.0, 0.0, edge as f32),
    );
    cx.push(PaintOp::PopClip);
    if fading {
        // the fading tiles by their opacity, the more opaque first: each is clipped away where a more opaque one
        // lies over it
        let mut levels: Vec<f64> = cutouts
            .iter()
            .map(|c| c.alpha.clamp(0.0, 1.0))
            .filter(|a| *a < 1.0)
            .collect();
        levels.sort_by(|a, b| b.total_cmp(a));
        levels.dedup();
        for level in levels {
            let mut above = cutouts
                .iter()
                .filter(|c| c.alpha.clamp(0.0, 1.0) > level)
                .peekable();
            let clipped = above.peek().is_some();
            if clipped {
                cx.push(PaintOp::PushClip(without(&mut above)));
            }
            for c in cutouts.iter().filter(|c| c.alpha.clamp(0.0, 1.0) == level) {
                cx.push(PaintOp::Fill {
                    shape: Shape::RoundedRect {
                        rect: c.rect,
                        radius: c.radius,
                    },
                    paint: black(edge * (1.0 - level)),
                });
            }
            if clipped {
                cx.push(PaintOp::PopClip);
            }
        }
        cx.push(PaintOp::PopClip);
    }
    let scale = cx.env().scale;
    match band_image(rect, top_reach, scale, resting) {
        Some((id, image, dst)) => cx.image(id, &image, dst, Sampling::Nearest),
        None => {
            for op in bands(rect, top_reach) {
                cx.push(op);
            }
        }
    }
}

/// The shadow's bands around `rect`: rings that never overlap, each filled at the darkening of its middle.
pub(super) fn bands(rect: Rect, top_reach: f64) -> Vec<PaintOp> {
    let black = |alpha: f64| Paint::Solid(Color::new(0.0, 0.0, 0.0, alpha as f32));
    let count = (SHADOW_REACH / SHADOW_BAND).ceil() as usize;
    let mut ops = Vec::with_capacity(count);
    for i in 0..count {
        let from = i as f64 * SHADOW_BAND;
        let alpha = shadow_at(from + SHADOW_BAND / 2.0);
        if alpha <= 0.0 {
            continue;
        }
        let (outer, outer_radius) = band_outline(rect, from + SHADOW_BAND, top_reach);
        let (inner, inner_radius) = band_outline(rect, from, top_reach);
        let mut path = rounded_path(outer, outer_radius);
        path.extend(rounded_path(inner, inner_radius));
        ops.push(PaintOp::Fill {
            shape: Shape::Path {
                verbs: path,
                fill_rule: FillRule::EvenOdd,
            },
            paint: black(alpha),
        });
    }
    ops
}

/// How many pictures of the shadow's bands the process keeps: the panel at rest with no view and with a view open.
/// Each is the window's size in device pixels, some 2.5 MiB at scale 1.5.
pub(super) const BAND_IMAGES: usize = 2;

/// What a band picture is made for: the outline's corners, the top reach and the scale, as bits.
pub(super) type BandKey = [u64; 6];

/// A picture of the shadow's bands at the window's device pixels, one texel per pixel, and where it goes.
pub(super) struct BandImage {
    pub(super) key: BandKey,
    pub(super) id: ResourceId,
    pub(super) image: ImageResource,
    pub(super) dst: Rect,
}

thread_local! {
    /// The band pictures made, the latest used last.
    static BAND_IMAGES_KEPT: RefCell<Vec<BandImage>> = const { RefCell::new(Vec::new()) };
}

/// Glyphs for the bands' renderer, which draws none.
pub(super) struct NoGlyphs;

impl GlyphRasterizer for NoGlyphs {
    fn rasterize(&mut self, _: &GlyphKey) -> Result<Option<RasterGlyph>, TextError> {
        Ok(None)
    }
}

/// The shadow's bands around `rect` as one picture of the window's device pixels at `scale`, drawn by the CPU
/// renderer the panel's class draws with, so the picture laid over one texel per pixel has the pixels the bands filled
/// in place have, to within two steps of rounding. A whole frame of the panel then lays one picture instead of filling some 26 rings each as large as the
/// window, which cost a frame most of its time; a panel opened again, whose window and renderer are new, finds the
/// picture. A picture is made only while the panel is `resting`: an outline that changes every frame (a detail view's
/// motion) is filled in place, as making its picture would cost the same and not be used again. The panel is the
/// window's root, so its points are the window's.
pub(super) fn band_image(
    rect: Rect,
    top_reach: f64,
    scale: f64,
    resting: bool,
) -> Option<(ResourceId, ImageResource, Rect)> {
    let key: BandKey = [
        rect.min_x().to_bits(),
        rect.min_y().to_bits(),
        rect.max_x().to_bits(),
        rect.max_y().to_bits(),
        top_reach.to_bits(),
        scale.to_bits(),
    ];
    BAND_IMAGES_KEPT.with_borrow_mut(|kept| {
        if let Some(i) = kept.iter().position(|b| b.key == key) {
            let b = kept.remove(i);
            let found = (b.id, b.image.clone(), b.dst);
            kept.push(b);
            return Some(found);
        }
        if !resting {
            return None;
        }
        let made = draw_bands(rect, top_reach, scale)?;
        let found = (made.id, made.image.clone(), made.dst);
        kept.push(BandImage { key, ..made });
        if kept.len() > BAND_IMAGES {
            kept.remove(0);
        }
        Some(found)
    })
}

/// Draws the bands around `rect` into a picture of the device pixels they reach at `scale`.
pub(super) fn draw_bands(rect: Rect, top_reach: f64, scale: f64) -> Option<BandImage> {
    if !(scale.is_finite() && scale > 0.0) {
        return None;
    }
    let (outer, _) = band_outline(rect, SHADOW_REACH, top_reach);
    let (x0, y0) = (
        (outer.min_x() * scale).floor(),
        (outer.min_y() * scale).floor(),
    );
    let (x1, y1) = (
        (outer.max_x() * scale).ceil(),
        (outer.max_y() * scale).ceil(),
    );
    let (width, height) = (x1 - x0, y1 - y0);
    if !(width >= 1.0 && height >= 1.0 && width * height <= f64::from(1u32 << 26)) {
        return None;
    }
    let (width, height) = (width as u32, height as u32);
    // the picture's first pixel is the window's pixel (x0, y0): the bands move by whole device pixels into it
    let at = Point::new(x0 / scale, y0 / scale);
    let node = PaintNode {
        layer: None,
        id: PaintNodeId(1),
        parent: None,
        revision: PaintRevision(1),
        transform: Transform([1.0, 0.0, 0.0, 1.0, -at.x, -at.y]),
        clip: None,
        group: None,
        bounds: Rect::new(
            0.0,
            0.0,
            f64::from(width) / scale,
            f64::from(height) / scale,
        ),
        hit: Rect::new(0.0, 0.0, 0.0, 0.0),
        ops: bands(rect, top_reach),
    };
    let list = DisplayList { nodes: vec![node] };
    let mut renderer = CpuRenderer::new(
        CpuConfig {
            max_pixels: 1 << 26,
            glyph_cache_bytes: 0,
            image_cache_bytes: 0,
            font_smoothing: true,
        },
        NoGlyphs,
    );
    let frame = renderer
        .render(&RenderRequest {
            list: &list,
            resources: &PaintResources::default(),
            scale,
            width,
            height,
            damage: &DamageRegion::Full,
        })
        .ok()?;
    let frame = renderer.frame(frame).ok()?;
    Some(BandImage {
        key: [0; 6],
        id: next_resource_id(),
        image: ImageResource {
            revision: 1,
            width,
            height,
            stride: frame.stride,
            format: PixelFormat::Rgba8Premultiplied,
            pixels: Arc::from(frame.pixels.as_slice()),
        },
        dst: Rect::new(
            at.x,
            at.y,
            f64::from(width) / scale,
            f64::from(height) / scale,
        ),
    })
}
