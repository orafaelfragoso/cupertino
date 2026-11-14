//! The GPU renderer against the CPU renderer on generated scenes, one primitive group at a time and all together, at
//! scales 1, 1.5 and 2 on odd window sizes, whole and then repainting a few damaged rectangles after a change: every
//! channel of every pixel within 2/255, but for a few pixels a step more (see [`STRAY_PIXELS`]).

use std::sync::Arc;

use cupertino_geometry::{Point, Rect};
use cupertino_theme::Color;
use cupertino_ui_render::{
    DamageRegion, DisplayList, Effect, FillRule, GradientStop, Group, ImageResource, Paint,
    PaintNode, PaintNodeId, PaintOp, PaintResources, PaintRevision, PathVerb, PixelFormat,
    ResourceId, ResourceRef, Sampling, Shape, Transform, UiRenderer,
};
use cupertino_ui_text::{
    FontId, GlyphFormat, GlyphId, GlyphKey, GlyphRasterizer, GlyphRun, PositionedGlyph,
    RasterGlyph, TextError,
};

use crate::common::{self, Frame, TOLERANCE};

/// The pixels of a frame that may be one step past the tolerance. The CPU lays an operation's layer, and a solid
/// stroke along a path, through its rasterizer's own pipeline, whose products round each piece apart; where several
/// such roundings stack on one pixel the frames can part by 3/255. Never more, and never on more pixels than these.
const STRAY_PIXELS: usize = 4;

/// Glyphs drawn from their key: coverage boxes of a size that follows the text size, and glyph 7 in colour.
#[derive(Clone, Copy)]
struct Boxes;

impl GlyphRasterizer for Boxes {
    fn rasterize(&mut self, key: &GlyphKey) -> Result<Option<RasterGlyph>, TextError> {
        let side = (key.size_q / 64).max(1);
        if key.glyph == GlyphId(0) {
            return Ok(None);
        }
        if key.glyph == GlyphId(7) {
            let width = side + 2;
            let pixels: Vec<u8> = (0..width * side)
                .flat_map(|i| {
                    let a = (i * 53 % 256) as u8;
                    let c = |k: u32| ((i * k % 256) as u16 * u16::from(a) / 255) as u8;
                    [c(17), c(91), c(43), a]
                })
                .collect();
            return Ok(Some(RasterGlyph {
                format: GlyphFormat::ColorPremultiplied,
                width,
                height: side,
                stride: width * 4,
                bearing_x: 0,
                bearing_y: side as i32 - 1,
                pixels: Arc::from(pixels),
            }));
        }
        let width = side / 2 + 1 + key.glyph.0 % 3;
        let pixels: Vec<u8> = (0..width * side)
            .map(|i| (i * 37 % 256) as u8 / 2 + key.subpixel_x * 16 + key.dilation_q * 8)
            .collect();
        Ok(Some(RasterGlyph {
            format: GlyphFormat::Coverage,
            width,
            height: side,
            stride: width,
            bearing_x: -1,
            bearing_y: side as i32 - 2,
            pixels: Arc::from(pixels),
        }))
    }
}

const PICTURE: ResourceId = ResourceId(1);
const PREMULTIPLIED: ResourceId = ResourceId(2);
const WIDE: ResourceId = ResourceId(3);

