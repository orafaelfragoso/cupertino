//! Drawing helpers shared by the dock's windows: fills, strokes, one line of text, an icon from its best rendition,
//! the keyboard focus ring, and the root node that hands the pointer and the keys to the model.

use cupertino_kit::controls::ImageSource;
use cupertino_kit::core::{
    next_resource_id, Change, Element, Environment, Event, EventCx, MeasureCx, PaintCx, Phase,
    Propagation, Role, SemanticsCx, Widget,
};
use cupertino_kit::geometry::{Point, Rect, Size};
use cupertino_kit::layout::{Constraints, ContainerKind, LayoutSpec, Measured, Overflow, SizeSpec};
use cupertino_kit::platform::{KeyEvent, MouseButton, PointerEvent};
use cupertino_kit::render::{ImageResource, Paint, PaintOp, PixelFormat, Sampling, Shape};
use cupertino_kit::text::{
    AttributedText, FontWeight, GlyphFormat, GlyphKey, GlyphRaster, GlyphRasterizer,
    ParagraphLayout, ParagraphStyle, TextStyle, TextSystem, Wrap,
};
use cupertino_kit::theme::{Color, ColorRole, StateVariant};

use super::turn::{self, Bitmap};
use super::Pointer;
use std::cell::RefCell;
use std::sync::Arc;

/// A colour as red, green, blue and alpha from 0 to 1.
pub type Rgba = [f32; 4];

pub fn color(c: Rgba) -> Color {
    Color::new(c[0], c[1], c[2], c[3])
}

/// `c` with its alpha multiplied by `alpha`.
pub fn faded(c: Rgba, alpha: f64) -> Rgba {
    [c[0], c[1], c[2], c[3] * alpha.clamp(0.0, 1.0) as f32]
}

pub fn is_dark(env: &Environment) -> bool {
    env.theme.preferences.appearance.is_dark()
}

pub fn is_high_contrast(env: &Environment) -> bool {
    env.theme.preferences.appearance.is_high_contrast()
}

fn shape(rect: Rect, radius: f64) -> Shape {
    if radius > 0.0 {
        Shape::RoundedRect { rect, radius }
    } else {
        Shape::Rect(rect)
    }
}

pub fn fill(cx: &mut PaintCx<'_>, rect: Rect, radius: f64, c: Color) {
    cx.push(PaintOp::Fill {
        shape: shape(rect, radius),
        paint: Paint::Solid(c),
    });
}

pub fn stroke(cx: &mut PaintCx<'_>, rect: Rect, radius: f64, width: f64, c: Color) {
    cx.push(PaintOp::Stroke {
        shape: shape(rect, radius),
        width,
        paint: Paint::Solid(c),
    });
}

pub fn circle(cx: &mut PaintCx<'_>, centre: Point, radius: f64, c: Color) {
    let rect = Rect::new(
        centre.x - radius,
        centre.y - radius,
        2.0 * radius,
        2.0 * radius,
    );
    fill(cx, rect, radius, c);
}

/// The keyboard focus ring around `rect`, in the theme's focus colour and width.
pub fn focus_ring(cx: &mut PaintCx<'_>, rect: Rect, radius: f64) {
    let theme = cx.env().theme.clone();
    let Some(c) = theme
        .colors
        .get(ColorRole::KeyboardFocusIndicator, StateVariant::NORMAL)
    else {
        return;
    };
    let width = theme.focus.width;
    let outset = width / 2.0 + 1.0;
    let ring = Rect::new(
        rect.x() - outset,
        rect.y() - outset,
        rect.width() + 2.0 * outset,
        rect.height() + 2.0 * outset,
    );
    stroke(cx, ring, radius + outset, width, c);
}

/// `s` at `size` points and `weight` on one line, cut with an ellipsis past `max_width` points; `None` for no text.
pub fn line(
    env: &Environment,
    s: &str,
    size: f64,
    weight: u16,
    max_width: Option<f64>,
) -> Option<ParagraphLayout> {
    if s.is_empty() {
        return None;
    }
    let style = TextStyle {
        weight: FontWeight(weight),
        ..TextStyle::new(size as f32)
    };
    let paragraph = ParagraphStyle {
        wrap: Wrap::None,
        max_lines: Some(1),
        ellipsis: true,
        ..ParagraphStyle::default()
    };
    let text = env
        .text
        .paragraph(&AttributedText::plain(s, style), &paragraph)
        .ok()?;
    Some(text.layout(max_width.map(|w| w.max(0.0) as f32)))
}

