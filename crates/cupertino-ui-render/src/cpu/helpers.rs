use super::*;

pub(super) fn bounded(area: Bounds, t: &Target<'_, '_>, halo: i32) -> Bounds {
    if t.space == EVERYWHERE
        && u64::from(area.width()) * u64::from(area.height()) > layers::LAYER_SCRATCH_PIXELS
    {
        area.intersect(&t.rect.outset(halo))
    } else {
        area
    }
}

/// `path` placed by `ts` with `holes` (window pixels; the target's top-left at `origin`) cut out of it, and the
/// transform to fill the result with. A hole is a contour of the opposite direction one pixel inside its rectangle, on
/// whole pixels: under the non-zero rule the rasterizer then skips its pixels, and as its edges lie on pixel
/// boundaries clear of every edge of the path, every pixel outside the rectangle is covered exactly as before.
pub(super) fn holed(
    path: Option<tiny_skia::Path>,
    ts: tiny_skia::Transform,
    holes: &[Bounds],
    origin: (i32, i32),
) -> (Option<tiny_skia::Path>, tiny_skia::Transform) {
    if holes.is_empty() {
        return (path, ts);
    }
    let Some(placed) = path.and_then(|p| p.transform(ts)) else {
        return (None, ts);
    };
    let mut b = tiny_skia::PathBuilder::new();
    b.push_path(&placed);
    for h in holes {
        let h = h.outset(-1).offset(-origin.0, -origin.1);
        if h.is_empty() {
            continue;
        }
        let (x0, y0, x1, y1) = (h.x0 as f32, h.y0 as f32, h.x1 as f32, h.y1 as f32);
        b.move_to(x0, y0);
        b.line_to(x0, y1);
        b.line_to(x1, y1);
        b.line_to(x1, y0);
        b.close();
    }
    (b.finish(), tiny_skia::Transform::identity())
}

pub(super) fn node_support(node: &PaintNode, scale: f64) -> i32 {
    node.ops
        .iter()
        .map(|op| match op {
            PaintOp::PushLayer { effects, .. } => effects::support(effects, scale),
            _ => 0,
        })
        .fold(0, i32::saturating_add)
}

/// Moves `pos` past the `PopLayer` that closes the layer just opened.
pub(super) fn skip_layer(node: &PaintNode, pos: &mut usize) {
    let mut depth = 1;
    while let Some(op) = node.ops.get(*pos) {
        *pos += 1;
        match op {
            PaintOp::PushLayer { .. } => depth += 1,
            PaintOp::PopLayer => {
                depth -= 1;
                if depth == 0 {
                    return;
                }
            }
            _ => {}
        }
    }
}

/// Multiplies `mask`, whose top-left pixel is window pixel `origin`, by the coverage of the clip `shape` placed by
/// `window` (node coordinates to window pixels).
pub(super) fn clip_by(
    mask: &mut Mask,
    shape: &Shape,
    window: tiny_skia::Transform,
    origin: (i32, i32),
) {
    if let Shape::Path { .. } = shape {
        let ts = window.post_translate(-origin.0 as f32, -origin.1 as f32);
        match geometry::path(shape) {
            Some(path) => mask.intersect_path(&path, geometry::fill_rule(shape), true, ts),
            None => mask.clear(),
        }
        return;
    }
    match raster::Outline::new(shape, window, None) {
        Some(outline) => outline.clip(mask, origin),
        None => mask.clear(),
    }
}

pub(super) fn full_mask(w: u32, h: u32) -> Result<Mask, RenderError> {
    let mut mask = Mask::new(w, h).ok_or(RenderError::ResourceLimit)?;
    mask.data_mut().fill(255);
    Ok(mask)
}

pub(super) fn skia_color(c: Color) -> Option<tiny_skia::Color> {
    let q = |v: f32| v.clamp(0.0, 1.0);
    tiny_skia::Color::from_rgba(q(c.r), q(c.g), q(c.b), q(c.a))
}

pub(super) fn stops(stops: &[crate::GradientStop]) -> Option<Vec<GradientStop>> {
    stops
        .iter()
        .map(|s| {
            Some(GradientStop::new(
                s.offset.clamp(0.0, 1.0),
                skia_color(s.color)?,
            ))
        })
        .collect()
}