fn resources() -> PaintResources {
    let straight: Vec<u8> = (0..8 * 8 * 4).map(|i| (i * 29 % 256) as u8).collect();
    let premultiplied: Vec<u8> = (0..5 * 7)
        .flat_map(|i: u32| {
            let a = (i * 71 % 256) as u8;
            let c = |k: u32| ((i * k % 256) as u16 * u16::from(a) / 255) as u8;
            [c(13), c(57), c(101), a]
        })
        .collect();
    let wide: Vec<u8> = (0..23 * 3)
        .flat_map(|i: u32| [(i * 11) as u8, (i * 5) as u8, 200, 255, 0, 0])
        .collect();
    PaintResources {
        images: [
            (
                PICTURE,
                ImageResource {
                    revision: 1,
                    width: 8,
                    height: 8,
                    stride: 32,
                    format: PixelFormat::Rgba8,
                    pixels: Arc::from(straight),
                },
            ),
            (
                PREMULTIPLIED,
                ImageResource {
                    revision: 1,
                    width: 5,
                    height: 7,
                    stride: 20,
                    format: PixelFormat::Rgba8Premultiplied,
                    pixels: Arc::from(premultiplied),
                },
            ),
            (
                WIDE,
                ImageResource {
                    revision: 1,
                    width: 23,
                    height: 3,
                    // rows padded by four bytes
                    stride: 23 * 4 + 4,
                    format: PixelFormat::Rgba8,
                    pixels: Arc::from(wide[..(23 * 4 + 4) * 3].to_vec()),
                },
            ),
        ]
        .into(),
    }
}

/// A small deterministic generator.
struct Draw(u64);

impl Draw {
    fn next(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) as u32
    }

    /// In `[lo, hi)`, on a grid of `1 / steps`; `steps` 0 for any value.
    fn value(&mut self, lo: f64, hi: f64, steps: u32) -> f64 {
        let unit = f64::from(self.next()) / f64::from(u32::MAX);
        let v = lo + (hi - lo) * unit;
        if steps == 0 {
            v
        } else {
            (v * f64::from(steps)).floor() / f64::from(steps)
        }
    }

    fn pick(&mut self, n: u32) -> u32 {
        self.next() % n
    }

    fn color(&mut self) -> Color {
        let a = match self.pick(5) {
            0 | 1 => 1.0,
            2 => 0.03,
            3 => 0.5,
            _ => self.value(0.0, 1.0, 0) as f32,
        };
        let mut c = || self.value(0.0, 1.0, 0) as f32;
        Color::new(c(), c(), c(), a)
    }

    fn rect(&mut self, w: f64, h: f64, steps: u32) -> Rect {
        let (x, y) = (self.value(-10.0, w, steps), self.value(-10.0, h, steps));
        Rect::new(
            x,
            y,
            self.value(0.5, w / 1.5, steps),
            self.value(0.5, h / 1.5, steps),
        )
    }

    fn grid(&mut self) -> u32 {
        [0, 1, 2, 4, 8][self.pick(5) as usize]
    }

    fn stops(&mut self) -> Vec<GradientStop> {
        let n = 2 + self.pick(4);
        let mut offsets: Vec<f32> = (0..n).map(|_| self.value(0.0, 1.0, 0) as f32).collect();
        offsets.sort_by(f32::total_cmp);
        if self.pick(2) == 0 {
            offsets[0] = 0.0;
            offsets[n as usize - 1] = 1.0;
        }
        offsets
            .into_iter()
            .map(|offset| GradientStop {
                offset,
                color: self.color(),
            })
            .collect()
    }

    fn transform(&mut self, origin: (f64, f64)) -> Transform {
        match self.pick(6) {
            0 => {
                let (s, c) = self.value(-0.6, 0.6, 0).sin_cos();
                Transform([c, s, -s, c, origin.0, origin.1])
            }
            1 => {
                let k = self.value(0.6, 1.7, 0);
                Transform([k, 0.0, 0.0, k * 0.9, origin.0, origin.1])
            }
            _ => Transform([1.0, 0.0, 0.0, 1.0, origin.0, origin.1]),
        }
    }
}

/// The primitive groups a scene may draw.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Shapes,
    Gradients,
    Glyphs,
    Images,
    Paths,
    Clips,
    Layers,
    Effects,
    All,
}

const SIZE: (f64, f64) = (67.0, 51.0);

fn node(id: u64, parent: Option<u64>, transform: Transform, ops: Vec<PaintOp>) -> PaintNode {
    PaintNode {
        layer: None,
        id: PaintNodeId(id),
        parent: parent.map(PaintNodeId),
        revision: PaintRevision(1),
        transform,
        clip: None,
        group: None,
        bounds: Rect::new(-20.0, -20.0, SIZE.0 + 40.0, SIZE.1 + 40.0),
        hit: Rect::new(0.0, 0.0, 0.0, 0.0),
        ops,
    }
}

