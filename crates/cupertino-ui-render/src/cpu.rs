use crate::effects::{self, div255};
use crate::geometry::{self, Bounds};
use crate::glyph_cache::GlyphCache;
use crate::image_cache::ImageCache;
use crate::validate;
use crate::{
    CpuFrame, DamageRegion, Paint, PaintNode, PaintOp, PixelFormat, PixelRect, RenderError,
    RenderFrameId, RenderRequest, Sampling, Shape, UiRenderer,
};
use cupertino_geometry::Rect;
use cupertino_theme::Color;
use cupertino_ui_text::{GlyphFormat, GlyphKey, GlyphRasterizer, RasterGlyph};
use tiny_skia::{
    FilterQuality, GradientStop, LinearGradient, Mask, Pattern, Pixmap, PixmapMut, PixmapPaint,
    Point, RadialGradient, SpreadMode, Stroke,
};

/// Largest glyph the renderer rasterizes, in physical pixels per em.
const MAX_GLYPH_PX: f64 = 8192.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpuConfig {
    /// The most pixels the target or one layer may have.
    pub max_pixels: u64,
    pub glyph_cache_bytes: usize,
    pub image_cache_bytes: usize,
    /// Asks the rasterizer for hinted glyphs.
    pub hint_glyphs: bool,
}

/// Paints display lists into a premultiplied RGBA image it keeps between frames, repainting only the damage.
pub struct CpuRenderer<R> {
    config: CpuConfig,
    rasterizer: R,
    glyphs: GlyphCache,
    images: ImageCache,
    frame: CpuFrame,
    scale: f64,
    /// The image holds the last frame's complete contents.
    valid: bool,
    next_id: u64,
}

impl<R: GlyphRasterizer> CpuRenderer<R> {
    pub fn new(config: CpuConfig, rasterizer: R) -> CpuRenderer<R> {
        CpuRenderer {
            glyphs: GlyphCache::new(config.glyph_cache_bytes),
            images: ImageCache::new(config.image_cache_bytes),
            frame: CpuFrame {
                id: RenderFrameId(0),
                width: 0,
                height: 0,
                stride: 0,
                format: PixelFormat::Rgba8Premultiplied,
                pixels: Vec::new(),
                damage: Vec::new(),
            },
            config,
            rasterizer,
            scale: 1.0,
            valid: false,
            next_id: 1,
        }
    }

    fn prepare(&mut self, request: &RenderRequest<'_>) -> Result<bool, RenderError> {
        validate::check(
            request.list,
            request.resources,
            request.scale,
            request.width,
            request.height,
        )?;
        let stride = request
            .width
            .checked_mul(4)
            .ok_or(RenderError::ResourceLimit)?;
        let len = u64::from(stride) * u64::from(request.height);
        if u64::from(request.width) * u64::from(request.height) > self.config.max_pixels {
            return Err(RenderError::ResourceLimit);
        }
        let resized = request.width != self.frame.width
            || request.height != self.frame.height
            || request.scale != self.scale
            || !self.valid;
        let full = resized || matches!(request.damage, DamageRegion::Full);
        self.valid = false;
        if full {
            let len = usize::try_from(len).map_err(|_| RenderError::ResourceLimit)?;
            self.frame.pixels.clear();
            self.frame
                .pixels
                .try_reserve_exact(len)
                .map_err(|_| RenderError::ResourceLimit)?;
            self.frame.pixels.resize(len, 0);
            self.frame.width = request.width;
            self.frame.height = request.height;
            self.frame.stride = stride;
            self.scale = request.scale;
        }
        Ok(full)
    }
}

impl<R: GlyphRasterizer> UiRenderer for CpuRenderer<R> {
    type Frame = CpuFrame;

    fn render(&mut self, request: &RenderRequest<'_>) -> Result<RenderFrameId, RenderError> {
        let full = self.prepare(request)?;
        let (width, height, scale) = (request.width, request.height, request.scale);
        let whole = Bounds::of_size(width, height);
        let dirty = match (full, request.damage) {
            (false, DamageRegion::Rects(rects)) => geometry::merge(
                rects
                    .iter()
                    .map(|r| Bounds::of_rect(r, scale).intersect(&whole))
                    .collect(),
            ),
            _ => vec![whole],
        };
        {
            let mut base = PixmapMut::from_bytes(&mut self.frame.pixels, width, height)
                .ok_or(RenderError::InvalidSize)?;
            let mut painter = Painter {
                config: &self.config,
                scale,
                resources: request.resources,
                glyphs: &mut self.glyphs,
                images: &mut self.images,
                rasterizer: &mut self.rasterizer,
            };
            for d in &dirty {
                clear(&mut base, d);
                for node in &request.list.nodes {
                    if Bounds::of_rect(&node.bounds, scale).touches(d) {
                        painter.node(&mut base, d, node)?;
                    }
                }
            }
        }
        self.glyphs.trim();
        self.images.trim();
        self.frame.damage = dirty
            .iter()
            .map(|d| PixelRect {
                x: d.x0 as u32,
                y: d.y0 as u32,
                width: d.width(),
                height: d.height(),
            })
            .collect();
        self.frame.id = RenderFrameId(self.next_id);
        self.next_id += 1;
        self.valid = true;
        Ok(self.frame.id)
    }

