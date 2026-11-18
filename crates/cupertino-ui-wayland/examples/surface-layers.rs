//! A window with one surface layer, submitted to the toolkit's window system ([`WindowSystem::submit`]), which draws
//! the window's frame and the layer's away from the UI thread and commits the layer before the window: the window's
//! frame is grey all over, the layer's frame red at [`LAYER`] in the window, and once both are shown the layer fades
//! out on the compositor's clock ([`FADE`]) while its content blur goes up and back down through samples
//! ([`BLUR_SAMPLES`]), and the window draws nothing more. Development check of the surface layers' presenter.
//!
//! `surface-layers --report PATH [--renderer]`
//!
//! With `--renderer` (and the GPU path turned on, `CUPERTINO_RENDERER=gpu`) there is no fade: once the process draws
//! on the GPU, the layer's frame is drawn by the toolkit's GPU renderer on the render thread, [`GPU_FRAMES`] times,
//! each with a window frame, and copied from the renderer's texture into the layer's own buffers.
//!
//! The report has one `key value` line per fact: whether the window system shows surface layers, the window's frames
//! shown, the layer frames shown with them (and those the GPU renderer drew), and `faded 1` once the fade was asked
//! for (or the GPU frames are all shown). It is written then and again when the window closes.

use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cupertino_geometry::{Rect, Size};
use cupertino_keyboard::modifiers::Profile;
use cupertino_ui_core::platform::{
    DecorationMode, LayerChange, LayerList, LayerProperty, PlatformEvent, RaisedChange,
    RendererKind, SharedRenderer, Shown, Submission, Submitted, SubmittedLayer, SurfaceLayerLook,
    WindowEvent, WindowRequest, WindowSpec, WindowSystem,
};
use cupertino_ui_render::gpu::GpuConfig;
use cupertino_ui_render::{
    Color, CpuFrame, DamageRegion, DisplayList, Paint, PaintNode, PaintNodeId, PaintOp,
    PaintResources, PaintRevision, PixelFormat, PixelRect, RenderError, RenderFrameId,
    RenderRequest, Shape, Transform, UiRenderer,
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

/// A renderer that fills every frame with one colour, as the CPU renderer hands frames over.
struct Fill {
    colour: [u8; 4],
    frame: Option<CpuFrame>,
}

impl UiRenderer for Fill {
    type Frame = CpuFrame;

    fn render(&mut self, request: &RenderRequest<'_>) -> Result<RenderFrameId, RenderError> {
        let id = RenderFrameId(self.frame.as_ref().map_or(1, |f| f.id.0 + 1));
        let (width, height) = (request.width, request.height);
        self.frame = Some(CpuFrame {
            id,
            width,
            height,
            stride: width * 4,
            format: PixelFormat::Rgba8Premultiplied,
            pixels: self.colour.repeat((width * height) as usize),
            damage: vec![PixelRect {
                x: 0,
                y: 0,
                width,
                height,
            }],
            device: None,
        });
        Ok(id)
    }

    fn frame(&self, id: RenderFrameId) -> Result<&CpuFrame, RenderError> {
        self.frame
            .as_ref()
            .filter(|f| f.id == id)
            .ok_or(RenderError::UnknownFrame(id))
    }
}

fn fill(colour: [u8; 4]) -> SharedRenderer {
    Arc::new(Mutex::new(Box::new(Fill {
        colour,
        frame: None,
    })))
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

/// The window's frame at `scale` and `size`, with the layer drawn by `layer` when there is one.
fn submission(
    window: &SharedRenderer,
    layer: Option<&SharedRenderer>,
    scale: f64,
    size: Size,
) -> Submission {
    let px = |v: f64| (v * scale).round() as u32;
    let surface_layers = layer.map(|renderer| {
        vec![SubmittedLayer {
            key: LAYER_KEY,
            rect: LAYER,
            look: SurfaceLayerLook::default(),
            layer: Some(LayerList {
                renderer: Arc::clone(renderer),
                list: Arc::new(red_list()),
                width: px(LAYER.width()),
                height: px(LAYER.height()),
                damage: DamageRegion::Full,
            }),
        }]
    });
    Submission {
        renderer: Arc::clone(window),
        list: Arc::new(DisplayList::default()),
        resources: Arc::new(PaintResources::default()),
        scale,
        width: px(size.width),
        height: px(size.height),
        damage: DamageRegion::Full,
        raised: RaisedChange::Kept,
        surface_layers,
    }
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
    let grey = fill(GREY);
    let red = fill(RED);
    let mut scale = 1.0_f64;
    let mut seen = Seen::default();
    let mut finished: Option<Instant> = None;
    // the latest size, drawn once the window is configured; a frame away (whether its layer is the GPU renderer's) is
    // shown before the next is submitted
    let mut configured = false;
    let mut size = WINDOW;
    let mut away: Option<bool> = None;
    let mut due = false;
    let mut drawing: Option<SharedRenderer> = None;
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
        for event in events.drain(..) {
            match event {
                PlatformEvent::RendererChanged(RendererKind::Gpu) if on_gpu => {
                    drawing = ws
                        .gpu_renderers()
                        .renderer(GpuConfig::default(), NoGlyphs)
                        .map(|r| -> SharedRenderer { Arc::new(Mutex::new(Box::new(r))) });
                }
                PlatformEvent::RendererChanged(RendererKind::Cpu) => drawing = None,
                PlatformEvent::Window { event, .. } => match event {
                    WindowEvent::Scale(s) => scale = s,
                    WindowEvent::Configure(c) => {
                        size = c.size.unwrap_or(WINDOW);
                        configured = true;
                        due |= seen.frames == 0;
                    }
                    WindowEvent::Frame(_) => due = true,
                    WindowEvent::Shown(shown) => {
                        if let (Shown::Presented(_), Some(with_gpu)) = (shown, away) {
                            seen.frames += 1;
                            seen.layer_frames += u32::from(layers);
                            seen.gpu_frames += u32::from(with_gpu);
                        }
                        away = None;
                        ws.request_frame(window);
                    }
                    WindowEvent::CloseRequested => {
                        seen.done = finished.is_some();
                        return write(report, layers, &seen);
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        if away.is_some() || !configured || !due || finished.is_some() {
            continue;
        }
        if seen.frames > 0 {
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
                                    Some(cupertino_theme::animation::Easing::LINEAR.over(FADE)),
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
            if seen.gpu_frames >= GPU_FRAMES {
                finished = Some(Instant::now());
                seen.done = true;
                write(report, layers, &seen)?;
                continue;
            }
            // on the GPU: the layer drawn by the GPU renderer with each window frame, once the process draws there
            let Some(gpu) = drawing.as_ref() else {
                continue;
            };
            let frame = submission(&grey, layers.then_some(gpu), scale, size);
            submit(&mut ws, window, &frame, &mut due, &mut away, layers)?;
            continue;
        }
        // drawn until the window's frame and the layer's are shown, at the window's latest scale
        let frame = submission(&grey, layers.then_some(&red), scale, size);
        submit(&mut ws, window, &frame, &mut due, &mut away, false)?;
    }
}

/// Submits `frame` of `window`: away (with whether its layer is the GPU renderer's, `gpu`) once taken, else due again
/// with the next frame the window system hands it.
fn submit(
    ws: &mut WaylandWindowSystem,
    window: cupertino_ui_core::platform::WindowId,
    frame: &Submission,
    due: &mut bool,
    away: &mut Option<bool>,
    gpu: bool,
) -> Result<(), String> {
    *due = false;
    match ws.submit(window, frame).map_err(|e| format!("{e:?}"))? {
        Submitted::Taken => *away = Some(gpu),
        Submitted::NotReady => ws.request_frame(window),
        Submitted::Declined => return Err("the window system declined the frame".into()),
    }
    Ok(())
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