fn path(draw: &mut Draw, rect: Rect) -> Shape {
    let p = |draw: &mut Draw| {
        Point::new(
            draw.value(rect.min_x(), rect.max_x(), 0),
            draw.value(rect.min_y(), rect.max_y(), 0),
        )
    };
    let mut verbs = vec![PathVerb::MoveTo(rect.origin)];
    for _ in 0..2 + draw.pick(3) {
        verbs.push(match draw.pick(3) {
            0 => PathVerb::LineTo(p(draw)),
            1 => PathVerb::QuadTo(p(draw), p(draw)),
            _ => PathVerb::CubicTo(p(draw), p(draw), p(draw)),
        });
    }
    verbs.push(PathVerb::Close);
    Shape::Path {
        verbs,
        fill_rule: if draw.pick(2) == 0 {
            FillRule::NonZero
        } else {
            FillRule::EvenOdd
        },
    }
}

fn shape(draw: &mut Draw, kind: Kind, rect: Rect, g: u32) -> Shape {
    let paths = matches!(kind, Kind::Paths | Kind::All);
    match draw.pick(if paths { 4 } else { 3 }) {
        0 => Shape::Rect(rect),
        1 | 2 => Shape::RoundedRect {
            rect,
            radius: draw.value(0.0, 14.0, g),
        },
        _ => path(draw, rect),
    }
}

fn paint(draw: &mut Draw, kind: Kind, rect: Rect) -> Paint {
    if !matches!(kind, Kind::Gradients | Kind::All) || draw.pick(2) == 0 {
        return Paint::Solid(draw.color());
    }
    if draw.pick(2) == 0 {
        Paint::Linear {
            start: Point::new(
                draw.value(rect.min_x() - 5.0, rect.max_x(), 0),
                draw.value(rect.min_y() - 5.0, rect.max_y(), 0),
            ),
            end: Point::new(
                draw.value(rect.min_x(), rect.max_x() + 5.0, 0),
                draw.value(rect.min_y(), rect.max_y() + 5.0, 0),
            ),
            stops: draw.stops(),
        }
    } else {
        Paint::Radial {
            center: Point::new(
                draw.value(rect.min_x(), rect.max_x(), 0),
                draw.value(rect.min_y(), rect.max_y(), 0),
            ),
            radius: draw.value(0.5, 30.0, 0),
            stops: draw.stops(),
        }
    }
}

