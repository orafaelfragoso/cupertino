//! What a window system relies on of the CPU renderer: a frame that repaints damage, a frame drawn into lent memory
//! and a frame whose layers were kept have the bytes of the same list drawn whole, and a request that cannot be
//! drawn is refused.

use crate::support;

use cupertino_geometry::{Point, Rect};
use cupertino_ui_render::{
    check_request, damage_rects, scroll_layers, Color, CpuConfig, CpuRenderer, DamageRegion,
    DisplayList, Group, Layer, MemoryTarget, Paint, PaintNode, PaintNodeId, PaintOp,
    PaintResources, PixelRect, RenderError, RenderRequest, ResourceId, ResourceRef, Sampling,
    Shape, Transform, UiRenderer,
};
use support::{node, overlay, pixels, scenes, Patterns, Scene, WINDOW};

/// The scales the CPU renderer promises the same bytes at.
const SCALES: [f64; 4] = [1.0, 1.25, 1.5, 2.0];

fn renderer(max_pixels: u64) -> CpuRenderer<Patterns> {
    CpuRenderer::new(
        CpuConfig {
            max_pixels,
            glyph_cache_bytes: 1 << 20,
            image_cache_bytes: 1 << 22,
            font_smoothing: true,
        },
        Patterns,
    )
}

fn request<'a>(
    list: &'a DisplayList,
    resources: &'a PaintResources,
    scale: f64,
    damage: &'a DamageRegion,
) -> RenderRequest<'a> {
    let (width, height) = pixels(scale);
    RenderRequest {
        list,
        resources,
        scale,
        width,
        height,
        damage,
    }
}

/// What a new renderer draws for the whole of `list`.
fn whole(list: &DisplayList, resources: &PaintResources, scale: f64) -> Vec<u8> {
    let mut fresh = renderer(1 << 24);
    let id = fresh
        .render(&request(list, resources, scale, &DamageRegion::Full))
        .unwrap();
    fresh.frame(id).unwrap().pixels.clone()
}

fn list_of(scene: &Scene) -> DisplayList {
    DisplayList {
        nodes: scene.nodes.clone(),
    }
}

/// The scene with a translucent node laid over it, and the rectangle that changed.
fn with_overlay(scene: &Scene) -> (DisplayList, Rect) {
    let (top, area) = overlay();
    let mut list = list_of(scene);
    list.nodes.push(top);
    (list, area)
}

#[test]
fn a_frame_repainting_only_its_damage_has_the_bytes_of_the_list_drawn_whole() {
    for scene in scenes() {
        for scale in SCALES {
            let (after, area) = with_overlay(&scene);
            let mut kept = renderer(1 << 24);
            kept.render(&request(
                &list_of(&scene),
                &scene.resources,
                scale,
                &DamageRegion::Full,
            ))
            .unwrap();
            let damage = DamageRegion::Rects(vec![area]);
            let id = kept
                .render(&request(&after, &scene.resources, scale, &damage))
                .unwrap();
            let frame = kept.frame(id).unwrap();
            assert!(
                frame.pixels == whole(&after, &scene.resources, scale),
                "{} at {scale}",
                scene.name
            );
            assert!(
                frame.pixels != whole(&list_of(&scene), &scene.resources, scale),
                "{} at {scale}: the change shows",
                scene.name
            );
            assert_eq!(
                frame.damage.len(),
                1,
                "{} at {scale}: the frame reports the rectangle it repainted",
                scene.name
            );
        }
    }
}

#[test]
fn a_frame_drawn_into_lent_memory_leaves_the_bytes_a_frame_of_its_own_image_does() {
    for scene in scenes() {
        for scale in SCALES {
            let (after, area) = with_overlay(&scene);
            let expected = whole(&after, &scene.resources, scale);
            let stride = pixels(scale).0 as usize * 4;

            // memory of unknown pixels is drawn whole
            let mut memory = vec![0x5Au8; expected.len()];
            let mut lent = renderer(1 << 24);
            assert!(lent.draws_into_memory());
            let (id, _) = lent
                .render_into(
                    &request(&after, &scene.resources, scale, &DamageRegion::Full),
                    MemoryTarget {
                        pixels: &mut memory,
                        stride,
                        missed: None,
                    },
                )
                .unwrap();
            assert!(
                memory == expected,
                "{} at {scale}: unknown memory",
                scene.name
            );
            assert!(
                lent.frame(id).is_err(),
                "the renderer's own image does not hold a frame drawn into memory"
            );

            // memory that holds the frame before, except a rectangle it missed, takes the damage and what it missed
            let mut held = renderer(1 << 24);
            held.render(&request(
                &list_of(&scene),
                &scene.resources,
                scale,
                &DamageRegion::Full,
            ))
            .unwrap();
            let mut memory = whole(&list_of(&scene), &scene.resources, scale);
            let missed = PixelRect {
                x: 2,
                y: 3,
                width: 9,
                height: 6,
            };
            for row in 3..9 {
                memory[row * stride + 2 * 4..row * stride + 11 * 4].fill(0x77);
            }
            let damage = DamageRegion::Rects(vec![area]);
            let (_, changed) = held
                .render_into(
                    &request(&after, &scene.resources, scale, &damage),
                    MemoryTarget {
                        pixels: &mut memory,
                        stride,
                        missed: Some(&[missed]),
                    },
                )
                .unwrap();
            assert!(memory == expected, "{} at {scale}: held memory", scene.name);
            assert!(!changed.is_empty(), "{} at {scale}", scene.name);
        }
    }
}

