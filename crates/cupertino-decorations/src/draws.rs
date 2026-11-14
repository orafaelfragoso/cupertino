//! A title bar, its buttons and the rim as a list of draws in device pixels: shapes through the shared distance
//! (an outline with per-corner extents and an exponent, a solid or linear shade, a stroke width), whole-pixel
//! rectangles, the buttons' glyph paths and the title's glyph coverage images. [`DrawList::paint`] lays them on the
//! CPU in order; a GPU renderer draws the same list with its own pipelines (the shapes by the same distance, the
//! paths and glyphs as coverage images), so both start from one description of the chrome.

use std::rc::Rc;

use crate::coverage::{self, Outline, Shade};
use crate::glyph::Mask;
use crate::palette::Rgba;
use tiny_skia::{Color, FillRule, LineCap, Paint, Pixmap, Stroke, Transform};

/// One glyph's coverage image at a whole device pixel, with the identity of its rasterization (the same glyph at
/// the same size, weight and subpixel position has the same `key`), so a renderer can keep it in an atlas.
#[derive(Clone, Debug)]
pub struct PlacedGlyph {
    /// Where the image's first pixel lies, device pixels from the list's origin.
    pub x: i32,
    pub y: i32,
    pub key: u64,
    pub mask: Rc<Mask>,
}

/// What one draw lays over what is below it (premultiplied source-over).
pub enum Draw {
    /// The shared coverage of `outline` or, with `stroke`, of the band that many device pixels wide inside it, in
    /// `shade`.
    Shape {
        outline: Outline,
        shade: Shade,
        stroke: Option<f32>,
    },
    /// A rectangle of whole device pixels in a straight colour, not antialiased.
    Rect {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        color: Rgba,
    },
    /// A path's antialiased coverage in a straight colour: filled (non-zero), or stroked `stroke` device pixels wide
    /// with round caps.
    Path {
        path: tiny_skia::Path,
        stroke: Option<f32>,
        color: Rgba,
    },
    /// A run of glyph coverage images, each pixel the straight colour times its coverage.
    Glyphs {
        glyphs: Vec<PlacedGlyph>,
        color: Rgba,
    },
}

/// The draws of one image `width` x `height` device pixels, first to last.
pub struct DrawList {
    pub width: u32,
    pub height: u32,
    pub draws: Vec<Draw>,
}

impl DrawList {
    pub fn new(width: u32, height: u32) -> DrawList {
        DrawList {
            width: width.max(1),
            height: height.max(1),
            draws: Vec::new(),
        }
    }

    /// The list painted on the CPU into a transparent image (premultiplied RGBA).
    pub fn paint(&self) -> Pixmap {
        let mut pm = Pixmap::new(self.width, self.height).expect("non-zero size");
        paint_into(&mut pm, &self.draws);
        pm
    }
}

/// Lay `draws` over `pm` in order.
pub fn paint_into(pm: &mut Pixmap, draws: &[Draw]) {
    for d in draws {
        match d {
            Draw::Shape {
                outline,
                shade,
                stroke,
            } => coverage::fill(pm, outline, shade, *stroke),
            Draw::Rect {
                x,
                y,
                width,
                height,
                color,
            } => {
                if let Some(rect) = tiny_skia::Rect::from_xywh(*x, *y, *width, *height) {
                    let mut p = solid(*color);
                    p.anti_alias = false;
                    pm.fill_rect(rect, &p, Transform::identity(), None);
                }
            }
            Draw::Path {
                path,
                stroke,
                color,
            } => {
                let p = solid(*color);
                match stroke {
                    Some(width) => {
                        let stroke = Stroke {
                            width: *width,
                            line_cap: LineCap::Round,
                            ..Stroke::default()
                        };
                        pm.stroke_path(path, &p, &stroke, Transform::identity(), None);
                    }
                    None => pm.fill_path(path, &p, FillRule::Winding, Transform::identity(), None),
                }
            }
            Draw::Glyphs { glyphs, color } => composite_glyphs(pm, glyphs, *color),
        }
    }
}

/// An antialiased paint of a straight colour, held to 0 to 1.
pub fn solid(c: Rgba) -> Paint<'static> {
    let mut p = Paint {
        anti_alias: true,
        ..Paint::default()
    };
    p.shader = tiny_skia::Shader::SolidColor(
        Color::from_rgba(
            c[0].clamp(0.0, 1.0),
            c[1].clamp(0.0, 1.0),
            c[2].clamp(0.0, 1.0),
            c[3].clamp(0.0, 1.0),
        )
        .expect("clamped components"),
    );
    p
}

/// Glyph images laid in a straight colour: premultiplied source-over on the stored (device-encoded) values, without
/// linearisation, contrast or lookup table.
pub fn composite_glyphs(pm: &mut Pixmap, glyphs: &[PlacedGlyph], color: Rgba) {
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    let data = pm.data_mut();
    for g in glyphs {
        let img = &g.mask;
        let (iw, ih) = (img.width as i32, img.height as i32);
        for yy in 0..ih {
            for xx in 0..iw {
                let (dx, dy) = (g.x + xx, g.y + yy);
                if dx < 0 || dy < 0 || dx >= w || dy >= h {
                    continue;
                }
                let cov = img.data[(yy * iw + xx) as usize] as f32 / 255.0 * color[3];
                if cov <= 0.0 {
                    continue;
                }
                let px = &mut data[((dy * w + dx) * 4) as usize..][..4];
                for (c, v) in px.iter_mut().zip([color[0], color[1], color[2]]) {
                    *c = ((v * cov * 255.0) + *c as f32 * (1.0 - cov))
                        .round()
                        .clamp(0.0, 255.0) as u8;
                }
                px[3] = ((cov * 255.0) + px[3] as f32 * (1.0 - cov))
                    .round()
                    .clamp(0.0, 255.0) as u8;
            }
        }
    }
}