fn op(draw: &mut Draw, kind: Kind, g: u32) -> Vec<PaintOp> {
    let (w, h) = SIZE;
    let rect = draw.rect(w / 2.0, h / 2.0, g);
    let glyphs = matches!(kind, Kind::Glyphs | Kind::All) && draw.pick(3) == 0;
    let images = matches!(kind, Kind::Images | Kind::All) && draw.pick(3) == 0;
    let clips = matches!(kind, Kind::Clips | Kind::All) && draw.pick(4) == 0;
    let layers = matches!(kind, Kind::Layers | Kind::Effects | Kind::All) && draw.pick(6) == 0;
    if glyphs {
        return vec![PaintOp::Glyphs {
            origin: rect.origin,
            run: GlyphRun {
                font: FontId(1),
                size: draw.value(4.0, 16.0, 4) as f32,
                glyphs: (0..1 + draw.pick(7))
                    .map(|i| PositionedGlyph {
                        id: GlyphId(draw.pick(9)),
                        x: i as f32 * draw.value(2.0, 9.0, 0) as f32,
                        y: draw.value(-1.0, 1.0, 0) as f32,
                        cluster: 0,
                    })
                    .collect(),
            },
            color: draw.color(),
        }];
    }
    if images {
        let (id, size) = match draw.pick(3) {
            0 => (PICTURE, (8.0, 8.0)),
            1 => (PREMULTIPLIED, (5.0, 7.0)),
            _ => (WIDE, (23.0, 3.0)),
        };
        let src = if draw.pick(2) == 0 {
            Rect::new(0.0, 0.0, size.0, size.1)
        } else {
            Rect::new(
                draw.value(0.0, size.0 / 2.0, 0),
                draw.value(0.0, size.1 / 2.0, 0),
                draw.value(0.5, size.0 / 2.0, 0),
                draw.value(0.5, size.1 / 2.0, 0),
            )
        };
        let dst = if draw.pick(3) == 0 {
            // one image pixel to a point
            Rect::new(
                rect.min_x().round(),
                rect.min_y().round(),
                src.width(),
                src.height(),
            )
        } else {
            rect
        };
        return vec![PaintOp::Image {
            image: ResourceRef { id, revision: 1 },
            src,
            dst,
            sampling: if draw.pick(2) == 0 {
                Sampling::Nearest
            } else {
                Sampling::Linear
            },
        }];
    }
    if clips {
        let at = draw.rect(w / 2.0, h / 2.0, g);
        let clip = shape(draw, kind, at, g);
        let inner = draw.rect(w / 2.0, h / 2.0, g);
        let mut ops = vec![PaintOp::PushClip(clip)];
        if draw.pick(2) == 0 {
            ops.push(PaintOp::PushClip(Shape::Rect(draw.rect(
                w / 2.0,
                h / 2.0,
                g,
            ))));
            let fill = shape(draw, kind, inner, g);
            ops.push(PaintOp::Fill {
                shape: fill,
                paint: paint(draw, kind, inner),
            });
            ops.push(PaintOp::PopClip);
        }
        let fill = shape(draw, kind, rect, g);
        ops.push(PaintOp::Fill {
            shape: fill,
            paint: paint(draw, kind, rect),
        });
        ops.push(PaintOp::PopClip);
        return ops;
    }
    if layers {
        let effects = if matches!(kind, Kind::Effects | Kind::All) {
            effects(draw)
        } else {
            Vec::new()
        };
        let inner = draw.rect(w / 2.0, h / 2.0, g);
        let fill = shape(draw, kind, inner, g);
        return vec![
            PaintOp::PushLayer {
                opacity: [1.0, 0.6, 0.25][draw.pick(3) as usize],
                effects,
            },
            PaintOp::Fill {
                shape: fill,
                paint: paint(draw, kind, inner),
            },
            PaintOp::Fill {
                shape: Shape::Rect(rect),
                paint: Paint::Solid(draw.color()),
            },
            PaintOp::PopLayer,
        ];
    }
    let s = shape(draw, kind, rect, g);
    let p = paint(draw, kind, rect);
    if draw.pick(4) == 0 {
        vec![PaintOp::Stroke {
            shape: s,
            width: [1.0, 0.5, 2.5, 6.0][draw.pick(4) as usize],
            paint: p,
        }]
    } else {
        vec![PaintOp::Fill { shape: s, paint: p }]
    }
}

fn effects(draw: &mut Draw) -> Vec<Effect> {
    (0..1 + draw.pick(2))
        .map(|_| match draw.pick(3) {
            0 => Effect::Blur {
                radius: draw.value(0.0, 12.0, 0),
            },
            1 => Effect::DropShadow {
                offset: Point::new(draw.value(-4.0, 4.0, 2), draw.value(-2.0, 6.0, 2)),
                blur: draw.value(0.0, 10.0, 0),
                color: draw.color(),
            },
            _ => Effect::InnerShadow {
                offset: Point::new(draw.value(-4.0, 4.0, 2), draw.value(-2.0, 6.0, 2)),
                blur: draw.value(0.0, 8.0, 0),
                color: draw.color(),
            },
        })
        .collect()
}