    fn frame(&self, id: RenderFrameId) -> Result<&CpuFrame, RenderError> {
        if self.valid && id == self.frame.id {
            Ok(&self.frame)
        } else {
            Err(RenderError::UnknownFrame(id))
        }
    }
}

fn clear(base: &mut PixmapMut<'_>, d: &Bounds) {
    let stride = base.width() as usize * 4;
    for y in d.y0..d.y1 {
        let row = y as usize * stride;
        base.data_mut()[row + d.x0 as usize * 4..row + d.x1 as usize * 4].fill(0);
    }
}

/// One pixmap being painted and where it sits in the window's pixels.
struct Target<'a, 'b> {
    pixmap: &'a mut PixmapMut<'b>,
    origin: (i32, i32),
    /// The window pixels the pixmap covers.
    rect: Bounds,
    /// The window pixels whose result is needed.
    limit: Bounds,
    clips: Vec<Mask>,
}

impl Target<'_, '_> {
    fn mask(&self) -> Option<&Mask> {
        self.clips.last()
    }
}

struct Painter<'a, R> {
    config: &'a CpuConfig,
    scale: f64,
    resources: &'a crate::PaintResources,
    glyphs: &'a mut GlyphCache,
    images: &'a mut ImageCache,
    rasterizer: &'a mut R,
}

