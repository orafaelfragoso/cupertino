//! Window shadows: the margins a shadow needs, the cached nine-slice image against a direct render, and the knock-out
//! beneath the window.

use cupertino_decorations::shadow::render_direct;
use cupertino_decorations::{ShadowParams, ShadowRasterizer};
use cupertino_geometry::{CornerStyle, Size};
use cupertino_theme::{ShadowOption, WindowShadowEntry};

fn params(active: bool, dark: bool) -> ShadowParams {
    ShadowParams::lookup(ShadowOption::Standard, active, dark, false)
}

fn entry(sigma: f64, offset: f64) -> ShadowParams {
    let mut entry: WindowShadowEntry = params(true, false).entry;
    entry.sigma = sigma;
    entry.vertical_offset = offset;
    ShadowParams::new(entry)
}

#[test]
fn margins_hold_the_blur_and_the_offset_shifts_the_shadow_down() {
    for scale in [1.0, 1.5, 2.0] {
        let p = params(true, false);
        let m = p.margins_px(scale);
        let (r, o) = (p.radius_px(scale), p.offset_px(scale));
        assert!(
            r as f64 >= p.sigma_px(scale),
            "the blur radius covers the sigma"
        );
        assert_eq!((m.left, m.right), (r, r));
        assert_eq!(m.bottom - m.top, 2 * o);

        let gpu = p.gpu(Size::new(300.0, 200.0), 12.0, scale);
        assert_eq!(
            gpu.margins_px,
            [m.left as f32, m.top as f32, m.right as f32, m.bottom as f32]
        );
        assert_eq!(
            gpu.rect_size_px,
            [(300.0 * scale) as f32, (200.0 * scale) as f32]
        );
        assert_eq!(gpu.corner_radius_px, (12.0 * scale) as f32);
    }
}

#[test]
fn the_active_window_casts_the_stronger_shadow() {
    for dark in [false, true] {
        let (active, inactive) = (params(true, dark), params(false, dark));
        assert!(active.density() > inactive.density(), "dark: {dark}");
    }
}

#[test]
fn a_large_window_shadow_is_the_stretched_cached_one() {
    let p = entry(10.0, 6.0);
    let style = CornerStyle::continuous();
    let mut rasterizer = ShadowRasterizer::new();
    for (scale, size) in [
        (1.0, Size::new(640.0, 480.0)),
        (2.0, Size::new(500.0, 380.0)),
    ] {
        let stretched = rasterizer.render(&p, size, 12.0, style, scale);
        let direct = render_direct(&p, size, 12.0, style, scale);
        assert_eq!(
            (stretched.width, stretched.height),
            (direct.width, direct.height)
        );
        assert_eq!(stretched.margins, direct.margins);
        let worst = stretched
            .data
            .iter()
            .zip(&direct.data)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap();
        assert!(
            worst <= 2,
            "scale {scale}: nine-slice differs from a direct render by {worst}"
        );
    }
    assert_eq!(rasterizer.cached_sources(), 2, "one source per scale");
    rasterizer.render(&p, Size::new(700.0, 500.0), 12.0, style, 1.0);
    assert_eq!(
        rasterizer.cached_sources(),
        2,
        "the source is shared between sizes"
    );
}

#[test]
fn a_frame_smaller_than_the_source_is_rendered_directly() {
    let p = entry(10.0, 6.0);
    let style = CornerStyle::continuous();
    let small = Size::new(40.0, 30.0);
    let image = ShadowRasterizer::new().render(&p, small, 12.0, style, 1.0);
    assert_eq!(image, render_direct(&p, small, 12.0, style, 1.0));
}

#[test]
fn knocking_out_clears_the_shadow_under_the_window_and_keeps_it_around() {
    let p = entry(10.0, 6.0);
    let style = CornerStyle::continuous();
    let size = Size::new(200.0, 150.0);
    let mut image = render_direct(&p, size, 12.0, style, 1.0);
    let (ox, oy) = image.frame_origin();
    let (cx, cy) = (ox + 100, oy + 75);
    let below_bottom = (ox + 100, oy + 150 + 4);
    assert!(
        image.alpha(cx, cy) > 0,
        "the blurred shadow reaches under the window"
    );
    assert!(image.alpha(below_bottom.0, below_bottom.1) > 0);
    let outside = image.alpha(below_bottom.0, below_bottom.1);

    image.knock_out((200, 150), 12.0, style);
    assert_eq!(image.alpha(cx, cy), 0);
    assert_eq!(image.alpha(below_bottom.0, below_bottom.1), outside);
}
