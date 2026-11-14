//! The CPU renderer's golden scenes drawn by the GPU renderer, against the same baselines and tolerance (2/255 per
//! channel at every pixel, none beyond): whole frames, and a frame repainting part of the last.

#![cfg(target_arch = "x86_64")]

#[path = "../../cupertino-ui-render/tests/goldens/scenes.rs"]
#[allow(dead_code)]
pub(crate) mod scenes;

use cupertino_ui_render::{DamageRegion, PaintResources};
use cupertino_ui_text::GlyphRaster;

use crate::common::{self, Frame};

#[test]
fn every_golden_scene_matches_its_baseline_on_the_gpu() {
    let Some(device) = common::device() else {
        return;
    };
    let system = scenes::text();
    let none = PaintResources::default();
    let mut failures = Vec::new();
    for (name, list, scale, damage) in scenes::scenes(&system) {
        let mut r = common::gpu(&device, GlyphRaster::new(system.clone()));
        let size = (
            (scenes::WIDTH * scale).round() as u32,
            (scenes::HEIGHT * scale).round() as u32,
        );
        let mut pixels = common::draw_gpu(
            &mut r,
            &Frame {
                list: &list,
                resources: &none,
                scale,
                size,
                damage: &DamageRegion::Full,
            },
        );
        let incremental = if let Some(d) = damage {
            pixels = common::draw_gpu(
                &mut r,
                &Frame {
                    list: &list,
                    resources: &none,
                    scale,
                    size,
                    damage: &DamageRegion::Rects(vec![d]),
                },
            );
            " (incremental)"
        } else {
            ""
        };
        match scenes::compare(&name, &(size.0, size.1, pixels)) {
            Ok((0, largest)) => eprintln!("{name}{incremental}: largest difference {largest}"),
            Ok((beyond, largest)) => failures.push(format!(
                "{name}{incremental}: {beyond} pixels beyond {}/255, largest difference {largest}",
                scenes::TOLERANCE
            )),
            Err(e) => failures.push(e),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
