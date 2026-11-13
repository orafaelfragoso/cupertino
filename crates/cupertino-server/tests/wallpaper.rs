//! Image wallpapers against a headless compositor: an image covers the output, the dark appearance takes its own image,
//! a change of `wallpaper` in `compositor.json` applies live, an image that is not there leaves the procedural
//! wallpaper, and the lightness of the strip under each output's menu bar is measured as that output shows the image,
//! turned outputs included. Tests skip (pass) when no EGL device is available.

use crate::common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::desktop::Desktop;
use cupertino_defaults::compositor::CompositorSettings;
use cupertino_server::backend::headless;
use cupertino_server::config::Config;
use cupertino_server::Cupertino;
use cupertino_theme::Appearance;
use smithay::output::Mode;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use smithay::utils::Transform;

const OUTPUT: (i32, i32) = (800, 600);

struct Harness {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
    dir: tempfile::TempDir,
}

impl Harness {
    fn new(tweak: impl FnOnce(&mut Config, &Path)) -> Option<Harness> {
        let event_loop: EventLoop<'static, Cupertino> =
            EventLoop::try_new().expect("create test event loop");
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        tweak(&mut config, dir.path());
        let mut state = Cupertino::new(
            Display::new().expect("create test display"),
            event_loop.handle(),
            event_loop.get_signal(),
            config,
            true,
            &cupertino_assets::Assets::builtin(),
        )
        .expect("create test compositor");
        if let Err(e) = headless::init(&mut state, OUTPUT, 1.0) {
            common::egl_unavailable(format!("{e:#}"));
            return None;
        }
        Some(Harness {
            state,
            event_loop,
            dir,
        })
    }

    fn wait_until(&mut self, what: &str, mut cond: impl FnMut(&Cupertino) -> bool) {
        let end = Instant::now() + common::HANG_GUARD;
        while Instant::now() < end {
            headless::step(
                &mut self.state,
                &mut self.event_loop,
                Duration::from_millis(5),
            )
            .unwrap();
            if cond(&self.state) {
                return;
            }
        }
        panic!("timed out waiting for {what}");
    }

    /// The output as rendered once the wallpaper image, which is read on a thread, is in.
    fn screenshot(&mut self) -> image::RgbaImage {
        loop {
            let shot = self.shoot();
            if !self.state.overlays.wallpaper.pending() {
                return shot;
            }
            self.wait_until("the wallpaper image", |s| !s.overlays.wallpaper.pending());
        }
    }

    fn shoot(&mut self) -> image::RgbaImage {
        let path = self.dir.path().join("shot.png");
        // the request list is drained once drawn, so a leftover shot would pass for the new one
        let _ = std::fs::remove_file(&path);
        self.state.screenshot_requests.push(path.clone());
        self.state.request_redraw();
        let p = path.clone();
        self.wait_until("the screenshot", move |_| p.exists());
        image::open(&path).unwrap().to_rgba8()
    }
}

fn png(dir: &Path, name: &str, size: (u32, u32), pixel: impl Fn(u32) -> [u8; 3]) -> PathBuf {
    let mut img = image::RgbImage::new(size.0, size.1);
    for (x, _, p) in img.enumerate_pixels_mut() {
        *p = image::Rgb(pixel(x));
    }
    let path = dir.join(name);
    img.save(&path).unwrap();
    path
}

fn close(p: &image::Rgba<u8>, want: [u8; 3]) -> bool {
    (0..3).all(|c| p[c].abs_diff(want[c]) <= 3)
}

