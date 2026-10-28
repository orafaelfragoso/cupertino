use cupertino_geometry::{Point, Rect};
use cupertino_theme::Color;
use cupertino_ui_render::{
    CpuConfig, CpuRenderer, DamageRegion, DisplayList, Effect, GradientStop, ImageResource, Paint,
    PaintNode, PaintNodeId, PaintOp, PaintResources, PaintRevision, PixelFormat, RenderError,
    RenderFrameId, RenderRequest, ResourceId, ResourceRef, Shape, Transform, UiRenderer,
};
use cupertino_ui_text::{
    FontId, GlyphFormat, GlyphId, GlyphKey, GlyphRasterizer, GlyphRun, PositionedGlyph,
    RasterGlyph, TextError,
};
use std::sync::Arc;

struct Boxes {
    fail: bool,
}

impl GlyphRasterizer for Boxes {
    fn rasterize(&mut self, key: &GlyphKey) -> Result<Option<RasterGlyph>, TextError> {
        if self.fail {
            return Err(TextError::UnknownFont(key.font));
        }
        let side = (key.size_q / 64 / 2).max(1);
        let pixels: Vec<u8> = (0..side * side)
            .map(|i| 255 - (i % 3) as u8 * 40 - key.subpixel_x * 8)
            .collect();
        Ok(Some(RasterGlyph {
            format: GlyphFormat::Coverage,
            width: side,
            height: side,
            stride: side,
            bearing_x: 0,
            bearing_y: side as i32,
            pixels: Arc::from(pixels),
        }))
    }
}

fn config() -> CpuConfig {
    CpuConfig {
        max_pixels: 1 << 22,
        glyph_cache_bytes: 1 << 20,
        image_cache_bytes: 1 << 20,
        hint_glyphs: false,
    }
}

fn renderer() -> CpuRenderer<Boxes> {
    CpuRenderer::new(config(), Boxes { fail: false })
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rect::new(x, y, w, h)
}

fn solid(r: f32, g: f32, b: f32, a: f32) -> Paint {
    Paint::Solid(Color::new(r, g, b, a))
}

fn node(id: u64, bounds: Rect, ops: Vec<PaintOp>) -> PaintNode {
    PaintNode {
        id: PaintNodeId(id),
        parent: None,
        revision: PaintRevision(0),
        transform: Transform::IDENTITY,
        clip: None,
        bounds,
        hit: bounds,
        ops,
    }
}

fn fill(shape: Shape, paint: Paint) -> PaintOp {
    PaintOp::Fill { shape, paint }
}

fn render(
    r: &mut CpuRenderer<Boxes>,
    list: &DisplayList,
    resources: &PaintResources,
    scale: f64,
    damage: &DamageRegion,
) -> Result<Vec<u8>, RenderError> {
    let id = r.render(&RenderRequest {
        list,
        resources,
        scale,
        width: 96,
        height: 64,
        damage,
    })?;
    Ok(r.frame(id)?.pixels.clone())
}

fn scene(shift: f64) -> DisplayList {
    let glyphs = GlyphRun {
        font: FontId(1),
        size: 12.0,
        glyphs: (0..4)
            .map(|i| PositionedGlyph {
                id: GlyphId(i),
                x: i as f32 * 7.3,
                y: 0.0,
                cluster: i as usize,
            })
            .collect(),
    };
    DisplayList {
        nodes: vec![
            node(
                1,
                rect(0.0, 0.0, 96.0, 64.0),
                vec![fill(
                    Shape::Rect(rect(0.0, 0.0, 96.0, 64.0)),
                    Paint::Linear {
                        start: Point { x: 0.0, y: 0.0 },
                        end: Point { x: 96.0, y: 64.0 },
                        stops: vec![
                            GradientStop {
                                offset: 0.0,
                                color: Color::new(0.1, 0.2, 0.6, 1.0),
                            },
                            GradientStop {
                                offset: 1.0,
                                color: Color::new(0.9, 0.5, 0.1, 1.0),
                            },
                        ],
                    },
                )],
            ),
            node(
                2,
                rect(10.0 + shift, 10.0, 40.0, 30.0),
                vec![
                    PaintOp::PushLayer {
                        opacity: 0.7,
                        effects: vec![Effect::DropShadow {
                            offset: Point { x: 2.0, y: 3.0 },
                            blur: 4.0,
                            color: Color::new(0.0, 0.0, 0.0, 0.5),
                        }],
                    },
                    fill(
                        Shape::RoundedRect {
                            rect: rect(10.0 + shift, 10.0, 40.0, 30.0),
                            radius: 6.0,
                        },
                        solid(1.0, 1.0, 1.0, 1.0),
                    ),
                    PaintOp::Glyphs {
                        origin: Point {
                            x: 14.0 + shift,
                            y: 30.0,
                        },
                        run: glyphs,
                        color: Color::new(0.0, 0.0, 0.0, 1.0),
                    },
                    PaintOp::PopLayer,
                ],
            ),
        ],
    }
}

