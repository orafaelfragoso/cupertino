//! A window presented through the toolkit's GPU path: its first frames are drawn into shared memory while the device
//! and programs warm up on a worker thread, then every frame is uploaded into dmabufs and committed with syncobj points
//! (or after the own fence without the protocol). Development check of the presenter, run by the server's tests.
//!
//! `gpu-window --report PATH [--frames N] [--size WxH] [--reset-every N] [--purge-every N] [--losses N]`
//!
//! - `--frames N`: frames to present in all (default 120).
//! - `--reset-every N` / `--purge-every N`: after N frames on a device, its next status check reads a reset (or a
//!   purge), `--losses` times in all (default 1).
//!
//! The window follows the compositor's configures, so the session drives resizes.
//!
//! The report has one `key value` line per fact: frames on each path, the warm-up's timing and the program cache, the
//! buffers, waits and modifier, the syncobj surface objects made and destroyed, and the renderer the process ended on.

use std::collections::BTreeSet;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use cupertino_gpu::Loss;
use cupertino_ui_platform::{PlatformEvent, RendererKind};
use cupertino_ui_render::{CpuFrame, PixelFormat, RenderFrameId};
use cupertino_ui_wayland::gpu_path::{GpuPath, Presented};
use cupertino_ui_wayland::gpu_present::{buffer_count, SurfaceStats};
use cupertino_ui_wayland::renderer::{self, Requested};
use cupertino_wayland_client::{
    BufferFormat, Connection, Decorations, Error, Event, PixelRect, ToplevelConfig,
};

/// The colour of a frame presented from a GPU buffer, and of one in shared memory, as 8-bit RGBA.
const GPU_COLOUR: [u8; 4] = [51, 115, 217, 255];
const SHM_COLOUR: [u8; 4] = [64, 179, 89, 255];
/// The longest the check runs before it gives up.
const LIMIT: Duration = Duration::from_secs(60);

struct Args {
    report: std::path::PathBuf,
    frames: u64,
    size: (u32, u32),
    reset_every: Option<u64>,
    purge_every: Option<u64>,
    losses: u64,
}

fn parse() -> Result<Args, String> {
    let mut args = Args {
        report: std::path::PathBuf::new(),
        frames: 120,
        size: (320, 240),
        reset_every: None,
        purge_every: None,
        losses: 1,
    };
    let mut it = std::env::args().skip(1);
    let number = |v: Option<String>| -> Result<u64, String> {
        v.and_then(|v| v.parse().ok())
            .ok_or_else(|| "a number".to_string())
    };
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--report" => args.report = it.next().ok_or("--report PATH")?.into(),
            "--frames" => args.frames = number(it.next())?,
            "--size" => {
                let v = it.next().ok_or("--size WxH")?;
                let (w, h) = v.split_once('x').ok_or("--size WxH")?;
                args.size = (
                    w.parse().map_err(|_| "--size WxH")?,
                    h.parse().map_err(|_| "--size WxH")?,
                );
            }
            "--reset-every" => args.reset_every = Some(number(it.next())?),
            "--purge-every" => args.purge_every = Some(number(it.next())?),
            "--losses" => args.losses = number(it.next())?,
            other => return Err(format!("unknown argument {other}")),
        }
    }
    if args.report.as_os_str().is_empty() {
        return Err("--report PATH is needed".into());
    }
    Ok(args)
}

fn main() -> ExitCode {
    let args = match parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("gpu-window: {e}");
            return ExitCode::from(2);
        }
    };
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("gpu-window: {e}");
            ExitCode::FAILURE
        }
    }
}

