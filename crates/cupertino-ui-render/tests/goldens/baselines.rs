use cupertino_geometry::{Point, Rect};
use cupertino_theme::Color;
use cupertino_ui_render::{
    CpuConfig, CpuRenderer, DamageRegion, DisplayList, Effect, GradientStop, Group, Paint,
    PaintNode, PaintNodeId, PaintOp, PaintResources, PaintRevision, RenderRequest, Shape,
    Transform, UiRenderer,
};
use cupertino_ui_text::{
    AttributedText, FontIndex, FontStyle, FontWeight, GlyphRaster, ParagraphStyle, TextStyle,
    TextSystem,
};
use std::path::PathBuf;
use std::sync::Arc;

const TOLERANCE: u8 = 2;
const WIDTH: f64 = 160.0;
const HEIGHT: f64 = 100.0;

const INTER: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../cupertino-assets/fonts/InterVariable-subset.ttf"
));

fn text() -> Arc<TextSystem> {
    let mut fonts = FontIndex::new();
    fonts
        .add_face(
            "Inter",
            FontWeight::REGULAR,
            FontStyle::Normal,
            Arc::new(INTER.to_vec()),
            0,
        )
        .unwrap();
    Arc::new(TextSystem::new(fonts, "Inter"))
}

fn renderer(system: &Arc<TextSystem>) -> CpuRenderer<GlyphRaster> {
    CpuRenderer::new(
        CpuConfig {
            max_pixels: 1 << 22,
            glyph_cache_bytes: 1 << 22,
            image_cache_bytes: 1 << 20,
            font_smoothing: true,
        },
        GlyphRaster::new(system.clone()),
    )
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rect::new(x, y, w, h)
}

fn solid(r: f32, g: f32, b: f32, a: f32) -> Paint {
    Paint::Solid(Color::new(r, g, b, a))
}