#[test]
fn incremental_repaint_equals_fresh_repaint() {
    for scale in [1.0, 1.5, 2.0] {
        let none = PaintResources::default();
        let mut inc = renderer();
        render(&mut inc, &scene(0.0), &none, scale, &DamageRegion::Full).unwrap();
        let damage = DamageRegion::Rects(vec![rect(0.0, 0.0, 70.0, 50.0)]);
        let moved = render(&mut inc, &scene(9.0), &none, scale, &damage).unwrap();
        let fresh = render(
            &mut renderer(),
            &scene(9.0),
            &none,
            scale,
            &DamageRegion::Full,
        )
        .unwrap();
        assert_eq!(moved, fresh, "scale {scale}");
    }
}

#[test]
fn damage_only_touches_its_region() {
    let none = PaintResources::default();
    let mut r = renderer();
    let before = render(&mut r, &scene(0.0), &none, 1.0, &DamageRegion::Full).unwrap();
    let after = render(
        &mut r,
        &scene(9.0),
        &none,
        1.0,
        &DamageRegion::Rects(vec![rect(0.0, 0.0, 8.0, 8.0)]),
    )
    .unwrap();
    let w = 96 * 4;
    for y in 0..64 {
        for x in 0..96 {
            let inside = x < 10 && y < 10;
            if !inside {
                assert_eq!(
                    before[y * w + x * 4..][..4],
                    after[y * w + x * 4..][..4],
                    "({x},{y})"
                );
            }
        }
    }
}

#[test]
fn group_opacity_applies_once_to_overlapping_children() {
    let square = |x: f64| {
        fill(
            Shape::Rect(rect(x, 8.0, 20.0, 20.0)),
            solid(1.0, 0.0, 0.0, 1.0),
        )
    };
    let layered = DisplayList {
        nodes: vec![node(
            1,
            rect(0.0, 0.0, 96.0, 64.0),
            vec![
                PaintOp::PushLayer {
                    opacity: 0.5,
                    effects: vec![],
                },
                square(8.0),
                square(18.0),
                PaintOp::PopLayer,
            ],
        )],
    };
    let none = PaintResources::default();
    let px = render(&mut renderer(), &layered, &none, 1.0, &DamageRegion::Full).unwrap();
    let at = |x: usize, y: usize| px[(y * 96 + x) * 4..][..4].to_vec();
    assert_eq!(at(10, 12), at(22, 12));
    assert_eq!(
        at(20, 12),
        at(10, 12),
        "the overlap is not darker than the rest"
    );
}

#[test]
fn stale_and_missing_images_are_refused_without_touching_the_frame() {
    let image = |revision: u64| PaintOp::Image {
        image: ResourceRef {
            id: ResourceId(7),
            revision,
        },
        src: rect(0.0, 0.0, 2.0, 2.0),
        dst: rect(4.0, 4.0, 16.0, 16.0),
        sampling: cupertino_ui_render::Sampling::Nearest,
    };
    let list = |revision| DisplayList {
        nodes: vec![node(1, rect(0.0, 0.0, 96.0, 64.0), vec![image(revision)])],
    };
    let mut resources = PaintResources::default();
    let mut r = renderer();
    let ok = render(
        &mut r,
        &DisplayList::default(),
        &resources,
        1.0,
        &DamageRegion::Full,
    )
    .unwrap();
    assert_eq!(
        render(&mut r, &list(1), &resources, 1.0, &DamageRegion::Full),
        Err(RenderError::MissingResource(ResourceId(7)))
    );
    resources.images.insert(
        ResourceId(7),
        ImageResource {
            revision: 2,
            width: 2,
            height: 2,
            stride: 8,
            format: PixelFormat::Rgba8,
            pixels: Arc::from([255u8, 0, 0, 255].repeat(4)),
        },
    );
    assert_eq!(
        render(&mut r, &list(1), &resources, 1.0, &DamageRegion::Full),
        Err(RenderError::StaleResource(ResourceId(7)))
    );
    let id = RenderFrameId(1);
    assert_eq!(
        r.frame(id).unwrap().pixels,
        ok,
        "the previous frame is still valid and unchanged"
    );
    let drawn = render(&mut r, &list(2), &resources, 1.0, &DamageRegion::Full).unwrap();
    assert_eq!(drawn[(8 * 96 + 8) * 4..][..4], [255, 0, 0, 255]);
}

#[test]
fn unbalanced_scopes_are_refused() {
    let list = DisplayList {
        nodes: vec![node(
            3,
            rect(0.0, 0.0, 8.0, 8.0),
            vec![PaintOp::PushClip(Shape::Rect(rect(0.0, 0.0, 4.0, 4.0)))],
        )],
    };
    assert_eq!(
        render(
            &mut renderer(),
            &list,
            &PaintResources::default(),
            1.0,
            &DamageRegion::Full
        ),
        Err(RenderError::UnbalancedScope(PaintNodeId(3)))
    );
}

