//! Nested backend: the compositor runs inside a window of the host session.

use std::time::Duration;

use anyhow::{anyhow, Result};
use cupertino_render_api::Renderer;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::Color32F;
use smithay::backend::winit::{self, WinitEvent, WinitEventLoop, WinitGraphicsBackend};
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::winit::dpi::LogicalSize;
use smithay::reexports::winit::platform::pump_events::PumpStatus;
use smithay::reexports::winit::window::Window;
use smithay::utils::Transform;

use super::Backend;
use crate::displays::{HeadCaps, HeadId, HeadInfo, HeadMode, HeadOp};
use crate::render::gles::{Gles, GlesResources, GlesTarget};
use crate::render::scene::RenderResources;
use crate::state::Cupertino;

pub const CLEAR: Color32F = Color32F::new(0.05, 0.05, 0.07, 1.0);
const REFRESH: Duration = Duration::from_micros(16_667);

pub struct WinitBackend {
    pub graphics: WinitGraphicsBackend<GlesRenderer>,
    /// The window's head once it is connected.
    pub head: Option<HeadId>,
    pub full_redraw: u8,
}

/// Open the nested window and register its output.
pub fn init(state: &mut Cupertino, size: (u32, u32), scale: Option<f64>) -> Result<WinitEventLoop> {
    let attrs = Window::default_attributes()
        .with_inner_size(LogicalSize::new(size.0 as f64, size.1 as f64))
        .with_title("Cupertino")
        .with_visible(true);
    let (mut graphics, events) = winit::init_from_attributes::<GlesRenderer>(attrs)
        .map_err(|e| anyhow!("winit backend: {e}"))?;
    graphics.window().set_cursor_visible(false);
    cupertino_diagnostic_events::set_context(
        "renderer",
        cupertino_diagnostic_events::Value::Public("gles"),
    );
    // the host's scale for the window unless the command line names one; stored settings do not apply
    let scale = scale.unwrap_or_else(|| graphics.scale_factor());
    state.render = Some(RenderResources::new(graphics.renderer())?);
    super::init_buffers(state, graphics.renderer());
    let size = graphics.window_size();
    state.backend = Some(Backend::Winit(Box::new(WinitBackend {
        graphics,
        head: None,
        full_redraw: 4,
    })));
    let info = HeadInfo {
        connector: "winit".into(),
        make: "Cupertino".into(),
        model: "nested window".into(),
        serial: None,
        physical_mm: (0, 0),
        modes: vec![HeadMode {
            width: size.w,
            height: size.h,
            refresh_mhz: 60_000,
        }],
        // the window is the only output and its size is the host's to change
        caps: HeadCaps {
            can_disable: false,
            custom_modes: false,
            transforms: false,
        },
        // the GL framebuffer of the window is upside down
        base_transform: Transform::Flipped180,
    };
    let (head, _) = state.connect_head(info, Some(scale));
    state.head_op(head, HeadOp::Enable)?;
    state.heads_changed();
    Ok(events)
}

/// Carry out `op` on the window's head: it can only be enabled (once, at start) and powered.
pub(crate) fn head_op(
    state: &mut Cupertino,
    w: &mut WinitBackend,
    head: HeadId,
    op: HeadOp,
) -> Result<()> {
    match op {
        HeadOp::Enable => {
            let output = state.prepare_head_output(head)?;
            state.create_head_output(head, output);
            w.head = Some(head);
            Ok(())
        }
        HeadOp::Disable | HeadOp::SetMode(_) => Err(anyhow!(
            "the nested window's output cannot be disabled or given another mode"
        )),
        HeadOp::Power(_) => Ok(()),
    }
}

