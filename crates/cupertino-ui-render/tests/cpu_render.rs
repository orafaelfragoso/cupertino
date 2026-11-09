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
use std::sync::{Arc, Mutex};

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
        font_smoothing: true,
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

    // a node the damage cuts is rasterized as in the whole frame, so its pixels come out the same; only the damaged
    // ones are laid over the frame
    let corner = DamageRegion::Rects(vec![rect(0.0, 0.0, 16.0, 16.0)]);
    let partial = cost_of(3840, 2160, &list, &corner);
    assert!(partial.dirty_pixels <= 17 * 17);
    assert_eq!(partial.clear.bytes, partial.dirty_pixels * 4);
    assert!(large.scratch_images.bytes * 100 < large.clear.bytes);
    assert_eq!(partial.scratch_images, large.scratch_images);
    assert_eq!(partial.masks, large.masks);
    assert_eq!(partial.node_composite.bytes, partial.dirty_pixels * 4);
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

/// What one frame of `list` did: the stats of a render after a full first frame.
fn frame_stats(list: &DisplayList, damage: &DamageRegion) -> cupertino_ui_render::RenderStats {
    let mut r = renderer();
    let none = PaintResources::default();
    render(&mut r, list, &none, 1.0, &DamageRegion::Full).unwrap();
    render(&mut r, list, &none, 1.0, damage).unwrap();
    *r.stats()
}

fn blue_square() -> PaintNode {
    node(
        1,
        rect(10.0, 10.0, 30.0, 30.0),
        vec![fill(
            Shape::Rect(rect(10.0, 10.0, 30.0, 30.0)),
            solid(0.0, 0.0, 1.0, 0.5),
        )],
    )
}

#[test]
fn a_node_nothing_clips_or_cuts_is_painted_straight_into_the_frame() {
    let stats = frame_stats(
        &DisplayList {
            nodes: vec![blue_square()],
        },
        &DamageRegion::Full,
    );
    assert_eq!(stats.nodes_painted, 1);
    assert_eq!(stats.scratch_images.count, 0);
    assert_eq!(stats.node_composite.count, 0);
}

/// The blue square's outline: an operation the span path leaves to the scratch image.
fn blue_outline() -> PaintNode {
    node(
        1,
        rect(9.0, 9.0, 32.0, 32.0),
        vec![PaintOp::Stroke {
            shape: Shape::Rect(rect(10.0, 10.0, 30.0, 30.0)),
            width: 1.0,
            paint: solid(0.0, 0.0, 1.0, 0.5),
        }],
    )
}

fn under_a_clip(square: PaintNode) -> DisplayList {
    DisplayList {
        nodes: vec![
            PaintNode {
                clip: Some(Shape::Rect(rect(0.0, 0.0, 20.0, 20.0))),
                ..node(9, rect(0.0, 0.0, 96.0, 64.0), vec![])
            },
            PaintNode {
                parent: Some(PaintNodeId(9)),
                ..square
            },
        ],
    }
}

#[test]
fn a_clipped_cut_or_grouped_node_keeps_its_scratch_image() {
    let stats = frame_stats(&under_a_clip(blue_outline()), &DamageRegion::Full);
    assert_eq!(stats.node_composite.count, 1, "under a clipping ancestor");
    let cut = DamageRegion::Rects(vec![rect(0.0, 0.0, 20.0, 20.0)]);
    let stats = frame_stats(
        &DisplayList {
            nodes: vec![blue_outline()],
        },
        &cut,
    );
    assert_eq!(stats.node_composite.count, 1, "cut by the damage");
    let stats = frame_stats(&faded_sheet(0.0), &DamageRegion::Full);
    assert_eq!(stats.group_composite.count, 1, "inside a group's layer");
}

#[test]
fn a_clipped_or_cut_rectangle_on_whole_pixels_is_written_as_rows() {
    let stats = frame_stats(&under_a_clip(blue_square()), &DamageRegion::Full);
    assert_eq!(
        (stats.spans.count, stats.node_composite.count),
        (1, 0),
        "under a clipping ancestor"
    );
    let cut = DamageRegion::Rects(vec![rect(0.0, 0.0, 20.0, 20.0)]);
    let stats = frame_stats(
        &DisplayList {
            nodes: vec![blue_square()],
        },
        &cut,
    );
    assert_eq!(
        (stats.spans.count, stats.node_composite.count),
        (1, 0),
        "cut by the damage"
    );
    // the damaged pixels reach a pixel past the damaged rectangle
    assert_eq!(
        stats.spans.bytes,
        11 * 11 * 4,
        "the square's part of the damage"
    );
}

