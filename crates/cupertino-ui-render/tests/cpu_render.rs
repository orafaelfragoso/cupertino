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

const BAD_FONT: FontId = FontId(99);

struct Boxes {
    fail: bool,
}

impl GlyphRasterizer for Boxes {
    fn rasterize(&mut self, key: &GlyphKey) -> Result<Option<RasterGlyph>, TextError> {
        if self.fail || key.font == BAD_FONT {
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
        group: None,
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
    glyph_node_in(1, FontId(1), count)
}

fn glyph_node_in(id: u64, font: FontId, count: u32) -> PaintNode {
    let run = GlyphRun {
        font,
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
        id,
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

fn renderer_with(max_pixels: u64) -> CpuRenderer<Boxes> {
    CpuRenderer::new(
        CpuConfig {
            max_pixels,
            ..config()
        },
        Boxes { fail: false },
    )
}

fn cost_of(
    width: u32,
    height: u32,
    list: &DisplayList,
    damage: &DamageRegion,
) -> cupertino_ui_render::RenderStats {
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
    r.render(&request).unwrap();
    *r.stats()
}

#[test]
fn cost_follows_damage_and_content_not_target_area() {
    let list = DisplayList {
        nodes: vec![glyph_node(2000)],
    };
    let full = DamageRegion::Full;
    let small = cost_of(512, 288, &list, &full);
    let large = cost_of(3840, 2160, &list, &full);
    assert_eq!(small.dirty_pixels, 512 * 288);
    assert_eq!(large.dirty_pixels, 3840 * 2160);
    assert_eq!(large.clear.bytes, large.dirty_pixels * 4);
    assert_eq!(
        (large.nodes_painted, large.glyphs, large.masks),
        (small.nodes_painted, small.glyphs, small.masks)
    );
    assert_eq!(large.scratch_images, small.scratch_images);
    assert_eq!(large.node_composite, small.node_composite);
    assert_eq!(large.mask_apply, small.mask_apply);

    let corner = DamageRegion::Rects(vec![rect(0.0, 0.0, 16.0, 16.0)]);
    let partial = cost_of(3840, 2160, &list, &corner);
    assert!(partial.dirty_pixels <= 17 * 17);
    assert_eq!(partial.clear.bytes, partial.dirty_pixels * 4);
    assert!(large.scratch_images.bytes * 100 < large.clear.bytes);
    assert_eq!(partial.scratch_images.bytes, partial.dirty_pixels * 4);
    assert_eq!(partial.node_composite.bytes, partial.dirty_pixels * 4);
    assert_eq!(partial.masks.bytes, partial.dirty_pixels);
}

fn child(id: u64, parent: u64, bounds: Rect, ops: Vec<PaintOp>) -> PaintNode {
    PaintNode {
        parent: Some(PaintNodeId(parent)),
        ..node(id, bounds, ops)
    }
}

fn red_square(x: f64) -> PaintOp {
    fill(
        Shape::Rect(rect(x, 8.0, 20.0, 20.0)),
        solid(1.0, 0.0, 0.0, 1.0),
    )
}

fn faded_sheet(shift: f64) -> DisplayList {
    let all = rect(0.0, 0.0, 96.0, 64.0);
    let sheet = PaintNode {
        group: Some(cupertino_ui_render::Group {
            opacity: 0.5,
            effects: vec![Effect::DropShadow {
                offset: Point { x: 2.0, y: 2.0 },
                blur: 4.0,
                color: Color::new(0.0, 0.0, 0.0, 0.6),
            }],
        }),
        clip: Some(Shape::RoundedRect {
            rect: rect(4.0, 4.0, 80.0, 50.0),
            radius: 14.0,
        }),
        ..node(1, all, vec![])
    };
    DisplayList {
        nodes: vec![
            sheet,
            child(2, 1, all, vec![red_square(8.0 + shift)]),
            child(3, 1, all, vec![red_square(18.0 + shift)]),
        ],
    }
}

#[test]
fn a_parents_group_opacity_applies_once_to_its_subtree() {
    let px = render(
        &mut renderer(),
        &faded_sheet(0.0),
        &PaintResources::default(),
        1.0,
        &DamageRegion::Full,
    )
    .unwrap();
    let at = |x: usize, y: usize| px[(y * 96 + x) * 4..][..4].to_vec();
    assert_eq!(at(10, 15), at(26, 15));
    assert_eq!(
        at(20, 15),
        at(10, 15),
        "the overlap of two children is not darker"
    );
    assert_ne!(at(10, 15)[3], 255, "the group is faded");
}

#[test]
fn a_parents_rounded_clip_clips_its_descendants() {
    let list = DisplayList {
        nodes: vec![
            PaintNode {
                clip: Some(Shape::RoundedRect {
                    rect: rect(0.0, 0.0, 40.0, 40.0),
                    radius: 20.0,
                }),
                ..node(1, rect(0.0, 0.0, 40.0, 40.0), vec![])
            },
            child(
                2,
                1,
                rect(0.0, 0.0, 40.0, 40.0),
                vec![fill(
                    Shape::Rect(rect(0.0, 0.0, 40.0, 40.0)),
                    solid(0.0, 0.0, 1.0, 1.0),
                )],
            ),
        ],
    };
    let px = render(
        &mut renderer(),
        &list,
        &PaintResources::default(),
        1.0,
        &DamageRegion::Full,
    )
    .unwrap();
    assert_eq!(px[(20 * 96 + 20) * 4 + 3], 255, "the middle is painted");
    assert_eq!(px[3], 0, "the corner outside the circle is not");
}

#[test]
fn subtree_incremental_repaint_equals_fresh_repaint() {
    for scale in [1.0, 1.5] {
        let none = PaintResources::default();
        let mut inc = renderer();
        render(
            &mut inc,
            &faded_sheet(0.0),
            &none,
            scale,
            &DamageRegion::Full,
        )
        .unwrap();
        let damage = DamageRegion::Rects(vec![rect(0.0, 0.0, 60.0, 40.0)]);
        let moved = render(&mut inc, &faded_sheet(6.0), &none, scale, &damage).unwrap();
        let fresh = render(
            &mut renderer(),
            &faded_sheet(6.0),
            &none,
            scale,
            &DamageRegion::Full,
        )
        .unwrap();
        assert_eq!(moved, fresh, "scale {scale}");
    }
}

#[test]
fn a_parent_that_is_not_an_earlier_node_is_refused() {
    let list = DisplayList {
        nodes: vec![child(2, 9, rect(0.0, 0.0, 8.0, 8.0), vec![])],
    };
    assert_eq!(
        render(
            &mut renderer(),
            &list,
            &PaintResources::default(),
            1.0,
            &DamageRegion::Full
        ),
        Err(RenderError::UnknownParent(PaintNodeId(2)))
    );
}

#[test]
fn a_nodes_clip_cuts_its_descendants_but_not_its_own_operations() {
    let area = rect(0.0, 0.0, 40.0, 40.0);
    let wide = fill(Shape::Rect(area), solid(0.0, 0.0, 1.0, 1.0));
    let list = DisplayList {
        nodes: vec![
            PaintNode {
                clip: Some(Shape::Rect(rect(0.0, 0.0, 10.0, 10.0))),
                ..node(1, area, vec![wide.clone()])
            },
            child(
                2,
                1,
                area,
                vec![fill(
                    Shape::Rect(rect(0.0, 0.0, 40.0, 40.0)),
                    solid(1.0, 0.0, 0.0, 1.0),
                )],
            ),
        ],
    };
    let px = render(
        &mut renderer(),
        &list,
        &PaintResources::default(),
        1.0,
        &DamageRegion::Full,
    )
    .unwrap();
    let at = |x: usize, y: usize| px[(y * 96 + x) * 4..][..4].to_vec();
    assert_eq!(
        at(5, 5),
        [255, 0, 0, 255],
        "the child shows inside the clip"
    );
    assert_eq!(
        at(30, 30),
        [0, 0, 255, 255],
        "the node's own fill is not clipped, the child's is"
    );
}

#[test]
fn duplicate_ids_and_runaway_depth_are_refused() {
    let leaf = |id: u64, parent: Option<u64>| PaintNode {
        parent: parent.map(PaintNodeId),
        ..node(id, rect(0.0, 0.0, 8.0, 8.0), vec![])
    };
    let none = PaintResources::default();
    let twice = DisplayList {
        nodes: vec![leaf(1, None), leaf(1, None)],
    };
    assert_eq!(
        render(&mut renderer(), &twice, &none, 1.0, &DamageRegion::Full),
        Err(RenderError::DuplicateNode(PaintNodeId(1)))
    );
    let chain = |levels: u64| DisplayList {
        nodes: (0..levels).map(|i| leaf(i, i.checked_sub(1))).collect(),
    };
    assert!(render(
        &mut renderer(),
        &chain(257),
        &none,
        1.0,
        &DamageRegion::Full
    )
    .is_ok());
    assert_eq!(
        render(
            &mut renderer(),
            &chain(258),
            &none,
            1.0,
            &DamageRegion::Full
        ),
        Err(RenderError::TooDeep(PaintNodeId(257)))
    );
}

fn stats_of(damage: &DamageRegion) -> cupertino_ui_render::RenderStats {
    let none = PaintResources::default();
    let mut r = renderer();
    render(&mut r, &scene(0.0), &none, 1.0, &DamageRegion::Full).unwrap();
    render(&mut r, &scene(0.0), &none, 1.0, damage).unwrap();
    *r.stats()
}

#[test]
fn counters_are_deterministic_and_follow_the_damage() {
    let corner = DamageRegion::Rects(vec![rect(0.0, 0.0, 12.0, 12.0)]);
    let (a, b) = (stats_of(&corner), stats_of(&corner));
    assert_eq!(a, b, "two runs count the same");
    let full = stats_of(&DamageRegion::Full);
    assert!(a.dirty_pixels < full.dirty_pixels);
    assert!(a.node_composite.bytes < full.node_composite.bytes);
    assert_eq!(a.clear.bytes, a.dirty_pixels * 4);
    assert_eq!(full.dirty_pixels, 96 * 64);
    assert_eq!(full.frame_bytes, 96 * 64 * 4);
    assert_eq!(full.groups_painted, 0);
    assert_eq!(full.layers.count, 1, "the shadowed layer");
    assert_eq!(full.glyphs, 4);
    assert_eq!(
        a.glyph_rasterize.count, 0,
        "the glyphs were cached by the first frame"
    );
    assert!(full.retained_glyphs.count > 0);
}

#[test]
fn stats_belong_to_their_own_frame_after_a_refusal_or_a_failed_paint() {
    let none = PaintResources::default();
    let mut r = renderer();
    render(&mut r, &scene(0.0), &none, 1.0, &DamageRegion::Full).unwrap();
    assert!(r.stats().dirty_pixels > 0);

    let refused = DisplayList {
        nodes: vec![child(2, 1, rect(0.0, 0.0, 8.0, 8.0), Vec::new())],
    };
    assert!(render(&mut r, &refused, &none, 1.0, &DamageRegion::Full).is_err());
    assert_eq!(*r.stats(), cupertino_ui_render::RenderStats::default());

    let failing = DisplayList {
        nodes: vec![
            glyph_node_in(1, FontId(1), 4),
            glyph_node_in(2, BAD_FONT, 4),
        ],
    };
    let failed = render(&mut r, &failing, &none, 1.0, &DamageRegion::Full);
    assert!(matches!(failed, Err(RenderError::Glyph(_))));
    assert_eq!(r.stats().glyph_rasterize.count, 4);

    let next = DisplayList {
        nodes: vec![glyph_node_in(1, FontId(1), 4)],
    };
    render(&mut r, &next, &none, 1.0, &DamageRegion::Full).unwrap();
    assert_eq!(
        r.stats().glyph_rasterize.count,
        0,
        "the failed frame's fills are not carried over"
    );
}