#[test]
fn a_failed_raster_exposes_no_frame_and_the_retry_repaints_in_full() {
    let none = PaintResources::default();
    let mut r = CpuRenderer::new(config(), Boxes { fail: true });
    let first = r.render(&RenderRequest {
        list: &scene(0.0),
        resources: &none,
        scale: 1.0,
        width: 96,
        height: 64,
        damage: &DamageRegion::Full,
    });
    assert!(matches!(first, Err(RenderError::Glyph(_))));
    assert!(r.frame(RenderFrameId(1)).is_err());
    let mut ok = CpuRenderer::new(config(), Boxes { fail: false });
    let want = render(&mut ok, &scene(0.0), &none, 1.0, &DamageRegion::Full).unwrap();
    let retried = render_in_place(&mut r, &none);
    assert_eq!(retried.damage.len(), 1);
    assert_eq!(
        (retried.damage[0].width, retried.damage[0].height),
        (96, 64)
    );
    assert_eq!(retried.pixels.len(), want.len());
}

fn render_in_place(
    r: &mut CpuRenderer<Boxes>,
    none: &PaintResources,
) -> cupertino_ui_render::CpuFrame {
    let list = DisplayList::default();
    let id = r
        .render(&RenderRequest {
            list: &list,
            resources: none,
            scale: 1.0,
            width: 96,
            height: 64,
            damage: &DamageRegion::Rects(vec![rect(0.0, 0.0, 4.0, 4.0)]),
        })
        .unwrap();
    r.frame(id).unwrap().clone()
}

#[test]
fn oversized_targets_hit_the_resource_limit() {
    let mut r = CpuRenderer::new(
        CpuConfig {
            max_pixels: 100,
            ..config()
        },
        Boxes { fail: false },
    );
    assert_eq!(
        render(
            &mut r,
            &DisplayList::default(),
            &PaintResources::default(),
            1.0,
            &DamageRegion::Full
        ),
        Err(RenderError::ResourceLimit)
    );
}

#[test]
fn absurd_effect_extents_are_refused_not_overflowed() {
    for effect in [
        Effect::Blur { radius: 1e9 },
        Effect::DropShadow {
            offset: Point { x: 1e12, y: 0.0 },
            blur: 1.0,
            color: Color::new(0.0, 0.0, 0.0, 1.0),
        },
    ] {
        let list = DisplayList {
            nodes: vec![node(
                1,
                rect(0.0, 0.0, 8.0, 8.0),
                vec![
                    PaintOp::PushLayer {
                        opacity: 1.0,
                        effects: vec![effect],
                    },
                    PaintOp::PopLayer,
                ],
            )],
        };
        assert_eq!(
            render(
                &mut renderer(),
                &list,
                &PaintResources::default(),
                1.0,
                &DamageRegion::Full
            ),
            Err(RenderError::ResourceLimit)
        );
    }
}

fn glyph_node(count: u32) -> PaintNode {
    let run = GlyphRun {
        font: FontId(1),
        size: 8.0,
        glyphs: (0..count)
            .map(|i| PositionedGlyph {
                id: GlyphId(i % 40),
                x: (i % 100) as f32 * 4.0,
                y: (i / 100) as f32 * 6.0,
                cluster: 0,
            })
            .collect(),
    };
    node(
        1,
        rect(0.0, 0.0, 400.0, 130.0),
        vec![
            PaintOp::PushClip(Shape::RoundedRect {
                rect: rect(0.0, 0.0, 400.0, 130.0),
                radius: 8.0,
            }),
            PaintOp::Glyphs {
                origin: Point { x: 0.0, y: 8.0 },
                run,
                color: Color::new(0.0, 0.0, 0.0, 1.0),
            },
            PaintOp::PopClip,
        ],
    )
}

fn best_of_three(
    width: u32,
    height: u32,
    list: &DisplayList,
    damage: &DamageRegion,
) -> std::time::Duration {
    let mut r = renderer_with(1 << 26);
    let request = RenderRequest {
        list,
        resources: &PaintResources::default(),
        scale: 1.0,
        width,
        height,
        damage,
    };
    r.render(&RenderRequest {
        damage: &DamageRegion::Full,
        ..request
    })
    .unwrap();
    (0..3)
        .map(|_| {
            let start = std::time::Instant::now();
            r.render(&request).unwrap();
            start.elapsed()
        })
        .min()
        .unwrap()
}

fn renderer_with(max_pixels: u64) -> CpuRenderer<Boxes> {
    CpuRenderer::new(
        CpuConfig {
            max_pixels,
            ..config()
        },
        Boxes { fail: false },
    )
}

#[test]
fn cost_follows_damage_and_content_not_target_area() {
    let list = DisplayList {
        nodes: vec![glyph_node(2000)],
    };
    let full = DamageRegion::Full;
    let small = best_of_three(512, 288, &list, &full);
    let large = best_of_three(3840, 2160, &list, &full);
    assert!(
        large < small * 6 + std::time::Duration::from_millis(20),
        "full repaint {large:?} on 4K vs {small:?} on 512x288"
    );
    let corner = DamageRegion::Rects(vec![rect(0.0, 0.0, 16.0, 16.0)]);
    let partial = best_of_three(3840, 2160, &list, &corner);
    assert!(
        partial * 5 < large,
        "a 16x16 damage {partial:?} vs full {large:?}"
    );
}