/// Words of `s` broken into lines at most `width` points wide.
pub fn paragraph(
    env: &Environment,
    s: &str,
    size: f64,
    weight: u16,
    width: f64,
) -> Option<ParagraphLayout> {
    if s.is_empty() {
        return None;
    }
    let style = TextStyle {
        weight: FontWeight(weight),
        ..TextStyle::new(size as f32)
    };
    let text = env
        .text
        .paragraph(&AttributedText::plain(s, style), &ParagraphStyle::default())
        .ok()?;
    Some(text.layout(Some(width.max(0.0) as f32)))
}

/// The width of `s` on one line, in points.
pub fn width(env: &Environment, s: &str, size: f64, weight: u16) -> f64 {
    line(env, s, size, weight, None).map_or(0.0, |l| l.size().width)
}

/// Draw `layout` with its top-left corner at `origin`.
pub fn glyphs(cx: &mut PaintCx<'_>, layout: &ParagraphLayout, origin: Point, c: Color) {
    for line in layout.lines() {
        for run in &line.runs {
            cx.push(PaintOp::Glyphs {
                origin: origin + run.origin,
                run: run.run.clone(),
                color: c,
            });
        }
    }
}

/// How much finer than the device a line of text is drawn level before it is turned, and the room kept around it for
/// what a glyph draws outside its line, in points.
const TURNED_TEXT_FINER: f64 = 2.0;
const TURNED_TEXT_ROOM: f64 = 2.0;

thread_local! {
    /// The glyphs of turned text, drawn by the text system they were last drawn for.
    static TURNED_GLYPHS: RefCell<Option<(Arc<TextSystem>, GlyphRaster)>> = const { RefCell::new(None) };
}

/// `layout` in the colour `c`, opaque, drawn level into a picture `finer` times as fine as a device of `scale` pixels
/// a point, with [`TURNED_TEXT_ROOM`] around it; and the picture's size in points.
fn level_text(env: &Environment, layout: &ParagraphLayout, c: Color, finer: f64) -> (Bitmap, Size) {
    let per = env.scale * finer;
    let size = layout.size();
    let width = ((size.width + 2.0 * TURNED_TEXT_ROOM) * per)
        .ceil()
        .max(1.0);
    let height = ((size.height + 2.0 * TURNED_TEXT_ROOM) * per)
        .ceil()
        .max(1.0);
    let mut level = Bitmap::new(width as u32, height as u32);
    TURNED_GLYPHS.with_borrow_mut(|kept| {
        if !kept
            .as_ref()
            .is_some_and(|(of, _)| Arc::ptr_eq(of, &env.text))
        {
            *kept = Some((env.text.clone(), GlyphRaster::new(env.text.clone())));
        }
        let Some((_, raster)) = kept else { return };
        for line in layout.lines() {
            for run in &line.runs {
                for g in &run.run.glyphs {
                    let x = (TURNED_TEXT_ROOM + run.origin.x + f64::from(g.x)) * per;
                    let y = (TURNED_TEXT_ROOM + run.origin.y + f64::from(g.y)) * per;
                    let whole = x.floor();
                    let key = GlyphKey::for_draw(
                        run.run.font,
                        g.id,
                        f64::from(run.run.size) * per,
                        x - whole,
                        true,
                        [c.r, c.g, c.b],
                    );
                    let Ok(Some(glyph)) = raster.rasterize(&key) else {
                        continue;
                    };
                    let left = whole as i64 + i64::from(glyph.bearing_x);
                    let top = y.round() as i64 - i64::from(glyph.bearing_y);
                    let stride = glyph.stride as usize;
                    match glyph.format {
                        GlyphFormat::Coverage => level.cover(
                            &glyph.pixels,
                            stride,
                            glyph.width,
                            glyph.height,
                            left,
                            top,
                            [c.r, c.g, c.b],
                        ),
                        GlyphFormat::ColorPremultiplied => {
                            level.draw(&glyph.pixels, stride, glyph.width, glyph.height, left, top);
                        }
                    }
                }
            }
        }
    });
    (level, Size::new(width / per, height / per))
}