#[test]
fn a_full_repaint_costs_in_proportion_to_its_area() {
    let (width, height) = (480.0, 320.0);
    let sheet = rect(20.0, 20.0, 440.0, 280.0);
    let background = rect(0.0, 0.0, width, height);
    let list = DisplayList {
        nodes: vec![
            node(
                1,
                background,
                vec![
                    fill(Shape::Rect(background), solid(1.0, 1.0, 1.0, 1.0)),
                    PaintOp::Glyphs {
                        origin: Point { x: 40.0, y: 310.0 },
                        run: GlyphRun {
                            font: FontId(1),
                            size: 8.0,
                            glyphs: (0..4)
                                .map(|i| PositionedGlyph {
                                    id: GlyphId(i),
                                    x: i as f32 * 6.0,
                                    y: 0.0,
                                    cluster: 0,
                                })
                                .collect(),
                        },
                        color: Color::new(0.0, 0.0, 0.0, 1.0),
                    },
                ],
            ),
            PaintNode {
                clip: Some(Shape::RoundedRect {
                    rect: sheet,
                    radius: 16.0,
                }),
                ..node(2, sheet, vec![])
            },
            child(
                3,
                2,
                sheet,
                vec![fill(Shape::Rect(sheet), solid(0.2, 0.4, 0.9, 0.8))],
            ),
        ],
    };
    let none = PaintResources::default();
    let at = |scale: f64| {
        let mut r = renderer();
        r.render(&RenderRequest {
            list: &list,
            resources: &none,
            scale,
            width: (width * scale) as u32,
            height: (height * scale) as u32,
            damage: &DamageRegion::Full,
        })
        .unwrap();
        let stats = *r.stats();
        assert_eq!(
            (stats.scratch_images.count, stats.node_composite.count),
            (2, 2),
            "the clipped node and the background's pixels around its glyphs take scratch images; the rest of the \
             background is written as rows"
        );
        // the glyphs' rectangle: four glyphs of 8 points, a pixel either side
        let sheet = (440.0 * scale + 4.0) * (280.0 * scale + 4.0) * 4.0;
        let glyphs = (30.0 * scale + 4.0) * (10.0 * scale + 4.0) * 4.0;
        assert!(
            stats.scratch_images.bytes as f64 <= sheet + glyphs,
            "the scratch images cover the clipped node and the glyphs, not the frame: {} bytes",
            stats.scratch_images.bytes
        );
        stats
    };
    let (small, large) = (at(1.0), at(2.0));
    assert_eq!(
        (small.nodes_painted, small.fills, small.glyphs),
        (large.nodes_painted, large.fills, large.glyphs)
    );
    assert_eq!(large.clear.bytes, 4 * small.clear.bytes);
    assert_eq!(small.clear.bytes, 480 * 320 * 4);
}

/// Glyph rasters ten em wide and half covered: wider than the box a node's extent allows a glyph.
struct Wide;

impl GlyphRasterizer for Wide {
    fn rasterize(&mut self, key: &GlyphKey) -> Result<Option<RasterGlyph>, TextError> {
        let em = key.size_q / 64;
        Ok(Some(RasterGlyph {
            format: GlyphFormat::Coverage,
            width: em * 10,
            height: em,
            stride: em * 10,
            bearing_x: 0,
            bearing_y: em as i32,
            pixels: Arc::from(vec![128u8; (em * em * 10) as usize]),
        }))
    }
}

#[test]
fn a_glyph_wider_than_its_box_is_cut_to_the_damage() {
    let run = GlyphRun {
        font: FontId(1),
        size: 8.0,
        glyphs: vec![PositionedGlyph {
            id: GlyphId(1),
            x: 0.0,
            y: 0.0,
            cluster: 0,
        }],
    };
    let list = DisplayList {
        nodes: vec![node(
            1,
            rect(10.0, 10.0, 20.0, 10.0),
            vec![PaintOp::Glyphs {
                origin: Point { x: 12.0, y: 18.0 },
                run,
                color: Color::new(0.0, 0.0, 0.0, 1.0),
            }],
        )],
    };
    let none = PaintResources::default();
    let mut r = CpuRenderer::new(config(), Wide);
    let mut frame = |damage: &DamageRegion| {
        let id = r
            .render(&RenderRequest {
                list: &list,
                resources: &none,
                scale: 1.0,
                width: 96,
                height: 64,
                damage,
            })
            .unwrap();
        r.frame(id).unwrap().pixels.clone()
    };
    let fresh = frame(&DamageRegion::Full);
    assert_ne!(
        fresh[(15 * 96 + 20) * 4 + 3],
        0,
        "the glyph is drawn in its box"
    );
    let repainted = frame(&DamageRegion::Rects(vec![rect(0.0, 0.0, 40.0, 40.0)]));
    assert!(
        repainted == fresh,
        "pixels outside the damage are not painted again"
    );
}

