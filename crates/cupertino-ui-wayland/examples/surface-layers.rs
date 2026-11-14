//! A window with one surface layer, presented through the toolkit's window system: the window's frame is grey all
//! over, the layer's frame red at [`LAYER`] in the window, and once both are shown the layer fades out on the
//! compositor's clock ([`FADE`]) while its content blur goes up and back down through samples ([`BLUR_SAMPLES`]), and
//! the window draws nothing more. Development check of the surface layers' presenter, run by the server's tests.
//!
//! `surface-layers --report PATH [--renderer]`
//!
//! With `--renderer` (and the GPU path turned on, `CUPERTINO_RENDERER=gpu`) there is no fade: once the process draws
//! on the GPU, the layer's frame is drawn by the toolkit's GPU renderer on the presenter's device, [`GPU_FRAMES`]
//! times, each handed over as a layer frame, which the presenter copies from the renderer's texture into the layer's
//! own buffers.
//!
//! The report has one `key value` line per fact: whether the window system shows surface layers, the window's frames
//! presented, the layer frames handed over (and those the GPU renderer drew), and `faded 1` once the fade was asked
//! for (or the GPU frames are all handed over). It is written then and again when the window closes.

use std::process::ExitCode;
use std::time::{Duration, Instant};

use cupertino_geometry::{Rect, Size};
use cupertino_modifiers::Profile;
use cupertino_ui_gpu::{GpuConfig, WindowRenderer};
use cupertino_ui_platform::{
    DecorationMode, LayerChange, LayerProperty, PlatformEvent, PresentOutcome, RendererKind,
    SurfaceLayer, SurfaceLayerLook, WindowEvent, WindowRequest, WindowSpec, WindowSystem,
};
use cupertino_ui_render::{
    Color, CpuFrame, DamageRegion, DisplayList, Paint, PaintNode, PaintNodeId, PaintOp,
    PaintResources, PaintRevision, PixelFormat, PixelRect, RenderFrameId, RenderRequest, Shape,
    Transform, UiRenderer,
};
use cupertino_ui_text::{GlyphKey, GlyphRasterizer, RasterGlyph, TextError};
use cupertino_ui_wayland::WaylandWindowSystem;

/// The window's size in points, and its layer's rect in it.
const WINDOW: Size = Size::new(240.0, 160.0);
const LAYER: Rect = Rect::new(40.0, 30.0, 120.0, 60.0);
const LAYER_KEY: u64 = 1;
/// The colours of the window and the layer, as 8-bit premultiplied RGBA.
const GREY: [u8; 4] = [128, 128, 128, 255];
const RED: [u8; 4] = [255, 0, 0, 255];
/// The layer's fade, in seconds, linear.
const FADE: f64 = 0.4;
/// The layer's content blur over the fade, in points: up and back down, which no single curve draws.
const BLUR_SAMPLES: [f32; 3] = [0.0, 8.0, 0.0];
/// The layer frames the GPU renderer draws (`--renderer`).
const GPU_FRAMES: u32 = 8;
/// How long the window stays after the fade is asked for.
const LINGER: Duration = Duration::from_millis(1500);
const LIMIT: Duration = Duration::from_secs(30);

fn frame(id: u64, width: u32, height: u32, colour: [u8; 4]) -> CpuFrame {
    CpuFrame {
        id: RenderFrameId(id),
        width,
        height,
        stride: width * 4,
        format: PixelFormat::Rgba8Premultiplied,
        pixels: colour.repeat((width * height) as usize),
        damage: vec![PixelRect {
            x: 0,
            y: 0,
            width,
            height,
        }],
        device: None,
    }
}

/// The toolkit renderer's text needs no glyphs here.
struct NoGlyphs;

impl GlyphRasterizer for NoGlyphs {
    fn rasterize(&mut self, _: &GlyphKey) -> Result<Option<RasterGlyph>, TextError> {
        Ok(None)
    }
}

/// The layer's paint: red all over, in its points.
fn red_list() -> DisplayList {
    let whole = Rect::new(0.0, 0.0, LAYER.width(), LAYER.height());
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
                paint: Paint::Solid(Color::from_rgba8(RED)),
            }],
            layer: None,
        }],
    }
}

/// What the run saw, as the report states it.
#[derive(Default)]
struct Seen {
    frames: u32,
    layer_frames: u32,
    gpu_frames: u32,
    done: bool,
}

fn write(report: &std::path::Path, layers: bool, seen: &Seen) -> Result<(), String> {
    let text = format!(
        "surface_layers {}\nframes {}\nlayer_frames {}\nlayer_gpu_frames {}\nfaded {}\n",
        u8::from(layers),
        seen.frames,
        seen.layer_frames,
        seen.gpu_frames,
        u8::from(seen.done)
    );
    std::fs::write(report, text).map_err(|e| e.to_string())
}

