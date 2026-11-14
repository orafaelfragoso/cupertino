use super::scenes::*;
use cupertino_geometry::Rect;
use cupertino_ui_render::{
    CpuConfig, CpuRenderer, DamageRegion, DisplayList, PaintResources, RenderRequest, UiRenderer,
};
use cupertino_ui_text::{GlyphRaster, TextSystem};
use std::sync::Arc;

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
