//! Image wallpapers against a headless compositor: an image covers the output, the dark appearance takes its own image,
//! a change of `wallpaper` in `compositor.json` applies live, and an image that is not there leaves the procedural
//! wallpaper. Tests skip (pass) when no EGL device is available.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cupertino_defaults::compositor::CompositorSettings;
use cupertino_server::backend::headless;
use cupertino_server::config::Config;
use cupertino_server::Cupertino;
use cupertino_theme::Appearance;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;

const OUTPUT: (i32, i32) = (800, 600);

struct Harness {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
    dir: tempfile::TempDir,
}

impl Harness {
    fn new(tweak: impl FnOnce(&mut Config, &Path)) -> Option<Harness> {
        let event_loop: EventLoop<'static, Cupertino> = EventLoop::try_new().ok()?;
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        tweak(&mut config, dir.path());
        let mut state = Cupertino::new(
            Display::new().ok()?,
            event_loop.handle(),
            event_loop.get_signal(),
            config,
            true,
            &cupertino_assets::Assets::builtin(),
        )
        .ok()?;
        if let Err(e) = headless::init(&mut state, OUTPUT, 1.0) {
            eprintln!("skipping: no EGL device ({e:#})");
            return None;
        }
        Some(Harness {
            state,
            event_loop,
            dir,
        })
    }

    fn wait_until(&mut self, what: &str, mut cond: impl FnMut(&Cupertino) -> bool) {
        let end = Instant::now() + Duration::from_secs(10);
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
        let path = self
            .dir
            .path()
            .join(format!("shot-{}.png", self.state.screenshot_requests.len()));
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