/// A column of rows in a scroll container, shown moved up by `offset` points.
fn scrolled_by(offset: f64) -> DisplayList {
    let viewport = Rect::new(8.0, 5.0, 40.0, 30.0);
    let mut nodes = vec![
        node(
            1,
            None,
            Rect::new(0.0, 0.0, WINDOW.0, WINDOW.1),
            vec![PaintOp::Fill {
                shape: Shape::Rect(Rect::new(0.0, 0.0, WINDOW.0, WINDOW.1)),
                paint: Paint::Solid(Color::from_rgba8(support::GROUND)),
            }],
        ),
        PaintNode {
            clip: Some(Shape::Rect(viewport)),
            layer: Some(Layer::Scroll {
                offset: Point::new(0.0, offset),
            }),
            ..node(2, None, viewport, vec![])
        },
    ];
    for i in 0..12u8 {
        let top = 5.0 + f64::from(i) * 14.0 - offset;
        let shade = 40 + i * 17;
        nodes.push(PaintNode {
            transform: Transform([1.0, 0.0, 0.0, 1.0, 0.0, top]),
            ..node(
                10 + u64::from(i),
                Some(2),
                Rect::new(8.0, top, 40.0, 12.0),
                vec![
                    PaintOp::Fill {
                        shape: Shape::RoundedRect {
                            rect: Rect::new(8.0, 0.0, 40.0, 12.0),
                            radius: 4.5,
                        },
                        paint: Paint::Solid(Color::from_rgba8([shade, 120, 255 - shade, 255])),
                    },
                    PaintOp::Stroke {
                        shape: Shape::Rect(Rect::new(10.5, 1.5, 20.0, 9.0)),
                        width: 1.5,
                        paint: Paint::Solid(Color::from_rgba8([255, 255, 255, 200])),
                    },
                ],
            )
        });
    }
    DisplayList { nodes }
}

#[test]
fn a_scroll_layer_scrolled_frame_by_frame_has_the_bytes_of_each_frame_drawn_whole() {
    let resources = PaintResources::default();
    // the scroll alone damages nothing: the content that moved is the layer's
    let no_damage = DamageRegion::Rects(vec![]);
    for scale in [1.0, 1.5, 2.0] {
        let mut kept = renderer(1 << 24);
        let mut shown = Vec::new();
        // down a little, down again, past the viewport, back up
        for offset in [0.0, 6.0, 12.0, 70.0, 64.0, 10.0] {
            let list = scrolled_by(offset);
            let id = kept
                .render(&request(&list, &resources, scale, &no_damage))
                .unwrap();
            let frame = kept.frame(id).unwrap();
            assert!(
                frame.pixels == whole(&list, &resources, scale),
                "scrolled to {offset} at {scale}"
            );
            assert!(
                frame.pixels != shown,
                "scrolled to {offset}: the content moved"
            );
            shown = frame.pixels.clone();
            if offset == 12.0 {
                // a scroll by a few rows paints the rows that came into view, not the viewport
                let viewport_pixels = (40.0 * 30.0 * scale * scale) as u64;
                let painted = kept.stats().layer_pixels;
                assert!(
                    painted > 0 && painted < viewport_pixels / 2,
                    "{painted} pixels painted of {viewport_pixels}"
                );
            }
        }
    }
}