#[test]
fn an_image_drawn_one_pixel_per_device_pixel_keeps_its_pixels_at_a_fractional_position() {
    let side = 6u32;
    let pixels: Vec<u8> = (0..side * side)
        .flat_map(|i| {
            [
                (i * 7 % 256) as u8,
                (i * 13 % 256) as u8,
                (i * 29 % 256) as u8,
                255,
            ]
        })
        .collect();
    let mut resources = PaintResources::default();
    resources.images.insert(
        ResourceId(9),
        ImageResource {
            revision: 1,
            width: side,
            height: side,
            stride: side * 4,
            format: PixelFormat::Rgba8Premultiplied,
            pixels: Arc::from(pixels.clone()),
        },
    );
    // 4 points at 1.5x are the image's 6 device pixels; the origin (3.3, 2.2) points is (4.95, 3.3) device pixels
    let dst = rect(3.3, 2.2, 4.0, 4.0);
    let list = DisplayList {
        nodes: vec![node(
            1,
            dst,
            vec![PaintOp::Image {
                image: ResourceRef {
                    id: ResourceId(9),
                    revision: 1,
                },
                src: rect(0.0, 0.0, 6.0, 6.0),
                dst,
                sampling: cupertino_ui_render::Sampling::Linear,
            }],
        )],
    };
    let frame = render(&mut renderer(), &list, &resources, 1.5, &DamageRegion::Full).unwrap();
    for y in 0..side as usize {
        let at = ((3 + y) * 96 + 5) * 4;
        assert_eq!(
            &frame[at..at + side as usize * 4],
            &pixels[y * side as usize * 4..(y + 1) * side as usize * 4],
            "row {y} lands on device pixels 5.. of row {}",
            3 + y
        );
    }
}

/// Records the size of every glyph it is asked to rasterize.
struct Sizes(Arc<Mutex<Vec<u32>>>);

impl GlyphRasterizer for Sizes {
    fn rasterize(&mut self, key: &GlyphKey) -> Result<Option<RasterGlyph>, TextError> {
        self.0.lock().unwrap().push(key.size_q);
        Boxes { fail: false }.rasterize(key)
    }
}

#[test]
fn glyphs_are_rasterized_at_each_scales_device_size_and_kept_for_both() {
    let sizes = Arc::new(Mutex::new(Vec::new()));
    let mut r = CpuRenderer::new(config(), Sizes(Arc::clone(&sizes)));
    let list = DisplayList {
        nodes: vec![glyph_node(4)],
    };
    let none = PaintResources::default();
    let mut frame = |scale: f64| {
        r.render(&RenderRequest {
            list: &list,
            resources: &none,
            scale,
            width: 96,
            height: 64,
            damage: &DamageRegion::Full,
        })
        .unwrap();
        r.stats().glyph_rasterize.count
    };
    assert_eq!((frame(1.0), frame(1.0), frame(2.0)), (4, 0, 4));
    assert_eq!(
        (frame(1.0), frame(2.0)),
        (0, 0),
        "both scales' rasters are kept"
    );
    let sizes = sizes.lock().unwrap().clone();
    assert_eq!(sizes.len(), 8);
    assert!(sizes[..4].iter().all(|s| *s == sizes[0]));
    assert!(
        sizes[4..].iter().all(|s| *s == 2 * sizes[0]),
        "at 2x the glyphs are rasterized at twice the size: {sizes:?}"
    );
}

