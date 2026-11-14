//! A window with one surface layer, presented through the toolkit's window system: the window's frame is grey all
//! over, the layer's frame red at [`LAYER`] in the window, and once both are shown the layer fades out on the
//! compositor's clock ([`FADE`]) while the window draws nothing more. Development check of the surface layers'
//! presenter, run by the server's tests.
//!
//! `surface-layers --report PATH`
//!
//! The report has one `key value` line per fact: whether the window system shows surface layers, the window's frames
//! presented, the layer frames handed over, and `faded 1` once the fade was asked for. It is written after the fade
//! is asked for and again when the window closes.

use std::process::ExitCode;
use std::time::{Duration, Instant};

use cupertino_geometry::{Rect, Size};
use cupertino_modifiers::Profile;
use cupertino_ui_platform::{
    DecorationMode, LayerChange, LayerProperty, PlatformEvent, PresentOutcome, SurfaceLayer,
    SurfaceLayerLook, WindowEvent, WindowRequest, WindowSpec, WindowSystem,
};
use cupertino_ui_render::{CpuFrame, PixelFormat, PixelRect, RenderFrameId};
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

fn run(report: &std::path::Path) -> Result<(), String> {
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
    let (mut scale, mut frames, mut layer_frames) = (1.0_f64, 0u32, 0u32);
    let mut faded: Option<Instant> = None;
    // the latest configure and size, drawn once both the window and its layer were shown
    let mut configure = None;
    let mut size = WINDOW;
    let write = |frames: u32, layer_frames: u32, faded: bool| {
        let text = format!(
            "surface_layers {}\nframes {frames}\nlayer_frames {layer_frames}\nfaded {}\n",
            u8::from(layers),
            u8::from(faded)
        );
        std::fs::write(report, text).map_err(|e| e.to_string())
    };
    let start = Instant::now();
    let mut events = Vec::new();
    loop {
        if start.elapsed() > LIMIT {
            return Err("timed out".into());
        }
        if faded.is_some_and(|t| t.elapsed() > LINGER) {
            return write(frames, layer_frames, true);
        }
        events.clear();
        ws.dispatch(Some(Duration::from_millis(10)), &mut events)
            .map_err(|e| format!("{e:?}"))?;
        for event in events.drain(..) {
            let PlatformEvent::Window { event, .. } = event else {
                continue;
            };
            match event {
                WindowEvent::Scale(s) => scale = s,
                WindowEvent::Configure(c) => {
                    size = c.size.unwrap_or(WINDOW);
                    configure = Some(c.id);
                }
                WindowEvent::Frame(_) if frames > 0 && faded.is_none() => {
                    // the first frame is shown: fade the layer out, drawing nothing
                    if layers {
                        ws.request(
                            window,
                            WindowRequest::Layer {
                                key: LAYER_KEY,
                                changes: vec![LayerChange {
                                    property: LayerProperty::Opacity,
                                    to: 0.0,
                                    curve: Some(cupertino_ui_platform::LayerCurve::Timed {
                                        duration: FADE,
                                        bezier: cupertino_ui_platform::CubicBezier::new(
                                            0.0, 0.0, 1.0, 1.0,
                                        ),
                                    }),
                                    velocity: 0.0,
                                }],
                            },
                        )
                        .map_err(|e| format!("{e:?}"))?;
                    }
                    faded = Some(Instant::now());
                    write(frames, layer_frames, true)?;
                }
                WindowEvent::CloseRequested => return write(frames, layer_frames, faded.is_some()),
                _ => {}
            }
        }
        // drawn until the window's frame and the layer's are both shown, at the window's latest scale
        let Some(id) = configure.filter(|_| frames == 0) else {
            continue;
        };
        let px = |v: f64| (v * scale).round() as u32;
        let content = frame(1, px(size.width), px(size.height), GREY);
        let red = frame(2, px(LAYER.width()), px(LAYER.height()), RED);
        if layers && layer_frames == 0 {
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
            layer_frames += 1;
        }
        let shown = ws
            .present(window, &content, Some(id))
            .map_err(|e| format!("{e:?}"))?;
        if shown == PresentOutcome::Presented {
            frames += 1;
            ws.request_frame(window);
        }
    }
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let report = match (args.next().as_deref(), args.next()) {
        (Some("--report"), Some(path)) => std::path::PathBuf::from(path),
        _ => {
            eprintln!("usage: surface-layers --report PATH");
            return ExitCode::from(2);
        }
    };
    match run(&report) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("surface-layers: {e}");
            ExitCode::FAILURE
        }
    }
}