#[test]
fn a_request_that_cannot_be_drawn_is_refused_and_the_renderer_goes_on() {
    let scenes = scenes();
    let by_name = |name| scenes.iter().find(|s| s.name == name).unwrap();
    let (fills, images) = (by_name("fills"), by_name("images"));
    let id = PaintNodeId;
    let square = Rect::new(0.0, 0.0, 4.0, 4.0);
    let picture = |at, revision| PaintOp::Image {
        image: ResourceRef {
            id: ResourceId(at),
            revision,
        },
        src: square,
        dst: square,
        sampling: Sampling::Linear,
    };
    let mut short = images.resources.clone();
    let image = short.images.get_mut(&ResourceId(7)).unwrap();
    image.pixels = image.pixels[..40].into();
    let deep = DisplayList {
        nodes: (1..=300)
            .map(|i| node(i, i.checked_sub(2).map(|p| p + 1), square, vec![]))
            .collect(),
    };

    // the second node of each scene is the one a row changes
    type Change = fn(&mut PaintNode);
    let nodes: Vec<(&str, &Scene, Change, RenderError)> = vec![
        (
            "a parent that is no earlier node",
            fills,
            |n| n.parent = Some(PaintNodeId(77)),
            RenderError::UnknownParent(id(2)),
        ),
        (
            "two nodes with one id",
            fills,
            |n| n.id = PaintNodeId(1),
            RenderError::DuplicateNode(id(1)),
        ),
        (
            "a clip never popped",
            fills,
            |n| {
                n.ops.push(PaintOp::PushClip(Shape::Rect(Rect::new(
                    0.0, 0.0, 4.0, 4.0,
                ))))
            },
            RenderError::UnbalancedScope(id(2)),
        ),
        (
            "a pop with nothing pushed",
            fills,
            |n| n.ops.push(PaintOp::PopLayer),
            RenderError::UnbalancedScope(id(2)),
        ),
        (
            "a rectangle that is not a number",
            fills,
            |n| n.bounds = Rect::new(f64::NAN, 0.0, 4.0, 4.0),
            RenderError::NonFiniteValue(id(2)),
        ),
        (
            "an opacity above one",
            fills,
            |n| {
                n.group = Some(Group {
                    opacity: 1.5,
                    effects: vec![],
                })
            },
            RenderError::NonFiniteValue(id(2)),
        ),
    ];
    let mut asks: Vec<(&str, DisplayList, &PaintResources, f64, u32, RenderError)> = nodes
        .into_iter()
        .map(|(what, scene, change, error)| {
            let mut list = list_of(scene);
            change(&mut list.nodes[1]);
            (what, list, &scene.resources, 1.0, 57, error)
        })
        .collect();
    let plain = list_of(fills);
    let none = &fills.resources;
    let mut with_picture = list_of(images);
    with_picture.nodes[1].ops.push(picture(7, 2));
    let mut missing = list_of(fills);
    missing.nodes[1].ops.push(picture(9, 1));
    asks.extend([
        (
            "an image the resources lack",
            missing,
            none,
            1.0,
            57,
            RenderError::MissingResource(ResourceId(9)),
        ),
        (
            "an image of another revision",
            with_picture,
            &images.resources,
            1.0,
            57,
            RenderError::StaleResource(ResourceId(7)),
        ),
        (
            "an image shorter than it says",
            list_of(images),
            &short,
            1.0,
            57,
            RenderError::InvalidImage(ResourceId(7)),
        ),
        (
            "a tree 300 deep",
            deep,
            none,
            1.0,
            57,
            RenderError::TooDeep(id(258)),
        ),
        (
            "a scale of zero",
            plain.clone(),
            none,
            0.0,
            57,
            RenderError::InvalidScale,
        ),
        (
            "a scale that is not a number",
            plain.clone(),
            none,
            f64::NAN,
            57,
            RenderError::InvalidScale,
        ),
        (
            "a scale of a thousand",
            plain.clone(),
            none,
            1000.0,
            57,
            RenderError::InvalidScale,
        ),
        (
            "no pixels",
            plain.clone(),
            none,
            1.0,
            0,
            RenderError::InvalidSize,
        ),
    ]);
    let mut one = renderer(1 << 24);
    let full = DamageRegion::Full;
    for (what, list, resources, scale, width, error) in &asks {
        let asked = RenderRequest {
            width: *width,
            ..request(list, resources, *scale, &full)
        };
        assert_eq!(check_request(&asked), Err(error.clone()), "{what}");
        assert_eq!(one.render(&asked), Err(error.clone()), "{what}");
    }
    assert_eq!(
        renderer(1000).render(&request(&plain, none, 1.0, &full)),
        Err(RenderError::ResourceLimit),
        "a target of more pixels than the renderer allows"
    );

    // none of them left the renderer unable to draw
    let id = one.render(&request(&plain, none, 1.5, &full)).unwrap();
    assert!(one.frame(id).unwrap().pixels == whole(&plain, none, 1.5));
}

