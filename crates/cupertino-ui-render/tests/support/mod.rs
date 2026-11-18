//! Display lists that exercise every kind of paint the renderers draw, and the glyph pixels they draw with.

use cupertino_geometry::{Point, Rect};
use cupertino_ui_render::{
    Color, Effect, FillRule, GradientStop, Group, ImageResource, Paint, PaintNode, PaintNodeId,
    PaintOp, PaintResources, PaintRevision, PathVerb, PixelFormat, ResourceId, ResourceRef,
    Sampling, Shape, Transform,
};
use cupertino_ui_text::{
    FontId, GlyphFormat, GlyphId, GlyphKey, GlyphRasterizer, GlyphRun, PositionedGlyph,
    RasterGlyph, TextError,
};
use std::sync::Arc;

/// The window the scenes fill, in points; odd, so that no scale lands it on whole pixels.
pub const WINDOW: (f64, f64) = (57.0, 41.0);

/// The pixels of a window of `WINDOW` points at `scale`.
pub fn pixels(scale: f64) -> (u32, u32) {
    (
        (WINDOW.0 * scale).ceil() as u32,
        (WINDOW.1 * scale).ceil() as u32,
    )
}

/// Glyphs drawn as patterns of the glyph's number: coverage ones below `COLOUR_FROM`, colour ones from it.
pub struct Patterns;

pub const COLOUR_FROM: u32 = 100;

impl GlyphRasterizer for Patterns {
    fn rasterize(&mut self, key: &GlyphKey) -> Result<Option<RasterGlyph>, TextError> {
        let side = (key.size_q / 64).max(4);
        let colour = key.glyph.0 >= COLOUR_FROM;
        let mut pixels = Vec::new();
        for y in 0..side {
            for x in 0..side {
                let coverage = (((x * 7 + y * 13 + key.glyph.0) % 5) * 63) as u8;
                if colour {
                    let a = u16::from(coverage);
                    pixels.extend([
                        (a * 200 / 255) as u8,
                        (a * 90 / 255) as u8,
                        (a * 30 / 255) as u8,
                        coverage,
                    ]);
                } else {
                    pixels.push(coverage);
                }
            }
        }
        let bytes = if colour { 4 } else { 1 };
        Ok(Some(RasterGlyph {
            format: if colour {
                GlyphFormat::ColorPremultiplied
            } else {
                GlyphFormat::Coverage
            },
            width: side,
            height: side,
            stride: side * bytes,
            bearing_x: 0,
            bearing_y: side as i32,
            pixels: Arc::from(pixels),
        }))
    }
}

pub struct Scene {
    pub name: &'static str,
    pub nodes: Vec<PaintNode>,
    pub resources: PaintResources,
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rect::new(x, y, w, h)
}

fn colour(r: u8, g: u8, b: u8, a: u8) -> Color {
    Color::from_rgba8([r, g, b, a])
}

fn solid(r: u8, g: u8, b: u8, a: u8) -> Paint {
    Paint::Solid(colour(r, g, b, a))
}

pub fn node(id: u64, parent: Option<u64>, bounds: Rect, ops: Vec<PaintOp>) -> PaintNode {
    PaintNode {
        id: PaintNodeId(id),
        parent: parent.map(PaintNodeId),
        revision: PaintRevision(1),
        transform: Transform::IDENTITY,
        clip: None,
        group: None,
        bounds,
        hit: bounds,
        ops,
        layer: None,
    }
}

fn fill(shape: Shape, paint: Paint) -> PaintOp {
    PaintOp::Fill { shape, paint }
}

fn stroke(shape: Shape, width: f64, paint: Paint) -> PaintOp {
    PaintOp::Stroke {
        shape,
        width,
        paint,
    }
}

fn whole() -> Rect {
    rect(0.0, 0.0, WINDOW.0, WINDOW.1)
}

/// The window's background colour, which every scene is drawn over.
pub const GROUND: [u8; 4] = [236, 240, 246, 255];

/// The window's background node.
fn ground() -> PaintNode {
    node(
        1,
        None,
        whole(),
        vec![fill(
            Shape::Rect(whole()),
            Paint::Solid(Color::from_rgba8(GROUND)),
        )],
    )
}

