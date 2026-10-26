//! A composed window image for samples and tests: shadow, content area, titlebar and rim.

use crate::render::{DecorationState, Decorations};
use crate::shadow::ShadowRasterizer;
use cupertino_geometry::{Radii, Rect, Size};
use tiny_skia::{FillRule, Paint, Pixmap, PixmapPaint, Transform};

/// Image of the decorated window with its shadow, plus the position of the frame inside it.
pub struct Preview {
    pub image: Pixmap,
    /// Top-left corner of the frame in the image, device pixels.
    pub frame_origin: (u32, u32),
}

/// Compose the window at `scale`. The content area is filled with the palette's window
/// background (a stand-in for client content).
pub fn render_preview(
    dec: &mut Decorations,
    shadows: &mut ShadowRasterizer,
    content: Size,
    scale: f64,
    state: &DecorationState,
) -> Preview {
    let layout = dec.layout(content);
    let frame = layout.frame.size;
    let params = dec.shadow_params(state.active);
    let r = dec.config.corner_radius();
    let style = dec.config.corner_style;
    let mut shadow = shadows.render(&params, frame, r, style, scale);
    let (fw, fh) = (
        (frame.width * scale).round() as u32,
        (frame.height * scale).round() as u32,
    );
    shadow.knock_out((fw, fh), r * scale, style);
    let mut image = Pixmap::new(shadow.width, shadow.height).expect("non-zero size");
    image.data_mut().copy_from_slice(&shadow.data);
    let (ox, oy) = shadow.frame_origin();

    let tb_px = (dec.config.metrics.titlebar_reserved(dec.config.style) * scale).round();
    let body = Rect::new(
        0.0,
        tb_px / scale,
        frame.width,
        frame.height - tb_px / scale,
    );
    if let Some(path) = crate::shape::rounded_path(body, Radii::bottom(r), style, scale) {
        let mut p = Paint {
            anti_alias: true,
            ..Paint::default()
        };
        let bg = dec.config.palette.window_background;
        p.set_color(tiny_skia::Color::from_rgba(bg[0], bg[1], bg[2], bg[3]).expect("valid colour"));
        image.fill_path(
            &path,
            &p,
            FillRule::Winding,
            Transform::from_translate(ox as f32, oy as f32),
            None,
        );
    }
    let titlebar = dec.render_titlebar(frame.width, scale, state);
    image.draw_pixmap(
        ox as i32,
        oy as i32,
        titlebar.as_ref(),
        &PixmapPaint::default(),
        Transform::identity(),
        None,
    );
    let rim = dec.render_rim(frame, scale);
    image.draw_pixmap(
        ox as i32,
        oy as i32,
        rim.as_ref(),
        &PixmapPaint::default(),
        Transform::identity(),
        None,
    );
    Preview {
        image,
        frame_origin: (ox, oy),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::TitlebarStyle;
    use crate::render::DecorationConfig;
    use cupertino_theme::Appearance;

    #[test]
    fn preview_layers_are_in_place() {
        let assets = cupertino_assets::Assets::discover();
        let mut dec = Decorations::new(
            DecorationConfig::builtin(Appearance::Light, TitlebarStyle::Standard),
            &assets,
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let mut shadows = ShadowRasterizer::new();
        let p = render_preview(
            &mut dec,
            &mut shadows,
            Size::new(400.0, 260.0),
            1.0,
            &DecorationState::default(),
        );
        let (ox, oy) = p.frame_origin;
        let px = |x: u32, y: u32| p.image.pixel(x, y).unwrap();
        assert_eq!(px(ox + 200, oy + 14).alpha(), 255, "titlebar");
        assert_eq!(px(ox + 200, oy + 150).alpha(), 255, "content");
        assert!(
            px(ox + 200, oy + 288 + 20).alpha() > 10,
            "shadow below the window"
        );
        assert_eq!(px(0, 0).alpha(), 0, "far corner is clear");
        assert!(
            px(ox, oy).alpha() < 40,
            "rounded corner shows only faint shadow"
        );
    }
}