fn pump(state: &mut Cupertino, events: &mut WinitEventLoop) -> bool {
    let status = events.dispatch_new_events(|event| match event {
        WinitEvent::Resized { size, .. } => {
            let head = match state.backend.as_mut() {
                Some(Backend::Winit(w)) => {
                    w.full_redraw = 4;
                    w.head
                }
                _ => None,
            };
            if let Some(head) = head {
                state.head_mode_replaced(
                    head,
                    HeadMode {
                        width: size.w,
                        height: size.h,
                        refresh_mhz: 60_000,
                    },
                );
            }
        }
        WinitEvent::Input(ev) => state.process_input_event(ev),
        WinitEvent::CloseRequested => state.running = false,
        WinitEvent::Redraw => state.request_redraw(),
        WinitEvent::Focus(_) => {}
    });
    !matches!(status, PumpStatus::Exit(_))
}

/// Render and present one frame of the nested output.
pub fn render(state: &mut Cupertino) {
    let Some(mut backend) = state.backend.take() else {
        return;
    };
    if let Backend::Winit(w) = &mut backend {
        render_inner(state, w);
    }
    state.backend = Some(backend);
}

fn render_inner(state: &mut Cupertino, w: &mut WinitBackend) {
    let idx = 0;
    if state.outputs.is_empty()
        || !state.displays.powered(state.outputs[idx].id)
        || !state.frame_due(idx, state.frame_now())
    {
        return;
    }
    let Some(composed) = state.compose_for_present(w.graphics.renderer(), idx) else {
        // without a scene (no render resources or no mode yet) nothing is drawn, but callbacks still go out
        let time = Duration::from(state.clock.now());
        state.process_screenshots(w.graphics.renderer(), idx);
        state.post_repaint(idx, time, Some(REFRESH), false);
        return;
    };
    w.full_redraw = w.full_redraw.saturating_sub(1);
    let age = if w.full_redraw > 0 {
        0
    } else {
        w.graphics.buffer_age().unwrap_or(0)
    };
    let Some((res, cache, tracker)) = state.gles_parts(idx) else {
        return;
    };
    let mut resources = GlesResources::new(res, cache, composed.resources);
    let result = Gles.render(
        &composed.frame,
        &mut resources,
        GlesTarget::Window {
            graphics: &mut w.graphics,
            tracker,
            age,
            clear: CLEAR,
        },
    );
    let time = Duration::from(state.clock.now());
    state.outputs[idx].scheduler.rendered();
    match result {
        Ok(completion) => {
            state.frame_drawn(idx);
            // nothing differs from the buffer: nothing to submit or to report as presented
            let damage = completion.damage().map(<[_]>::to_vec);
            let presented = damage.is_some();
            match damage {
                Some(d) => match w.graphics.submit(Some(&d)) {
                    Ok(()) => state.outputs[idx].scheduler.queued(),
                    Err(e) => tracing::warn!(error = %e, "submitting the frame failed"),
                },
                None => state.outputs[idx].scheduler.empty(),
            }
            state.process_screenshots(w.graphics.renderer(), idx);
            state.post_repaint(idx, time, Some(REFRESH), presented);
        }
        Err(e) => {
            state.process_screenshots(w.graphics.renderer(), idx);
            state.frame_failed(idx, time, REFRESH, &e);
        }
    }
}

/// Main loop of the nested backend.
pub fn run(
    state: &mut Cupertino,
    event_loop: &mut EventLoop<'static, Cupertino>,
    mut events: WinitEventLoop,
    deadline: Option<std::time::Instant>,
) {
    while state.running {
        if !pump(state, &mut events) {
            break;
        }
        state.tick();
        if state.wants_frames() {
            render(state);
        }
        let timeout = if state.wm.wants_tick(state.now()) {
            Duration::from_millis(2)
        } else {
            Duration::from_millis(8)
        };
        if event_loop.dispatch(Some(timeout), state).is_err() {
            break;
        }
        let _ = state.display_handle.flush_clients();
        if deadline.is_some_and(|d| std::time::Instant::now() >= d) {
            break;
        }
    }
    if let Some(Backend::Winit(w)) = state.backend.as_mut() {
        if let Some(res) = state.render.as_ref() {
            res.destroy(w.graphics.renderer());
        }
    }
}
