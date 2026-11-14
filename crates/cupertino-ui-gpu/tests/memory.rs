//! What a GPU renderer holds on its device: every texture it made is given back when it is dropped, and a window that
//! is not seen keeps only its frame.

use cupertino_geometry::Rect;
use cupertino_ui_gpu::GpuRenderer;
use cupertino_ui_render::DamageRegion;
use cupertino_ui_text::GlyphRasterizer;

use crate::common::{self, Frame};
use crate::parity::{resources, scene, Boxes, Kind, SIZE};

/// Draws scenes of every kind, whole and then under damage, into one renderer; the window's size in pixels.
fn draw_everything<R: GlyphRasterizer>(r: &mut GpuRenderer<R>, scale: f64) -> (u32, u32) {
    let resources = resources();
    let size = (
        (SIZE.0 * scale).ceil() as u32,
        (SIZE.1 * scale).ceil() as u32,
    );
    for (seed, kind) in [
        (900, Kind::Images),
        (901, Kind::Glyphs),
        (902, Kind::Paths),
        (903, Kind::Clips),
        (904, Kind::Layers),
        (905, Kind::Effects),
        (906, Kind::All),
        (907, Kind::All),
    ] {
        let list = scene(seed, kind);
        for damage in [
            DamageRegion::Full,
            DamageRegion::Rects(vec![Rect::new(3.0, 4.0, 20.0, 11.0)]),
        ] {
            let frame = Frame {
                list: &list,
                resources: &resources,
                scale,
                size,
                damage: &damage,
            };
            common::draw_gpu(r, &frame);
        }
    }
    size
}

#[test]
fn a_dropped_renderer_gives_back_every_texture_it_made_and_a_hidden_one_keeps_only_its_frame() {
    let Some(device) = common::device() else {
        return;
    };
    let before = device.live_textures();
    for scale in [1.0, 1.5, 2.0] {
        let mut r = common::gpu(&device, Boxes);
        // the programs' own: a texture bound where a program reads none
        let made = device.live_textures();
        let size = draw_everything(&mut r, scale);
        let drawing = device.live_textures();
        assert!(
            drawing.textures > made.textures + 1,
            "scale {scale}: drawing keeps atlases, images and targets ({drawing:?})"
        );
        // not seen (a window's renderer trims when it drops its layers): the atlases, pooled targets, images and kept
        // layers go, the frame stays
        r.trim();
        let frame = u64::from(size.0) * u64::from(size.1) * 4;
        let hidden = device.live_textures();
        assert_eq!(
            (hidden.textures, hidden.bytes),
            (made.textures + 1, made.bytes + frame),
            "scale {scale}: a hidden window keeps its frame alone"
        );
        assert!(r.read_pixels().is_ok(), "and the frame is still there");
        // seen again, it draws as before
        draw_everything(&mut r, scale);
        drop(r);
        assert_eq!(
            device.live_textures(),
            before,
            "scale {scale}: a dropped renderer gives back all it made"
        );
    }
}
