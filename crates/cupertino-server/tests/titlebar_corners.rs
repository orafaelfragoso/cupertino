//! The title bar the compositor draws for a window keeps to the window's outline at its top corners: at a display at
//! 1.5, drawn at 2 and shrunk or drawn straight at 1.5, in the dark and the light appearance, over a bright desktop,
//! every pixel well outside the window's rounded outline shows the desktop and every pixel just inside it shows the
//! bar, never the desktop. A bar whose corners are tighter than the outline stands out past the outline and its rim.

use crate::common;

use common::desktop::Desktop;
use cupertino_defaults::compositor::{FractionalScaleMode, FractionalScaleModes};
use cupertino_geometry::{CornerStyle, Rect, RoundedRect};
use cupertino_raster_math::coverage::rounded_distance;
use cupertino_server::backend::headless;
use cupertino_server::config::Config;
use cupertino_theme::Appearance;
use image::RgbaImage;
use smithay::input::pointer::CursorImageStatus;
use wayland_protocols::xdg::decoration::zv1::client::zxdg_toplevel_decoration_v1::Mode;

const SCALE: f64 = 1.5;
/// The desktop's solid colour, which no pixel inside the window may show.
const DESKTOP: [u8; 3] = [255, 0, 255];
/// How far from the outline, in display pixels, a pixel is taken to lie clear of it: the outline's half point, the
/// edge's coverage and the shrink's reach.
const CLEAR: f32 = 1.5;
/// How deep inside the outline the pixels checked for the bar lie, in display pixels: the window buttons lie deeper.
const BAND: f32 = 4.0;

/// The desktop, dimmed by the window's shadow at most: its hue, not the bar's grey.
fn shows_desktop(p: [u8; 4]) -> bool {
    p[1] < 0x10 && p[0] > 0x60 && p[0].abs_diff(p[2]) < 0x10
}

/// The bar's grey (dark or light), its rim or its outline over it: no trace of the desktop's hue.
fn shows_bar(p: [u8; 4]) -> bool {
    p[0].abs_diff(p[1]) < 0x08 && p[1].abs_diff(p[2]) < 0x08
}

/// The display's image with one window whose client asks for `decorations`, and the window's frame in display pixels.
fn shown(
    appearance: Appearance,
    mode: FractionalScaleMode,
    decorations: Mode,
) -> (RgbaImage, Rect) {
    let config = Config {
        appearance,
        wallpaper_color: Some(DESKTOP),
        fractional_scale_mode: FractionalScaleModes {
            others: Some(mode),
            displays: Default::default(),
        },
        ..Config::default()
    };
    let mut d = Desktop::configured(
        config,
        &[headless::HeadlessDisplay {
            name: "headless-1".into(),
            size: (1200, 900),
            physical_mm: (0, 0),
        }],
        SCALE,
    );
    d.state.cursor_status = CursorImageStatus::Hidden;
    d.clients
        .push(common::Client::decorated(d.socket(), decorations));
    let deadline = std::time::Instant::now() + common::HANG_GUARD;
    while d.state.wm.windows().count() != 1 {
        d.pump();
        assert!(
            std::time::Instant::now() < deadline,
            "the client did not map"
        );
    }
    d.at(3000);
    let id = d.state.wm.key_window().expect("the window is key");
    assert_eq!(
        d.state.entry(id).unwrap().draws_own_frame(),
        decorations == Mode::ClientSide,
        "who draws the window's title bar"
    );
    let f = d.state.wm.window(id).unwrap().geometry();
    let o = d.state.outputs[0].rect;
    let frame = Rect::new(
        (f.x - o.x) * SCALE,
        (f.y - o.y) * SCALE,
        f.w * SCALE,
        f.h * SCALE,
    );
    let mut backend = d.state.backend.take().unwrap();
    let image = d
        .state
        .draw_output_image(backend.renderer(), 0, false, false)
        .expect("an image of the display");
    d.state.backend = Some(backend);
    (image, frame)
}

