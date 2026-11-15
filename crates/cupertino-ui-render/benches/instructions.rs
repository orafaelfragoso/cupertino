//! Instruction counts of the CPU renderer drawing a window at 2x: a window of 640 x 400 points (1280 x 800 pixels) with
//! a toolbar and its title, a sidebar of 12 labels, a list of 20 rows of text, a blurred banner and a card with a drop
//! shadow. Its first frame (every glyph rasterized, every scratch image new), the same frame painted whole again with
//! the caches warm, and one row's damage repainted.
//!
//! `scripts/dev/microbench cupertino-ui-render`

use cupertino_geometry::{Point, Rect};
use cupertino_theme::Color;
use cupertino_ui_render::{
    CpuConfig, CpuRenderer, DamageRegion, DisplayList, Effect, GradientStop, Group, Paint,
    PaintNode, PaintNodeId, PaintOp, PaintResources, PaintRevision, RenderFrameId, RenderRequest,
    Shape, Transform, UiRenderer,
};
use cupertino_ui_text::{
    AttributedText, FontIndex, FontStyle, FontWeight, GlyphRaster, ParagraphStyle, TextStyle,
    TextSystem,
};
use gungraun::prelude::*;
use std::hint::black_box;
use std::sync::Arc;

const FONT: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../cupertino-assets/fonts/InterVariable-subset.ttf"
));
/// The window in points and the scale it is drawn at.
const WIDTH: f64 = 640.0;
const HEIGHT: f64 = 400.0;
const SCALE: f64 = 2.0;
const ROWS: usize = 20;
const ROW_HEIGHT: f64 = 22.0;
const LIST_TOP: f64 = 96.0;

fn text_system() -> Arc<TextSystem> {
    let mut fonts = FontIndex::new();
    fonts
        .add_face(
            "Inter",
            FontWeight::REGULAR,
            FontStyle::Normal,
            Arc::new(FONT.to_vec()),
            0,
        )
        .unwrap();
    Arc::new(TextSystem::new(fonts, "Inter"))
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rect::new(x, y, w, h)
}

fn solid(r: f32, g: f32, b: f32, a: f32) -> Paint {
    Paint::Solid(Color::new(r, g, b, a))
}

