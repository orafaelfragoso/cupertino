//! The process's render thread draws the windows of every instance on its one device.

use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use cupertino_geometry::Rect;
use cupertino_ui_render::gpu::{GpuConfig, GpuRenderer, WindowFrame, WindowRenderer};
use cupertino_ui_render::render_thread::{self, Resident, Status, RENDER_THREAD};
use cupertino_ui_render::{
    Color, DamageRegion, DisplayList, Paint, PaintNode, PaintNodeId, PaintOp, PaintResources,
    PaintRevision, RenderRequest, Shape, Transform, UiRenderer,
};
use cupertino_ui_text::{GlyphKey, GlyphRasterizer, RasterGlyph, TextError};

/// The windows here have no text.
struct NoGlyphs;

impl GlyphRasterizer for NoGlyphs {
    fn rasterize(&mut self, _: &GlyphKey) -> Result<Option<RasterGlyph>, TextError> {
        Ok(None)
    }
}

/// A run without a device fails only when the environment says one must be there.
fn required() -> bool {
    ["CUPERTINO_GPU_REQUIRED", "CUPERTINO_EGL_REQUIRED"]
        .iter()
        .any(|v| std::env::var_os(v).is_some_and(|v| v == "1"))
}

/// A window of `size` filled with `rgba`.
fn filled(size: (u32, u32), rgba: [u8; 4]) -> DisplayList {
    let whole = Rect::new(0.0, 0.0, f64::from(size.0), f64::from(size.1));
    DisplayList {
        nodes: vec![PaintNode {
            id: PaintNodeId(1),
            parent: None,
            revision: PaintRevision(1),
            transform: Transform::IDENTITY,
            clip: None,
            group: None,
            bounds: whole,
            hit: whole,
            ops: vec![PaintOp::Fill {
                shape: Shape::Rect(whole),
                paint: Paint::Solid(Color::from_rgba8(rgba)),
            }],
            layer: None,
        }],
    }
}

/// What one instance's UI thread saw of the frame it drew.
struct Drawn {
    pixels: Vec<u8>,
    /// The device its frame's texture is on.
    device: usize,
    /// The render thread's device then.
    render_threads: usize,
    /// The name of the thread that drew it.
    thread: Option<String>,
}

/// On a UI thread of its own: a window's renderer on the process's device `generation`, one frame of `rgba` drawn.
fn instance(generation: u64, rgba: [u8; 4]) -> Drawn {
    let size = (37, 23);
    let renderer = render_thread::run(move |on| {
        let device = on.device_of(generation).expect("the process's device");
        let programs = on.programs().expect("its programs");
        let renderer = GpuRenderer::with_programs(
            Rc::clone(device),
            Rc::clone(programs),
            GpuConfig::default(),
            NoGlyphs,
        );
        Resident::new(on, renderer)
    });
    let mut window = WindowRenderer::new(renderer);
    let id = window
        .render(&RenderRequest {
            list: &filled(size, rgba),
            resources: &PaintResources::default(),
            scale: 1.0,
            width: size.0,
            height: size.1,
            damage: &DamageRegion::Full,
        })
        .expect("a frame");
    let frame = window.frame(id).expect("the frame drawn");
    let on_device = frame
        .device
        .as_ref()
        .and_then(|d| d.0.as_any().downcast_ref::<WindowFrame>())
        .expect("its image stays on the device");
    let (device, render_threads, thread) = render_thread::run(|on| {
        (
            Rc::as_ptr(on_device.texture(on).device()) as usize,
            on.device().map_or(0, |d| Rc::as_ptr(d) as usize),
            std::thread::current().name().map(str::to_owned),
        )
    });
    let pixels = frame.in_memory().expect("read back").pixels.clone();
    Drawn {
        pixels,
        device,
        render_threads,
        thread,
    }
}

#[test]
fn the_windows_of_two_instances_are_drawn_by_the_one_render_thread_on_its_one_device() {
    let cache = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("render-thread-programs");
    let claim = render_thread::request(None, Some(&cache), || {});
    let ready = match render_thread::wait(Duration::from_secs(60)) {
        Status::Ready(ready) => ready,
        other => {
            assert!(!required(), "no GPU device: {other:?}");
            eprintln!("skipped: no GPU device ({other:?})");
            return;
        }
    };
    let colours = [[200, 40, 10, 255], [10, 90, 220, 255]];
    let drawn: Vec<Drawn> = std::thread::scope(|s| {
        let threads: Vec<_> = colours
            .iter()
            .map(|&rgba| s.spawn(move || instance(ready.generation, rgba)))
            .collect();
        threads.into_iter().map(|t| t.join().unwrap()).collect()
    });
    for (drawn, rgba) in drawn.iter().zip(colours) {
        assert_eq!(drawn.thread.as_deref(), Some(RENDER_THREAD));
        assert_eq!(
            drawn.device, drawn.render_threads,
            "drawn on the render thread's device"
        );
        assert!(
            drawn.pixels.as_chunks::<4>().0.iter().all(|px| *px == rgba),
            "each window shows its own frame"
        );
    }
    assert_eq!(
        drawn[0].device, drawn[1].device,
        "one device for both instances"
    );
    match render_thread::status() {
        Status::Ready(now) => assert_eq!(now.generation, ready.generation, "no second device"),
        other => panic!("the device went while claimed: {other:?}"),
    }
    drop(claim);
    assert!(
        matches!(render_thread::status(), Status::Idle),
        "the last claim lets the device go"
    );
}