fn stops(colours: &[(f32, [u8; 4])]) -> Vec<GradientStop> {
    colours
        .iter()
        .map(|&(offset, c)| GradientStop {
            offset,
            color: Color::from_rgba8(c),
        })
        .collect()
}

fn image(side: u32) -> ImageResource {
    let mut pixels = Vec::new();
    for y in 0..side {
        for x in 0..side {
            let on = (x / 2 + y / 2) % 2 == 0;
            pixels.extend(if on {
                [250, 80, 40, 255]
            } else {
                [30, 120, 220, 160]
            });
        }
    }
    ImageResource {
        revision: 1,
        width: side,
        height: side,
        stride: side * 4,
        format: PixelFormat::Rgba8,
        pixels: Arc::from(pixels),
    }
}

/// One scene for each kind of paint, over the window's background.
pub fn scenes() -> Vec<Scene> {
    let mut scenes = Vec::new();
    let mut scene = |name, nodes: Vec<PaintNode>, resources: PaintResources| {
        let mut all = vec![ground()];
        all.extend(nodes);
        scenes.push(Scene {
            name,
            nodes: all,
            resources,
        });
    };
    let none = PaintResources::default;

    scene(
        "fills",
        vec![node(
            2,
            None,
            whole(),
            vec![
                fill(
                    Shape::Rect(rect(3.3, 4.2, 20.0, 12.6)),
                    solid(200, 40, 30, 255),
                ),
                fill(
                    Shape::RoundedRect {
                        rect: rect(18.5, 9.5, 22.0, 16.0),
                        radius: 5.5,
                    },
                    solid(20, 90, 200, 160),
                ),
                fill(
                    Shape::Superellipse {
                        rect: rect(5.0, 20.0, 26.0, 17.0),
                        extent: 9.0,
                        exponent: 5.0,
                        outset: 0.0,
                    },
                    solid(30, 160, 80, 220),
                ),
            ],
        )],
        none(),
    );
    scene(
        "strokes",
        vec![node(
            2,
            None,
            whole(),
            vec![
                stroke(
                    Shape::Rect(rect(4.0, 4.0, 22.0, 14.0)),
                    1.0,
                    solid(20, 20, 20, 255),
                ),
                stroke(
                    Shape::RoundedRect {
                        rect: rect(8.5, 10.5, 30.0, 20.0),
                        radius: 6.0,
                    },
                    2.5,
                    solid(200, 40, 30, 200),
                ),
                stroke(
                    Shape::Superellipse {
                        rect: rect(30.0, 22.0, 22.0, 15.0),
                        extent: 6.0,
                        exponent: 4.0,
                        outset: 0.75,
                    },
                    1.5,
                    solid(20, 90, 200, 255),
                ),
                stroke(
                    Shape::Path {
                        verbs: vec![
                            PathVerb::MoveTo(Point::new(4.0, 36.0)),
                            PathVerb::CubicTo(
                                Point::new(14.0, 24.0),
                                Point::new(22.0, 44.0),
                                Point::new(30.0, 30.0),
                            ),
                        ],
                        fill_rule: FillRule::NonZero,
                    },
                    2.0,
                    solid(30, 160, 80, 255),
                ),
            ],
        )],
        none(),
    );
    scene(
        "gradients",
        vec![node(
            2,
            None,
            whole(),
            vec![
                fill(
                    Shape::Rect(rect(2.0, 2.0, 30.0, 18.0)),
                    Paint::Linear {
                        start: Point::new(2.0, 2.0),
                        end: Point::new(32.0, 20.0),
                        stops: stops(&[
                            (0.0, [255, 60, 40, 255]),
                            (0.4, [250, 220, 40, 128]),
                            (1.0, [30, 60, 220, 255]),
                        ]),
                    },
                ),
                fill(
                    Shape::RoundedRect {
                        rect: rect(24.5, 14.5, 30.0, 24.0),
                        radius: 8.0,
                    },
                    Paint::Radial {
                        center: Point::new(39.0, 26.0),
                        radius: 15.0,
                        stops: stops(&[(0.0, [255, 255, 255, 255]), (1.0, [20, 40, 120, 90])]),
                    },
                ),
            ],
        )],
        none(),
    );
    let star = vec![
        PathVerb::MoveTo(Point::new(28.5, 4.0)),
        PathVerb::LineTo(Point::new(40.0, 36.0)),
        PathVerb::LineTo(Point::new(12.0, 16.0)),
        PathVerb::LineTo(Point::new(45.0, 16.0)),
        PathVerb::LineTo(Point::new(17.0, 36.0)),
        PathVerb::Close,
    ];
    scene(
        "paths",
        vec![node(
            2,
            None,
            whole(),
            vec![
                fill(
                    Shape::Path {
                        verbs: star.clone(),
                        fill_rule: FillRule::EvenOdd,
                    },
                    solid(200, 40, 30, 255),
                ),
                fill(
                    Shape::Path {
                        verbs: vec![
                            PathVerb::MoveTo(Point::new(2.0, 30.0)),
                            PathVerb::QuadTo(Point::new(10.0, 18.0), Point::new(20.0, 38.0)),
                            PathVerb::Close,
                        ],
                        fill_rule: FillRule::NonZero,
                    },
                    solid(20, 90, 200, 180),
                ),
            ],
        )],
        none(),
    );
    scene(
        "clips",
        vec![
            PaintNode {
                clip: Some(Shape::RoundedRect {
                    rect: rect(6.25, 5.25, 30.0, 22.0),
                    radius: 9.0,
                }),
                ..node(2, None, whole(), vec![])
            },
            node(
                3,
                Some(2),
                whole(),
                vec![fill(
                    Shape::Rect(whole()),
                    Paint::Linear {
                        start: Point::new(0.0, 0.0),
                        end: Point::new(57.0, 0.0),
                        stops: stops(&[(0.0, [250, 120, 20, 255]), (1.0, [120, 20, 200, 255])]),
                    },
                )],
            ),
            PaintNode {
                clip: Some(Shape::Path {
                    verbs: vec![
                        PathVerb::MoveTo(Point::new(30.0, 20.0)),
                        PathVerb::LineTo(Point::new(55.0, 22.0)),
                        PathVerb::LineTo(Point::new(42.0, 40.0)),
                        PathVerb::Close,
                    ],
                    fill_rule: FillRule::NonZero,
                }),
                ..node(4, None, whole(), vec![])
            },
            node(
                5,
                Some(4),
                whole(),
                vec![fill(Shape::Rect(whole()), solid(20, 140, 90, 255))],
            ),
        ],
        none(),
    );
    scene(
        "groups",
        vec![
            PaintNode {
                group: Some(Group {
                    opacity: 0.5,
                    effects: vec![],
                }),
                ..node(
                    2,
                    None,
                    rect(3.0, 3.0, 30.0, 22.0),
                    vec![
                        fill(
                            Shape::Rect(rect(3.0, 3.0, 20.0, 14.0)),
                            solid(200, 40, 30, 255),
                        ),
                        fill(
                            Shape::Rect(rect(12.5, 9.5, 20.0, 15.0)),
                            solid(20, 90, 200, 255),
                        ),
                    ],
                )
            },
            PaintNode {
                group: Some(Group {
                    opacity: 1.0,
                    effects: vec![Effect::DropShadow {
                        offset: Point::new(1.0, 3.0),
                        blur: 4.0,
                        color: colour(0, 0, 0, 140),
                    }],
                }),
                ..node(
                    3,
                    None,
                    rect(30.0, 6.0, 24.0, 18.0),
                    vec![fill(
                        Shape::RoundedRect {
                            rect: rect(34.0, 8.0, 16.0, 12.0),
                            radius: 4.0,
                        },
                        solid(250, 250, 250, 255),
                    )],
                )
            },
            PaintNode {
                group: Some(Group {
                    opacity: 0.9,
                    effects: vec![Effect::Blur { radius: 2.5 }],
                }),
                ..node(
                    4,
                    None,
                    rect(4.0, 24.0, 22.0, 14.0),
                    vec![fill(
                        Shape::Rect(rect(8.0, 27.0, 14.0, 8.0)),
                        solid(30, 160, 80, 255),
                    )],
                )
            },
            PaintNode {
                group: Some(Group {
                    opacity: 1.0,
                    effects: vec![Effect::InnerShadow {
                        offset: Point::new(1.0, 2.0),
                        blur: 3.0,
                        color: colour(0, 0, 0, 160),
                    }],
                }),
                ..node(
                    5,
                    None,
                    rect(30.0, 26.0, 24.0, 12.0),
                    vec![fill(
                        Shape::RoundedRect {
                            rect: rect(31.0, 27.0, 22.0, 10.0),
                            radius: 5.0,
                        },
                        solid(240, 200, 60, 255),
                    )],
                )
            },
        ],
        none(),
    );
    let mut images = PaintResources::default();
    images.images.insert(ResourceId(7), image(12));
    let picture = |src, dst, sampling| PaintOp::Image {
        image: ResourceRef {
            id: ResourceId(7),
            revision: 1,
        },
        src,
        dst,
        sampling,
    };
    scene(
        "images",
        vec![node(
            2,
            None,
            whole(),
            vec![
                picture(
                    rect(0.0, 0.0, 12.0, 12.0),
                    rect(4.0, 4.0, 12.0, 12.0),
                    Sampling::Nearest,
                ),
                picture(
                    rect(0.0, 0.0, 12.0, 12.0),
                    rect(20.2, 3.3, 30.0, 22.0),
                    Sampling::Linear,
                ),
                picture(
                    rect(2.0, 2.0, 8.0, 8.0),
                    rect(6.4, 22.4, 10.0, 10.0),
                    Sampling::Nearest,
                ),
                picture(
                    rect(0.0, 0.0, 12.0, 12.0),
                    rect(24.0, 28.0, 7.0, 7.0),
                    Sampling::Linear,
                ),
            ],
        )],
        images,
    );
    let run = |font, glyphs: &[u32], size| GlyphRun {
        font: FontId(font),
        size,
        glyphs: glyphs
            .iter()
            .enumerate()
            .map(|(i, &g)| PositionedGlyph {
                id: GlyphId(g),
                x: i as f32 * 9.3,
                y: 0.0,
                cluster: i,
            })
            .collect(),
    };
    scene(
        "glyphs",
        vec![node(
            2,
            None,
            whole(),
            vec![
                PaintOp::Glyphs {
                    origin: Point::new(3.4, 14.6),
                    run: run(0, &[1, 2, 3, 4, 5], 10.0),
                    color: colour(20, 20, 20, 255),
                },
                PaintOp::Glyphs {
                    origin: Point::new(5.0, 31.0),
                    run: run(0, &[COLOUR_FROM, COLOUR_FROM + 1, 6], 14.0),
                    color: colour(200, 40, 30, 255),
                },
            ],
        )],
        none(),
    );
    scene(
        "transforms",
        vec![PaintNode {
            transform: Transform([0.94, 0.34, -0.34, 0.94, 20.0, 2.0]),
            ..node(
                2,
                None,
                whole(),
                vec![
                    fill(
                        Shape::Rect(rect(0.0, 0.0, 26.0, 16.0)),
                        solid(200, 40, 30, 255),
                    ),
                    stroke(
                        Shape::RoundedRect {
                            rect: rect(2.0, 2.0, 22.0, 12.0),
                            radius: 4.0,
                        },
                        1.5,
                        solid(255, 255, 255, 255),
                    ),
                ],
            )
        }],
        none(),
    );
    scenes
}

/// A node laid over a scene, as an animated element would be: translucent, at fractions of a point.
pub fn overlay() -> (PaintNode, Rect) {
    let area = rect(11.3, 7.7, 24.4, 17.1);
    (
        PaintNode {
            revision: PaintRevision(2),
            ..node(
                90,
                None,
                area,
                vec![fill(
                    Shape::RoundedRect {
                        rect: area,
                        radius: 6.0,
                    },
                    solid(250, 250, 250, 170),
                )],
            )
        },
        area,
    )
}