impl<R: GlyphRasterizer> Painter<'_, R> {
    /// Paints one node into a scratch image of just the pixels it touches, then lays that over the frame, so a
    /// node costs in proportion to its own size and no mask spans the window.
    fn node(
        &mut self,
        base: &mut PixmapMut<'_>,
        dirty: &Bounds,
        node: &PaintNode,
    ) -> Result<(), RenderError> {
        let halo = node_support(node, self.scale);
        let area = Bounds::of_rect(&node.bounds, self.scale).intersect(&dirty.outset(halo));
        if area.is_empty() {
            return Ok(());
        }
        if u64::from(area.width()) * u64::from(area.height()) > self.config.max_pixels {
            return Err(RenderError::ResourceLimit);
        }
        let mut scratch =
            Pixmap::new(area.width(), area.height()).ok_or(RenderError::ResourceLimit)?;
        {
            let mut pm = scratch.as_mut();
            let mut target = Target {
                pixmap: &mut pm,
                origin: (area.x0, area.y0),
                rect: area,
                limit: area,
                clips: Vec::new(),
            };
            if let Some(clip) = &node.clip {
                self.push_clip(&mut target, node, clip)?;
            }
            let mut pos = 0;
            self.run(&mut target, node, &mut pos, halo)?;
        }
        let visible = area.intersect(dirty);
        if visible.is_empty() {
            return Ok(());
        }
        let shown = if visible == area {
            scratch
        } else {
            let crop = tiny_skia::IntRect::from_xywh(
                visible.x0 - area.x0,
                visible.y0 - area.y0,
                visible.width(),
                visible.height(),
            )
            .ok_or(RenderError::ResourceLimit)?;
            scratch.clone_rect(crop).ok_or(RenderError::ResourceLimit)?
        };
        base.draw_pixmap(
            visible.x0,
            visible.y0,
            shown.as_ref(),
            &PixmapPaint::default(),
            tiny_skia::Transform::identity(),
            None,
        );
        Ok(())
    }

    fn run(
        &mut self,
        t: &mut Target<'_, '_>,
        node: &PaintNode,
        pos: &mut usize,
        halo: i32,
    ) -> Result<(), RenderError> {
        while let Some(op) = node.ops.get(*pos) {
            *pos += 1;
            match op {
                PaintOp::Fill { shape, paint } => self.fill(t, node, shape, paint),
                PaintOp::Stroke {
                    shape,
                    width,
                    paint,
                } => self.stroke(t, node, shape, *width, paint),
                PaintOp::Glyphs { origin, run, color } => {
                    self.glyph_run(t, node, origin, run, *color)?
                }
                PaintOp::Image {
                    image,
                    src,
                    dst,
                    sampling,
                } => self.image(t, node, image.id, src, dst, *sampling)?,
                PaintOp::PushClip(shape) => self.push_clip(t, node, shape)?,
                PaintOp::PopClip => {
                    t.clips.pop();
                }
                PaintOp::PushLayer { opacity, effects } => {
                    self.layer(t, node, pos, *opacity, effects, halo)?
                }
                PaintOp::PopLayer => return Ok(()),
            }
        }
        Ok(())
    }

    fn ts(&self, t: &Target<'_, '_>, node: &PaintNode) -> tiny_skia::Transform {
        geometry::to_target(&node.transform, self.scale, t.origin)
    }

    fn push_clip(
        &self,
        t: &mut Target<'_, '_>,
        node: &PaintNode,
        shape: &Shape,
    ) -> Result<(), RenderError> {
        let (w, h) = (t.pixmap.width(), t.pixmap.height());
        let mut mask = match t.mask() {
            Some(m) => m.clone(),
            None => full_mask(w, h)?,
        };
        match geometry::path(shape) {
            Some(path) => {
                mask.intersect_path(&path, geometry::fill_rule(shape), true, self.ts(t, node))
            }
            None => mask.clear(),
        }
        t.clips.push(mask);
        Ok(())
    }

    fn fill(&self, t: &mut Target<'_, '_>, node: &PaintNode, shape: &Shape, paint: &Paint) {
        let (Some(path), Some(paint)) = (geometry::path(shape), skia_paint(paint)) else {
            return;
        };
        let ts = self.ts(t, node);
        t.pixmap.fill_path(
            &path,
            &paint,
            geometry::fill_rule(shape),
            ts,
            t.clips.last(),
        );
    }

    fn stroke(
        &self,
        t: &mut Target<'_, '_>,
        node: &PaintNode,
        shape: &Shape,
        width: f64,
        paint: &Paint,
    ) {
        let (Some(path), Some(paint)) = (geometry::path(shape), skia_paint(paint)) else {
            return;
        };
        let stroke = Stroke {
            width: width as f32,
            ..Stroke::default()
        };
        let ts = self.ts(t, node);
        t.pixmap
            .stroke_path(&path, &paint, &stroke, ts, t.clips.last());
    }

    fn image(
        &mut self,
        t: &mut Target<'_, '_>,
        node: &PaintNode,
        id: crate::ResourceId,
        src: &Rect,
        dst: &Rect,
        sampling: Sampling,
    ) -> Result<(), RenderError> {
        let image = &self.resources.images[&id];
        let whole = self.images.get(id, image, self.config.max_pixels)?;
        if src.width() <= 0.0 || src.height() <= 0.0 || dst.width() <= 0.0 || dst.height() <= 0.0 {
            return Ok(());
        }
        let (x0, y0) = (src.min_x().floor().max(0.0), src.min_y().floor().max(0.0));
        let (x1, y1) = (
            src.max_x().ceil().min(f64::from(image.width)),
            src.max_y().ceil().min(f64::from(image.height)),
        );
        let Some(crop) =
            tiny_skia::IntRect::from_xywh(x0 as i32, y0 as i32, (x1 - x0) as u32, (y1 - y0) as u32)
        else {
            return Ok(());
        };
        let Some(part) = whole.clone_rect(crop) else {
            return Ok(());
        };
        let (sx, sy) = (dst.width() / src.width(), dst.height() / src.height());
        let place = tiny_skia::Transform::from_row(
            sx as f32,
            0.0,
            0.0,
            sy as f32,
            (dst.min_x() + (x0 - src.min_x()) * sx) as f32,
            (dst.min_y() + (y0 - src.min_y()) * sy) as f32,
        );
        let quality = match sampling {
            Sampling::Nearest => FilterQuality::Nearest,
            Sampling::Linear => FilterQuality::Bilinear,
        };
        let paint = tiny_skia::Paint {
            shader: Pattern::new(part.as_ref(), SpreadMode::Pad, quality, 1.0, place),
            ..tiny_skia::Paint::default()
        };
        let Some(path) = geometry::path(&Shape::Rect(*dst)) else {
            return Ok(());
        };
        let ts = self.ts(t, node);
        t.pixmap.fill_path(
            &path,
            &paint,
            tiny_skia::FillRule::Winding,
            ts,
            t.clips.last(),
        );
        Ok(())
    }

    fn glyph_run(
        &mut self,
        t: &mut Target<'_, '_>,
        node: &PaintNode,
        origin: &cupertino_geometry::Point,
        run: &cupertino_ui_text::GlyphRun,
        color: Color,
    ) -> Result<(), RenderError> {
        let [a, b, c, d, e, f] = node.transform.0;
        let size_px = f64::from(run.size) * self.scale * a.hypot(b);
        if size_px > MAX_GLYPH_PX {
            return Err(RenderError::ResourceLimit);
        }
        let size_q = (size_px * 64.0).round() as u32;
        for g in &run.glyphs {
            let (lx, ly) = (origin.x + f64::from(g.x), origin.y + f64::from(g.y));
            let px = (a * lx + c * ly + e) * self.scale;
            let py = (b * lx + d * ly + f) * self.scale;
            let ix = px.floor();
            let key = GlyphKey {
                font: run.font,
                glyph: g.id,
                size_q,
                subpixel_x: (((px - ix) * 4.0) as u8).min(3),
                hinted: self.config.hint_glyphs,
            };
            let Some(glyph) = self.glyphs.get(key, self.rasterizer)? else {
                continue;
            };
            let x = ix as i32 + glyph.bearing_x - t.origin.0;
            let y = py.round() as i32 - glyph.bearing_y - t.origin.1;
            blend_glyph(
                t.pixmap,
                t.clips.last().map(Mask::data),
                x,
                y,
                &glyph,
                color,
            );
        }
        Ok(())
    }

    fn layer(
        &mut self,
        t: &mut Target<'_, '_>,
        node: &PaintNode,
        pos: &mut usize,
        opacity: f32,
        effects: &[crate::Effect],
        halo: i32,
    ) -> Result<(), RenderError> {
        let area = Bounds::of_rect(&node.bounds, self.scale)
            .intersect(&t.limit.outset(halo))
            .intersect(&t.rect);
        if area.is_empty() {
            skip_layer(node, pos);
            return Ok(());
        }
        if u64::from(area.width()) * u64::from(area.height()) > self.config.max_pixels {
            return Err(RenderError::ResourceLimit);
        }
        let mut layer =
            Pixmap::new(area.width(), area.height()).ok_or(RenderError::ResourceLimit)?;
        {
            let mut pm = layer.as_mut();
            let mut inner = Target {
                pixmap: &mut pm,
                origin: (area.x0, area.y0),
                rect: area,
                limit: area,
                clips: Vec::new(),
            };
            self.run(&mut inner, node, pos, halo)?;
        }
        for effect in effects {
            effects::apply(&mut layer, effect, self.scale);
        }
        let paint = PixmapPaint {
            opacity,
            ..PixmapPaint::default()
        };
        t.pixmap.draw_pixmap(
            area.x0 - t.origin.0,
            area.y0 - t.origin.1,
            layer.as_ref(),
            &paint,
            tiny_skia::Transform::identity(),
            t.clips.last(),
        );
        Ok(())
    }
}