fn check(mode: FractionalScaleMode) {
    for appearance in [Appearance::Dark, Appearance::Light] {
        let (image, frame) = shown(appearance, mode, Mode::ServerSide);
        let window = Config::default().window;
        let style = if window.continuous_corners {
            CornerStyle::continuous()
        } else {
            CornerStyle::Circular
        };
        let radius = window.corner_radius * SCALE;
        let extent = RoundedRect::new(frame, radius, style).extents()[0] as f32;
        let centre = [
            (frame.min_x() + frame.width() / 2.0) as f32,
            (frame.min_y() + frame.height() / 2.0) as f32,
        ];
        let half = [(frame.width() / 2.0) as f32, (frame.height() / 2.0) as f32];
        let (top, left, right) = (
            frame.min_y().floor() as u32,
            frame.min_x().floor() as u32,
            frame.max_x().ceil() as u32,
        );
        let reach = extent.ceil() as u32 + 2;
        let (mut outside, mut inside) = (0, 0);
        for y in top.saturating_sub(2)..top + reach {
            for x in (left.saturating_sub(2)..left + reach).chain(right - reach..right + 2) {
                let p = [x as f32 + 0.5 - centre[0], y as f32 + 0.5 - centre[1]];
                let d = rounded_distance(p, half, extent, style.exponent() as f32);
                let px = image.get_pixel(x, y).0;
                if d > CLEAR {
                    outside += 1;
                    assert!(
                        shows_desktop(px),
                        "{appearance:?} {mode:?}: ({x}, {y}), {d:.2} px outside the window's outline, shows the \
                         title bar, not the desktop: {px:?}"
                    );
                } else if d < -CLEAR && d > -BAND {
                    inside += 1;
                    assert!(
                        shows_bar(px),
                        "{appearance:?} {mode:?}: ({x}, {y}), {:.2} px inside the window's outline, shows the \
                         desktop: {px:?}",
                        -d
                    );
                }
            }
        }
        assert!(
            outside > 100 && inside > 50,
            "the corners were looked at: {outside} outside, {inside} inside"
        );
    }
}

#[test]
fn a_title_bar_keeps_to_the_window_s_rounded_corners_drawn_at_two_and_shrunk() {
    check(FractionalScaleMode::Supersample);
}

#[test]
fn a_title_bar_keeps_to_the_window_s_rounded_corners_drawn_straight_at_one_and_a_half() {
    check(FractionalScaleMode::Direct);
}

/// The client's own square, opaque frame: its blue, which the desktop's magenta never is.
fn shows_client(p: [u8; 4]) -> bool {
    p[0] < 0x40 && p[1] < 0x40 && p[2] > 0xa0
}

/// A window whose client draws its own decorations with square corners is cut to the same rounded outline as one the
/// compositor decorates, at all four corners, at a display at 1.5 drawn straight: every pixel well outside the outline
/// shows the desktop, every pixel just inside it the client's frame.
#[test]
fn a_client_drawn_window_with_square_corners_is_cut_to_the_native_outline_at_one_and_a_half() {
    let (image, frame) = shown(
        Appearance::Dark,
        FractionalScaleMode::Direct,
        Mode::ClientSide,
    );
    let window = Config::default().window;
    assert!(window.continuous_corners);
    let style = CornerStyle::continuous();
    let radius = cupertino_theme::WINDOW_CORNER_RADIUS_REGULAR * SCALE;
    let extent = RoundedRect::new(frame, radius, style).extents()[0] as f32;
    let centre = [
        (frame.min_x() + frame.width() / 2.0) as f32,
        (frame.min_y() + frame.height() / 2.0) as f32,
    ];
    let half = [(frame.width() / 2.0) as f32, (frame.height() / 2.0) as f32];
    let (top, left, right, bottom) = (
        frame.min_y().floor() as u32,
        frame.min_x().floor() as u32,
        frame.max_x().ceil() as u32,
        frame.max_y().ceil() as u32,
    );
    let reach = extent.ceil() as u32 + 2;
    let (mut outside, mut inside) = (0, 0);
    let rows = (top.saturating_sub(2)..top + reach).chain(bottom - reach..bottom + 2);
    for y in rows {
        for x in (left.saturating_sub(2)..left + reach).chain(right - reach..right + 2) {
            let p = [x as f32 + 0.5 - centre[0], y as f32 + 0.5 - centre[1]];
            let d = rounded_distance(p, half, extent, style.exponent() as f32);
            let px = image.get_pixel(x, y).0;
            if d > CLEAR {
                outside += 1;
                assert!(
                    shows_desktop(px),
                    "({x}, {y}), {d:.2} px outside the outline, shows the client's square corner: {px:?}"
                );
            } else if d < -CLEAR && d > -BAND {
                inside += 1;
                assert!(
                    shows_client(px),
                    "({x}, {y}), {:.2} px inside the outline, does not show the client: {px:?}",
                    -d
                );
            }
        }
    }
    assert!(
        outside > 200 && inside > 100,
        "the four corners were looked at: {outside} outside, {inside} inside"
    );
}