/// Draw the picture `level`, which standing level fills `placed`, turned clockwise by `angle` radians about `pivot`
/// and shown at `alpha`. `placed` and `pivot` are in the node's coordinates, and `node` is where the node's own
/// top-left corner is in its window, so the picture's pixels are the window's. `fine` for a picture at rest; one drawn
/// anew at every frame of a motion is sampled once a pixel.
#[allow(clippy::too_many_arguments)]
pub fn turned_picture(
    cx: &mut PaintCx<'_>,
    level: &Bitmap,
    placed: Rect,
    pivot: Point,
    angle: f64,
    alpha: f64,
    node: Point,
    fine: bool,
) {
    let scale = cx.env().scale;
    let in_window = |p: Point| Point::new(p.x + node.x, p.y + node.y);
    let Some(out) = turn::turned(
        level,
        Rect::from_origin_size(in_window(placed.origin), placed.size),
        in_window(pivot),
        angle,
        scale,
        alpha,
        fine,
    ) else {
        return;
    };
    let image = ImageResource {
        revision: 0,
        width: out.bitmap.width,
        height: out.bitmap.height,
        stride: out.bitmap.width * 4,
        format: PixelFormat::Rgba8Premultiplied,
        pixels: out.bitmap.pixels.into(),
    };
    let dst = Rect::new(
        out.rect.x() - node.x,
        out.rect.y() - node.y,
        out.rect.width(),
        out.rect.height(),
    );
    cx.image(next_resource_id(), &image, dst, Sampling::Linear);
}

/// Draw `layout`, which standing level has its top-left corner at `origin`, turned clockwise by `angle` radians about
/// `pivot`: the line and every glyph on it stand at the angle. `node` is where the node's top-left corner is in its
/// window; `fine` as for [`turned_picture`].
#[allow(clippy::too_many_arguments)]
pub fn glyphs_turned(
    cx: &mut PaintCx<'_>,
    layout: &ParagraphLayout,
    origin: Point,
    pivot: Point,
    angle: f64,
    c: Color,
    node: Point,
    fine: bool,
) {
    let env = cx.env().clone();
    let (level, size) = level_text(&env, layout, c, TURNED_TEXT_FINER);
    let placed = Rect::new(
        origin.x - TURNED_TEXT_ROOM,
        origin.y - TURNED_TEXT_ROOM,
        size.width,
        size.height,
    );
    turned_picture(cx, &level, placed, pivot, angle, f64::from(c.a), node, fine);
}

/// `icon` centred in `rect`, as large as fits, turned clockwise by `angle` radians about `pivot`.
/// `node` is where the node's top-left corner is in its window; `fine` as for [`turned_picture`].
pub fn image_turned(
    cx: &mut PaintCx<'_>,
    icon: &[ImageSource],
    rect: Rect,
    pivot: Point,
    angle: f64,
    node: Point,
    fine: bool,
) {
    let scale = cx.env().scale;
    let side = rect.width().max(rect.height());
    let Some(source) = rendition(icon, side, scale) else {
        return;
    };
    let image = &source.image;
    let (w, h) = (f64::from(image.width), f64::from(image.height));
    if w < 1.0 || h < 1.0 {
        return;
    }
    let fit = (rect.width() / w).min(rect.height() / h);
    let (dw, dh) = (w * fit, h * fit);
    let placed = Rect::new(
        rect.x() + (rect.width() - dw) / 2.0,
        rect.y() + (rect.height() - dh) / 2.0,
        dw,
        dh,
    );
    let mut level = Bitmap::new(image.width, image.height);
    let straight = image.format == PixelFormat::Rgba8;
    for (y, row) in image
        .pixels
        .chunks(image.stride.max(1) as usize)
        .take(image.height as usize)
        .enumerate()
    {
        for (x, px) in row
            .as_chunks::<4>()
            .0
            .iter()
            .take(image.width as usize)
            .enumerate()
        {
            let at = (y * image.width as usize + x) * 4;
            let alpha = u32::from(px[3]);
            for (c, &value) in px.iter().enumerate().take(3) {
                level.pixels[at + c] = if straight {
                    ((u32::from(value) * alpha + 127) / 255) as u8
                } else {
                    value
                };
            }
            level.pixels[at + 3] = px[3];
        }
    }
    turned_picture(cx, &level, placed, pivot, angle, 1.0, node, fine);
}