#[test]
fn an_image_covers_the_output_and_the_dark_appearance_takes_its_own() {
    let Some(mut h) = Harness::new(|config, dir| {
        // 1600 x 400: red on the left half, blue on the right; the 4:3 output shows the middle third of it
        config.wallpaper.light = Some(png(dir, "day.png", (1600, 400), |x| {
            if x < 800 {
                [200, 20, 20]
            } else {
                [20, 20, 200]
            }
        }));
        config.wallpaper.dark = Some(png(dir, "night.png", (64, 64), |_| [20, 200, 20]));
    }) else {
        return;
    };
    let shot = h.screenshot();
    assert_eq!(shot.dimensions(), (800, 600));
    assert!(
        close(shot.get_pixel(100, 300), [200, 20, 20]),
        "left: {:?}",
        shot.get_pixel(100, 300)
    );
    assert!(
        close(shot.get_pixel(700, 300), [20, 20, 200]),
        "right: {:?}",
        shot.get_pixel(700, 300)
    );
    assert!(
        close(shot.get_pixel(100, 10), [200, 20, 20])
            && close(shot.get_pixel(700, 590), [20, 20, 200])
    );

    h.state.set_appearance(Appearance::Dark);
    let shot = h.screenshot();
    assert!(
        close(shot.get_pixel(400, 300), [20, 200, 20]),
        "{:?}",
        shot.get_pixel(400, 300)
    );
    assert!(close(shot.get_pixel(100, 10), [20, 200, 20]));
}

#[test]
fn a_change_of_the_wallpaper_in_the_settings_applies_live() {
    let Some(mut h) = Harness::new(|_, _| {}) else {
        return;
    };
    let procedural = *h.screenshot().get_pixel(400, 300);
    let path = png(h.dir.path(), "new.png", (32, 32), |_| [200, 120, 20]);
    let before = CompositorSettings::default();
    let after = CompositorSettings {
        wallpaper: Some(path.display().to_string()),
        ..CompositorSettings::default()
    };
    h.state.apply_compositor_change(&before, &after);
    let shot = h.screenshot();
    assert!(
        close(shot.get_pixel(400, 300), [200, 120, 20]),
        "{:?}",
        shot.get_pixel(400, 300)
    );
    assert_ne!(*shot.get_pixel(400, 300), procedural);

    // the key goes: the procedural wallpaper is back
    h.state.apply_compositor_change(&after, &before);
    assert_eq!(*h.screenshot().get_pixel(400, 300), procedural);
}

#[test]
fn an_image_that_cannot_be_read_leaves_the_procedural_wallpaper() {
    let procedural = {
        let Some(mut h) = Harness::new(|_, _| {}) else {
            return;
        };
        *h.screenshot().get_pixel(400, 300)
    };
    let Some(mut h) = Harness::new(|config, dir| {
        std::fs::write(dir.join("broken.png"), b"not an image").unwrap();
        config.wallpaper.light = Some(dir.join("broken.png"));
        config.wallpaper.dark = Some(dir.join("missing.png"));
    }) else {
        return;
    };
    assert_eq!(
        *h.screenshot().get_pixel(400, 300),
        procedural,
        "unreadable"
    );
    h.state.set_appearance(Appearance::Dark);
    let dark = *h.screenshot().get_pixel(400, 300);
    assert_ne!(
        dark, procedural,
        "the dark procedural wallpaper differs from the light"
    );
    h.state.set_appearance(Appearance::Light);
    assert_eq!(*h.screenshot().get_pixel(400, 300), procedural);
}

/// An image of `size` whose top `dark_rows` rows are near black and the rest near white.
fn dark_topped(dir: &Path, name: &str, size: (u32, u32), dark_rows: u32) -> PathBuf {
    let mut img = image::RgbImage::new(size.0, size.1);
    for (_, y, p) in img.enumerate_pixels_mut() {
        *p = image::Rgb(if y < dark_rows { [10; 3] } else { [245; 3] });
    }
    let path = dir.join(name);
    img.save(&path).unwrap();
    path
}

/// Step `d` until every output has a measure under its bar; the measures, in output order.
fn measures(d: &mut Desktop) -> Vec<u16> {
    let end = Instant::now() + common::HANG_GUARD;
    for frame in 1.. {
        d.at(frame * 16);
        let measured: Option<Vec<u16>> = d
            .state
            .outputs
            .iter()
            .map(|o| d.state.backdrop_lightness(o.id))
            .collect();
        if let Some(measured) = measured {
            return measured;
        }
        assert!(Instant::now() < end, "timed out waiting for the measures");
    }
    unreachable!()
}

fn display(name: &str, size: (i32, i32)) -> headless::HeadlessDisplay {
    headless::HeadlessDisplay {
        name: name.into(),
        size,
        physical_mm: (0, 0),
    }
}