fn node(id: u64, bounds: Rect, ops: Vec<PaintOp>) -> PaintNode {
    PaintNode {
        layer: None,
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

fn child(id: u64, parent: u64, bounds: Rect, ops: Vec<PaintOp>) -> PaintNode {
    PaintNode {
        layer: None,
        parent: Some(PaintNodeId(parent)),
        ..node(id, bounds, ops)
    }
}

fn fill(shape: Shape, paint: Paint) -> PaintOp {
    PaintOp::Fill { shape, paint }
}

fn rounded(r: Rect, radius: f64) -> Shape {
    Shape::RoundedRect { rect: r, radius }
}

fn backdrop(color: Paint) -> PaintNode {
    let all = rect(0.0, 0.0, WIDTH, HEIGHT);
    node(1, all, vec![fill(Shape::Rect(all), color)])
}

/// `label` at `at` (the top-left of its layout) in 15 pt Inter.
fn label(system: &TextSystem, label: &str, at: Point, color: Color) -> Vec<PaintOp> {
    let paragraph = system
        .paragraph(
            &AttributedText::plain(label, TextStyle::new(15.0)),
            &ParagraphStyle::default(),
        )
        .unwrap();
    let layout = paragraph.layout(None);
    layout
        .lines()
        .iter()
        .flat_map(|line| &line.runs)
        .map(|run| PaintOp::Glyphs {
            origin: Point {
                x: at.x + run.origin.x,
                y: at.y + run.origin.y,
            },
            run: run.run.clone(),
            color,
        })
        .collect()
}

/// Rounded fills, a hairline stroke, a linear gradient and a shadowed card.
fn chrome() -> DisplayList {
    let card = rect(96.0, 20.0, 48.0, 56.0);
    DisplayList {
        nodes: vec![
            backdrop(solid(0.93, 0.93, 0.94, 1.0)),
            node(
                2,
                rect(12.0, 14.0, 70.0, 26.0),
                vec![
                    fill(
                        rounded(rect(12.0, 14.0, 70.0, 26.0), 7.0),
                        solid(0.2, 0.45, 0.95, 1.0),
                    ),
                    PaintOp::Stroke {
                        shape: rounded(rect(12.5, 14.5, 69.0, 25.0), 6.5),
                        width: 1.0,
                        paint: solid(0.0, 0.0, 0.0, 0.25),
                    },
                ],
            ),
            node(
                3,
                rect(12.0, 54.0, 70.0, 30.0),
                vec![fill(
                    rounded(rect(12.0, 54.0, 70.0, 30.0), 5.0),
                    Paint::Linear {
                        start: Point { x: 12.0, y: 54.0 },
                        end: Point { x: 82.0, y: 84.0 },
                        stops: vec![
                            GradientStop {
                                offset: 0.0,
                                color: Color::new(0.95, 0.4, 0.3, 1.0),
                            },
                            GradientStop {
                                offset: 1.0,
                                color: Color::new(0.3, 0.2, 0.8, 1.0),
                            },
                        ],
                    },
                )],
            ),
            PaintNode {
                layer: None,
                group: Some(Group {
                    opacity: 1.0,
                    effects: vec![Effect::DropShadow {
                        offset: Point { x: 0.0, y: 3.0 },
                        blur: 6.0,
                        color: Color::new(0.0, 0.0, 0.0, 0.35),
                    }],
                }),
                // the bounds hold the shadow: its offset and three times its blur past the card
                ..node(
                    4,
                    rect(
                        card.x() - 18.0,
                        card.y() - 18.0,
                        card.width() + 36.0,
                        card.height() + 39.0,
                    ),
                    vec![fill(rounded(card, 10.0), solid(1.0, 1.0, 1.0, 1.0))],
                )
            },
        ],
    }
}

/// Light text on a dark half and dark text on a light half, through the real rasterizer with smoothing on.
fn text_scene(system: &TextSystem) -> DisplayList {
    let (dark, light) = (rect(0.0, 0.0, WIDTH, 50.0), rect(0.0, 50.0, WIDTH, 50.0));
    let mut on_dark = vec![fill(Shape::Rect(dark), solid(0.11, 0.11, 0.12, 1.0))];
    on_dark.extend(label(
        system,
        "Quartz 0123 fig",
        Point { x: 10.0, y: 14.0 },
        Color::new(0.95, 0.95, 0.96, 1.0),
    ));
    let mut on_light = vec![fill(Shape::Rect(light), solid(0.98, 0.98, 0.98, 1.0))];
    on_light.extend(label(
        system,
        "Quartz 0123 fig",
        Point { x: 10.0, y: 64.0 },
        Color::new(0.1, 0.1, 0.1, 1.0),
    ));
    DisplayList {
        nodes: vec![node(1, dark, on_dark), node(2, light, on_light)],
    }
}

/// Two overlapping squares in a group at half opacity: their overlap is no darker than either.
fn faded(shift: f64) -> DisplayList {
    let all = rect(0.0, 0.0, WIDTH, HEIGHT);
    let square = |x: f64| {
        fill(
            Shape::Rect(rect(x + shift, 25.0, 50.0, 50.0)),
            solid(0.85, 0.1, 0.1, 1.0),
        )
    };
    DisplayList {
        nodes: vec![
            backdrop(solid(1.0, 1.0, 1.0, 1.0)),
            PaintNode {
                layer: None,
                group: Some(Group {
                    opacity: 0.5,
                    effects: vec![],
                }),
                ..node(2, all, vec![])
            },
            child(3, 2, all, vec![square(30.0)]),
            child(4, 2, all, vec![square(60.0)]),
        ],
    }
}

/// A rounded clip over a gradient child and a label.
fn clipped(system: &TextSystem) -> DisplayList {
    let sheet = rect(16.0, 12.0, 128.0, 76.0);
    let mut inside = vec![fill(
        Shape::Rect(rect(0.0, 0.0, WIDTH, HEIGHT)),
        Paint::Linear {
            start: Point { x: 0.0, y: 0.0 },
            end: Point { x: 0.0, y: HEIGHT },
            stops: vec![
                GradientStop {
                    offset: 0.0,
                    color: Color::new(0.2, 0.6, 0.4, 1.0),
                },
                GradientStop {
                    offset: 1.0,
                    color: Color::new(0.1, 0.2, 0.5, 1.0),
                },
            ],
        },
    )];
    inside.extend(label(
        system,
        "clipped label",
        Point { x: 4.0, y: 40.0 },
        Color::new(1.0, 1.0, 1.0, 1.0),
    ));
    DisplayList {
        nodes: vec![
            backdrop(solid(0.9, 0.9, 0.9, 1.0)),
            PaintNode {
                layer: None,
                clip: Some(rounded(sheet, 18.0)),
                ..node(2, sheet, vec![])
            },
            child(3, 2, rect(0.0, 0.0, WIDTH, HEIGHT), inside),
        ],
    }
}

/// Renders `list` at `scale`: whole, or whole then again with only `damage` repainted.
fn render(
    r: &mut CpuRenderer<GlyphRaster>,
    list: &DisplayList,
    scale: f64,
    damage: Option<Rect>,
) -> (u32, u32, Vec<u8>) {
    let none = PaintResources::default();
    let (width, height) = (
        (WIDTH * scale).round() as u32,
        (HEIGHT * scale).round() as u32,
    );
    let mut frame = |damage: &DamageRegion| {
        let id = r
            .render(&RenderRequest {
                list,
                resources: &none,
                scale,
                width,
                height,
                damage,
            })
            .unwrap();
        r.frame(id).unwrap().pixels.clone()
    };
    let mut pixels = frame(&DamageRegion::Full);
    if let Some(damage) = damage {
        pixels = frame(&DamageRegion::Rects(vec![damage]));
    }
    assert!(
        pixels.as_chunks::<4>().0.iter().all(|p| p[3] == 255),
        "a scene lies on an opaque backdrop"
    );
    (width, height, pixels)
}

fn path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/goldens/{name}.png"))
}

/// How far `actual` is from the baseline `name`: `Err` for a size mismatch or a missing baseline, otherwise the
/// number of pixels beyond the tolerance and the largest channel difference.
fn compare(
    name: &str,
    (width, height, actual): &(u32, u32, Vec<u8>),
) -> Result<(usize, u8), String> {
    let expected = image::open(path(name))
        .map_err(|e| format!("{name}: {e}"))?
        .into_rgba8();
    if expected.dimensions() != (*width, *height) {
        return Err(format!(
            "{name}: {:?} against {width}x{height}",
            expected.dimensions()
        ));
    }
    let mut beyond = 0;
    let mut largest = 0;
    for (a, b) in actual
        .as_chunks::<4>()
        .0
        .iter()
        .zip(expected.as_raw().as_chunks::<4>().0)
    {
        let delta = a.iter().zip(b).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
        largest = largest.max(delta);
        if delta > TOLERANCE {
            beyond += 1;
        }
    }
    Ok((beyond, largest))
}

fn record(name: &str, (width, height, pixels): &(u32, u32, Vec<u8>)) {
    std::fs::create_dir_all(path(name).parent().unwrap()).unwrap();
    image::save_buffer(
        path(name),
        pixels,
        *width,
        *height,
        image::ExtendedColorType::Rgba8,
    )
    .unwrap();
}

fn scenes(system: &TextSystem) -> Vec<(String, DisplayList, f64, Option<Rect>)> {
    let mut all = Vec::new();
    for scale in [1.0, 2.0] {
        all.push((format!("chrome@{scale}x"), chrome(), scale, None));
        all.push((format!("group-opacity@{scale}x"), faded(0.0), scale, None));
    }
    for scale in [1.0, 1.5, 2.0] {
        all.push((format!("text@{scale}x"), text_scene(system), scale, None));
        all.push((format!("clip@{scale}x"), clipped(system), scale, None));
        // an incremental frame repainting the left half matches the same baseline
        all.push((
            format!("clip@{scale}x"),
            clipped(system),
            scale,
            Some(rect(0.0, 0.0, WIDTH / 2.0, HEIGHT)),
        ));
    }
    all
}

#[test]
fn every_scene_matches_its_baseline() {
    let system = text();
    let update = std::env::var("CUPERTINO_UPDATE_GOLDENS").as_deref() == Ok("1");
    let mut recorded = 0;
    let mut failures = Vec::new();
    for (name, list, scale, damage) in scenes(&system) {
        let frame = render(&mut renderer(&system), &list, scale, damage);
        if update {
            if damage.is_none() {
                record(&name, &frame);
                recorded += 1;
            }
            continue;
        }
        let incremental = if damage.is_some() {
            " (incremental)"
        } else {
            ""
        };
        match compare(&name, &frame) {
            Ok((0, _)) => {}
            Ok((beyond, largest)) => failures.push(format!(
                "{name}{incremental}: {beyond} pixels beyond {TOLERANCE}/255, largest difference {largest}"
            )),
            Err(e) => failures.push(e),
        }
    }
    assert!(
        !update,
        "re-recorded {recorded} goldens; rerun without CUPERTINO_UPDATE_GOLDENS"
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_one_point_shift_is_beyond_the_tolerance() {
    let system = text();
    let shifted = render(&mut renderer(&system), &faded(1.0), 1.0, None);
    let (beyond, _) = compare("group-opacity@1x", &shifted).unwrap();
    assert!(beyond > 0, "moving the squares 1 pt passes the comparison");
}