/// One line of `s` starting at `x`, centred on `cy`, cut at `max_width`; returns its width.
#[allow(clippy::too_many_arguments)]
pub fn text(
    cx: &mut PaintCx<'_>,
    s: &str,
    size: f64,
    weight: u16,
    x: f64,
    cy: f64,
    max_width: Option<f64>,
    c: Color,
) -> f64 {
    let env = cx.env().clone();
    let Some(layout) = line(&env, s, size, weight, max_width) else {
        return 0.0;
    };
    let height = layout.size().height;
    glyphs(cx, &layout, Point::new(x, cy - height / 2.0), c);
    layout.size().width
}

/// The rendition of `icon` that best covers `side` points at the device's scale: the smallest at least as large,
/// else the largest.
pub fn rendition(icon: &[ImageSource], side: f64, scale: f64) -> Option<&ImageSource> {
    let wanted = side * scale;
    icon.iter()
        .filter(|s| f64::from(s.image.width.max(s.image.height)) >= wanted - 0.5)
        .min_by_key(|s| s.image.width.max(s.image.height))
        .or_else(|| {
            icon.iter()
                .max_by_key(|s| s.image.width.max(s.image.height))
        })
}

/// `icon` with its colours multiplied by `by` (0 to 1): the same shapes, darker. Each rendition is a new image.
pub fn darkened(icon: &[ImageSource], by: f32) -> Vec<ImageSource> {
    icon.iter()
        .map(|source| {
            let image = &source.image;
            let mut pixels = image.pixels.to_vec();
            let row = image.width as usize * 4;
            for line in pixels.chunks_mut(image.stride.max(1) as usize) {
                for px in line
                    .iter_mut()
                    .take(row)
                    .enumerate()
                    .filter(|(i, _)| i % 4 < 3)
                {
                    *px.1 = (f32::from(*px.1) * by).round() as u8;
                }
            }
            ImageSource {
                id: next_resource_id(),
                image: ImageResource {
                    pixels: pixels.into(),
                    revision: 0,
                    ..image.clone()
                },
            }
        })
        .collect()
}

/// `icon` centred in `rect`, as large as fits.
pub fn image(cx: &mut PaintCx<'_>, icon: &[ImageSource], rect: Rect) {
    let scale = cx.env().scale;
    let side = rect.width().max(rect.height());
    let Some(source) = rendition(icon, side, scale) else {
        return;
    };
    let (w, h) = (
        f64::from(source.image.width),
        f64::from(source.image.height),
    );
    let fit = (rect.width() / w).min(rect.height() / h);
    let (dw, dh) = (w * fit, h * fit);
    let dst = Rect::new(
        rect.x() + (rect.width() - dw) / 2.0,
        rect.y() + (rect.height() - dh) / 2.0,
        dw,
        dh,
    );
    cx.image(source.id, &source.image, dst, Sampling::Linear);
}

/// Places a leaf at `rect` of its absolute parent.
pub fn placed(rect: Rect) -> LayoutSpec {
    LayoutSpec {
        width: SizeSpec::Fixed(rect.width().max(0.0)),
        height: SizeSpec::Fixed(rect.height().max(0.0)),
        position: rect.origin,
        ..LayoutSpec::new(ContainerKind::Leaf)
    }
}

