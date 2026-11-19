//! `Cupertino`-side frame statistics glue feeding [`cupertino_server_backend::frame_stats`].

pub use cupertino_server_backend::frame_stats::*;

use std::time::{Duration, Instant};

/// Snapshot current compositor material and upload counters into [`Totals`].
pub fn totals_of(state: &crate::state::Cupertino) -> Totals {
    let ms = state
        .render
        .as_ref()
        .and_then(|r| r.material.as_ref())
        .map(|m| m.borrow().stats)
        .unwrap_or_default();
    let up = crate::render::uploads::total();
    Totals {
        material: MaterialCounts {
            backdrop_misses: ms.backdrop_misses,
            backdrop_hits: ms.backdrop_hits,
            allocations: ms.allocations,
            passes: ms.passes,
            blur_passes: ms.blur_passes,
            blur_texels: ms.blur_texels,
        },
        decoration_rasters: crate::render::chrome::counts().painted,
        hud_rasters: crate::render::chrome::counts().hud_painted,
        uploads: UploadCounts {
            decoration: up.decoration,
            decoration_bytes: up.decoration_bytes,
            hud: up.hud,
            hud_bytes: up.hud_bytes,
        },
        decoration_draws: crate::render::chrome::counts().drawn,
        hud_draws: crate::render::chrome::counts().hud_drawn,
    }
}

/// Before a frame is composed: the glass passes time themselves from here on, and their counters are noted.
pub fn start_glass_timing(state: &mut crate::state::Cupertino) {
    if state.frame_stats.is_none() {
        return;
    }
    let totals = totals_of(state);
    if let Some(stats) = state.frame_stats.as_mut() {
        stats.before = totals;
    }
    if let Some(material) = state.render.as_ref().and_then(|r| r.material.as_ref()) {
        let mut m = material.borrow_mut();
        m.timing = true;
        m.glass_time = GlassTime::default();
    }
}

/// The outputs that owe a frame at `now` (the frame clock), noted for [`frame_rendered`].
pub fn note_owed(state: &mut crate::state::Cupertino, now: Duration) {
    if state.frame_stats.is_none() {
        return;
    }
    let due: Vec<(usize, Duration)> = (0..state.outputs.len())
        .filter(|&idx| state.displays.powered(state.outputs[idx].id))
        .filter_map(|idx| state.frame_deadline(idx, now).map(|at| (idx, at)))
        .filter(|(_, at)| *at <= now)
        .collect();
    if let Some(stats) = state.frame_stats.as_mut() {
        for (idx, at) in due {
            stats.owe(idx, at);
        }
    }
}

/// Once the frame's GPU work finished: the GPU times of its timed passes. Call with the renderer's context current.
///
/// # Safety
/// `gl` must be the current context of the renderer the material pipeline was made on.
pub unsafe fn collect_pass_times(
    state: &crate::state::Cupertino,
    gl: &smithay::backend::renderer::gles::ffi::Gles2,
) {
    if let Some(material) = state.render.as_ref().and_then(|r| r.material.as_ref()) {
        material.borrow_mut().collect_pass_times(gl);
    }
}

/// A frame of output `idx` was rendered and its GPU work finished: write its line.
pub fn frame_rendered(
    state: &mut crate::state::Cupertino,
    idx: usize,
    compose: Duration,
    render: Duration,
    submit: Duration,
    gpu: Option<Duration>,
) {
    let clock = state.frame_now();
    let glass = state
        .render
        .as_ref()
        .and_then(|r| r.material.as_ref())
        .map(|m| m.borrow().glass_time)
        .unwrap_or_default();
    let totals = totals_of(state);
    let Some(stats) = state.frame_stats.as_mut() else {
        return;
    };
    let now = Instant::now();
    let late = stats.shown(idx, clock);
    let counts = totals.material;
    let before = stats.before.material;
    let record = FrameRecord {
        shown: now.saturating_duration_since(stats.start),
        compose,
        render,
        submit,
        glass,
        blurred: counts.backdrop_misses - before.backdrop_misses,
        reused: counts.backdrop_hits - before.backdrop_hits,
        late,
        missed: missed_refreshes(late, stats.refresh),
        latency: stats.latency.presented(now),
        allocated: counts.allocations - before.allocations,
        passes: counts.passes - before.passes,
        gpu,
        counts: totals.since(&stats.before),
        pointer_latency: stats.latency.pointer_presented(now),
    };
    stats.write(&record);
}

/// An input arrived (benchmark runs and the DRM trace only).
pub fn note_input(state: &mut crate::state::Cupertino) {
    if let Some(stats) = state.frame_stats.as_mut() {
        stats.latency.input(Instant::now());
    }
    if let Some(trace) = state.drm_trace.as_mut() {
        trace.input(Instant::now());
    }
}

/// The pointer moved (benchmark runs and the DRM trace only): an input that is also timed on its own.
pub fn note_pointer_motion(state: &mut crate::state::Cupertino) {
    let at = Instant::now();
    if let Some(stats) = state.frame_stats.as_mut() {
        stats.latency.pointer(at);
    }
    if let Some(trace) = state.drm_trace.as_mut() {
        trace.pointer(at);
    }
}

/// A client committed (benchmark runs only).
pub fn note_commit(state: &mut crate::state::Cupertino) {
    if let Some(stats) = state.frame_stats.as_mut() {
        stats.latency.commit(Instant::now());
    }
}
