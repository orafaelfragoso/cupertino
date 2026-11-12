//! The server-drawn cursor: its size follows the user cursor scale, and shaking the pointer grows
//! it five times until the shaking stops. The compositor runs in-process on an
//! offscreen EGL device; tests skip (pass) when no EGL device is available.

use crate::common;

use std::time::Duration;

use cupertino_server::backend::headless;
use cupertino_server::config::Config;
use cupertino_server::render::CupertinoElement;
use cupertino_server::Cupertino;
use smithay::backend::renderer::element::Element;
use smithay::input::pointer::CursorIcon;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use smithay::utils::Scale;

const OUTPUT: (i32, i32) = (1000, 700);

struct Harness {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
}

impl Harness {
    fn new(tweak: impl FnOnce(&mut Config)) -> Option<Harness> {
        Self::with_scale(1.0, tweak)
    }

    fn with_scale(scale: f64, tweak: impl FnOnce(&mut Config)) -> Option<Harness> {
        let event_loop: EventLoop<'static, Cupertino> =
            EventLoop::try_new().expect("creating the event loop");
        let display: Display<Cupertino> = Display::new().expect("creating the Wayland display");
        let mut cfg = Config::default();
        tweak(&mut cfg);
        let mut state = Cupertino::new(
            display,
            event_loop.handle(),
            event_loop.get_signal(),
            cfg,
            true,
            &cupertino_assets::Assets::builtin(),
        )
        .expect("creating the compositor");
        match headless::init(&mut state, OUTPUT, scale) {
            Ok(()) => {
                let t = state.now();
                state.fixed_time = Some(t);
                Some(Harness { state, event_loop })
            }
            Err(e) => {
                common::egl_unavailable(format!("{e:#}"));
                None
            }
        }
    }

    /// Move the pointer to `x`, 10 ms of model time after the previous motion.
    fn move_to(&mut self, x: f64) {
        let t = self.state.now() + 0.01;
        let time = t;
        common::set_time(&mut self.state, &mut self.event_loop, time);
        self.state
            .pointer_moved_to((x, 300.0).into(), (t * 1000.0) as u32, None);
    }

    /// Width and height of the drawn cursor, pixels: the topmost element of the output.
    fn cursor_size(&mut self) -> (i32, i32) {
        let mut backend = self.state.backend.take().unwrap();
        let elements = self.state.build_output_elements(backend.renderer(), 0);
        self.state.backend = Some(backend);
        let cursor = elements.first().expect("the cursor is drawn");
        assert!(
            matches!(cursor, CupertinoElement::Image(_)),
            "the topmost element is the themed cursor"
        );
        let g = cursor.geometry(Scale::from(1.0));
        (g.size.w, g.size.h)
    }

    /// Shake horizontally between x = 100 and 400 in 50 pt steps every 10 ms for `strokes` strokes.
    fn shake(&mut self, strokes: u32) {
        let mut x = 100.0;
        for stroke in 0..strokes {
            let dir = if stroke % 2 == 0 { 1.0 } else { -1.0 };
            for _ in 0..6 {
                x += 50.0 * dir;
                self.move_to(x);
            }
        }
    }
}

/// Within 3 %: a theme rasterises each nominal size on its own (shadow padding included), so
/// the image a size picks is not exactly proportional to the size.
fn close(a: (i32, i32), b: (f64, f64)) -> bool {
    (f64::from(a.0) - b.0).abs() <= 0.03 * b.0 && (f64::from(a.1) - b.1).abs() <= 0.03 * b.1
}

#[test]
fn shaking_the_pointer_grows_the_cursor_five_times_and_it_shrinks_back_once_still() {
    let Some(mut h) = Harness::new(|_| {}) else {
        return;
    };
    h.move_to(250.0);
    let normal = h.cursor_size();

    h.shake(11);
    assert_eq!(
        h.state.shake_multiplier(),
        5.0,
        "ten reversals of a line shake"
    );
    let grown = h.cursor_size();
    assert!(
        close(
            grown,
            (f64::from(normal.0) * 5.0, f64::from(normal.1) * 5.0)
        ),
        "{grown:?} is five times {normal:?}"
    );

    // still: from 0.1 s after the last reversal every evaluation shrinks the cursor a step
    let time = h.state.now() + 0.15;
    common::set_time(&mut h.state, &mut h.event_loop, time);
    let mut steps = 0;
    while h.state.evaluate_cursor_shake() {
        steps += 1;
        assert!(steps < 10, "the multiplier decays to 1");
    }
    assert_eq!(steps, 4, "5 → 4 → 3 → 2 → 1.5 → 1");
    assert_eq!(h.cursor_size(), normal);
    // the evaluations' timer stops by itself once the cursor is back to normal
    for _ in 0..5 {
        headless::step(&mut h.state, &mut h.event_loop, Duration::from_millis(20)).unwrap();
    }
    assert_eq!(h.state.shake_multiplier(), 1.0);
}

#[test]
fn the_cursor_size_setting_scales_the_cursor_and_shaking_multiplies_it() {
    let Some(mut normal) = Harness::new(|_| {}) else {
        return;
    };
    normal.move_to(250.0);
    let base = normal.cursor_size();
    let Some(mut h) = Harness::new(|c| c.pointer.cursor_scale = 2.0) else {
        return;
    };
    h.move_to(250.0);
    let doubled = h.cursor_size();
    assert!(
        close(doubled, (f64::from(base.0) * 2.0, f64::from(base.1) * 2.0)),
        "{doubled:?} is twice {base:?}"
    );
    h.shake(11);
    let grown = h.cursor_size();
    assert!(
        close(grown, (f64::from(base.0) * 10.0, f64::from(base.1) * 10.0)),
        "{grown:?} is ten times {base:?}"
    );
}

#[test]
fn shaking_does_nothing_when_shake_to_locate_is_off() {
    let Some(mut h) = Harness::new(|c| c.pointer.shake_to_locate = false) else {
        return;
    };
    h.shake(11);
    assert_eq!(h.state.shake_multiplier(), 1.0);
}

/// The theme image is drawn whole at any output scale. The image picked for a scale-2 output is
/// larger than the cursor's size in points (48 px for 24 pt); the element has to sample all of it,
/// not the top-left `size`-point corner (the arrow showed only its lower-right part).
#[test]
fn the_themed_cursor_samples_the_whole_image_on_a_scale_2_output() {
    let Some(mut h) = Harness::with_scale(2.0, |_| {}) else {
        return;
    };
    h.move_to(250.0);
    let t = std::time::Duration::from(h.state.clock.now());
    let image = h
        .state
        .cursor_loader
        .frame(CursorIcon::Default, 48, t)
        .pixels;
    let mut backend = h.state.backend.take().unwrap();
    let elements = h.state.build_output_elements(backend.renderer(), 0);
    h.state.backend = Some(backend);
    let cursor = elements.first().expect("the cursor is drawn");
    assert!(
        matches!(cursor, CupertinoElement::Image(_)),
        "the topmost element is the themed cursor"
    );
    let src = cursor.src();
    assert_eq!((src.loc.x, src.loc.y), (0.0, 0.0));
    assert_eq!(
        (src.size.w, src.size.h),
        (f64::from(image.0), f64::from(image.1))
    );
}