fn node_support(node: &PaintNode, scale: f64) -> i32 {
    node.ops
        .iter()
        .map(|op| match op {
            PaintOp::PushLayer { effects, .. } => effects::support(effects, scale),
            _ => 0,
        })
        .fold(0, i32::saturating_add)
}

/// Moves `pos` past the `PopLayer` that closes the layer just opened.
fn skip_layer(node: &PaintNode, pos: &mut usize) {
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

fn full_mask(w: u32, h: u32) -> Result<Mask, RenderError> {
    let mut mask = Mask::new(w, h).ok_or(RenderError::ResourceLimit)?;
    mask.data_mut().fill(255);
    Ok(mask)
}

fn skia_color(c: Color) -> Option<tiny_skia::Color> {
    let q = |v: f32| v.clamp(0.0, 1.0);
    tiny_skia::Color::from_rgba(q(c.r), q(c.g), q(c.b), q(c.a))
}

fn stops(stops: &[crate::GradientStop]) -> Option<Vec<GradientStop>> {
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

fn skia_paint(paint: &Paint) -> Option<tiny_skia::Paint<'static>> {
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

/// Source-over of a glyph image at (`x`, `y`) in the pixmap, through the clip mask.
fn blend_glyph(
    pm: &mut PixmapMut<'_>,
    mask: Option<&[u8]>,
    x: i32,
    y: i32,
    g: &RasterGlyph,
    color: Color,
) {
    let (w, h) = (pm.width() as i32, pm.height() as i32);
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
        if py < 0 || py >= h {
            continue;
        }
        for gx in 0..g.width as i32 {
            let px = x + gx;
            if px < 0 || px >= w {
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