#[test]
fn the_strip_under_the_bar_is_measured_once_the_image_is_up_and_again_when_it_changes() {
    let Some(mut h) = Harness::new(|config, dir| {
        // dark only in the 30 rows under the 24 pt bar
        config.wallpaper.light = Some(dark_topped(dir, "dark-top.png", (800, 600), 30));
    }) else {
        return;
    };
    let out = h.state.outputs[0].id;
    h.wait_until("the measure", |s| s.backdrop_lightness(out).is_some());
    let dark = h.state.backdrop_lightness(out).unwrap();
    assert!(dark < 100, "{dark}");

    let path = dark_topped(h.dir.path(), "light.png", (800, 600), 0);
    let before = CompositorSettings::default();
    let after = CompositorSettings {
        wallpaper: Some(path.display().to_string()),
        ..CompositorSettings::default()
    };
    h.state.apply_compositor_change(&before, &after);
    h.wait_until("the new measure", |s| {
        s.backdrop_lightness(out).is_some_and(|l| l > 900)
    });
}

#[test]
fn each_output_is_measured_at_its_own_crop() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::default();
    config.wallpaper.light = Some(dark_topped(dir.path(), "w.png", (600, 600), 60));
    // the square output shows the image's top; the wide one only its middle rows, which are light
    let mut d = Desktop::configured(
        config,
        &[display("square", (600, 600)), display("wide", (1200, 300))],
        1.0,
    );
    let measured = measures(&mut d);
    assert!(measured[0] < 100 && measured[1] > 900, "{measured:?}");
}

#[test]
fn a_quarter_turned_output_is_measured_at_its_logical_top() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::default();
    // portrait, as the turned output's logical space: dark across the top
    config.wallpaper.light = Some(dark_topped(dir.path(), "w.png", (600, 900), 60));
    let mut d = Desktop::configured(config, &[display("turned", (900, 600))], 1.0);
    let id = d.state.outputs[0].id;
    d.state.configure_output(
        id,
        Mode {
            size: (900, 600).into(),
            refresh: 60_000,
        },
        1.0,
        Transform::_90,
        (0, 0),
    );
    let measured = measures(&mut d);
    assert!(measured[0] < 100, "{measured:?}");
}

#[test]
fn an_output_that_is_off_when_its_image_comes_in_holds_nothing_up() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::default();
    config.wallpaper.light = Some(dark_topped(dir.path(), "w.png", (600, 600), 60));
    let mut d = Desktop::configured(
        config,
        &[display("square", (600, 600)), display("wide", (1200, 300))],
        1.0,
    );
    // both outputs ask for the image in their first frame; the second goes off before its answer is put up
    d.at(16);
    let second = d.state.outputs[1].id;
    d.state.set_output_power(second, false).unwrap();
    let end = Instant::now() + common::HANG_GUARD;
    let mut frame = 2;
    while d.state.overlays.wallpaper.pending() || d.state.animating() {
        d.at(frame * 16);
        frame += 1;
        assert!(
            Instant::now() < end,
            "frames are still wanted: pending {}, animating {}",
            d.state.overlays.wallpaper.pending(),
            d.state.animating()
        );
    }
    assert!(
        d.state.backdrop_lightness(second).is_some_and(|l| l > 900),
        "the output that is off is measured too: {:?}",
        d.state.backdrop_lightness(second)
    );
}

#[test]
fn a_quarter_turned_output_draws_the_image_upright() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::default();
    config.wallpaper.light = Some(dark_topped(dir.path(), "w.png", (600, 900), 60));
    let mut d = Desktop::configured(config, &[display("turned", (900, 600))], 1.0);
    let id = d.state.outputs[0].id;
    d.state.configure_output(
        id,
        Mode {
            size: (900, 600).into(),
            refresh: 60_000,
        },
        1.0,
        Transform::_90,
        (0, 0),
    );
    measures(&mut d);
    let shot = dir.path().join("turned.png");
    d.state.screenshot_requests.push(shot.clone());
    d.state.request_redraw();
    let end = Instant::now() + common::HANG_GUARD;
    let mut frame = 1000;
    while !shot.exists() {
        d.at(frame * 16);
        frame += 1;
        assert!(Instant::now() < end, "no screenshot");
    }
    // the screenshot is the output as it is shown, upright: its logical space at the mode's size turned on its side.
    // The image's dark top must be at the top, not a crop of its light middle
    let shot = image::open(&shot).unwrap().to_rgba8();
    assert_eq!(shot.dimensions(), (600, 900));
    // under the bar and below it, in the middle where no shell process draws
    let top = shot.get_pixel(300, 40);
    let below = shot.get_pixel(300, 450);
    assert!(
        top[0] < 60 && below[0] > 200,
        "top {top:?}, below {below:?}"
    );
}

