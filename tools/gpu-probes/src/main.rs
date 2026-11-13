//! Probes for the GPU rendering plan (`docs/plans/gpu-rendering.md`, step 0). Each measures one cost a GPU toolkit
//! process would pay, in fresh processes so that nothing an earlier measurement loaded is counted again:
//!
//!   gpu-probes context [--platform gbm|device] [--programs N] [--runs N]
//!       context creation, the first program, later programs and teardown: time and private memory
//!   gpu-probes texture [--platform gbm|device] [--size WxH] [--count N] [--runs N]
//!       render targets in total GPU memory and in the process's GPU memory, the first one and later ones
//!   gpu-probes binary [--platform gbm|device] [--runs N]
//!       a saved program binary as the first program of a fresh process, against compiling it
//!   gpu-probes fence --mode fence|plain [--frames N] [--platform gbm|device]
//!       resident memory over many frames, with a native fence per frame or without
//!   gpu-probes depth [--seconds S] [--dry-run] [--card PATH]
//!       an 8-bit and a 10-bit gradient on the primary plane (needs the display: run from a text console)
//!
//! The driver's on-disk program cache is turned off (and pointed at a scratch folder) unless a probe says otherwise,
//! so compile times are cold. Every probe prints one JSON object per line.

mod depth;
mod gl;
mod mem;

use std::process::Command;
use std::time::Instant;

use gl::*;
use mem::{GpuMemory, Memory};

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

fn platform(args: &[String]) -> Platform {
    arg(args, "--platform")
        .and_then(|p| Platform::parse(&p))
        .unwrap_or(Platform::Gbm)
}

fn number(args: &[String], name: &str, default: u64) -> u64 {
    arg(args, name)
        .and_then(|n| n.parse().ok())
        .unwrap_or(default)
}

fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1e3
}

fn mem_json(m: Memory) -> String {
    format!(
        r#""rss_kib":{},"pss_kib":{},"private_kib":{},"private_dirty_kib":{}"#,
        m.rss,
        m.pss,
        m.private(),
        m.private_dirty
    )
}

/// A scratch folder for the driver's program cache, so the probes never touch the person's.
fn cache_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("gpu-probes-cache-{}", unsafe { libc::getuid() }));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Runs this program again with `args`, the driver's disk cache on or off, and returns its output lines.
fn child(args: &[&str], disk_cache: bool) -> Vec<String> {
    let exe = std::env::current_exe().expect("own path");
    let out = Command::new(exe)
        .args(args)
        .env("__GL_SHADER_DISK_CACHE", if disk_cache { "1" } else { "0" })
        .env("__GL_SHADER_DISK_CACHE_PATH", cache_dir())
        .env("__GL_SHADER_DISK_CACHE_SKIP_CLEANUP", "1")
        .output()
        .expect("child ran");
    if !out.status.success() {
        eprintln!("{}", String::from_utf8_lossy(&out.stderr));
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_owned)
        .collect()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = &args[1.min(args.len())..];
    match args.first().map(String::as_str) {
        Some("context") => runs(rest, "context-once", false),
        Some("context-once") => context_once(rest),
        Some("texture") => runs(rest, "texture-once", false),
        Some("texture-once") => texture_once(rest),
        Some("binary") => binary(rest),
        Some("binary-once") => binary_once(rest),
        Some("fence") => fence(rest),
        Some("depth") => depth::run(rest),
        _ => {
            eprintln!("usage: gpu-probes context|texture|binary|fence|depth [options] (see the source's first lines)");
            std::process::exit(2);
        }
    }
}

/// `--runs N` fresh processes of `once`, their lines printed with the run's number.
fn runs(args: &[String], once: &str, disk_cache: bool) {
    let n = number(args, "--runs", 5);
    for run in 0..n {
        let mut a = vec![once];
        a.extend(args.iter().map(String::as_str));
        for line in child(&a, disk_cache) {
            println!("{{\"run\":{run},{}", &line[1..]);
        }
    }
}

