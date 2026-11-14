//! `cupertino-gpu-slice`: the smallest GPU client of the desktop, to prove the buffer handoff before the toolkit uses
//! it. It draws solid rectangles with GLES into dmabufs on the device the compositor names and attaches them through
//! `zwp_linux_dmabuf_v1` version 4, synchronised explicitly. Development only; never packaged.
//!
//! ```text
//! cupertino-gpu-slice [--frames N | --seconds S | --minutes M] [--size WxH] [--shm-every K] [--export]
//!                     [--control-shm] [--report PATH]
//! cupertino-gpu-slice --offscreen FRAMES [--no-fence]
//! ```
//!
//! A window run prints its report (`key value` lines). With `--control-shm` a second window of the same size draws
//! from shared memory beside it for the same time, and both reports are printed with the comparison of their missed
//! refreshes. `--offscreen` draws without a compositor and prints the resident-memory slope per frame, with the
//! fence path of a window (native fence, timeline point, eventfd wait) or, with `--no-fence`, a CPU wait instead.

mod client;
mod gles;
mod model;
mod syncobj;

use std::os::fd::{AsFd, AsRawFd};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use wayland_client::Connection;

const USAGE: &str =
    "usage: cupertino-gpu-slice [--frames N | --seconds S | --minutes M] [--size WxH] \
[--shm-every K] [--export] [--control-shm] [--report PATH] | --offscreen FRAMES [--no-fence]";

/// The window's size when none is given, in buffer pixels.
const DEFAULT_SIZE: (i32, i32) = (640, 400);

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("cupertino-gpu-slice: {e}");
            ExitCode::FAILURE
        }
    }
}

fn value<T: std::str::FromStr>(args: &[String], i: &mut usize) -> Result<T, String> {
    *i += 1;
    args.get(*i)
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| format!("{} needs a value\n{USAGE}", args[*i - 1]))
}

fn run(args: Vec<String>) -> Result<ExitCode, String> {
    let mut options = client::Options {
        size: DEFAULT_SIZE,
        frames: None,
        duration: None,
        shm_every: None,
        prefer_export: false,
        shm_only: false,
        title: "GPU slice".into(),
    };
    let (mut control, mut report, mut offscreen, mut fence) = (false, None, None, true);
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--frames" => options.frames = Some(value(&args, &mut i)?),
            "--seconds" => options.duration = Some(Duration::from_secs_f64(value(&args, &mut i)?)),
            "--minutes" => {
                options.duration =
                    Some(Duration::from_secs_f64(value::<f64>(&args, &mut i)? * 60.0))
            }
            "--size" => {
                let s: String = value(&args, &mut i)?;
                let (w, h) = s.split_once('x').ok_or(USAGE)?;
                options.size = (w.parse().map_err(|_| USAGE)?, h.parse().map_err(|_| USAGE)?);
            }
            "--shm-every" => options.shm_every = Some(value(&args, &mut i)?),
            "--export" => options.prefer_export = true,
            "--control-shm" => control = true,
            "--report" => report = Some(value::<String>(&args, &mut i)?),
            "--offscreen" => offscreen = Some(value::<u64>(&args, &mut i)?),
            "--no-fence" => fence = false,
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(ExitCode::SUCCESS);
            }
            other => return Err(format!("unknown argument {other}\n{USAGE}")),
        }
        i += 1;
    }
    if let Some(frames) = offscreen {
        return offscreen_loop(frames, fence, options.prefer_export);
    }
    if options.frames.is_none() && options.duration.is_none() {
        options.frames = Some(600);
    }
    let control_thread = control.then(|| {
        let mut shm = options.clone();
        shm.shm_only = true;
        shm.title = "GPU slice control (shared memory)".into();
        std::thread::spawn(move || {
            let connection = Connection::connect_to_env().map_err(|e| format!("connect: {e}"))?;
            client::run(connection, &shm)
        })
    });
    let connection = Connection::connect_to_env().map_err(|e| format!("connect: {e}"))?;
    let gpu = client::run(connection, &options)?;
    let mut out = gpu.lines();
    if let Some(thread) = control_thread {
        let shm = thread.join().map_err(|_| "the control window panicked")??;
        out.push_str(
            &shm.lines()
                .lines()
                .map(|l| format!("control_{l}\n"))
                .collect::<String>(),
        );
        let verdict = gpu.gaps.missed <= shm.gaps.missed;
        out.push_str(&format!(
            "comparison gpu_missed={} control_missed={} {}\n",
            gpu.gaps.missed,
            shm.gaps.missed,
            if verdict { "pass" } else { "fail" }
        ));
    }
    print!("{out}");
    if let Some(path) = report {
        std::fs::write(&path, &out).map_err(|e| format!("{path}: {e}"))?;
    }
    Ok(if gpu.fallback.is_none() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(2)
    })
}