/// What the run saw.
#[derive(Default)]
struct Seen {
    gpu_frames: u64,
    shm_frames: u64,
    busy: u64,
    first_frame: Option<(Duration, &'static str)>,
    to_gpu: u64,
    to_cpu: u64,
    losses: u64,
    /// The stats of surfaces' GPU buffers a loss freed, and of the current ones.
    banked: SurfaceStats,
    current: SurfaceStats,
    sizes: BTreeSet<(u32, u32)>,
    shm_with_syncobj: u64,
    fell_back: bool,
    warm: Vec<String>,
}

fn add(a: &mut SurfaceStats, b: SurfaceStats) {
    a.frames += b.frames;
    a.allocations += b.allocations;
    a.release_waits += b.release_waits;
    a.fence_waits += b.fence_waits;
    if b.modifier != 0 || a.modifier == 0 {
        a.modifier = b.modifier;
    }
}

fn run(args: &Args) -> Result<(), Box<dyn std::error::Error>> {
    let started = Instant::now();
    let mut conn = Connection::connect()?;
    let mut gpu = GpuPath::start_with(&mut conn, Requested::Gpu, false);
    let toplevel = conn.toplevel(&ToplevelConfig {
        app_id: "gpu-window".into(),
        title: "GPU window".into(),
        min_size: None,
        max_size: None,
        decorations: Decorations::Server,
        foreign_parent: None,
    })?;
    let surface = toplevel.surface();
    let mut seen = Seen::default();
    let mut configured = false;
    let mut pending = false;
    let mut on_device = 0u64;
    let mut injected = 0u64;
    while seen.gpu_frames + seen.shm_frames < args.frames {
        if started.elapsed() > LIMIT {
            return Err("the check ran out of time".into());
        }
        let stalled = pending || !configured;
        for event in conn.wait(Some(if stalled {
            Duration::from_millis(2)
        } else {
            Duration::ZERO
        }))? {
            match event {
                Event::Configure { .. } => configured = true,
                Event::Frame(_) => pending = false,
                Event::Close(_) => return Ok(()),
                _ => {}
            }
        }
        gpu.poll(&mut conn, None);
        for event in gpu.take_events() {
            match event {
                PlatformEvent::RendererChanged(RendererKind::Gpu) => {
                    seen.to_gpu += 1;
                    on_device = 0;
                    if let Some((timing, after)) = &gpu.warmed {
                        seen.warm.push(format!(
                            "{} {} {} {} {} {}",
                            after.as_micros(),
                            timing.context.as_micros(),
                            timing.programs.as_micros(),
                            timing.cache.loaded,
                            timing.cache.compiled,
                            timing.cache.stored,
                        ));
                    }
                }
                PlatformEvent::RendererChanged(RendererKind::Cpu) => seen.to_cpu += 1,
                _ => {}
            }
        }
        if pending || !configured {
            continue;
        }
        if gpu.draws(surface) && gpu.buffer_free(&conn, surface) == Some(false) {
            seen.busy += 1;
            // the release point signals through an eventfd the connection does not poll
            std::thread::sleep(Duration::from_millis(1));
            continue;
        }
        // losses, injected after enough frames on one device
        for (every, loss) in [
            (args.reset_every, Loss::Reset { guilty: Some(true) }),
            (args.purge_every, Loss::Purge),
        ] {
            if let Some(every) = every {
                if gpu.on() && on_device >= every && injected < args.losses {
                    gpu.inject_loss(loss);
                    injected += 1;
                    on_device = 0;
                }
            }
        }
        let size = args.size;
        let pixels = match conn.frame_size(surface, size) {
            Ok(s) => (s.width, s.height),
            Err(Error::NotConfigured) => continue,
            Err(e) => return Err(e.into()),
        };
        let cpu = solid(pixels, GPU_COLOUR);
        let damage = [PixelRect {
            x: 0,
            y: 0,
            width: pixels.0,
            height: pixels.1,
        }];
        let was_on = gpu.on();
        match gpu.present(&mut conn, surface, size, &cpu, &damage, true)? {
            Presented::Gpu => {
                seen.gpu_frames += 1;
                on_device += 1;
                pending = true;
                seen.first_frame.get_or_insert((started.elapsed(), "gpu"));
                seen.sizes.insert(pixels);
            }
            Presented::Busy => {
                seen.busy += 1;
                continue;
            }
            Presented::Resized => continue,
            Presented::Shm => {
                if was_on && !gpu.on() {
                    // the device was lost on this frame
                    seen.losses += 1;
                }
                if gpu.fell_back(surface) {
                    seen.fell_back = true;
                }
                if present_shm(&mut conn, surface, size)? {
                    if conn.has_syncobj_surface(surface).unwrap_or(false) {
                        seen.shm_with_syncobj += 1;
                    }
                    seen.shm_frames += 1;
                    pending = true;
                    seen.first_frame.get_or_insert((started.elapsed(), "cpu"));
                    seen.sizes.insert(pixels);
                }
            }
        }
        let stats = gpu.stats(surface).unwrap_or_default();
        if stats.frames < seen.current.frames {
            let gone = std::mem::take(&mut seen.current);
            add(&mut seen.banked, gone);
        }
        seen.current = stats;
    }
    // the last frame's callback, so the compositor saw it
    let end = Instant::now() + Duration::from_secs(2);
    while pending && Instant::now() < end {
        for event in conn.wait(Some(Duration::from_millis(5)))? {
            if matches!(event, Event::Frame(_)) {
                pending = false;
            }
        }
    }
    report(args, &conn, &gpu, &mut seen);
    Ok(())
}

/// A frame of one colour, as the CPU renderer hands frames over.
fn solid(size: (u32, u32), rgba: [u8; 4]) -> CpuFrame {
    CpuFrame {
        id: RenderFrameId(0),
        width: size.0,
        height: size.1,
        stride: size.0 * 4,
        format: PixelFormat::Rgba8Premultiplied,
        pixels: rgba.repeat((size.0 * size.1) as usize),
        damage: Vec::new(),
    }
}

/// Present a frame into shared memory; whether one went out.
fn present_shm(
    conn: &mut Connection,
    surface: cupertino_wayland_client::SurfaceId,
    size: (u32, u32),
) -> Result<bool, Error> {
    let mut lease = match conn.lease(surface, size, BufferFormat::Argb8888) {
        Ok(l) => l,
        Err(Error::NoFreeBuffer | Error::NotConfigured) => return Ok(false),
        Err(e) => return Err(e),
    };
    let [r, g, b, a] = SHM_COLOUR;
    for px in lease.pixels_mut().as_chunks_mut::<4>().0 {
        *px = [b, g, r, a];
    }
    lease.present()?;
    Ok(true)
}

fn report(args: &Args, conn: &Connection, gpu: &GpuPath, seen: &mut Seen) {
    let current = std::mem::take(&mut seen.current);
    add(&mut seen.banked, current);
    let stats = seen.banked;
    let choice = renderer::active();
    let (made, destroyed) = conn.syncobj_surfaces();
    let (first_ms, first_renderer) = seen
        .first_frame
        .map(|(d, r)| (d.as_secs_f64() * 1000.0, r))
        .unwrap_or((0.0, "none"));
    let lines = [
        ("frames", (seen.gpu_frames + seen.shm_frames).to_string()),
        ("gpu_frames", seen.gpu_frames.to_string()),
        ("shm_frames", seen.shm_frames.to_string()),
        ("busy", seen.busy.to_string()),
        ("first_frame_ms", format!("{first_ms:.1}")),
        ("first_frame_renderer", first_renderer.to_string()),
        // per warm-up: switch after start, context, programs (microseconds), programs loaded, compiled, stored
        ("warm_ups", seen.warm.join(",")),
        ("to_gpu", seen.to_gpu.to_string()),
        ("to_cpu", seen.to_cpu.to_string()),
        ("losses", seen.losses.to_string()),
        ("renderer", choice.kind.name().to_string()),
        ("reason", choice.reason.label()),
        ("explicit_sync", u8::from(gpu.explicit()).to_string()),
        ("buffers", buffer_count(gpu.explicit()).to_string()),
        ("allocations", stats.allocations.to_string()),
        ("release_waits", stats.release_waits.to_string()),
        ("fence_waits", stats.fence_waits.to_string()),
        ("modifier", format!("0x{:016x}", stats.modifier)),
        ("sizes", seen.sizes.len().to_string()),
        ("syncobj_surfaces_created", made.to_string()),
        ("syncobj_surfaces_destroyed", destroyed.to_string()),
        ("shm_with_syncobj", seen.shm_with_syncobj.to_string()),
        ("fell_back", u8::from(seen.fell_back).to_string()),
        ("device", gpu.device_name().unwrap_or_default()),
    ];
    let text: String = lines.iter().map(|(k, v)| format!("{k} {v}\n")).collect();
    if let Err(e) = std::fs::write(&args.report, text) {
        eprintln!("gpu-window: writing the report: {e}");
    }
}