pub(crate) fn skia_paint(paint: &Paint) -> Option<tiny_skia::Paint<'static>> {
    let shader = match paint {
        Paint::Solid(c) => tiny_skia::Shader::SolidColor(skia_color(*c)?),
        Paint::Linear {
            start,
            end,
            stops: s,
        } => LinearGradient::new(
            Point::from_xy(start.x as f32, start.y as f32),
            Point::from_xy(end.x as f32, end.y as f32),
            stops(s)?,
            SpreadMode::Pad,
            tiny_skia::Transform::identity(),
        )?,
        Paint::Radial {
            center,
            radius,
            stops: s,
        } => {
            let c = Point::from_xy(center.x as f32, center.y as f32);
            RadialGradient::new(
                c,
                0.0,
                c,
                *radius as f32,
                stops(s)?,
                SpreadMode::Pad,
                tiny_skia::Transform::identity(),
            )?
        }
    };
    Some(tiny_skia::Paint {
        shader,
        anti_alias: true,
        ..tiny_skia::Paint::default()
    })
}

/// Source-over of the premultiplied `src`, scaled by `opacity`, onto the pixmap with its top-left pixel at `at`, in
/// 8-bit integers. Whatever of `src` falls outside the pixmap is not drawn.
pub(super) fn source_over(
    dst: &mut PixmapMut<'_>,
    at: (i32, i32),
    src: PixmapRef<'_>,
    opacity: f32,
) {
    let whole = Bounds::of_size(src.width(), src.height());
    source_over_part(dst, at, src, &whole, opacity);
}

/// [`source_over`] of the pixels `part` of `src` (in its own pixels), with the part's top-left pixel at `at`.
/// What a layer laid over at `opacity` is multiplied by, from 0 (it changes nothing) to 255.
pub(super) fn opacity_factor(opacity: f32) -> u32 {
    (opacity.clamp(0.0, 1.0) * 255.0).round() as u32
}

pub(super) fn source_over_part(
    dst: &mut PixmapMut<'_>,
    at: (i32, i32),
    src: PixmapRef<'_>,
    part: &Bounds,
    opacity: f32,
) {
    let k = opacity_factor(opacity);
    // most of a layer is often empty: runs of empty pixels are passed over, and nothing is scaled or laid for them; an
    // opaque run laid over whole replaces what it covers
    lay_part(dst, at, src, part, |s, d| {
        if empty(s) {
            return;
        }
        if k == 255 && opaque(s) {
            d.copy_from_slice(s);
            return;
        }
        over_run(s, d, k);
    });
}

/// The pixels `part` of `src` laid over `color` (premultiplied) and written to the pixmap with the part's top-left pixel
/// at `at`, whatever the pixmap held there: the bytes of `color` written first and `src` laid over it with
/// [`source_over_part`], in one pass.
pub(super) fn over_color_part(
    dst: &mut PixmapMut<'_>,
    at: (i32, i32),
    src: PixmapRef<'_>,
    part: &Bounds,
    color: [u8; 4],
) {
    let c = u32::from_le_bytes(color);
    lay_part(dst, at, src, part, |s, d| {
        if empty(s) {
            for d in d.as_chunks_mut::<4>().0 {
                *d = color;
            }
        } else if opaque(s) {
            d.copy_from_slice(s);
        } else {
            for (s, d) in s.as_chunks::<4>().0.iter().zip(d.as_chunks_mut::<4>().0) {
                *d = effects::over_px(u32::from_le_bytes(*s), c).to_le_bytes();
            }
        }
    });
}

/// Calls `run` on each run of up to 16 pixels of the pixels `part` of `src` and the pixels of the pixmap they land on,
/// with the part's top-left pixel at `at`. Whatever of `src` falls outside the pixmap is not drawn.
pub(super) fn lay_part(
    dst: &mut PixmapMut<'_>,
    at: (i32, i32),
    src: PixmapRef<'_>,
    part: &Bounds,
    mut run: impl FnMut(&[u8], &mut [u8]),
) {
    let stride = dst.width() as usize * 4;
    let part = part.intersect(&Bounds::of_size(src.width(), src.height()));
    if part.is_empty() {
        return;
    }
    let src_stride = src.width() as usize * 4;
    let (from, row) = (part.x0 as usize * 4, part.width() as usize * 4);
    let (Ok(x), Ok(top)) = (usize::try_from(at.0), usize::try_from(at.1)) else {
        debug_assert!(false, "the layer starts before the target");
        return;
    };
    let fits = x.checked_mul(4).and_then(|left| left.checked_add(row));
    debug_assert!(
        fits.is_some_and(|right| right <= stride),
        "the layer is wider than the target"
    );
    if fits.is_none_or(|right| right > stride) {
        return;
    }
    let dst = dst.data_mut();
    let lines = src
        .data()
        .chunks_exact(src_stride)
        .skip(part.y0 as usize)
        .take(part.height() as usize)
        .map(|line| &line[from..from + row]);
    for (y, s) in lines.enumerate() {
        let start = top
            .checked_add(y)
            .and_then(|line| line.checked_mul(stride))
            .and_then(|line| line.checked_add(x * 4));
        let Some(d) = start.and_then(|start| dst.get_mut(start..start.checked_add(row)?)) else {
            debug_assert!(false, "the layer is taller than the target");
            return;
        };
        for (s, d) in s.chunks(4 * 16).zip(d.chunks_mut(4 * 16)) {
            run(s, d);
        }
    }
}

