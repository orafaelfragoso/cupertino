//! Opt-in timing matrix: `cargo test --release -p cupertino-ui-render --test suite render_matrix:: -- --ignored --nocapture`.
//! Prints cold, steady (one row changes) and full-repaint timings with the counters of each case. Timings from a
//! shared machine are indicative only; the counters are exact.

use cupertino_geometry::{Point, Rect};
use cupertino_theme::Color;
use cupertino_ui_render::{
    CpuConfig, CpuRenderer, DamageRegion, DisplayList, Paint, PaintNode, PaintNodeId, PaintOp,
    PaintResources, PaintRevision, RenderRequest, Shape, Transform, UiRenderer,
};
use cupertino_ui_text::{
    FontId, GlyphFormat, GlyphId, GlyphKey, GlyphRasterizer, GlyphRun, PositionedGlyph,
    RasterGlyph, TextError,
};
use std::sync::Arc;
use std::time::{Duration, Instant};

struct Blocks;

impl GlyphRasterizer for Blocks {
    fn rasterize(&mut self, key: &GlyphKey) -> Result<Option<RasterGlyph>, TextError> {
        let side = (key.size_q / 64).max(1);
        Ok(Some(RasterGlyph {
            format: GlyphFormat::Coverage,
            width: side,
            height: side,
            stride: side,
            bearing_x: 0,
            bearing_y: side as i32,
            pixels: Arc::from(vec![200u8; (side * side) as usize]),
        }))
    }
}

fn rows(count: u32, tick: u32) -> DisplayList {
    let nodes = (0..count)
        .map(|i| {
            let y = f64::from(i) * 24.0 % 1000.0;
            let bounds = Rect::new(0.0, y, 600.0, 22.0);
            let run = GlyphRun {
                font: FontId(1),
                size: 13.0,
                glyphs: (0..40)
                    .map(|g| PositionedGlyph {
                        id: GlyphId((g + if i == 0 { tick } else { 0 }) % 60),
                        x: g as f32 * 7.0,
                        y: 0.0,
                        cluster: 0,
                    })
                    .collect(),
            };
            PaintNode {
                layer: None,
                id: PaintNodeId(u64::from(i)),
                parent: None,
                revision: PaintRevision(u64::from(if i == 0 { tick } else { 0 })),
                transform: Transform::IDENTITY,
                clip: None,
                group: None,
                bounds,
                hit: bounds,
                ops: vec![
                    PaintOp::Fill {
                        shape: Shape::RoundedRect {
                            rect: bounds,
                            radius: 6.0,
                        },
                        paint: Paint::Solid(Color::new(0.9, 0.9, 0.95, 1.0)),
                    },
                    PaintOp::Glyphs {
                        origin: Point {
                            x: 8.0,
                            y: y + 16.0,
                        },
                        run,
                        color: Color::new(0.0, 0.0, 0.0, 1.0),
                    },
                ],
            }
        })
        .collect();
    DisplayList { nodes }
}

const TILE: f64 = 80.0;
const GAP: f64 = 8.0;
const COLUMNS: u32 = 10;

fn tile(index: u32) -> Rect {
    Rect::new(
        GAP + f64::from(index % COLUMNS) * (TILE + GAP),
        GAP + f64::from(index / COLUMNS) * (TILE + GAP),
        TILE,
        TILE,
    )
}

fn fill(id: u64, revision: u64, bounds: Rect, color: Color) -> PaintNode {
    PaintNode {
        layer: None,
        id: PaintNodeId(id),
        parent: None,
        revision: PaintRevision(revision),
        transform: Transform::IDENTITY,
        clip: None,
        group: None,
        bounds,
        hit: bounds,
        ops: vec![PaintOp::Fill {
            shape: Shape::RoundedRect {
                rect: bounds,
                radius: 12.0,
            },
            paint: Paint::Solid(color),
        }],
    }
}

fn switcher(count: u32, selected: u32, step: u64) -> DisplayList {
    let last = tile(count - 1);
    let panel = Rect::new(
        0.0,
        0.0,
        f64::from(count.min(COLUMNS)) * (TILE + GAP) + GAP,
        last.y() + TILE + GAP,
    );
    let mut nodes = vec![fill(0, 0, panel, Color::new(0.2, 0.2, 0.2, 0.8))];
    nodes.extend((0..count).map(|i| {
        let color = if i == selected {
            Color::new(0.3, 0.5, 1.0, 1.0)
        } else {
            Color::new(0.9, 0.9, 0.95, 1.0)
        };
        let revision = if i == selected || (i + 1) % count == selected {
            step
        } else {
            0
        };
        fill(u64::from(i) + 1, revision, tile(i), color)
    }));
    DisplayList { nodes }
}

fn window(width: u32, height: u32, tick: u32) -> DisplayList {
    let mut list = rows(10, tick);
    list.nodes.insert(
        0,
        fill(
            1000,
            u64::from(tick),
            Rect::new(0.0, 0.0, f64::from(width), f64::from(height)),
            Color::new(1.0, 1.0, 1.0, 1.0),
        ),
    );
    list
}

fn renderer() -> CpuRenderer<Blocks> {
    CpuRenderer::new(
        CpuConfig {
            max_pixels: 1 << 26,
            glyph_cache_bytes: 1 << 24,
            image_cache_bytes: 1 << 24,
            font_smoothing: true,
        },
        Blocks,
    )
}