/// An opaque background and a tree of nodes drawing `kind`, some of them clipping, grouping or moved.
fn scene(seed: u64, kind: Kind) -> DisplayList {
    let mut draw = Draw(seed);
    let (w, h) = SIZE;
    let mut nodes = vec![node(
        1,
        None,
        Transform([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]),
        vec![PaintOp::Fill {
            shape: Shape::Rect(Rect::new(0.0, 0.0, w, h)),
            paint: Paint::Solid(Color::new(0.9, 0.88, 0.85, 1.0)),
        }],
    )];
    let mut parents = vec![1];
    let count = if kind == Kind::All { 30 } else { 18 };
    for id in 2..2 + count {
        let g = draw.grid();
        let origin = (draw.value(-5.0, w / 2.0, g), draw.value(-5.0, h / 2.0, g));
        let transform = draw.transform(origin);
        let ops = (0..1 + draw.pick(3))
            .flat_map(|_| op(&mut draw, kind, g))
            .collect();
        let parent = parents[draw.pick(parents.len() as u32) as usize];
        let mut n = node(id, Some(parent), transform, ops);
        if matches!(kind, Kind::Clips | Kind::All) && draw.pick(4) == 0 {
            let r = draw.rect(w, h, g);
            n.clip = Some(shape(&mut draw, kind, r, g));
        }
        if matches!(kind, Kind::Layers | Kind::Effects | Kind::All) && draw.pick(4) == 0 {
            n.group = Some(Group {
                opacity: [1.0, 0.7, 0.4][draw.pick(3) as usize],
                effects: if matches!(kind, Kind::Effects | Kind::All) {
                    effects(&mut draw)
                } else {
                    Vec::new()
                },
            });
        }
        nodes.push(n);
        parents.push(id);
    }
    DisplayList { nodes }
}

const SCALES: [f64; 3] = [1.0, 1.5, 2.0];