/// Whether every pixel of `bytes` is opaque, without an early exit, so it vectorizes.
#[inline]
pub(super) fn opaque(bytes: &[u8]) -> bool {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .fold(255, |all, p| all & p[3])
        == 255
}

/// Whether every byte of `bytes` is zero, read a word at a time and without an early exit, so it vectorizes.
#[inline]
pub(super) fn empty(bytes: &[u8]) -> bool {
    let (words, rest) = bytes.as_chunks::<8>();
    words.iter().fold(0, |any, w| any | u64::from_ne_bytes(*w)) == 0 && rest.iter().all(|&b| b == 0)
}

/// [`source_over_part`] of one run of pixels of `s` onto `d` at opacity `k` (0..=255), without a branch per pixel so
/// that it vectorizes: an empty source pixel leaves `d` as it is and an opaque one replaces it (scaling by 255 and by 0
/// is exact), the bytes the cases taken apart would give.
pub(super) fn over_run(s: &[u8], d: &mut [u8], k: u32) {
    let pairs = s.as_chunks::<4>().0.iter().zip(d.as_chunks_mut::<4>().0);
    if k == 255 {
        for (s, d) in pairs {
            *d = effects::over_px(u32::from_le_bytes(*s), u32::from_le_bytes(*d)).to_le_bytes();
        }
    } else {
        for (s, d) in pairs {
            let s = effects::mul_px(u32::from_le_bytes(*s), k);
            *d = effects::over_px(s, u32::from_le_bytes(*d)).to_le_bytes();
        }
    }
}

/// Source-over of a glyph image at (`x`, `y`) in the pixmap, through the clip mask and inside `limit` (pixmap
/// pixels): a raster can reach past the box its node's extent allows.
/// Where an image operation lands: the source pixels it reads, how they are placed in node coordinates, the node's
/// transform to the target (moved onto whole pixels for an image drawn one source pixel per device pixel), and, when
/// whole source pixels fill a destination on whole pixels, the target pixel their top-left corner falls on.
pub(super) struct Placement {
    pub(super) crop: tiny_skia::IntRect,
    pub(super) place: tiny_skia::Transform,
    pub(super) ts: tiny_skia::Transform,
    pub(super) whole: Option<(i32, i32)>,
}

/// Whether `dst` under `ts` covers exactly the device pixels an image cut to `crop` covers at `at`.
pub(super) fn on_pixels(
    ts: &tiny_skia::Transform,
    dst: &Rect,
    at: (f32, f32),
    crop: tiny_skia::IntRect,
) -> bool {
    const SLACK: f32 = 1e-3;
    if ts.kx != 0.0 || ts.ky != 0.0 {
        return false;
    }
    let edge =
        |v: f64, scale: f32, shift: f32, want: f32| (v as f32 * scale + shift - want).abs() < SLACK;
    edge(dst.min_x(), ts.sx, ts.tx, at.0)
        && edge(dst.max_x(), ts.sx, ts.tx, at.0 + crop.width() as f32)
        && edge(dst.min_y(), ts.sy, ts.ty, at.1)
        && edge(dst.max_y(), ts.sy, ts.ty, at.1 + crop.height() as f32)
}

/// `v` rounded to the nearest whole number, a half to the even one, for `v` in 0 to 2^22: adding 2^23 leaves no
/// fraction bits, so the addition itself rounds. The same result as `f32::round_ties_even`, with no call to make.
#[inline(always)]
pub(super) fn to_even(v: f32) -> f32 {
    const SHIFT: f32 = 8_388_608.0;
    (v + SHIFT) - SHIFT
}