fn run(report: &std::path::Path, on_gpu: bool) -> Result<(), String> {
    let mut ws =
        WaylandWindowSystem::connect(Profile::cupertino()).map_err(|e| format!("{e:?}"))?;
    let layers = ws.capabilities().surface_layers;
    let window = ws
        .create_window(&WindowSpec {
            title: "Surface layers".into(),
            app_id: "surface-layers".into(),
            size: WINDOW,
            min_size: None,
            max_size: None,
            decorations: DecorationMode::None,
            foreign_parent: None,
            input_region: None,
        })
        .map_err(|e| format!("{e:?}"))?;
    let mut scale = 1.0_f64;
    let mut seen = Seen::default();
    let mut finished: Option<Instant> = None;
    // the latest configure and size, drawn once both the window and its layer were shown
    let mut configure = None;
    let mut size = WINDOW;
    let mut drawing: Option<WindowRenderer<NoGlyphs>> = None;
    let start = Instant::now();
    let mut events = Vec::new();
    loop {
        if start.elapsed() > LIMIT {
            return Err("timed out".into());
        }
        if finished.is_some_and(|t| t.elapsed() > LINGER) {
            return write(report, layers, &seen);
        }
        events.clear();
        ws.dispatch(Some(Duration::from_millis(10)), &mut events)
            .map_err(|e| format!("{e:?}"))?;
        let mut frame_due = false;
        for event in events.drain(..) {
            match event {
                PlatformEvent::RendererChanged(RendererKind::Gpu) if on_gpu => {
                    drawing = ws.gpu_renderers().renderer(GpuConfig::default(), NoGlyphs);
                }
                PlatformEvent::RendererChanged(RendererKind::Cpu) => drawing = None,
                PlatformEvent::Window { event, .. } => match event {
                    WindowEvent::Scale(s) => scale = s,
                    WindowEvent::Configure(c) => {
                        size = c.size.unwrap_or(WINDOW);
                        configure = Some(c.id);
                    }
                    WindowEvent::Frame(_) => frame_due = true,
                    WindowEvent::CloseRequested => {
                        seen.done = finished.is_some();
                        return write(report, layers, &seen);
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        let px = |v: f64| (v * scale).round() as u32;
        if seen.frames > 0 && finished.is_none() && frame_due {
            if !on_gpu {
                // the first frame is shown: fade the layer out, drawing nothing
                if layers {
                    ws.request(
                        window,
                        WindowRequest::Layer {
                            key: LAYER_KEY,
                            changes: vec![
                                LayerChange::new(
                                    LayerProperty::Opacity,
                                    0.0,
                                    Some(cupertino_ui_platform::LayerCurve::Timed {
                                        duration: FADE,
                                        bezier: cupertino_ui_platform::CubicBezier::new(
                                            0.0, 0.0, 1.0, 1.0,
                                        ),
                                    }),
                                ),
                                LayerChange::through(
                                    LayerProperty::ContentBlur,
                                    FADE,
                                    BLUR_SAMPLES.into(),
                                ),
                            ],
                        },
                    )
                    .map_err(|e| format!("{e:?}"))?;
                }
                finished = Some(Instant::now());
                seen.done = true;
                write(report, layers, &seen)?;
                continue;
            }
            // on the GPU: the layer drawn by the GPU renderer, the window's frame again for the next frame callback
            if let Some(r) = drawing.as_mut() {
                let (w, h) = (px(LAYER.width()), px(LAYER.height()));
                let id = r
                    .render(&RenderRequest {
                        list: &red_list(),
                        resources: &PaintResources::default(),
                        scale,
                        width: w,
                        height: h,
                        damage: &DamageRegion::Full,
                    })
                    .map_err(|e| format!("the GPU renderer: {e:?}"))?;
                let drawn = r.frame(id).map_err(|e| format!("its frame: {e:?}"))?;
                let shown = ws
                    .present_surface_layers(
                        window,
                        &[SurfaceLayer {
                            key: LAYER_KEY,
                            rect: LAYER,
                            look: SurfaceLayerLook::default(),
                            frame: Some(drawn),
                        }],
                    )
                    .map_err(|e| format!("{e:?}"))?;
                if shown == PresentOutcome::Presented && drawn.device.is_some() {
                    seen.gpu_frames += 1;
                }
                if seen.gpu_frames >= GPU_FRAMES {
                    finished = Some(Instant::now());
                    seen.done = true;
                    write(report, layers, &seen)?;
                }
            }
            let content = frame(3, px(size.width), px(size.height), GREY);
            ws.present(window, &content, None)
                .map_err(|e| format!("{e:?}"))?;
            ws.request_frame(window);
            continue;
        }
        // drawn until the window's frame and the layer's are both shown, at the window's latest scale
        let Some(id) = configure.filter(|_| seen.frames == 0) else {
            continue;
        };
        let content = frame(1, px(size.width), px(size.height), GREY);
        let red = frame(2, px(LAYER.width()), px(LAYER.height()), RED);
        if layers && seen.layer_frames == 0 {
            let shown = ws
                .present_surface_layers(
                    window,
                    &[SurfaceLayer {
                        key: LAYER_KEY,
                        rect: LAYER,
                        look: SurfaceLayerLook::default(),
                        frame: Some(&red),
                    }],
                )
                .map_err(|e| format!("{e:?}"))?;
            if shown != PresentOutcome::Presented {
                continue;
            }
            seen.layer_frames += 1;
        }
        let shown = ws
            .present(window, &content, Some(id))
            .map_err(|e| format!("{e:?}"))?;
        if shown == PresentOutcome::Presented {
            seen.frames += 1;
            ws.request_frame(window);
        }
    }
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let (mut report, mut on_gpu) = (None, false);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--report" => report = args.next().map(std::path::PathBuf::from),
            "--renderer" => on_gpu = true,
            _ => report = None,
        }
    }
    let Some(report) = report else {
        eprintln!("usage: surface-layers --report PATH [--renderer]");
        return ExitCode::from(2);
    };
    match run(&report, on_gpu) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("surface-layers: {e}");
            ExitCode::FAILURE
        }
    }
}