/// The resident set of this process, in bytes.
fn resident() -> u64 {
    let statm = std::fs::read_to_string("/proc/self/statm").unwrap_or_default();
    let pages: u64 = statm
        .split_whitespace()
        .nth(1)
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    // Safety: sysconf has no preconditions.
    pages * unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as u64
}

/// Least-squares slope of `samples` (frame, bytes).
fn slope(samples: &[(f64, f64)]) -> f64 {
    let n = samples.len() as f64;
    let (sx, sy) = samples
        .iter()
        .fold((0.0, 0.0), |(a, b), (x, y)| (a + x, b + y));
    let (mx, my) = (sx / n, sy / n);
    let (num, den) = samples.iter().fold((0.0, 0.0), |(a, b), (x, y)| {
        (a + (x - mx) * (y - my), b + (x - mx) * (x - mx))
    });
    if den == 0.0 {
        0.0
    } else {
        num / den
    }
}

/// Samples of the resident set a run takes.
const SAMPLES: u64 = 200;

/// Draw `frames` frames offscreen into two GBM buffers, as a window would without presenting: each frame's fence
/// becomes a timeline point that is waited for through an eventfd (or, without fences, the CPU waits for the GPU).
/// Prints the resident-memory slope per frame after the first tenth of the run.
fn offscreen_loop(frames: u64, fence: bool, export: bool) -> Result<ExitCode, String> {
    let node_path = std::fs::read_dir("/dev/dri")
        .map_err(|e| format!("/dev/dri: {e}"))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("renderD"))
        })
        .min()
        .ok_or("no render node")?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&node_path)
        .map_err(|e| format!("{}: {e}", node_path.display()))?;
    let node = syncobj::Node(file);
    let gpu = gles::Gpu::new(Some(node.0.as_raw_fd()), export)?;
    let size = DEFAULT_SIZE;
    let modifiers = match gpu.allocator {
        gles::Allocator::Gbm => {
            let renderable = gpu.renderable_modifiers(gles::FORMAT_ARGB8888);
            if renderable.is_empty() {
                vec![gles::MODIFIER_INVALID]
            } else {
                renderable
            }
        }
        gles::Allocator::Export => gpu
            .export_format()
            .map(|(_, m)| vec![m])
            .unwrap_or_default(),
    };
    let images = [
        gpu.allocate(size, gles::FORMAT_ARGB8888, &modifiers)?,
        gpu.allocate(size, gles::FORMAT_ARGB8888, &modifiers)?,
    ];
    let timeline = node.timeline().map_err(|e| format!("syncobj: {e}"))?;
    let every = (frames / SAMPLES).max(1);
    let mut samples = Vec::new();
    let start = Instant::now();
    for n in 0..frames {
        let image = &images[(n % 2) as usize];
        gpu.draw(image, &[([0, 0, size.0, size.1], model::final_colour(n))]);
        if fence {
            let f = gpu.fence()?;
            node.signal_with(&timeline, f.as_fd(), n + 1)
                .map_err(|e| format!("timeline point: {e}"))?;
            let wait = node
                .wait_fd(&timeline, n + 1)
                .map_err(|e| format!("eventfd: {e}"))?;
            syncobj::wait_readable(wait.as_fd(), -1);
        } else {
            gpu.finish();
        }
        if n % every == 0 {
            samples.push((n as f64, resident() as f64));
        }
    }
    let elapsed = start.elapsed();
    let kept: Vec<_> = samples
        .iter()
        .copied()
        .filter(|(x, _)| *x >= frames as f64 / 10.0)
        .collect();
    println!(
        "offscreen frames {frames} fence {} allocator {} modifier {:#018x} seconds {:.1} resident_start_mib {:.1} resident_end_mib {:.1} bytes_per_frame {:.2}",
        u8::from(fence),
        gpu.allocator.name(),
        images[0].modifier,
        elapsed.as_secs_f64(),
        samples.first().map_or(0.0, |s| s.1) / 1048576.0,
        samples.last().map_or(0.0, |s| s.1) / 1048576.0,
        slope(&kept),
    );
    node.destroy(timeline);
    for image in images {
        gpu.free(image);
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_soak_slope_is_the_least_squares_growth_per_frame() {
        let samples: Vec<_> = (0..10)
            .map(|i| (i as f64 * 100.0, 1000.0 + i as f64 * 1600.0))
            .collect();
        assert!((slope(&samples) - 16.0).abs() < 1e-9);
        assert_eq!(slope(&[(1.0, 5.0), (1.0, 7.0)]), 0.0);
    }
}
