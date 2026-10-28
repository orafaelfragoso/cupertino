//! Opt-in timing matrix: `cargo test --release -p cupertino-ui-render --test render_matrix -- --ignored --nocapture`.
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
            let config = CpuConfig {
                max_pixels: 1 << 26,
                glyph_cache_bytes: 1 << 24,
                image_cache_bytes: 1 << 24,
                hint_glyphs: false,
            };
            let mut r = CpuRenderer::new(config, Blocks);
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