#[test]
fn an_image_is_decoded_once_and_drawn_at_each_scales_device_size() {
    let texels: [[u8; 4]; 4] = [
        [255, 0, 0, 255],
        [0, 255, 0, 255],
        [0, 0, 255, 255],
        [255, 255, 255, 255],
    ];
    let mut resources = PaintResources::default();
    resources.images.insert(
        ResourceId(5),
        ImageResource {
            revision: 1,
            width: 2,
            height: 2,
            stride: 8,
            format: PixelFormat::Rgba8Premultiplied,
            pixels: Arc::from(texels.concat()),
        },
    );
    let dst = rect(8.0, 8.0, 4.0, 4.0);
    let list = DisplayList {
        nodes: vec![node(
            1,
            dst,
            vec![PaintOp::Image {
                image: ResourceRef {
                    id: ResourceId(5),
                    revision: 1,
                },
                src: rect(0.0, 0.0, 2.0, 2.0),
                dst,
                sampling: cupertino_ui_render::Sampling::Nearest,
            }],
        )],
    };
    let mut r = renderer();
    let at = |frame: &[u8], x: usize, y: usize| -> [u8; 4] {
        frame[(y * 96 + x) * 4..(y * 96 + x) * 4 + 4]
            .try_into()
            .unwrap()
    };
    let one = render(&mut r, &list, &resources, 1.0, &DamageRegion::Full).unwrap();
    assert_eq!(r.stats().image_decode.count, 1);
    assert_eq!(
        (at(&one, 8, 8), at(&one, 11, 11), at(&one, 12, 12)),
        (texels[0], texels[3], [0; 4])
    );
    let two = render(&mut r, &list, &resources, 2.0, &DamageRegion::Full).unwrap();
    assert_eq!(
        r.stats().image_decode.count,
        0,
        "the decode is kept across scales"
    );
    assert_eq!(
        (
            at(&two, 16, 16),
            at(&two, 19, 16),
            at(&two, 20, 16),
            at(&two, 16, 20),
            at(&two, 23, 23),
            at(&two, 24, 24)
        ),
        (texels[0], texels[0], texels[1], texels[2], texels[3], [0; 4]),
        "the 4 point image covers 8 device pixels at 2x"
    );
}

#[test]
fn a_blur_reads_only_its_own_layer_never_what_is_drawn_under_it() {
    let under = node(
        1,
        rect(0.0, 0.0, 96.0, 64.0),
        vec![fill(
            Shape::Rect(rect(0.0, 0.0, 48.0, 64.0)),
            solid(1.0, 0.0, 0.0, 1.0),
        )],
    );
    let blurred = node(
        2,
        rect(0.0, 0.0, 96.0, 64.0),
        vec![
            PaintOp::PushLayer {
                opacity: 1.0,
                effects: vec![Effect::Blur { radius: 6.0 }],
            },
            // invisible, so the layer spans the red edge
            fill(
                Shape::Rect(rect(36.0, 0.0, 1.0, 1.0)),
                solid(0.0, 0.0, 0.0, 0.0),
            ),
            fill(
                Shape::Rect(rect(76.0, 24.0, 10.0, 10.0)),
                solid(0.0, 0.0, 1.0, 1.0),
            ),
            PaintOp::PopLayer,
        ],
    );
    let none = PaintResources::default();
    let alone = render(
        &mut renderer(),
        &DisplayList {
            nodes: vec![under.clone()],
        },
        &none,
        1.0,
        &DamageRegion::Full,
    )
    .unwrap();
    let both = render(
        &mut renderer(),
        &DisplayList {
            nodes: vec![under, blurred],
        },
        &none,
        1.0,
        &DamageRegion::Full,
    )
    .unwrap();
    // the blurred square's reach: three box passes of 3 px, 9 px around it
    let reach = rect(67.0, 15.0, 28.0, 28.0);
    for y in 0..64 {
        for x in 0..96 {
            let at = (y * 96 + x) * 4;
            if !reach.contains(Point::new(x as f64 + 0.5, y as f64 + 0.5)) {
                assert_eq!(
                    both[at..at + 4],
                    alone[at..at + 4],
                    "({x}, {y}): outside the blurred square nothing changes, the red edge stays sharp"
                );
            }
        }
    }
    let blue = (29 * 96 + 72) * 4;
    assert!(
        both[blue + 2] > 0 && both[blue + 2] < 255,
        "the layer's own square is blurred past its edge"
    );
}