/// What a [`Root`] draws under its children, in its own coordinates.
pub type Underlay = Box<dyn Fn(&mut PaintCx<'_>)>;

/// The root of a window: it fills the window, lays its children out where the model put them, draws `under`, and
/// reports the pointer (captured from a press to its release, so a drag goes on outside the window) and the keys no
/// child took.
pub struct Root<M> {
    pub role: Role,
    pub name: String,
    pub pointer: Option<Box<dyn Fn(Pointer) -> M>>,
    /// The message of a key no child took; `None` lets it go on.
    pub key: Option<Box<dyn Fn(KeyEvent) -> Option<M>>>,
    /// The children are one roving group: one Tab stop, the arrows move between them.
    pub roving: bool,
    /// Sent when the keyboard leaves the window.
    pub on_blur: Option<M>,
    /// Opacity of the whole window's content.
    pub alpha: f64,
    /// Changes every time what `under` draws changes.
    pub revision: u64,
    pub under: Option<Underlay>,
    /// Sent at every frame the compositor gives the window while it is set: what moves by itself advances with the
    /// display's frames.
    pub frame: Option<M>,
    pub children: Vec<Element<M>>,
}

impl<M: Clone + 'static> Widget<M> for Root<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            overflow: Overflow::Clip,
            ..LayoutSpec::new(ContainerKind::Absolute)
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.revision != old.revision
            || self.alpha != old.alpha
            || self.name != old.name
            || self.role != old.role
            || self.frame.is_some() != old.frame.is_some()
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), constraints: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(constraints.width.ideal(), constraints.height.ideal()),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match event {
            // on the way down to a tile, or at the root itself: over empty space, on a strip with no children, and
            // while the root holds the pointer from a press
            Event::Pointer(pointer, _) if cx.phase() != Phase::Bubble => {
                let Some(input) = &self.pointer else {
                    return Propagation::Continue;
                };
                let pointer = match *pointer {
                    PointerEvent::Enter { position } | PointerEvent::Motion { position } => {
                        Pointer::Move(position)
                    }
                    PointerEvent::Leave => Pointer::Leave,
                    PointerEvent::Button {
                        position,
                        button: MouseButton::Left,
                        pressed: true,
                        mods,
                        ..
                    } => {
                        cx.capture_pointer();
                        Pointer::Press(position, mods)
                    }
                    // the press that opened this popup, let go where it went down, is no click on the popup
                    PointerEvent::Button {
                        button: MouseButton::Left,
                        pressed: false,
                        ..
                    } if !cx.has_pointer_capture() && cx.opening_press().is_some() => {
                        return Propagation::Stop;
                    }
                    PointerEvent::Button {
                        position,
                        button: MouseButton::Left,
                        pressed: false,
                        ..
                    } => {
                        cx.release_pointer();
                        Pointer::Release(position)
                    }
                    PointerEvent::Button {
                        position,
                        button: MouseButton::Right,
                        pressed: true,
                        ..
                    } => Pointer::Secondary(position),
                    PointerEvent::Scroll {
                        position, delta, ..
                    } => Pointer::Scroll(position, delta),
                    PointerEvent::Button { .. } => return Propagation::Stop,
                };
                cx.emit(input(pointer));
                Propagation::Stop
            }
            Event::Key(key, _) if cx.phase() != Phase::Capture => {
                match self.key.as_ref().and_then(|input| input(*key)) {
                    Some(message) => {
                        cx.emit(message);
                        Propagation::Stop
                    }
                    None => Propagation::Continue,
                }
            }
            Event::AnimationFrame(_) => {
                if let Some(frame) = &self.frame {
                    cx.emit(frame.clone());
                    cx.request_animation_frame();
                }
                Propagation::Continue
            }
            Event::FocusWithin { gained: false, .. } | Event::Focus { gained: false, .. } => {
                if let Some(blur) = &self.on_blur {
                    cx.emit(blur.clone());
                }
                Propagation::Continue
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        if self.frame.is_some() {
            cx.request_animation_frame();
        }
        if self.alpha < 1.0 {
            cx.set_group(self.alpha.clamp(0.0, 1.0) as f32, Vec::new());
        }
        if let Some(under) = &self.under {
            under(cx);
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(self.role);
        cx.name(self.name.clone());
        if self.roving {
            cx.roving_group();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cupertino_kit::core::next_resource_id;
    use cupertino_kit::render::{ImageResource, PixelFormat};
    use std::sync::Arc;

    fn source(px: u32) -> ImageSource {
        ImageSource {
            id: next_resource_id(),
            image: ImageResource {
                revision: 0,
                width: px,
                height: px,
                stride: px * 4,
                format: PixelFormat::Rgba8Premultiplied,
                pixels: Arc::from(vec![0; (px * px * 4) as usize]),
            },
        }
    }

    #[test]
    fn the_rendition_is_the_smallest_that_covers_the_box_else_the_largest() {
        let icon = [source(64), source(128), source(256)];
        let side = |s: Option<&ImageSource>| s.map(|s| s.image.width);
        assert_eq!(side(rendition(&icon, 64.0, 1.0)), Some(64));
        assert_eq!(side(rendition(&icon, 64.0, 2.0)), Some(128));
        assert_eq!(side(rendition(&icon, 100.0, 1.0)), Some(128));
        assert_eq!(side(rendition(&icon, 128.0, 3.0)), Some(256));
        assert_eq!(side(rendition(&[], 64.0, 1.0)), None);
    }
}