#[test]
fn damage_is_grown_to_whole_pixels_cut_to_the_target_merged_and_sorted() {
    let px = |x, y, width, height| PixelRect {
        x,
        y,
        width,
        height,
    };
    let r = Rect::new;
    let rows: Vec<(&str, Vec<Rect>, f64, Vec<PixelRect>)> = vec![
        (
            "a pixel of margin on every side",
            vec![r(10.0, 10.0, 5.0, 5.0)],
            1.0,
            vec![px(9, 9, 7, 7)],
        ),
        (
            "whole pixels at a fractional scale",
            vec![r(10.2, 10.2, 5.0, 5.0)],
            1.5,
            vec![px(14, 14, 10, 10)],
        ),
        (
            "cut at the target's corner",
            vec![r(0.0, 0.0, 3.0, 3.0)],
            1.0,
            vec![px(0, 0, 4, 4)],
        ),
        (
            "cut at the far edge",
            vec![r(95.0, 95.0, 20.0, 20.0)],
            1.0,
            vec![px(94, 94, 6, 6)],
        ),
        (
            "a rectangle outside",
            vec![r(150.0, 10.0, 5.0, 5.0)],
            1.0,
            vec![],
        ),
        (
            "rectangles that touch merge",
            vec![r(10.0, 10.0, 5.0, 5.0), r(15.0, 10.0, 5.0, 5.0)],
            1.0,
            vec![px(9, 9, 12, 7)],
        ),
        (
            "apart they stay, top to bottom",
            vec![r(50.0, 60.0, 4.0, 4.0), r(10.0, 10.0, 4.0, 4.0)],
            1.0,
            vec![px(9, 9, 6, 6), px(49, 59, 6, 6)],
        ),
        ("no damage", vec![], 1.0, vec![]),
    ];
    for (what, rects, scale, expected) in rows {
        assert_eq!(damage_rects(&rects, scale, 100, 100), expected, "{what}");
    }
}

#[test]
fn only_a_clipped_scroll_node_that_just_moves_keeps_its_descendants_in_a_layer() {
    let bounds = Rect::new(0.0, 0.0, 10.0, 10.0);
    let scroll = |id, parent: Option<u64>| PaintNode {
        clip: Some(Shape::Rect(bounds)),
        layer: Some(Layer::Scroll {
            offset: Point::new(0.0, 4.0),
        }),
        ..node(id, parent, bounds, vec![])
    };
    let plain = |id, parent: u64| node(id, Some(parent), bounds, vec![]);
    let faded = |node: PaintNode| PaintNode {
        group: Some(Group {
            opacity: 0.5,
            effects: vec![],
        }),
        ..node
    };
    type Case = (&'static str, Vec<PaintNode>, Vec<Option<usize>>);
    let rows: Vec<Case> = vec![
        (
            "a scroll node and its child",
            vec![scroll(1, None), plain(2, 1)],
            vec![None, Some(0)],
        ),
        (
            "a grandchild",
            vec![scroll(1, None), plain(2, 1), plain(3, 2)],
            vec![None, Some(0), Some(0)],
        ),
        (
            "a node that is no scroll node",
            vec![plain(1, 0), plain(2, 1)]
                .into_iter()
                .map(|mut n| {
                    n.parent = None;
                    n
                })
                .collect(),
            vec![None, None],
        ),
        (
            "a scroll node without a clip",
            vec![
                PaintNode {
                    clip: None,
                    ..scroll(1, None)
                },
                plain(2, 1),
            ],
            vec![None, None],
        ),
        (
            "a transform that scales",
            vec![
                PaintNode {
                    transform: Transform([2.0, 0.0, 0.0, 2.0, 0.0, 0.0]),
                    ..scroll(1, None)
                },
                plain(2, 1),
            ],
            vec![None, None],
        ),
        (
            "a group on the node",
            vec![faded(scroll(1, None)), plain(2, 1)],
            vec![None, None],
        ),
        (
            "a scroll node in a kept layer",
            vec![scroll(1, None), scroll(2, Some(1)), plain(3, 2)],
            vec![None, Some(0), Some(0)],
        ),
        (
            "a scroll node under a group",
            vec![
                faded(node(1, None, bounds, vec![])),
                scroll(2, Some(1)),
                plain(3, 2),
            ],
            vec![None, None, None],
        ),
    ];
    for (what, nodes, expected) in rows {
        assert_eq!(scroll_layers(&DisplayList { nodes }), expected, "{what}");
    }
}