/// The same logical scene at the desktop scales, drawn by one renderer going from scale to scale: our own criterion
/// for complete assets and aligned edges. Each frame equals a fresh renderer's at that scale (no glyph or image raster
/// of another scale is reused), an opaque rectangle whose edges are snapped to device pixels covers whole pixels with
/// nothing half covered beside it, and the text has ink at every scale, growing with the scale's square.
#[test]
fn the_same_scene_at_every_desktop_scale_is_complete_and_snapped_edges_stay_sharp() {
    let (width, height) = (96.0, 64.0);
    let snap = |v: f64, scale: f64| (v * scale).round() / scale;
    let frame_at = |r: &mut CpuRenderer<Boxes>, scale: f64| {
        let (x0, y0, x1, y1) = (
            snap(20.3, scale),
            snap(12.7, scale),
            snap(61.1, scale),
            snap(41.4, scale),
        );
        let list = DisplayList {
            nodes: vec![
                node(
                    1,
                    rect(0.0, 0.0, width, height),
                    vec![fill(
                        Shape::Rect(rect(0.0, 0.0, width, height)),
                        solid(1.0, 1.0, 1.0, 1.0),
                    )],
                ),
                node(
                    2,
                    rect(x0, y0, x1 - x0, y1 - y0),
                    vec![fill(
                        Shape::Rect(rect(x0, y0, x1 - x0, y1 - y0)),
                        solid(0.0, 0.0, 1.0, 1.0),
                    )],
                ),
                node(
                    3,
                    rect(0.0, 44.0, width, 20.0),
                    vec![PaintOp::Glyphs {
                        origin: Point { x: 8.0, y: 58.0 },
                        run: GlyphRun {
                            font: FontId(1),
                            size: 12.0,
                            glyphs: (0..4)
                                .map(|i| PositionedGlyph {
                                    id: GlyphId(i),
                                    x: i as f32 * 9.0,
                                    y: 0.0,
                                    cluster: i as usize,
                                })
                                .collect(),
                        },
                        color: Color::new(0.0, 0.0, 0.0, 1.0),
                    }],
                ),
            ],
        };
        let (w, h) = (
            (width * scale).round() as u32,
            (height * scale).round() as u32,
        );
        let id = r
            .render(&RenderRequest {
                list: &list,
                resources: &PaintResources::default(),
                scale,
                width: w,
                height: h,
                damage: &DamageRegion::Full,
            })
            .unwrap();
        let pixels = r.frame(id).unwrap().pixels.clone();
        (
            pixels,
            (w, h),
            (x0 * scale, y0 * scale, x1 * scale, y1 * scale),
        )
    };
    let mut travelling = renderer();
    let mut ink_at_one = 0;
    for scale in [1.0, 1.25, 1.5, 1.75, 2.0, 1.25, 1.0] {
        let (pixels, (w, h), (x0, y0, x1, y1)) = frame_at(&mut travelling, scale);
        let (fresh, ..) = frame_at(&mut renderer(), scale);
        assert!(
            pixels == fresh,
            "{scale}: the frame equals a fresh renderer's"
        );
        assert_eq!(pixels.len(), (w * h * 4) as usize);
        let px = |x: u32, y: u32| {
            let i = ((y * w + x) * 4) as usize;
            [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
        };
        let (x0, y0, x1, y1) = (
            x0.round() as u32,
            y0.round() as u32,
            x1.round() as u32,
            y1.round() as u32,
        );
        let mid = (y0 + y1) / 2;
        for x in x0..x1 {
            assert_eq!(px(x, mid), [0, 0, 255, 255], "{scale}: ({x}, {mid}) inside");
        }
        assert_eq!(
            px(x0 - 1, mid),
            [255, 255, 255, 255],
            "{scale}: left of the edge"
        );
        assert_eq!(
            px(x1, mid),
            [255, 255, 255, 255],
            "{scale}: right of the edge"
        );
        let centre = (x0 + x1) / 2;
        assert_eq!(
            px(centre, y0 - 1),
            [255, 255, 255, 255],
            "{scale}: above the edge"
        );
        assert_eq!(px(centre, y0), [0, 0, 255, 255], "{scale}: the top row");
        // the glyph boxes lie below the rectangle, on the white ground
        let ink = (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .filter(|&(x, y)| (y as f64) >= y1 as f64 && px(x, y)[0] < 250)
            .count();
        if scale == 1.0 {
            ink_at_one = ink_at_one.max(ink);
        }
        assert!(ink > 0, "{scale}: the text has ink");
        if ink_at_one > 0 {
            let expected = ink_at_one as f64 * scale * scale;
            assert!(
                (ink as f64) > expected * 0.5 && (ink as f64) < expected * 2.0,
                "{scale}: {ink} ink pixels, about {expected} expected"
            );
        }
    }
}