fn context_once(args: &[String]) {
    let platform = platform(args);
    let programs = number(args, "--programs", 8) as u32;
    // a different variant in every run and process, so no cache the driver keeps in memory or on disk can answer
    let seed = std::process::id().wrapping_mul(1000);
    let line = |stage: &str, t: f64, extra: &str| {
        println!(
            r#"{{"probe":"context","platform":"{}","stage":"{stage}","ms":{t:.3},{}{extra}}}"#,
            platform.name(),
            mem_json(Memory::now())
        )
    };
    line("start", 0.0, "");
    unsafe {
        let t = Instant::now();
        let gpu = match Gpu::new(platform) {
            Ok(gpu) => gpu,
            Err(e) => {
                println!(
                    r#"{{"probe":"context","platform":"{}","error":"{e}"}}"#,
                    platform.name()
                );
                return;
            }
        };
        line(
            "context",
            ms(t),
            &format!(r#","renderer":"{}""#, gl_string(GL_RENDERER)),
        );
        let target = Target::new(256, 256).expect("target");
        let t = Instant::now();
        let prog = program(seed, false).expect("first program");
        draw(prog, &target);
        glFinish();
        line("first_program", ms(t), "");
        let t = Instant::now();
        for i in 1..=programs {
            let p = program(seed + i, false).expect("program");
            draw(p, &target);
        }
        glFinish();
        line(
            "later_programs",
            ms(t) / f64::from(programs.max(1)),
            &format!(r#","programs":{programs}"#),
        );
        target.delete();
        let t = Instant::now();
        gpu.teardown();
        line("teardown", ms(t), "");
    }
}

fn size(args: &[String]) -> (i32, i32) {
    arg(args, "--size")
        .and_then(|s| {
            let (w, h) = s.split_once('x')?;
            Some((w.parse().ok()?, h.parse().ok()?))
        })
        .unwrap_or((3840, 2160))
}

fn texture_once(args: &[String]) {
    let platform = platform(args);
    let (w, h) = size(args);
    let count = number(args, "--count", 4);
    let Some(nvml) = GpuMemory::open() else {
        println!(r#"{{"probe":"texture","error":"no GPU memory library"}}"#);
        return;
    };
    let pid = std::process::id();
    let line = |stage: &str, extra: &str| {
        println!(
            r#"{{"probe":"texture","platform":"{}","stage":"{stage}","size":"{w}x{h}","gpu_used_kib":{},"process_gpu_kib":{},{}{extra}}}"#,
            platform.name(),
            nvml.used_kib(),
            nvml.process_kib(pid)
                .map_or("null".into(), |k| k.to_string()),
            mem_json(Memory::now())
        )
    };
    line("start", "");
    unsafe {
        let gpu = Gpu::new(platform).expect("context");
        let small = Target::new(16, 16).expect("target");
        let prog = program(pid, false).expect("program");
        draw(prog, &small);
        glFinish();
        line("context_and_program", "");
        let mut targets = Vec::new();
        for i in 1..=count {
            let target = Target::new(w, h).expect("target");
            draw(prog, &target);
            glFinish();
            targets.push(target);
            line("target", &format!(r#","targets":{i}"#));
        }
        for t in targets {
            t.delete();
        }
        glFinish();
        line("targets_deleted", "");
        small.delete();
        gpu.teardown();
    }
}

fn binary(args: &[String]) {
    let n = number(args, "--runs", 5);
    let platform = arg(args, "--platform").unwrap_or_else(|| "gbm".into());
    let path = cache_dir().join("program.bin");
    let path = path.to_string_lossy().into_owned();
    // the binary, saved once with the cache off
    for line in child(
        &["binary-once", "save", &path, "--platform", &platform],
        false,
    ) {
        println!("{line}");
    }
    // the driver's own disk cache primed with the same source, for the warm-cache comparison
    child(
        &["binary-once", "compile", &path, "--platform", &platform],
        true,
    );
    for run in 0..n {
        for (how, cache) in [("load", false), ("compile", false), ("compile", true)] {
            for line in child(&["binary-once", how, &path, "--platform", &platform], cache) {
                let cache = if cache { "on" } else { "off" };
                println!("{{\"run\":{run},\"disk_cache\":\"{cache}\",{}", &line[1..]);
            }
        }
    }
}

/// The program every `binary-once` uses: one fixed source, so the saved binary and the warm cache are of it.
const BINARY_VARIANT: u32 = 7;

fn binary_once(args: &[String]) {
    let how = args.first().map(String::as_str).unwrap_or("compile");
    let path = args.get(1).cloned().unwrap_or_default();
    let platform = platform(args);
    unsafe {
        let gpu = Gpu::new(platform).expect("context");
        let target = Target::new(256, 256).expect("target");
        glFinish();
        let before = Memory::now();
        let t = Instant::now();
        let prog = match how {
            "save" | "compile" => program(BINARY_VARIANT, how == "save").expect("program"),
            _ => {
                let data = std::fs::read(&path).expect("saved binary");
                let format = u32::from_le_bytes(data[..4].try_into().unwrap());
                let prog = glCreateProgram();
                glProgramBinary(
                    prog,
                    format,
                    data[4..].as_ptr().cast(),
                    (data.len() - 4) as GLsizei,
                );
                if let Err(e) = linked(prog) {
                    println!(r#"{{"probe":"binary","how":"load","error":"{e}"}}"#);
                    return;
                }
                prog
            }
        };
        draw(prog, &target);
        glFinish();
        let took = ms(t);
        let after = Memory::now();
        if how == "save" {
            let mut len = 0;
            glGetProgramiv(prog, GL_PROGRAM_BINARY_LENGTH, &mut len);
            let mut data = vec![0u8; len.max(0) as usize];
            let (mut written, mut format) = (0, 0);
            glGetProgramBinary(
                prog,
                len,
                &mut written,
                &mut format,
                data.as_mut_ptr().cast(),
            );
            let mut file = format.to_le_bytes().to_vec();
            file.extend_from_slice(&data[..written.max(0) as usize]);
            std::fs::write(&path, file).expect("binary written");
            println!(r#"{{"probe":"binary","how":"save","bytes":{written},"format":{format}}}"#);
        } else {
            println!(
                r#"{{"probe":"binary","platform":"{}","how":"{how}","ms":{took:.3},"added_private_kib":{},"added_private_dirty_kib":{},{}}}"#,
                platform.name(),
                after.private() as i64 - before.private() as i64,
                after.private_dirty as i64 - before.private_dirty as i64,
                mem_json(after)
            );
        }
        target.delete();
        gpu.teardown();
    }
}

/// Least-squares slope of `ys` over `xs`.
fn slope(xs: &[f64], ys: &[f64]) -> f64 {
    let n = xs.len() as f64;
    let (mx, my) = (xs.iter().sum::<f64>() / n, ys.iter().sum::<f64>() / n);
    let num: f64 = xs.iter().zip(ys).map(|(x, y)| (x - mx) * (y - my)).sum();
    let den: f64 = xs.iter().map(|x| (x - mx) * (x - mx)).sum();
    if den == 0.0 {
        0.0
    } else {
        num / den
    }
}

fn fence(args: &[String]) {
    let platform = platform(args);
    let frames = number(args, "--frames", 10_000_000);
    let every = number(args, "--sample-every", 100_000).max(1);
    let fenced = arg(args, "--mode").as_deref() == Some("fence");
    let mode = if fenced { "fence" } else { "plain" };
    unsafe {
        let gpu = Gpu::new(platform).expect("context");
        if fenced && !gpu.has("EGL_ANDROID_native_fence_sync") {
            println!(
                r#"{{"probe":"fence","platform":"{}","error":"no EGL_ANDROID_native_fence_sync"}}"#,
                platform.name()
            );
            return;
        }
        let create: Option<CreateSync> = proc("eglCreateSyncKHR");
        let destroy: Option<DestroySync> = proc("eglDestroySyncKHR");
        let dup: Option<DupNativeFence> = proc("eglDupNativeFenceFDANDROID");
        let target = Target::new(64, 64).expect("target");
        let prog = program(1, false).expect("program");
        let attribs = [EGL_NONE];
        // what each frame draws: `full` sets its state and uniforms again (by name), `lean` only draws, `none` draws
        // nothing (the fence or the wait alone)
        let how = arg(args, "--draw").unwrap_or_else(|| "lean".into());
        draw(prog, &target);
        let (mut xs, mut ys) = (Vec::new(), Vec::new());
        let start = Instant::now();
        for frame in 0..frames {
            match how.as_str() {
                "full" => draw(prog, &target),
                "none" => {}
                _ => glDrawArrays(GL_TRIANGLES, 0, 3),
            }
            if fenced {
                // the fence a client hands the compositor with each frame: made, exported, waited for, closed
                let sync =
                    create.unwrap()(gpu.display, EGL_SYNC_NATIVE_FENCE_ANDROID, attribs.as_ptr());
                glFlush();
                let fd = dup.unwrap()(gpu.display, sync);
                if fd >= 0 {
                    let mut p = libc::pollfd {
                        fd,
                        events: libc::POLLIN,
                        revents: 0,
                    };
                    libc::poll(&mut p, 1, 1000);
                    libc::close(fd);
                }
                destroy.unwrap()(gpu.display, sync);
            } else {
                glFinish();
            }
            if frame % every == 0 {
                let rss = mem::resident_kib();
                xs.push(frame as f64);
                ys.push(rss as f64 * 1024.0);
                println!(
                    r#"{{"probe":"fence","platform":"{}","mode":"{mode}","frame":{frame},"rss_kib":{rss},"s":{:.1}}}"#,
                    platform.name(),
                    start.elapsed().as_secs_f64()
                );
            }
        }
        // the first tenth warms caches and pools; the slope is of the rest
        let skip = xs.len() / 10;
        println!(
            r#"{{"probe":"fence","platform":"{}","mode":"{mode}","draw":"{how}","frames":{frames},"bytes_per_frame":{:.6},"seconds":{:.1}}}"#,
            platform.name(),
            slope(&xs[skip..], &ys[skip..]),
            start.elapsed().as_secs_f64()
        );
        target.delete();
        gpu.teardown();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_slope_of_a_line_is_its_gradient() {
        let xs: Vec<f64> = (0..10).map(f64::from).collect();
        let ys: Vec<f64> = xs.iter().map(|x| 3.0 * x + 7.0).collect();
        assert!((super::slope(&xs, &ys) - 3.0).abs() < 1e-9);
        assert_eq!(super::slope(&[1.0, 1.0], &[2.0, 5.0]), 0.0);
    }
}