/// Draws each seed's scene on both renderers at every scale, whole and then after changes with a few damaged
/// rectangles, and fails naming every frame beyond the tolerance.
fn compare(kind: Kind, seeds: std::ops::Range<u64>) {
    let Some(device) = common::device() else {
        return;
    };
    let resources = resources();
    let mut failures = Vec::new();
    let mut largest = 0;
    for seed in seeds {
        for scale in SCALES {
            let mut list = scene(seed, kind);
            // odd sizes at every scale
            let size = (
                ((SIZE.0 * scale).ceil() as u32) | 1,
                ((SIZE.1 * scale).ceil() as u32) | 1,
            );
            let mut cpu = common::cpu(Boxes);
            let mut gpu = common::gpu(&device, Boxes);
            let mut draw = Draw(seed ^ 0x5eed);
            for frame in 0..3 {
                let damage = if frame == 0 {
                    DamageRegion::Full
                } else {
                    let i = 1 + draw.pick(list.nodes.len() as u32 - 1) as usize;
                    for op in &mut list.nodes[i].ops {
                        if let PaintOp::Fill { paint, .. } = op {
                            *paint = Paint::Solid(draw.color());
                        }
                    }
                    DamageRegion::Rects(
                        (0..1 + draw.pick(3))
                            .map(|_| {
                                let g = [0, 1, 4][draw.pick(3) as usize];
                                draw.rect(SIZE.0, SIZE.1, g)
                            })
                            .collect(),
                    )
                };
                let f = Frame {
                    list: &list,
                    resources: &resources,
                    scale,
                    size,
                    damage: &damage,
                };
                let a = common::draw_cpu(&mut cpu, &f);
                let b = common::draw_gpu(&mut gpu, &f);
                let (beyond, most, first) = common::difference(&a, &b, size.0);
                largest = largest.max(most);
                if beyond > STRAY_PIXELS || most > TOLERANCE + 1 {
                    let (x, y, p, q) = first.unwrap();
                    let culprit = if frame == 0 {
                        culprit(&device, &list, &resources, scale, size)
                    } else {
                        // which renderer's repaint differs from its own frame drawn whole
                        let whole = Frame {
                            damage: &DamageRegion::Full,
                            ..f
                        };
                        let c = common::draw_cpu(&mut common::cpu(Boxes), &whole);
                        let g = common::draw_gpu(&mut common::gpu(&device, Boxes), &whole);
                        format!(
                            "\n  damage {damage:?}; cpu repaint against whole: {:?}; gpu: {:?}",
                            common::difference(&a, &c, size.0),
                            common::difference(&b, &g, size.0)
                        )
                    };
                    failures.push(format!(
                        "seed {seed} scale {scale} frame {frame}: {beyond} pixels beyond, largest {most}; first at \
                         ({x}, {y}): cpu {p:?} gpu {q:?}{culprit}"
                    ));
                    break;
                }
            }
        }
    }
    eprintln!("largest difference {largest}");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The first node whose addition to the scene makes the frames differ, with its ancestors.
fn culprit(
    device: &std::rc::Rc<cupertino_gpu::Device>,
    list: &DisplayList,
    resources: &PaintResources,
    scale: f64,
    size: (u32, u32),
) -> String {
    for n in 1..=list.nodes.len() {
        let part = DisplayList {
            nodes: list.nodes[..n].to_vec(),
        };
        let f = Frame {
            list: &part,
            resources,
            scale,
            size,
            damage: &DamageRegion::Full,
        };
        let a = common::draw_cpu(&mut common::cpu(Boxes), &f);
        let b = common::draw_gpu(&mut common::gpu(device, Boxes), &f);
        if common::difference(&a, &b, size.0).0 > 0 {
            let mut chain = vec![&list.nodes[n - 1]];
            while let Some(parent) = chain.last().unwrap().parent {
                chain.push(list.nodes.iter().find(|m| m.id == parent).unwrap());
            }
            return format!("\n  from node {n}: {chain:#?}");
        }
    }
    String::new()
}

#[test]
fn rectangles_and_rounded_rectangles_match_the_cpu() {
    compare(Kind::Shapes, 0..8);
}

#[test]
fn gradients_match_the_cpu() {
    compare(Kind::Gradients, 100..108);
}

#[test]
fn glyph_runs_match_the_cpu() {
    compare(Kind::Glyphs, 200..208);
}

#[test]
fn images_match_the_cpu() {
    compare(Kind::Images, 300..308);
}

#[test]
fn paths_match_the_cpu() {
    compare(Kind::Paths, 400..408);
}

#[test]
fn clips_match_the_cpu() {
    compare(Kind::Clips, 500..508);
}

#[test]
fn layers_with_opacity_match_the_cpu() {
    compare(Kind::Layers, 600..608);
}

#[test]
fn blur_and_shadows_match_the_cpu() {
    compare(Kind::Effects, 700..708);
}

#[test]
fn scenes_of_everything_match_the_cpu() {
    compare(Kind::All, 800..812);
}

/// Where the scroll layer's clip shows its content, in points.
const VIEW: Rect = Rect {
    origin: Point { x: 4.0, y: 8.0 },
    size: cupertino_geometry::Size {
        width: 55.0,
        height: 36.0,
    },
};

/// `list` as the content of a kept scroll layer (`Layer::Scroll`) moved by `offset`, over a background and under a
/// translucent bar across the layer's top edge, as a scroll view paints it.
fn scrolled(list: &DisplayList, offset: Point, clip: Shape) -> DisplayList {
    let whole = Shape::Rect(Rect::new(0.0, 0.0, SIZE.0, SIZE.1));
    let mut nodes = vec![node(
        8000,
        None,
        Transform([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]),
        vec![PaintOp::Fill {
            shape: whole,
            paint: Paint::Solid(Color::new(0.3, 0.5, 0.7, 1.0)),
        }],
    )];
    let mut layer = node(
        9000,
        None,
        Transform([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]),
        Vec::new(),
    );
    layer.clip = Some(clip);
    layer.layer = Some(cupertino_ui_render::Layer::Scroll { offset });
    nodes.push(layer);
    for n in &list.nodes {
        let mut moved = n.clone();
        moved.transform.0[4] -= offset.x;
        moved.transform.0[5] -= offset.y;
        moved.bounds = n.bounds.translate(-offset.x, -offset.y);
        moved.parent = Some(n.parent.unwrap_or(PaintNodeId(9000)));
        nodes.push(moved);
    }
    nodes.push(node(
        8001,
        None,
        Transform([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]),
        vec![PaintOp::Fill {
            shape: Shape::RoundedRect {
                rect: Rect::new(0.0, 4.0, SIZE.0, 9.0),
                radius: 3.0,
            },
            paint: Paint::Solid(Color::new(0.95, 0.9, 0.8, 0.6)),
        }],
    ));
    DisplayList { nodes }
}

/// A scroll layer scrolled frame by frame by whole device pixels (down, up, sideways, past its whole viewport, not at
/// all) and by a part of one, its frames' damage leaving out what only moved with it as a frame's damage does, now
/// and then a node of it changed with the layer's view damaged: every frame of the GPU renderer, which keeps the
/// layer's content and paints only what comes into view, is the same list drawn whole by a renderer of its own.
#[test]
fn scroll_layers_scrolled_frame_by_frame_are_the_frames_drawn_whole() {
    let Some(device) = common::device() else {
        return;
    };
    let resources = resources();
    let mut failures = Vec::new();
    for seed in 900..906 {
        for scale in SCALES {
            for clip in [
                Shape::Rect(VIEW),
                Shape::RoundedRect {
                    rect: VIEW,
                    radius: 6.0,
                },
            ] {
                let mut content = scene(seed, Kind::All);
                let size = (
                    ((SIZE.0 * scale).ceil() as u32) | 1,
                    ((SIZE.1 * scale).ceil() as u32) | 1,
                );
                let mut gpu = common::gpu(&device, Boxes);
                // a renderer of its own drawing every frame whole
                let mut drawn_whole = common::gpu(&device, Boxes);
                let mut draw = Draw(seed ^ 0x5c40);
                let mut offset = Point::new(0.0, 0.0);
                for frame in 0..12 {
                    let damage = if frame == 0 {
                        DamageRegion::Full
                    } else {
                        let px = |n: f64| n / scale;
                        let (dx, dy) = match draw.pick(7) {
                            0 => (0.0, 120.0),
                            1 => (f64::from(3 + draw.pick(5)), 0.0),
                            2 => (0.0, 0.0),
                            3 => (0.0, 0.5),
                            _ => (0.0, f64::from(draw.pick(41)) - 20.0),
                        };
                        offset = Point::new(offset.x + px(dx), offset.y + px(dy));
                        let mut rects = Vec::new();
                        if frame % 3 == 0 {
                            let i = 1 + draw.pick(content.nodes.len() as u32 - 1) as usize;
                            for op in &mut content.nodes[i].ops {
                                if let PaintOp::Fill { paint, .. } = op {
                                    *paint = Paint::Solid(draw.color());
                                }
                            }
                            // the changed node shows inside the layer's view
                            rects.push(VIEW);
                        }
                        DamageRegion::Rects(rects)
                    };
                    let list = scrolled(&content, offset, clip.clone());
                    let f = Frame {
                        list: &list,
                        resources: &resources,
                        scale,
                        size,
                        damage: &damage,
                    };
                    let b = common::draw_gpu(&mut gpu, &f);
                    let whole = Frame {
                        damage: &DamageRegion::Full,
                        ..f
                    };
                    let a = common::draw_gpu(&mut drawn_whole, &whole);
                    let (beyond, most, first) = common::difference(&a, &b, size.0);
                    if beyond > STRAY_PIXELS || most > TOLERANCE + 1 {
                        let (x, y, p, q) = first.unwrap();
                        failures.push(format!(
                            "seed {seed} scale {scale} {clip:?} frame {frame} offset {offset:?}: {beyond} pixels \
                             beyond, largest {most}; first at ({x}, {y}): whole {p:?} scrolled {q:?}"
                        ));
                        break;
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A frame of a scroll layer drawn whole is the CPU renderer's: both keep the layer's content under none of its
/// clips and lay it over through them, painting in place only where a clip covers part of a pixel.
#[test]
fn scroll_layers_drawn_whole_match_the_cpu() {
    let Some(device) = common::device() else {
        return;
    };
    let resources = resources();
    let mut failures = Vec::new();
    for seed in 900..911 {
        for scale in SCALES {
            for clip in [
                Shape::Rect(VIEW),
                Shape::RoundedRect {
                    rect: VIEW,
                    radius: 6.0,
                },
            ] {
                let size = (
                    ((SIZE.0 * scale).ceil() as u32) | 1,
                    ((SIZE.1 * scale).ceil() as u32) | 1,
                );
                let list = scrolled(&scene(seed, Kind::All), Point::new(0.0, 0.0), clip.clone());
                let f = Frame {
                    list: &list,
                    resources: &resources,
                    scale,
                    size,
                    damage: &DamageRegion::Full,
                };
                let a = common::draw_cpu(&mut common::cpu(Boxes), &f);
                let b = common::draw_gpu(&mut common::gpu(&device, Boxes), &f);
                let (beyond, most, first) = common::difference(&a, &b, size.0);
                if beyond > STRAY_PIXELS || most > TOLERANCE + 1 {
                    let (x, y, p, q) = first.unwrap();
                    failures.push(format!(
                        "seed {seed} scale {scale} {clip:?}: {beyond} pixels beyond, largest {most}; first at \
                         ({x}, {y}): cpu {p:?} gpu {q:?}"
                    ));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A kept scroll layer scrolled by whole device pixels paints only the rows that come into view (and what the
/// damage names) into its texture, and the frame lays the layer's viewport over; a frame that does not move it and
/// damages nothing paints nothing.
#[test]
fn a_kept_scroll_layer_paints_only_the_rows_that_come_into_view() {
    let Some(device) = common::device() else {
        return;
    };
    let resources = resources();
    let scale = 1.5;
    let size = (
        (SIZE.0 * scale).ceil() as u32,
        (SIZE.1 * scale).ceil() as u32,
    );
    let content = scene(903, Kind::All);
    let mut gpu = common::gpu(&device, Boxes);
    // the viewport: the clip's pixels, as a clip's bounds count them, one pixel out
    let reach = |lo: f64, hi: f64| ((hi * scale).ceil() - (lo * scale).floor()) as u64 + 2;
    let view = (
        reach(VIEW.min_x(), VIEW.max_x()),
        reach(VIEW.min_y(), VIEW.max_y()),
    );
    let mut offset = 0.0;
    for (frame, (rows, damage)) in [
        (0i32, None),
        (14, Some(VIEW)),
        (14, None),
        (0, None),
        (-9, None),
    ]
    .into_iter()
    .enumerate()
    {
        offset += f64::from(rows) / scale;
        let list = scrolled(&content, Point::new(0.0, offset), Shape::Rect(VIEW));
        let rects = DamageRegion::Rects(damage.into_iter().collect());
        let f = Frame {
            list: &list,
            resources: &resources,
            scale,
            size,
            damage: if frame == 0 {
                &DamageRegion::Full
            } else {
                &rects
            },
        };
        let id = gpu.render(&f.request()).expect("the GPU frame");
        let stats = *gpu.stats();
        let laid: u64 = gpu
            .frame(id)
            .unwrap()
            .damage
            .iter()
            .map(|d| u64::from(d.width) * u64::from(d.height))
            .sum();
        assert_eq!(stats.kept_layers, 1, "frame {frame}");
        // the rows that came into view, and the damage over the whole viewport
        let painted = match (frame, damage) {
            (0, _) => view.0 * view.1,
            (_, Some(_)) => view.0 * view.1 + view.0 * u64::from(rows.unsigned_abs()),
            _ => view.0 * u64::from(rows.unsigned_abs()),
        };
        assert_eq!(stats.scroll_pixels, painted, "frame {frame}");
        let shown = if rows == 0 && damage.is_none() {
            0
        } else {
            view.0 * view.1
        };
        if frame > 0 {
            assert_eq!(laid, shown, "frame {frame}");
        }
    }
}
