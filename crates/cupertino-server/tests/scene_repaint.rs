//! Repainting only what changed gives the picture of repainting everything. A frame is drawn twice from the same
//! elements: incrementally, into the buffer the previous frame left (one frame of buffer age, through the output's own
//! damage tracker), and in full, into a fresh buffer. The two must be the same picture after every kind of change the
//! scene can make: nothing, a window moving, a window going, a window animating its opacity and corners, a material
//! over what changed, and an output changing scale. The pictures may differ by the goldens' 2/255 per channel: a
//! translucent edge drawn over the previous frame's pixels is rounded once more than the same edge drawn over the
//! cleared buffer (a pixel of a shadow edge differs by one step in one channel); stale or missing pixels differ by far
//! more.

use crate::common;

use common::desktop::Desktop;
use cupertino_server::backend::{headless, Backend};
use cupertino_server::render::screenshot::render_to_image;
use cupertino_wm::Command;
use image::RgbaImage;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::backend::renderer::{Bind, ExportMem, Offscreen};
use smithay::utils::{Buffer, Rectangle, Size, Transform};

/// The goldens' tolerance per colour channel.
const CHANNEL_TOLERANCE: u8 = 2;

/// An output's buffer and damage tracker, kept from frame to frame.
struct Buffered {
    texture: GlesTexture,
    tracker: OutputDamageTracker,
    size: (i32, i32),
    drawn: bool,
}

impl Buffered {
    fn new(renderer: &mut GlesRenderer, size: (i32, i32), scale: f64) -> Self {
        let buf: Size<i32, Buffer> = size.into();
        Self {
            texture: renderer.create_buffer(Fourcc::Abgr8888, buf).unwrap(),
            tracker: OutputDamageTracker::new(size, scale, Transform::Normal),
            size,
            drawn: false,
        }
    }

    /// Draw `elements` over the previous frame: the first frame has no age, the next ones one.
    fn draw(
        &mut self,
        renderer: &mut GlesRenderer,
        elements: &[cupertino_server::render::CupertinoElement],
    ) -> RgbaImage {
        let age = usize::from(self.drawn);
        self.drawn = true;
        let buf: Size<i32, Buffer> = self.size.into();
        let mut target = renderer.bind(&mut self.texture).unwrap();
        self.tracker
            .render_output(renderer, &mut target, age, elements, headless::CLEAR)
            .unwrap();
        let mapping = renderer
            .copy_framebuffer(&target, Rectangle::from_size(buf), Fourcc::Abgr8888)
            .unwrap();
        let bytes = renderer.map_texture(&mapping).unwrap().to_vec();
        RgbaImage::from_raw(self.size.0 as u32, self.size.1 as u32, bytes).unwrap()
    }
}

/// Build the elements of output `idx` once and draw them both ways.
fn both_ways(d: &mut Desktop, buffered: &mut Buffered, idx: usize, what: &str) {
    let mut backend = d.state.backend.take().unwrap();
    #[allow(irrefutable_let_patterns)]
    let Backend::Headless(ref mut h) = backend
    else {
        panic!("headless backend required")
    };
    let elements = d.state.build_output_elements(&mut h.renderer, idx);
    let scale = d.state.outputs[idx].scale;
    let incremental = buffered.draw(&mut h.renderer, &elements);
    let full = render_to_image(
        &mut h.renderer,
        &elements,
        buffered.size.into(),
        scale,
        headless::CLEAR,
    )
    .unwrap();
    d.state.backend = Some(backend);
    let differing: Vec<_> = incremental
        .enumerate_pixels()
        .zip(full.pixels())
        .filter(|((_, _, a), b)| {
            a.0.iter()
                .zip(b.0)
                .any(|(a, b)| a.abs_diff(b) > CHANNEL_TOLERANCE)
        })
        .map(|((x, y, a), b)| (x, y, a.0, b.0))
        .take(5)
        .collect();
    assert!(
        differing.is_empty(),
        "{what}: pixels beyond the tolerance after an incremental repaint {differing:?}"
    );
}

fn start(scale: f64, windows: usize) -> (Desktop, Buffered) {
    let mut d = Desktop::new(scale);
    for _ in 0..windows {
        d.window();
    }
    d.at(3000);
    let mode = d.state.outputs[0].output.current_mode().unwrap().size;
    let mut backend = d.state.backend.take().unwrap();
    #[allow(irrefutable_let_patterns)]
    let Backend::Headless(ref mut h) = backend
    else {
        panic!("headless backend required")
    };
    let buffered = Buffered::new(&mut h.renderer, (mode.w, mode.h), scale);
    d.state.backend = Some(backend);
    (d, buffered)
}

#[test]
fn an_unchanged_scene_repaints_to_itself() {
    for scale in [1.0, 1.5] {
        let (mut d, mut b) = start(scale, 2);
        both_ways(&mut d, &mut b, 0, "first frame");
        for step in 0..3 {
            d.at(3016 + step * 16);
            both_ways(&mut d, &mut b, 0, "unchanged");
        }
    }
}

#[test]
fn a_moving_window_repaints_to_the_full_picture() {
    let (mut d, mut b) = start(1.0, 2);
    both_ways(&mut d, &mut b, 0, "first frame");
    let id = d.state.wm.key_window().unwrap();
    d.state.command(Command::ToggleZoom { window: id });
    let t0 = d.state.now();
    for step in 1..=8 {
        common::set_time(&mut d.state, &mut d.event_loop, t0 + f64::from(step) * 0.04);
        d.state.tick();
        d.pump();
        both_ways(&mut d, &mut b, 0, &format!("zoom step {step}"));
    }
}

