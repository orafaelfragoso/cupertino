//! One headless display set to several scales in turn, with a mapped window: the output's logical extent follows each
//! scale while its mode stays the panel's pixels, the window keeps its logical frame, the scale windows are asked to
//! draw at (and so their buffers) follows, and panel pixels map to the same logical points the output is drawn with.
//! Needs an offscreen EGL renderer, like the other desktop tests.

use crate::common;

use common::absolute;
use common::desktop::Desktop;
use cupertino_displays::geometry::{raster, scale_in_120ths};
use cupertino_displays::Extent;
use smithay::wayland::compositor::with_states;
use smithay::wayland::fractional_scale::with_fractional_scale;

const PANEL: (i32, i32) = (3024, 1964);

/// The window's frame in points: x, y, width, height.
struct Frame {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

fn frame(d: &Desktop) -> Frame {
    let r = d
        .state
        .wm
        .windows()
        .next()
        .expect("the window")
        .lifecycle
        .target_frame();
    Frame {
        x: r.x,
        y: r.y,
        w: r.w,
        h: r.h,
    }
}

fn set_scale(d: &mut Desktop, scale: f64) {
    let head = &d.state.displays.heads[0];
    let (id, mut state) = (head.id, head.state);
    state.scale = scale;
    d.state
        .configure_heads(&[(id, state)], false)
        .expect("the scale applies");
    d.state.heads_changed();
    for _ in 0..5 {
        d.pump();
    }
}

#[test]
fn a_window_keeps_its_logical_frame_through_several_scale_changes() {
    let mut d = Desktop::sized(PANEL, 2.0);
    d.window();
    for _ in 0..5 {
        d.pump();
    }
    let first = frame(&d);
    let output = d.state.outputs[0].id;
    let mut generation = d.state.outputs[0].scale_generation;
    // the scales of the desktop captures, then the built-in panel's sizes with larger text and more space
    let steps = [
        (1.0, (3024, 1964)),
        (1.25, (2419, 1571)),
        (1.5, (2016, 1309)),
        (1.75, (1728, 1122)),
        (2.0, (1512, 982)),
        (3024.0 / 1024.0, (1024, 665)),
        (3024.0 / 1800.0, (1800, 1169)),
        (2.0, (1512, 982)),
    ];
    for (scale, logical) in steps {
        set_scale(&mut d, scale);
        let geometry = d.state.output_geometry(output).expect("the output");
        assert_eq!(
            geometry.mode,
            Extent::new(3024, 1964),
            "{scale}: the panel keeps its pixels"
        );
        assert_eq!(
            geometry.logical,
            Extent::new(logical.0, logical.1),
            "{scale}"
        );
        let entry = d.state.output_entry(output).unwrap();
        assert_eq!(
            (entry.rect.w, entry.rect.h),
            (f64::from(logical.0), f64::from(logical.1))
        );
        assert!(
            entry.scale_generation > generation,
            "{scale}: the change is signalled"
        );
        generation = entry.scale_generation;

        let now = frame(&d);
        assert_eq!(
            (now.x, now.y, now.w, now.h),
            (first.x, first.y, first.w, first.h),
            "{scale}: the window's frame in points"
        );

        // the window is asked to draw at the output's scale, so its buffer is its points at that scale
        let surface = d.state.windows[0].surface().clone();
        let preferred = with_states(&surface, |s| {
            with_fractional_scale(s, |fs| fs.preferred_scale())
        });
        assert_eq!(preferred, Some(scale), "{scale}");
        let size = Extent::new(now.w as u32, now.h as u32);
        let buffer = raster(size, scale_in_120ths(scale).unwrap()).unwrap();
        let exact = (
            f64::from(size.width) * scale,
            f64::from(size.height) * scale,
        );
        assert!(
            (f64::from(buffer.width) - exact.0).abs() <= scale
                && (f64::from(buffer.height) - exact.1).abs() <= scale,
            "{scale}: a {buffer:?} buffer for {size:?} points"
        );

        // panel pixels map to the points the output is drawn with: the window's corner and the panel's far corner
        let corner = d
            .state
            .output_pixels_to_global(output, (now.x * scale, now.y * scale))
            .unwrap();
        assert!(
            (corner.x - now.x).abs() < 1e-9 && (corner.y - now.y).abs() < 1e-9,
            "{scale}"
        );
        let far = d
            .state
            .output_pixels_to_global(output, (f64::from(PANEL.0), f64::from(PANEL.1)))
            .unwrap();
        assert!(
            (far.x - f64::from(logical.0)).abs() <= 0.5,
            "{scale}: {far:?}"
        );
        assert!(
            (far.y - f64::from(logical.1)).abs() <= 0.5,
            "{scale}: {far:?}"
        );
    }
}

#[test]
fn a_scale_that_leaves_no_logical_size_is_refused_and_changes_nothing() {
    let mut d = Desktop::sized(PANEL, 2.0);
    let head = &d.state.displays.heads[0];
    let (id, state) = (head.id, head.state);
    let output = d.state.outputs[0].id;
    let before = d.state.output_geometry(output).unwrap();
    for scale in [0.0, -2.0, f64::NAN, f64::INFINITY, 1e6] {
        let wanted = cupertino_server::displays::HeadState { scale, ..state };
        assert!(
            d.state.configure_heads(&[(id, wanted)], false).is_err(),
            "{scale}"
        );
        d.pump();
        assert_eq!(d.state.output_geometry(output), Some(before), "{scale}");
    }
}

/// An absolute pointer reports a share of its range; the server spans it over the panel pixels of the output under
/// the pointer and maps them through the output's exact scale, as drawing does, not its rounded logical extent.
#[test]
fn an_absolute_pointer_lands_where_the_output_draws_that_pixel() {
    let mut d = Desktop::sized(PANEL, 2.0);
    // 3024 / 1.7 = 1778.8: the logical extent is rounded, the drawing scale is not
    set_scale(&mut d, 1.7);
    let rect = d.state.outputs[0].rect;
    assert_ne!(rect.w, f64::from(PANEL.0) / 1.7);
    d.state.process_input_event(absolute::motion(0.999, 0.5));
    let at = d.state.pointer.current_location();
    let expected = (
        rect.x + 0.999 * f64::from(PANEL.0) / 1.7,
        rect.y + 0.5 * f64::from(PANEL.1) / 1.7,
    );
    assert!(
        (at.x - expected.0).abs() < 1e-9 && (at.y - expected.1).abs() < 1e-9,
        "{at:?} for {expected:?}"
    );
}

/// With two displays, the device spans the one its pointer is on, not the first.
#[test]
fn an_absolute_pointer_spans_the_output_under_it() {
    let display = |name: &str| cupertino_server::backend::headless::HeadlessDisplay {
        name: name.into(),
        size: (1000, 700),
        physical_mm: (0, 0),
    };
    let mut d = Desktop::with_displays(&[display("headless-1"), display("headless-2")], 1.0);
    let second = d.state.outputs[1].rect;
    let centre = |r: cupertino_wm::Rect| (r.x + r.w / 2.0, r.y + r.h / 2.0);
    let (x, y) = centre(second);
    d.state.pointer_moved_to((x, y).into(), 0, None);
    d.state.process_input_event(absolute::motion(0.25, 0.25));
    let at = d.state.pointer.current_location();
    assert_eq!(
        (at.x, at.y),
        (second.x + 250.0, second.y + 175.0),
        "a quarter of the second output"
    );
}