fn at<'a>(
    list: &'a DisplayList,
    resources: &'a PaintResources,
    damage: &'a DamageRegion,
    target: (f64, u32, u32),
) -> RenderRequest<'a> {
    RenderRequest {
        list,
        resources,
        scale: target.0,
        width: target.1,
        height: target.2,
        damage,
    }
}

fn summary(mut samples: Vec<Duration>) -> String {
    samples.sort();
    let at = |q: f64| samples[((samples.len() - 1) as f64 * q).round() as usize];
    format!(
        "min {:?} median {:?} p95 {:?}",
        samples[0],
        at(0.5),
        at(0.95)
    )
}

#[test]
#[ignore = "timing matrix; run with --release --ignored --nocapture"]
fn matrix() {
    let none = PaintResources::default();
    for (width, height, scale) in [(1920u32, 1080u32, 1.0f64), (1920, 1080, 2.0)] {
        let (width, height) = (
            (f64::from(width) * scale) as u32,
            (f64::from(height) * scale) as u32,
        );
        for count in [1u32, 10, 100] {
            let mut r = renderer();
            let first = rows(count, 0);
            let started = Instant::now();
            r.render(&at(
                &first,
                &none,
                &DamageRegion::Full,
                (scale, width, height),
            ))
            .unwrap();
            let cold = started.elapsed();
            let cold_stats = *r.stats();
            let mut steady = Vec::new();
            for tick in 1..=60 {
                let list = rows(count, tick);
                let damage = DamageRegion::Rects(vec![Rect::new(0.0, 0.0, 600.0, 22.0)]);
                let started = Instant::now();
                r.render(&at(&list, &none, &damage, (scale, width, height)))
                    .unwrap();
                steady.push(started.elapsed());
            }
            let steady_stats = *r.stats();
            let mut full = Vec::new();
            for tick in 61..=80 {
                let list = rows(count, tick);
                let started = Instant::now();
                r.render(&at(
                    &list,
                    &none,
                    &DamageRegion::Full,
                    (scale, width, height),
                ))
                .unwrap();
                full.push(started.elapsed());
            }
            println!("{width}x{height} scale {scale} rows {count}");
            println!("  cold {cold:?}  {cold_stats:?}");
            println!("  steady {}  {steady_stats:?}", summary(steady));
            println!("  full   {}  {:?}", summary(full), r.stats());
        }
    }
}

fn step_damage(count: u32, selected: u32) -> DamageRegion {
    DamageRegion::Rects(vec![tile((selected + count - 1) % count), tile(selected)])
}

#[test]
fn switcher_step_repaints_only_the_old_and_new_highlight() {
    let none = PaintResources::default();
    let mut r = renderer();
    r.render(&at(
        &switcher(10, 0, 0),
        &none,
        &DamageRegion::Full,
        (2.0, 1920, 400),
    ))
    .unwrap();
    r.render(&at(
        &switcher(10, 1, 1),
        &none,
        &step_damage(10, 1),
        (2.0, 1920, 400),
    ))
    .unwrap();
    let stats = r.stats();
    assert_eq!(stats.dirty_rects, 2);
    assert_eq!(stats.dirty_pixels, 2 * 162 * 162);
    assert_eq!(stats.nodes_painted, 4);
}

#[test]
#[ignore = "timing matrix; run with --release --ignored --nocapture"]
fn switcher_matrix() {
    let none = PaintResources::default();
    for scale in [1.0f64, 2.0] {
        let (width, height) = ((1920.0 * scale) as u32, (1080.0 * scale) as u32);
        for count in [1u32, 10, 100] {
            let mut r = renderer();
            let started = Instant::now();
            r.render(&at(
                &switcher(count, 0, 0),
                &none,
                &DamageRegion::Full,
                (scale, width, height),
            ))
            .unwrap();
            let open = started.elapsed();
            let open_stats = *r.stats();
            let mut steps = Vec::new();
            for step in 1..=60u32 {
                let selected = step % count;
                let list = switcher(count, selected, u64::from(step));
                let damage = step_damage(count, selected);
                let started = Instant::now();
                r.render(&at(&list, &none, &damage, (scale, width, height)))
                    .unwrap();
                steps.push(started.elapsed());
            }
            println!("switcher {width}x{height} scale {scale} windows {count}");
            println!("  open {open:?}  {open_stats:?}");
            println!("  step {}  {:?}", summary(steps), r.stats());
        }
    }
}

#[test]
#[ignore = "timing matrix; run with --release --ignored --nocapture"]
fn resize_matrix() {
    let none = PaintResources::default();
    for scale in [1.0f64, 2.0] {
        let mut r = renderer();
        let mut steps = Vec::new();
        let mut largest = None;
        let first = window(772, 485, 0);
        r.render(&at(
            &first,
            &none,
            &DamageRegion::Full,
            (scale, (772.0 * scale) as u32, (485.0 * scale) as u32),
        ))
        .unwrap();
        for step in 0..40u32 {
            let (width, height) = (800 + step * 28, 500 + step * 15);
            let list = window(width, height, step + 1);
            let (width, height) = (
                (f64::from(width) * scale) as u32,
                (f64::from(height) * scale) as u32,
            );
            let started = Instant::now();
            r.render(&at(
                &list,
                &none,
                &DamageRegion::Full,
                (scale, width, height),
            ))
            .unwrap();
            steps.push(started.elapsed());
            largest = Some((width, height, *r.stats()));
        }
        let (width, height, stats) = largest.unwrap();
        println!("resize scale {scale} 40 steps up to {width}x{height}");
        println!("  step {}  last {stats:?}", summary(steps));
    }
}