#[test]
fn the_procedural_wallpaper_is_measured_by_its_average_colour_once_per_change() {
    use cupertino_server::render::scene::wallpaper_tint;
    use cupertino_server::render::wallpaper::BarBands;
    // the average colour as it is: the bar draws nothing between the wallpaper and its text
    let under_bar = |appearance: Appearance| {
        BarBands::even(wallpaper_tint(appearance == Appearance::Dark)).lightness()
    };
    let Some(mut h) = Harness::new(|_, _| {}) else {
        return;
    };
    let out = h.state.outputs[0].id;
    let light = under_bar(Appearance::Light);
    assert_ne!(light, under_bar(Appearance::Dark));
    h.wait_until("the measure", |s| s.backdrop_lightness(out) == Some(light));
    // told once: no output is marked for frames again by it
    for _ in 0..20 {
        h.wait_until("a frame", |_| true);
        assert!(!h.state.outputs[0].scheduler.damaged() || h.state.animating());
    }
    h.state.set_appearance(Appearance::Dark);
    let dark = under_bar(Appearance::Dark);
    h.wait_until("the dark measure", |s| {
        s.backdrop_lightness(out) == Some(dark)
    });
}

#[test]
fn an_output_whose_new_image_another_output_took_in_is_marked_for_a_frame() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::default();
    let first = dark_topped(dir.path(), "first.png", (600, 600), 60);
    config.wallpaper.light = Some(first.clone());
    let mut d = Desktop::configured(
        config,
        &[display("one", (600, 600)), display("two", (600, 600))],
        1.0,
    );
    measures(&mut d);
    // a new image for both: each output asks in its frame, then only the first output composes
    let second = dark_topped(dir.path(), "second.png", (600, 600), 0);
    let before = CompositorSettings {
        wallpaper: Some(first.display().to_string()),
        ..CompositorSettings::default()
    };
    let after = CompositorSettings {
        wallpaper: Some(second.display().to_string()),
        ..CompositorSettings::default()
    };
    d.state.apply_compositor_change(&before, &after);
    let two = d.state.outputs[1].id;
    let mut backend = d.state.backend.take().unwrap();
    #[allow(irrefutable_let_patterns)]
    let cupertino_server::backend::Backend::Headless(ref mut renderer) = backend
    else {
        panic!("headless backend required")
    };
    // the second output's frame asks after taking in what was read, so its answer can only come in a later frame
    let mut frame = 1000;
    d.state.fixed_time = Some(f64::from(frame) * 0.016);
    d.state.build_output_elements(&mut renderer.renderer, 0);
    d.state.build_output_elements(&mut renderer.renderer, 1);
    d.state.outputs[1].scheduler.called_back();
    assert!(d.state.overlays.wallpaper.pending());
    let end = Instant::now() + common::HANG_GUARD;
    while d.state.overlays.wallpaper.pending() {
        assert!(Instant::now() < end, "no answer");
        std::thread::sleep(Duration::from_millis(5));
        frame += 1;
        d.state.fixed_time = Some(f64::from(frame) * 0.016);
        d.state.build_output_elements(&mut renderer.renderer, 0);
    }
    d.state.backend = Some(backend);
    assert!(
        d.state.backdrop_lightness(two).is_some_and(|l| l > 900),
        "the second output's new image is put up: {:?}",
        d.state.backdrop_lightness(two)
    );
    assert!(
        d.state.outputs[1].scheduler.damaged(),
        "the second output draws its new image at its next frame"
    );
}