fn fill(shape: Shape, paint: Paint) -> PaintOp {
    PaintOp::Fill { shape, paint }
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

/// `text` at `at` (the top-left of its layout) in `size`-point text.
fn label(system: &TextSystem, text: &str, size: f32, at: Point, color: Color) -> Vec<PaintOp> {
    let paragraph = system
        .paragraph(
            &AttributedText::plain(text, TextStyle::new(size)),
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

fn gradient(start: Point, end: Point, from: Color, to: Color) -> Paint {
    Paint::Linear {
        start,
        end,
        stops: vec![
            GradientStop {
                offset: 0.0,
                color: from,
            },
            GradientStop {
                offset: 1.0,
                color: to,
            },
        ],
    }
}

fn window(system: &TextSystem) -> DisplayList {
    let ink = Color::new(0.1, 0.1, 0.1, 1.0);
    let secondary = Color::new(0.45, 0.45, 0.47, 1.0);
    let all = rect(0.0, 0.0, WIDTH, HEIGHT);
    let mut nodes = vec![node(
        1,
        all,
        vec![fill(Shape::Rect(all), solid(0.97, 0.97, 0.98, 1.0))],
    )];

    let toolbar = rect(0.0, 0.0, WIDTH, 52.0);
    let mut ops = vec![fill(Shape::Rect(toolbar), solid(0.93, 0.93, 0.94, 1.0))];
    ops.extend(label(
        system,
        "Documents",
        15.0,
        Point { x: 200.0, y: 16.0 },
        ink,
    ));
    nodes.push(node(2, toolbar, ops));

    let sidebar = rect(0.0, 52.0, 180.0, HEIGHT - 52.0);
    let mut ops = vec![fill(Shape::Rect(sidebar), solid(0.9, 0.91, 0.93, 1.0))];
    for (i, place) in [
        "Recents",
        "Desktop",
        "Documents",
        "Downloads",
        "Pictures",
        "Music",
        "Videos",
        "Home",
        "Trash",
        "Network",
        "Backup drive",
        "Shared",
    ]
    .iter()
    .enumerate()
    {
        let y = 64.0 + i as f64 * 24.0;
        ops.extend(label(system, place, 13.0, Point { x: 20.0, y }, ink));
    }
    nodes.push(node(3, sidebar, ops));

    // a banner whose own content is blurred: two colour ramps and a disc
    let banner = rect(196.0, 60.0, 428.0, 28.0);
    nodes.push(PaintNode {
        group: Some(Group {
            opacity: 1.0,
            effects: vec![Effect::Blur { radius: 8.0 }],
        }),
        ..node(
            4,
            rect(180.0, 44.0, 460.0, 60.0),
            vec![
                fill(
                    Shape::RoundedRect {
                        rect: banner,
                        radius: 8.0,
                    },
                    gradient(
                        Point { x: 196.0, y: 60.0 },
                        Point { x: 624.0, y: 88.0 },
                        Color::new(0.2, 0.45, 0.95, 1.0),
                        Color::new(0.95, 0.4, 0.3, 1.0),
                    ),
                ),
                fill(
                    Shape::RoundedRect {
                        rect: rect(380.0, 62.0, 24.0, 24.0),
                        radius: 12.0,
                    },
                    solid(1.0, 1.0, 1.0, 0.8),
                ),
            ],
        )
    });

    for i in 0..ROWS {
        let y = LIST_TOP + i as f64 * ROW_HEIGHT;
        let row = rect(180.0, y, WIDTH - 180.0 - 200.0, ROW_HEIGHT);
        let shade = if i % 2 == 0 { 1.0 } else { 0.96 };
        let mut ops = vec![fill(Shape::Rect(row), solid(shade, shade, shade, 1.0))];
        let name = format!("Quarterly report {i}.pdf");
        ops.extend(label(
            system,
            &name,
            13.0,
            Point {
                x: 196.0,
                y: y + 4.0,
            },
            ink,
        ));
        ops.extend(label(
            system,
            "12 Nov 2026",
            11.0,
            Point {
                x: 360.0,
                y: y + 5.0,
            },
            secondary,
        ));
        nodes.push(node(10 + i as u64, row, ops));
    }

    // a card with a drop shadow; its bounds hold the shadow, its offset and three times its blur past the card
    let card = rect(460.0, 120.0, 160.0, 200.0);
    let mut ops = vec![fill(
        Shape::RoundedRect {
            rect: card,
            radius: 12.0,
        },
        solid(1.0, 1.0, 1.0, 1.0),
    )];
    ops.extend(label(
        system,
        "Info",
        15.0,
        Point { x: 476.0, y: 136.0 },
        ink,
    ));
    ops.extend(label(
        system,
        "PDF document, 2.4 MB",
        11.0,
        Point { x: 476.0, y: 160.0 },
        secondary,
    ));
    nodes.push(PaintNode {
        group: Some(Group {
            opacity: 1.0,
            effects: vec![Effect::DropShadow {
                offset: Point { x: 0.0, y: 6.0 },
                blur: 12.0,
                color: Color::new(0.0, 0.0, 0.0, 0.3),
            }],
        }),
        ..node(
            100,
            rect(
                card.x() - 36.0,
                card.y() - 36.0,
                card.width() + 72.0,
                card.height() + 78.0,
            ),
            ops,
        )
    });
    DisplayList { nodes }
}

struct Scene {
    renderer: CpuRenderer<GlyphRaster>,
    list: DisplayList,
    resources: PaintResources,
}

impl Scene {
    fn new() -> Scene {
        let system = text_system();
        let renderer = CpuRenderer::new(
            CpuConfig {
                max_pixels: 1 << 22,
                glyph_cache_bytes: 1 << 22,
                image_cache_bytes: 1 << 20,
                font_smoothing: true,
            },
            GlyphRaster::new(system.clone()),
        );
        Scene {
            renderer,
            list: window(&system),
            resources: PaintResources::default(),
        }
    }

    /// The scene after its first frame, every cache warm.
    fn drawn() -> Scene {
        let mut scene = Scene::new();
        scene.render(&DamageRegion::Full);
        scene
    }

    fn render(&mut self, damage: &DamageRegion) -> RenderFrameId {
        self.renderer
            .render(&RenderRequest {
                list: &self.list,
                resources: &self.resources,
                scale: SCALE,
                width: (WIDTH * SCALE) as u32,
                height: (HEIGHT * SCALE) as u32,
                damage,
            })
            .unwrap()
    }
}

#[library_benchmark]
#[bench::window_2x(setup = Scene::new)]
fn first_frame(mut scene: Scene) -> RenderFrameId {
    black_box(scene.render(&DamageRegion::Full))
}

#[library_benchmark]
#[bench::window_2x(setup = Scene::drawn)]
fn repaint_whole(mut scene: Scene) -> RenderFrameId {
    black_box(scene.render(&DamageRegion::Full))
}

#[library_benchmark]
#[bench::window_2x(setup = Scene::drawn)]
fn repaint_one_row(mut scene: Scene) -> RenderFrameId {
    let row = rect(
        180.0,
        LIST_TOP + 3.0 * ROW_HEIGHT,
        WIDTH - 380.0,
        ROW_HEIGHT,
    );
    black_box(scene.render(&DamageRegion::Rects(vec![row])))
}

library_benchmark_group!(
    name = window_frame,
    benchmarks = [first_frame, repaint_whole, repaint_one_row]
);
main!(library_benchmark_groups = window_frame);