#[test]
fn a_window_going_away_repaints_to_the_full_picture() {
    let (mut d, mut b) = start(1.0, 2);
    both_ways(&mut d, &mut b, 0, "first frame");
    d.clients.pop();
    let deadline = std::time::Instant::now() + common::HANG_GUARD;
    while d.state.wm.windows().count() != 1 {
        d.pump();
        assert!(
            std::time::Instant::now() < deadline,
            "the window did not go"
        );
    }
    d.at(3100);
    both_ways(&mut d, &mut b, 0, "after the window went");
}

#[test]
fn a_window_animating_opacity_and_corners_repaints_to_the_full_picture() {
    let mut d = Desktop::new(1.5);
    let mode = d.state.outputs[0].output.current_mode().unwrap().size;
    let mut backend = d.state.backend.take().unwrap();
    #[allow(irrefutable_let_patterns)]
    let Backend::Headless(ref mut h) = backend
    else {
        panic!("headless backend required")
    };
    let mut b = Buffered::new(&mut h.renderer, (mode.w, mode.h), 1.5);
    d.state.backend = Some(backend);
    d.window();
    // the opening animation fades and scales the window with its rounded corners
    for ms in [0u32, 30, 60, 90, 150, 250, 400, 800] {
        d.at(ms);
        both_ways(&mut d, &mut b, 0, &format!("opening at {ms} ms"));
    }
}

#[test]
fn a_material_over_changing_content_repaints_to_the_full_picture() {
    let (mut d, mut b) = start(1.0, 2);
    d.state.accessibility.settings.color_filter =
        Some(cupertino_defaults::accessibility::ColorFilter::Grayscale);
    both_ways(&mut d, &mut b, 0, "first frame with the filter");
    let id = d.state.wm.key_window().unwrap();
    d.state.command(Command::ToggleZoom { window: id });
    let t0 = d.state.now();
    for step in 1..=6 {
        common::set_time(&mut d.state, &mut d.event_loop, t0 + f64::from(step) * 0.05);
        d.state.tick();
        d.pump();
        both_ways(&mut d, &mut b, 0, &format!("filter over zoom step {step}"));
    }
}

#[test]
fn outputs_changing_scale_repaint_to_the_full_picture() {
    let display = |name: &str, size| headless::HeadlessDisplay {
        name: name.into(),
        size,
        physical_mm: (0, 0),
    };
    let mut d = Desktop::with_displays(
        &[
            display("headless-1", (1000, 700)),
            display("headless-2", (800, 600)),
        ],
        1.0,
    );
    d.window();
    d.at(3000);
    let mut buffers: Vec<Buffered> = Vec::new();
    {
        let mut backend = d.state.backend.take().unwrap();
        #[allow(irrefutable_let_patterns)]
        let Backend::Headless(ref mut h) = backend
        else {
            panic!("headless backend required")
        };
        for idx in 0..2 {
            let mode = d.state.outputs[idx].output.current_mode().unwrap().size;
            buffers.push(Buffered::new(
                &mut h.renderer,
                (mode.w, mode.h),
                d.state.outputs[idx].scale,
            ));
        }
        d.state.backend = Some(backend);
    }
    for (idx, buffer) in buffers.iter_mut().enumerate() {
        both_ways(&mut d, buffer, idx, "both outputs at 1x");
    }
    // the second output changes scale; the buffer and the tracker follow as the compositor's own do
    let id = d.state.outputs[1].id;
    let mode = d.state.outputs[1].output.current_mode().unwrap();
    d.state
        .configure_output(id, mode, 1.5, Transform::Normal, (1000, 0));
    {
        let mut backend = d.state.backend.take().unwrap();
        #[allow(irrefutable_let_patterns)]
        let Backend::Headless(ref mut h) = backend
        else {
            panic!("headless backend required")
        };
        buffers[1] = Buffered::new(&mut h.renderer, (mode.size.w, mode.size.h), 1.5);
        d.state.backend = Some(backend);
    }
    d.at(3100);
    for (idx, buffer) in buffers.iter_mut().enumerate() {
        both_ways(&mut d, buffer, idx, "after the second output changed scale");
    }
    d.at(3200);
    for (idx, buffer) in buffers.iter_mut().enumerate() {
        both_ways(&mut d, buffer, idx, "settled after the scale change");
    }
}

#[test]
fn each_due_render_pass_counts_one_compose_render_and_callback_round() {
    let mut d = Desktop::new(1.0);
    d.window();
    d.at(3000);
    let before = d.state.outputs[0].scheduler.counts();
    d.state.request_redraw();
    headless::render(&mut d.state);
    d.state.request_redraw();
    headless::render(&mut d.state);
    headless::render(&mut d.state);
    let after = d.state.outputs[0].scheduler.counts();
    assert_eq!(after.composes - before.composes, 2);
    assert_eq!(after.renders - before.renders, 2);
    assert_eq!(after.callbacks - before.callbacks, 2);
    assert_eq!(
        (after.empty - before.empty) + (after.queued - before.queued),
        2,
        "every render is either empty or handed to the display: {after:?}"
    );
}