/// Lays the `crop` of a premultiplied image, its rows `stride` bytes apart in `image`, over the pixmap with its top-left
/// corner on pixel `at`, one source pixel per pixel, under the clip `mask`. The arithmetic is the one a pattern fill of the same image computes, in the same
/// order and precision, so the pixels are the same; a direct loop skips the fill's per-pixel sampling.
pub(super) fn lay_over(
    pm: &mut PixmapMut<'_>,
    mask: Option<&[u8]>,
    image: &[u8],
    stride: usize,
    crop: tiny_skia::IntRect,
    at: (i32, i32),
) {
    const UNIT: f32 = 1.0 / 255.0;
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    let (x0, x1) = (at.0.max(0), (at.0 + crop.width() as i32).min(w));
    let (y0, y1) = (at.1.max(0), (at.1 + crop.height() as i32).min(h));
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let (dw, n) = (w as usize, (x1 - x0) as usize);
    let sx = (x0 - at.0 + crop.x()) as usize;
    let (out, _) = pm.data_mut().as_chunks_mut::<4>();
    for y in y0..y1 {
        let sy = (y - at.1 + crop.y()) as usize;
        let row = y as usize * dw + x0 as usize;
        let from = image[sy * stride + sx * 4..][..n * 4].as_chunks::<4>().0;
        let to = &mut out[row..][..n];
        let covers = mask.map(|m| &m[row..][..n]);
        for (i, (s, d)) in from.iter().zip(to.iter_mut()).enumerate() {
            let cover = covers.map_or(255, |m| m[i]);
            if cover == 0 || u32::from_ne_bytes(*s) == 0 {
                continue;
            }
            // opaque, or over nothing: the sum is the source pixel itself
            if cover == 255 && (s[3] == 255 || u32::from_ne_bytes(*d) == 0) {
                *d = *s;
                continue;
            }
            let mut v = s.map(|b| f32::from(b) * UNIT);
            if cover != 255 {
                let c = f32::from(cover) / 255.0;
                v = v.map(|value| value * c);
            }
            let keep = 1.0 - v[3];
            for (b, value) in d.iter_mut().zip(v) {
                let o = f32::from(*b) * UNIT * keep + value;
                *b = to_even(o.clamp(0.0, 1.0) * 255.0) as u8;
            }
        }
    }
}

pub(super) fn blend_glyph(
    pm: &mut PixmapMut<'_>,
    mask: Option<&[u8]>,
    limit: &Bounds,
    x: i32,
    y: i32,
    g: &RasterGlyph,
    color: Color,
) {
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    let (x0, x1) = (limit.x0.max(0), limit.x1.min(w));
    let (y0, y1) = (limit.y0.max(0), limit.y1.min(h));
    let tint = color
        .premultiplied()
        .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u32);
    let stride = pm.width() as usize * 4;
    let bytes_per = match g.format {
        GlyphFormat::Coverage => 1,
        GlyphFormat::ColorPremultiplied => 4,
    };
    for gy in 0..g.height as i32 {
        let py = y + gy;
        if py < y0 || py >= y1 {
            continue;
        }
        for gx in 0..g.width as i32 {
            let px = x + gx;
            if px < x0 || px >= x1 {
                continue;
            }
            let at = gy as usize * g.stride as usize + gx as usize * bytes_per;
            let clip = mask.map_or(255, |m| {
                u32::from(m[py as usize * w as usize + px as usize])
            });
            let src: [u32; 4] = match g.format {
                GlyphFormat::Coverage => {
                    let cov = div255(u32::from(g.pixels[at]) * clip);
                    [0, 1, 2, 3].map(|i| div255(tint[i] * cov))
                }
                GlyphFormat::ColorPremultiplied => {
                    let s = &g.pixels[at..at + 4];
                    let k = div255(tint[3] * clip);
                    [0, 1, 2, 3].map(|i| div255(u32::from(s[i]) * k))
                }
            };
            if src[3] == 0 && src[0] == 0 && src[1] == 0 && src[2] == 0 {
                continue;
            }
            let out = &mut pm.data_mut()[py as usize * stride + px as usize * 4..][..4];
            let keep = 255 - src[3];
            for i in 0..4 {
                out[i] = (src[i] + div255(u32::from(out[i]) * keep)).min(255) as u8;
            }
        }
    }
}
