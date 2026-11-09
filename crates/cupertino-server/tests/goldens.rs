//! Own-design headless baselines. Missing EGL is a failure, never a skipped pass.
//!
//! Run with `ci/goldens.sh`.
//! The scene tests are ignored: their baselines are valid on the renderer in `renderer.txt` only, the pinned image of
//! `ci/goldens/Dockerfile`. `ci/goldens.sh` runs them there (and in CI), `ci/goldens.sh --update` re-records them.
//! Run elsewhere with `--include-ignored`, a foreign renderer reports `SKIPPED` unless `CUPERTINO_GOLDENS_REQUIRED=1`.
//! Geometry is exact, in stacking order. Pixels allow at most 2/255 per RGBA channel at every pixel,
//! with zero pixels allowed beyond that tolerance. There is no spatial tolerance or ignored region.
//! `CUPERTINO_GOLDEN_MUTATE_WINDOW=1` moves the front window exactly one logical pixel and must fail comparison.
//! Perf is opt-in with `CUPERTINO_GOLDEN_PERF=1 CUPERTINO_GOLDEN_PERF_REPORT=/absolute/report.json
//! cargo test -p cupertino-server --test goldens headless_frame_and_memory_report -- --ignored`.
//! These are full repaint/readback costs, not display frame latency; the JSON records build profile and raw repeats.

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cupertino_server::backend::{headless, Backend};
use cupertino_server::cli::start_shell_supervision;
use cupertino_server::config::{Config, ShellProcess};
use cupertino_server::dbus::DbusHandle;
use cupertino_server::input::{ActionPlan, ShellOp};
use cupertino_server::render::screenshot::render_to_image;
use cupertino_server::Cupertino;
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use smithay::backend::input::InputEvent;
use smithay::backend::renderer::gles::ffi;
use smithay::backend::renderer::utils::with_renderer_surface_state;
use smithay::desktop::layer_map_for_output;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use smithay::wayland::compositor::{
    with_surface_tree_downward, SurfaceAttributes, TraversalAction,
};

const SIZE: (i32, i32) = (1000, 700);
// Permit rounding by the GLES implementation, without permitting any displaced pixels.
// Only Mesa's EGL vendor is visible, and it is forced to its software rasteriser, so no host GPU takes part.
const MESA_EGL_VENDOR: &str = "/usr/share/glvnd/egl_vendor.d/50_mesa.json";
const CHANNEL_TOLERANCE: u8 = 2;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", deny_unknown_fields)]
enum Input {
    Pointer { x: f64, y: f64 },
    Button { code: u32, pressed: bool },
    Key { code: u32, pressed: bool },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct Event {
    milliseconds: u32,
    input: Input,
}

#[derive(Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct Structure {
    size: [i32; 2],
    milliseconds: u32,
    windows: Vec<[f64; 4]>,
    layers: Vec<(String, [i32; 4])>,
    overview: Vec<[f64; 4]>,
}

struct Scene {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
    clients: Vec<common::Client>,
    size: (i32, i32),
    milliseconds: u32,
    /// Whether the injected device was announced to the server.
    device_added: bool,
}

impl Scene {
    fn new(size: (i32, i32)) -> Self {
        Self::with_processes(size, Vec::new())
    }

    fn with_processes(size: (i32, i32), shell_processes: Vec<ShellProcess>) -> Self {
        let event_loop = EventLoop::try_new().unwrap();
        let supervised = !shell_processes.is_empty();
        let mut state = Cupertino::new(
            Display::new().unwrap(),
            event_loop.handle(),
            event_loop.get_signal(),
            Config {
                shell_processes,
                ..Config::default()
            },
            true,
            &cupertino_assets::Assets::builtin(),
        )
        .unwrap();
        state.fixed_time = Some(0.0);
        headless::init(&mut state, size, 1.0).expect("goldens require an offscreen EGL renderer");
        if supervised {
            state.dbus = Some(DbusHandle::start().expect("the control bus worker"));
            start_shell_supervision(&state, &event_loop)
                .expect("the supervision timer is inserted");
        }
        Self {
            state,
            event_loop,
            clients: Vec::new(),
            size,
            milliseconds: 0,
            device_added: false,
        }
    }

    fn pump(&mut self) {
        headless::step(
            &mut self.state,
            &mut self.event_loop,
            Duration::from_millis(2),
        )
        .unwrap();
    }

    fn wait_until(&mut self, what: &str, mut condition: impl FnMut(&Scene) -> bool) {
        let deadline = Instant::now() + common::HANG_GUARD;
        while !condition(self) {
            self.pump();
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
        }
    }

    /// Pump until the scene is idle: two captures ten steps apart are equal and no shell surface waits for a frame. The
    /// shell processes' display clocks stand still ([`common::shell::TEST_CLOCK`]) and the server's model clock is
    /// pinned, so nothing changes by itself after that; real time bounds the wait only as a deadline.
    fn settle(&mut self) {
        let deadline = Instant::now() + common::HANG_GUARD;
        let mut last = (self.structure(), self.image());
        loop {
            assert!(Instant::now() < deadline, "scene never settled");
            for _ in 0..10 {
                self.pump();
            }
            let current = (self.structure(), self.image());
            if current == last && !self.awaiting_frame() {
                return;
            }
            last = current;
        }
    }

    /// Whether a layer or lock surface, or a subsurface of one, asked for a frame callback it has not got.
    fn awaiting_frame(&self) -> bool {
        let mut surfaces = Vec::new();
        for (idx, output) in self.state.outputs.iter().enumerate() {
            let map = layer_map_for_output(&output.output);
            surfaces.extend(map.layers().map(|l| l.wl_surface().clone()));
            surfaces.extend(self.state.lock_surface_of(idx));
        }
        surfaces.iter().any(|surface| {
            let mut waiting = false;
            with_surface_tree_downward(
                surface,
                (),
                |_, _, _| TraversalAction::DoChildren(()),
                |_, states, _| {
                    waiting |= !states
                        .cached_state
                        .get::<SurfaceAttributes>()
                        .current()
                        .frame_callbacks
                        .is_empty();
                },
                |_, _, _| true,
            );
            waiting
        })
    }

    fn mapped_layers(&self, namespace: &str) -> usize {
        let map = layer_map_for_output(&self.state.outputs[0].output);
        let count = map
            .layers()
            .filter(|layer| layer.namespace() == namespace)
            .filter(|layer| {
                map.layer_geometry(layer)
                    .is_some_and(|r| r.size.w > 0 && r.size.h > 0)
            })
            .count();
        count
    }

    fn at(&mut self, milliseconds: u32) {
        assert!(
            milliseconds >= self.milliseconds,
            "trace time moved backwards"
        );
        self.milliseconds = milliseconds;
        let time = f64::from(milliseconds) / 1000.0;
        common::set_time(&mut self.state, &mut self.event_loop, time);
        self.state.tick();
    }

    fn window(&mut self) {
        let expected = self.clients.len() + 1;
        let socket = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap())
            .join(self.state.socket_name.as_ref().unwrap());
        self.clients.push(common::Client::toplevel(socket));
        let deadline = Instant::now() + common::HANG_GUARD;
        loop {
            self.pump();
            if self.state.wm.windows().count() == expected {
                break;
            }
            assert!(Instant::now() < deadline, "testclient did not map");
        }
    }

    /// Inject `event` where a device's events enter the server (`process_input_event`), at its time on the model clock.
    fn input(&mut self, event: Event) {
        self.at(event.milliseconds);
        if !self.device_added {
            self.state
                .process_input_event::<injected::Backend>(InputEvent::DeviceAdded {
                    device: injected::Device,
                });
            self.device_added = true;
        }
        // a pointer position is a point of the first output; the device reports it within its range
        let logical = self.state.outputs.first().map(|o| (o.rect.w, o.rect.h));
        self.state
            .process_input_event(injected::event(&event, logical.unwrap_or((1.0, 1.0))));
        self.pump();
    }

    fn structure(&self) -> Structure {
        let rect = |r: cupertino_wm::Rect| [r.x, r.y, r.w, r.h];
        let windows = self
            .state
            .wm
            .stacking_order()
            .iter()
            .map(|id| rect(self.state.wm.window(*id).unwrap().geometry()))
            .collect();
        let map = layer_map_for_output(&self.state.outputs[0].output);
        let mut layers: Vec<_> = map
            .layers()
            .map(|layer| {
                let r = map
                    .layer_geometry(layer)
                    .expect("mapped layer has geometry");
                (
                    layer.namespace().to_string(),
                    [r.loc.x, r.loc.y, r.size.w, r.size.h],
                )
            })
            .collect();
        layers.sort();
        Structure {
            size: [self.size.0, self.size.1],
            milliseconds: self.milliseconds,
            windows,
            layers,
            overview: self
                .state
                .shell
                .overview
                .as_ref()
                .map(|o| {
                    o.frames(self.state.now())
                        .iter()
                        .map(|f| rect(f.frame))
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    fn shift_window_one_pixel(&mut self) {
        let id = *self.state.wm.stacking_order().last().unwrap();
        let before = self.state.wm.window(id).unwrap().geometry();
        self.state.command(cupertino_wm::Command::BeginMove {
            window: id,
            pointer: cupertino_wm::Point::new(0.0, 0.0),
        });
        self.state.command(cupertino_wm::Command::PointerMotion {
            pointer: cupertino_wm::Point::new(1.0, 0.0),
            modifiers: cupertino_wm::Modifiers::default(),
        });
        self.state.command(cupertino_wm::Command::PointerRelease);
        let after = self.state.wm.window(id).unwrap().geometry();
        assert_eq!(
            [after.x, after.y, after.w, after.h],
            [before.x + 1.0, before.y, before.w, before.h]
        );
    }

    /// The GL renderer and version this scene is drawn with; baselines are only valid on the renderer they were recorded on.
    fn renderer_identity(&mut self) -> String {
        let Some(Backend::Headless(h)) = self.state.backend.as_mut() else {
            panic!("headless backend required")
        };
        h.renderer
            .with_context(|gl| {
                let read = |name| unsafe {
                    std::ffi::CStr::from_ptr(gl.GetString(name).cast())
                        .to_string_lossy()
                        .into_owned()
                };
                let renderer = read(ffi::RENDERER);
                // llvmpipe appends the host CPU's vector width, which rounds to the same pixels.
                let renderer = match renderer.rsplit_once(", ") {
                    Some((name, width)) if width.ends_with(" bits)") => format!("{name})"),
                    _ => renderer,
                };
                format!("{renderer} / {}", read(ffi::VERSION))
            })
            .unwrap()
    }

    fn image(&mut self) -> RgbaImage {
        let mut backend = self.state.backend.take().unwrap();
        let Backend::Headless(ref mut h) = backend else {
            panic!("headless backend required")
        };
        let elements = self.state.build_output_elements(&mut h.renderer, 0);
        let image = render_to_image(
            &mut h.renderer,
            &elements,
            self.size.into(),
            1.0,
            headless::CLEAR,
        )
        .unwrap();
        self.state.backend = Some(backend);
        image
    }
}

fn compare_pixels(name: &str, expected: &RgbaImage, actual: &RgbaImage) {
    assert_eq!(expected.dimensions(), actual.dimensions());
    let mismatches = expected
        .pixels()
        .zip(actual.pixels())
        .filter(|(a, b)| {
            a.0.iter()
                .zip(b.0)
                .any(|(a, b)| a.abs_diff(b) > CHANNEL_TOLERANCE)
        })
        .count();
    assert_eq!(
        mismatches, 0,
        "{name}: pixels exceed the per-channel tolerance"
    );
}

/// Baselines this worker re-recorded.
static RECORDED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

const RERUN: &str = "; rerun without CUPERTINO_UPDATE_GOLDENS";

/// An update run fails after recording, so a `CUPERTINO_UPDATE_GOLDENS` that leaks into a test run can never pass it.
fn fail_after_update(update: bool) {
    if update {
        let message = format!(
            "re-recorded {} goldens{RERUN}",
            RECORDED.load(std::sync::atomic::Ordering::Relaxed)
        );
        println!("{message}");
        panic!("{message}");
    }
}

/// The re-record message when it is the worker's only failure: libtest's failure status and a single panic, that of
/// `fail_after_update`. The parent then fails with that message alone, so ci/goldens_log.py tells an update from an
/// update followed by another failure.
fn only_re_recorded(status: std::process::ExitStatus, stderr: &str) -> Option<&str> {
    if status.code() != Some(101) {
        return None;
    }
    let lines: Vec<&str> = stderr.lines().collect();
    let panics: Vec<usize> = (0..lines.len())
        .filter(|&i| lines[i].starts_with("thread '") && lines[i].contains(" panicked at "))
        .collect();
    let [at] = panics[..] else { return None };
    lines
        .get(at + 1)
        .copied()
        .filter(|message| message.starts_with("re-recorded ") && message.ends_with(RERUN))
}

fn baseline(root: &Path, name: &str, scene: &mut Scene, update: bool) {
    let structure = scene.structure();
    let image = scene.image();
    let geometry = root.join(format!("{name}.json"));
    let png = root.join(format!("{name}.png"));
    if update {
        RECORDED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::fs::create_dir_all(root).unwrap();
        std::fs::write(geometry, serde_json::to_vec_pretty(&structure).unwrap()).unwrap();
        image.save(png).unwrap();
        std::fs::write(root.join("renderer.txt"), scene.renderer_identity()).unwrap();
    } else {
        let expected: Structure =
            serde_json::from_slice(&std::fs::read(geometry).unwrap()).unwrap();
        assert_eq!(expected, structure, "{name} structural geometry changed");
        compare_pixels(name, &image::open(png).unwrap().to_rgba8(), &image);
    }
}

/// Whether the baselines in `root` apply to this renderer. A foreign renderer fails when `CUPERTINO_GOLDENS_REQUIRED=1`
/// (`ci/goldens.sh`, the CI job) and otherwise reports a skip, which is never a pass of the baselines.
fn baselines_apply(root: &Path, update: bool) -> bool {
    if update {
        return true;
    }
    let running = Scene::new(SIZE).renderer_identity();
    let recorded = std::fs::read_to_string(root.join("renderer.txt")).unwrap();
    if recorded == running {
        return true;
    }
    let message = format!(
        "baseline recorded on {recorded}, running on {running}; re-record in the pinned image with ci/goldens.sh --update"
    );
    assert!(
        std::env::var("CUPERTINO_GOLDENS_REQUIRED").as_deref() != Ok("1"),
        "{message}"
    );
    eprintln!("SKIPPED: {message}");
    false
}

fn fixture(size: (i32, i32), windows: usize) -> Scene {
    let mut scene = Scene::new(size);
    for _ in 0..windows {
        scene.window();
    }
    scene.at(1000);
    scene.pump();
    scene
}

fn isolated(test: &str, shell: bool, work: impl FnOnce(&str)) {
    if std::env::var_os("CUPERTINO_GOLDEN_WORKER").is_some() {
        work(&std::env::var("DBUS_SESSION_BUS_ADDRESS").expect("the parent starts the bus"));
        return;
    }
    let bus = common::shell::private_bus();
    let scratch = tempfile::tempdir().unwrap();
    let home = scratch.path().join("home");
    let runtime = scratch.path().join("runtime");
    std::fs::create_dir(&home).unwrap();
    std::fs::create_dir(&runtime).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut child = common::spawn::cleared(std::env::current_exe().unwrap(), &home);
    child
        .args(["--exact", test, "--nocapture", "--include-ignored"])
        .env("CUPERTINO_GOLDEN_WORKER", "1")
        .env("DBUS_SESSION_BUS_ADDRESS", &bus.address)
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
        .env("XDG_RUNTIME_DIR", runtime)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_DATA_DIRS", home.join("data"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("__EGL_VENDOR_LIBRARY_FILENAMES", MESA_EGL_VENDOR)
        .env("LIBGL_ALWAYS_SOFTWARE", "1")
        .env("GALLIUM_DRIVER", "llvmpipe")
        .env("LANG", "C.UTF-8")
        .env("TZ", "UTC");
    if shell {
        common::shell::bin_dir();
        common::fixtures::forward(&mut child);
    }
    for key in [
        "CUPERTINO_UPDATE_GOLDENS",
        "CUPERTINO_GOLDENS_REQUIRED",
        "CUPERTINO_GOLDEN_PERF",
        "CUPERTINO_GOLDEN_PERF_REPORT",
        "CUPERTINO_GOLDEN_MUTATE_WINDOW",
    ] {
        if let Some(value) = std::env::var_os(key) {
            child.env(key, value);
        }
    }
    let output = child.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    print!("{stdout}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    if let Some(message) = only_re_recorded(output.status, &stderr) {
        panic!("{message}");
    }
    assert!(
        output.status.success(),
        "isolated golden worker failed ({}):\n{}",
        output.status,
        stderr
            .lines()
            .map(|line| format!("    {line}\n"))
            .collect::<String>()
    );
    assert!(
        stdout.lines().any(|l| l == format!("test {test} ... ok")),
        "the worker did not run {test}: its name is passed to isolated() and must match the test function"
    );
}

#[test]
fn pixel_comparison_fails_on_a_change_at_unchanged_geometry() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/goldens");
    let expected = image::open(root.join("desktop-windows.png"))
        .unwrap()
        .to_rgba8();
    let mut within = expected.clone();
    within.get_pixel_mut(500, 350).0[0] =
        within.get_pixel(500, 350).0[0].saturating_add(CHANNEL_TOLERANCE);
    compare_pixels("desktop-windows", &expected, &within);
    let mut beyond = expected.clone();
    beyond.get_pixel_mut(500, 350).0[0] = beyond.get_pixel(500, 350).0[0] ^ 0x40;
    let failure =
        std::panic::catch_unwind(|| compare_pixels("desktop-windows", &expected, &beyond))
            .expect_err("a changed pixel must fail the comparison");
    let message = failure.downcast_ref::<String>().unwrap();
    assert!(
        message.contains("pixels exceed the per-channel tolerance"),
        "{message}"
    );
    assert!(message.contains("left: 1"), "{message}");
}

#[test]
fn only_a_lone_re_record_panic_is_an_update() {
    use std::os::unix::process::ExitStatusExt;
    let failed = std::process::ExitStatus::from_raw(101 << 8);
    let recorded = "\nthread 'desktop_goldens_and_input_replay' (5631) panicked at crates/cupertino-server/tests/goldens.rs:367:9:\nre-recorded 3 goldens; rerun without CUPERTINO_UPDATE_GOLDENS\nnote: run with `RUST_BACKTRACE=1` environment variable to display a backtrace\n";
    assert_eq!(
        only_re_recorded(failed, recorded),
        Some("re-recorded 3 goldens; rerun without CUPERTINO_UPDATE_GOLDENS")
    );
    let again = format!(
        "{recorded}\nthread '<unnamed>' (5640) panicked at crates/cupertino-server/tests/goldens.rs:689:9:\ninjected\n"
    );
    assert_eq!(only_re_recorded(failed, &again), None);
    let killed = std::process::ExitStatus::from_raw(9);
    assert_eq!(only_re_recorded(killed, recorded), None);
}

#[derive(Serialize)]
struct PerfSample {
    size: [i32; 2],
    scene: &'static str,
    repeat: usize,
    frame_readback_ns: Vec<u128>,
    rss_before_bytes: u64,
    rss_after_bytes: u64,
    rgba_bytes_observed: usize,
}

fn rss_bytes() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    let row = status
        .lines()
        .find(|line| line.starts_with("VmRSS:"))
        .expect("Linux VmRSS");
    row.split_whitespace()
        .nth(1)
        .unwrap()
        .parse::<u64>()
        .unwrap()
        * 1024
}

#[test]
#[ignore = "opt-in full repaint and GPU readback measurements"]
fn headless_frame_and_memory_report() {
    assert_eq!(std::env::var("CUPERTINO_GOLDEN_PERF").as_deref(), Ok("1"));
    isolated("headless_frame_and_memory_report", false, |_| {
        let path = std::env::var_os("CUPERTINO_GOLDEN_PERF_REPORT")
            .expect("set CUPERTINO_GOLDEN_PERF_REPORT to an absolute JSON output path");
        assert!(Path::new(&path).is_absolute());
        let mut samples = Vec::new();
        for size in [(1920, 1080), (2560, 1440), (3840, 2160)] {
            for repeat in 0..5 {
                for (name, count) in [("desktop-empty", 0), ("desktop-windows", 2)] {
                    let mut scene = fixture(size, count);
                    for _ in 0..3 {
                        std::hint::black_box(scene.image());
                    }
                    let rss_before_bytes = rss_bytes();
                    let mut frame_readback_ns = Vec::new();
                    let mut rgba_bytes_observed = 0;
                    for frame in 0..5 {
                        scene.at(1000 + frame * 16);
                        let start = Instant::now();
                        let image = scene.image();
                        frame_readback_ns.push(start.elapsed().as_nanos());
                        assert_eq!(image.dimensions(), (size.0 as u32, size.1 as u32));
                        rgba_bytes_observed += std::hint::black_box(image.as_raw()).len();
                        if count > 0 {
                            let id = *scene.state.wm.stacking_order().last().unwrap();
                            let r = scene.state.wm.window(id).unwrap().geometry();
                            assert_eq!(
                                image
                                    .get_pixel((r.x + r.w / 2.0) as u32, (r.y + r.h / 2.0) as u32)
                                    .0,
                                [0, 0, 255, 255],
                                "the testclient content was actually rendered"
                            );
                        }
                    }
                    samples.push(PerfSample {
                        size: [size.0, size.1],
                        scene: name,
                        repeat,
                        frame_readback_ns,
                        rss_before_bytes,
                        rss_after_bytes: rss_bytes(),
                        rgba_bytes_observed,
                    });
                }
            }
        }
        let report = serde_json::json!({
            "schema": 1,
            "measurement": "full scene construction, offscreen allocation, repaint, GPU completion and RGBA readback; excludes PNG encoding and event dispatch",
            "memory": "whole worker VmRSS, including drivers and allocator retention; not per-scene allocation or GPU memory",
            "baseline": "desktop-empty at the same resolution, interleaved with desktop-windows",
            "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
            "repeats": 5,
            "warmup_frames": 3,
            "frames_per_repeat": 5,
            "samples": samples,
        });
        std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    });
}

#[test]
#[ignore = "baselines are valid on the pinned renderer only: ci/goldens.sh"]
fn desktop_goldens_and_input_replay() {
    isolated("desktop_goldens_and_input_replay", false, |_| {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/goldens");
        let update = std::env::var("CUPERTINO_UPDATE_GOLDENS").as_deref() == Ok("1");
        if !baselines_apply(&root, update) {
            return;
        }
        let mut desktop = fixture(SIZE, 0);
        baseline(&root, "desktop-empty", &mut desktop, update);
        drop(desktop);
        let mut windows = fixture(SIZE, 2);
        if std::env::var("CUPERTINO_GOLDEN_MUTATE_WINDOW").as_deref() == Ok("1") {
            assert!(!update, "mutation runs may never update goldens");
            windows.shift_window_one_pixel();
        }
        baseline(&root, "desktop-windows", &mut windows, update);

        // XKB keycodes are evdev + 8. Ctrl+Up enters overview through the device entry and the real key router. The trace
        // is this injected gesture as written, not a capture of a device; re-recording writes it, a comparison replays it.
        let trace_path = root.join("overview-trace.json");
        let trace = if update {
            vec![
                Event {
                    milliseconds: 1100,
                    input: Input::Pointer { x: 500.0, y: 350.0 },
                },
                Event {
                    milliseconds: 1200,
                    input: Input::Key {
                        code: 37,
                        pressed: true,
                    },
                },
                Event {
                    milliseconds: 1201,
                    input: Input::Key {
                        code: 111,
                        pressed: true,
                    },
                },
                Event {
                    milliseconds: 1202,
                    input: Input::Key {
                        code: 111,
                        pressed: false,
                    },
                },
                Event {
                    milliseconds: 1203,
                    input: Input::Key {
                        code: 37,
                        pressed: false,
                    },
                },
            ]
        } else {
            serde_json::from_slice(&std::fs::read(&trace_path).unwrap()).unwrap()
        };
        for event in &trace {
            windows.input(event.clone());
        }
        assert!(
            windows.state.shell.overview_open(),
            "trace must open overview"
        );
        if update {
            std::fs::write(trace_path, serde_json::to_vec_pretty(&trace).unwrap()).unwrap();
        }
        windows.at(3000);
        baseline(&root, "overview", &mut windows, update);
        fail_after_update(update);
    });
}

#[test]
#[ignore = "baselines are valid on the pinned renderer only: ci/goldens.sh"]
fn shell_process_goldens() {
    isolated("shell_process_goldens", true, |bus| {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/goldens");
        let update = std::env::var("CUPERTINO_UPDATE_GOLDENS").as_deref() == Ok("1");
        if !baselines_apply(&root, update) {
            return;
        }
        let dock = || common::shell::process("cupertino-dock", bus, &[]);

        let mut scene = Scene::with_processes(SIZE, vec![dock()]);
        scene.wait_until("the dock bar", |s| {
            s.state.dock_rect(s.state.outputs[0].id).is_some()
        });
        scene.settle();
        baseline(&root, "dock", &mut scene, update);

        let mut scene = Scene::with_processes(SIZE, vec![dock()]);
        scene.window();
        scene.wait_until("the dock bar", |s| {
            s.state.dock_rect(s.state.outputs[0].id).is_some()
        });
        scene.state.execute_plan(ActionPlan {
            commands: Vec::new(),
            ops: vec![ShellOp::OpenForceQuit],
        });
        scene.wait_until("the dialog", |s| {
            s.mapped_layers("cupertino-force-quit") == 1
        });
        scene.settle();
        baseline(&root, "force-quit-dialog", &mut scene, update);

        let launcher = common::shell::process("cupertino-launcher", bus, &["--no-files"]);
        let mut scene = Scene::with_processes(SIZE, vec![launcher]);
        let connection =
            zbus::block_on(zbus::connection::Builder::address(bus).unwrap().build()).unwrap();
        scene.wait_until("the launcher to own its control name", |_| {
            has_owner(&connection, "org.cupertino.Launcher1")
        });
        scene.state.execute_plan(ActionPlan {
            commands: Vec::new(),
            ops: vec![ShellOp::ToggleLauncher],
        });
        scene.wait_until("the backdrop and the panel", |s| {
            s.mapped_layers("cupertino-launcher") == 2
        });
        scene.settle();
        baseline(&root, "launcher", &mut scene, update);

        // the lock process finds its session through logind on the system bus: a stand-in on a bus of its own
        let system = common::shell::private_bus();
        let _logind = fake_logind::serve(&system.address);
        let mut scene = Scene::with_processes(SIZE, vec![login_process(bus, &system.address)]);
        scene.wait_until("the lock process to own its control name", |_| {
            has_owner(&connection, "org.cupertino.Login1")
        });
        scene.state.execute_plan(ActionPlan {
            commands: Vec::new(),
            ops: vec![ShellOp::PowerDialog],
        });
        scene.wait_until("the power panel", |s| s.mapped_layers("login-panel") == 1);
        scene.settle();
        baseline(&root, "power-panel", &mut scene, update);
        // a lock request that finds no lock screen connected goes to the owner of the name, and the server trusts only
        // its own login process there: the one before must be gone first
        drop(scene);
        let deadline = Instant::now() + common::HANG_GUARD;
        while has_owner(&connection, "org.cupertino.Login1") {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for the lock process before to give up its control name"
            );
            std::thread::sleep(Duration::from_millis(2));
        }

        let mut scene = Scene::with_processes(SIZE, vec![login_process(bus, &system.address)]);
        scene.window();
        scene.wait_until("the lock process to own its control name", |_| {
            has_owner(&connection, "org.cupertino.Login1")
        });
        scene.state.lock_session();
        scene.wait_until("the lock screen", |s| {
            s.state.session_lock.is_active()
                && s.state.lock_surface_of(0).is_some_and(|surface| {
                    with_renderer_surface_state(&surface, |st| st.buffer().is_some())
                        .unwrap_or(false)
                })
        });
        scene.settle();
        baseline(&root, "lock-surface", &mut scene, update);
        fail_after_update(update);
    });
}

/// The lock process on the session bus `bus`, finding its session through the stand-in logind on `system`.
fn login_process(bus: &str, system: &str) -> ShellProcess {
    let mut login = common::shell::process("cupertino-login", bus, &[]);
    login.command.splice(
        1..1,
        [
            format!("DBUS_SYSTEM_BUS_ADDRESS={system}"),
            format!("XDG_SESSION_ID={}", fake_logind::SESSION_ID),
        ],
    );
    login
}

/// Whether `name` has an owner on the bus of `connection`.
fn has_owner(connection: &zbus::Connection, name: &str) -> bool {
    zbus::block_on(connection.call_method(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        Some("org.freedesktop.DBus"),
        "NameHasOwner",
        &name,
    ))
    .unwrap()
    .body()
    .deserialize::<bool>()
    .unwrap()
}

/// A stand-in for logind for the lock process: session [`SESSION_ID`] is active on `seat0`, every power action is
/// allowed, and each inhibitor is one end of a socket pair whose other end the stand-in keeps.
mod fake_logind {
    use std::os::fd::OwnedFd;
    use std::sync::Mutex;
    use zbus::zvariant::{self, OwnedObjectPath};

    pub const SESSION_ID: &str = "1";
    const SESSION_PATH: &str = "/org/freedesktop/login1/session/_31";

    struct Manager {
        held: Mutex<Vec<OwnedFd>>,
    }

    #[zbus::interface(name = "org.freedesktop.login1.Manager")]
    impl Manager {
        fn get_session(&self, id: &str) -> zbus::fdo::Result<OwnedObjectPath> {
            if id != SESSION_ID {
                return Err(zbus::fdo::Error::Failed(format!("no session {id}")));
            }
            Ok(OwnedObjectPath::try_from(SESSION_PATH).unwrap())
        }

        fn inhibit(
            &self,
            _what: &str,
            _who: &str,
            _why: &str,
            _mode: &str,
        ) -> zbus::fdo::Result<zvariant::OwnedFd> {
            let (kept, given) = std::os::unix::net::UnixStream::pair()
                .map_err(|e| zbus::fdo::Error::IOError(e.to_string()))?;
            self.held.lock().unwrap().push(kept.into());
            Ok(OwnedFd::from(given).into())
        }

        fn can_suspend(&self) -> String {
            "yes".to_owned()
        }

        fn can_reboot(&self) -> String {
            "yes".to_owned()
        }

        fn can_power_off(&self) -> String {
            "yes".to_owned()
        }
    }

    struct Session;

    #[zbus::interface(name = "org.freedesktop.login1.Session")]
    impl Session {
        #[zbus(property)]
        fn id(&self) -> String {
            SESSION_ID.to_owned()
        }

        #[zbus(property, name = "Type")]
        fn session_type(&self) -> String {
            "wayland".to_owned()
        }

        #[zbus(property)]
        fn active(&self) -> bool {
            true
        }

        #[zbus(property, name = "VTNr")]
        fn vt_nr(&self) -> u32 {
            2
        }

        #[zbus(property)]
        fn seat(&self) -> (String, OwnedObjectPath) {
            (
                "seat0".to_owned(),
                OwnedObjectPath::try_from("/org/freedesktop/login1/seat/seat0").unwrap(),
            )
        }
    }

    /// Serve the stand-in on the bus at `address` for as long as the returned connection lives.
    pub fn serve(address: &str) -> zbus::blocking::Connection {
        zbus::blocking::connection::Builder::address(address)
            .unwrap()
            .name("org.freedesktop.login1")
            .unwrap()
            .serve_at(
                "/org/freedesktop/login1",
                Manager {
                    held: Mutex::new(Vec::new()),
                },
            )
            .unwrap()
            .serve_at(SESSION_PATH, Session)
            .unwrap()
            .build()
            .unwrap()
    }
}

/// The goldens' input device: a keyboard and an absolute pointer whose events enter the server through
/// `process_input_event`, as a real device's do. A golden records positions as points of the first output; the
/// absolute pointer reports each as a share of its range, which the server spans over the output's panel pixels.
mod injected {
    use super::{Event, Input};
    use smithay::backend::input::{
        AbsolutePositionEvent, ButtonState, DeviceCapability, Event as InputEventTrait,
        InputBackend, InputEvent, KeyState, KeyboardKeyEvent, PointerButtonEvent,
        PointerMotionAbsoluteEvent, UnusedEvent,
    };
    use smithay::input::keyboard::Keycode;
    use std::path::PathBuf;

    #[derive(Clone, Debug, PartialEq, Eq, Hash)]
    pub struct Device;

    impl smithay::backend::input::Device for Device {
        fn id(&self) -> String {
            "goldens-input".to_owned()
        }

        fn name(&self) -> String {
            "goldens input".to_owned()
        }

        fn has_capability(&self, capability: DeviceCapability) -> bool {
            matches!(
                capability,
                DeviceCapability::Keyboard | DeviceCapability::Pointer
            )
        }

        fn usb_id(&self) -> Option<(u32, u32)> {
            None
        }

        fn syspath(&self) -> Option<PathBuf> {
            None
        }
    }

    #[derive(Debug)]
    pub struct Backend;

    impl InputBackend for Backend {
        type Device = Device;
        type KeyboardKeyEvent = Key;
        type PointerAxisEvent = UnusedEvent;
        type PointerButtonEvent = Button;
        type PointerMotionEvent = UnusedEvent;
        type PointerMotionAbsoluteEvent = Absolute;
        type GestureSwipeBeginEvent = UnusedEvent;
        type GestureSwipeUpdateEvent = UnusedEvent;
        type GestureSwipeEndEvent = UnusedEvent;
        type GesturePinchBeginEvent = UnusedEvent;
        type GesturePinchUpdateEvent = UnusedEvent;
        type GesturePinchEndEvent = UnusedEvent;
        type GestureHoldBeginEvent = UnusedEvent;
        type GestureHoldEndEvent = UnusedEvent;
        type TouchDownEvent = UnusedEvent;
        type TouchUpEvent = UnusedEvent;
        type TouchMotionEvent = UnusedEvent;
        type TouchCancelEvent = UnusedEvent;
        type TouchFrameEvent = UnusedEvent;
        type TabletToolAxisEvent = UnusedEvent;
        type TabletToolProximityEvent = UnusedEvent;
        type TabletToolTipEvent = UnusedEvent;
        type TabletToolButtonEvent = UnusedEvent;
        type SwitchToggleEvent = UnusedEvent;
        type SpecialEvent = UnusedEvent;
    }

    macro_rules! timed {
        ($($event:ident),+) => {$(
            impl InputEventTrait<Backend> for $event {
                fn time(&self) -> u64 {
                    u64::from(self.milliseconds) * 1000
                }

                fn device(&self) -> Device {
                    Device
                }
            }
        )+};
    }

    pub struct Key {
        milliseconds: u32,
        code: u32,
        pressed: bool,
    }

    pub struct Button {
        milliseconds: u32,
        code: u32,
        pressed: bool,
    }

    pub struct Absolute {
        milliseconds: u32,
        /// The position as a share of the device's range, 0 to 1 along each axis.
        x: f64,
        y: f64,
    }

    timed!(Key, Button, Absolute);

    impl KeyboardKeyEvent<Backend> for Key {
        fn key_code(&self) -> Keycode {
            self.code.into()
        }

        fn state(&self) -> KeyState {
            if self.pressed {
                KeyState::Pressed
            } else {
                KeyState::Released
            }
        }

        fn count(&self) -> u32 {
            u32::from(self.pressed)
        }
    }

    impl PointerButtonEvent<Backend> for Button {
        fn button_code(&self) -> u32 {
            self.code
        }

        fn state(&self) -> ButtonState {
            if self.pressed {
                ButtonState::Pressed
            } else {
                ButtonState::Released
            }
        }
    }

    impl AbsolutePositionEvent<Backend> for Absolute {
        fn x(&self) -> f64 {
            self.x
        }

        fn y(&self) -> f64 {
            self.y
        }

        fn x_transformed(&self, width: i32) -> f64 {
            self.x * f64::from(width)
        }

        fn y_transformed(&self, height: i32) -> f64 {
            self.y * f64::from(height)
        }
    }

    impl PointerMotionAbsoluteEvent<Backend> for Absolute {}

    /// The device event of `event`, on an output `logical` points wide and tall.
    pub fn event(event: &Event, logical: (f64, f64)) -> InputEvent<Backend> {
        let milliseconds = event.milliseconds;
        match event.input {
            Input::Pointer { x, y } => InputEvent::PointerMotionAbsolute {
                event: Absolute {
                    milliseconds,
                    x: x / logical.0,
                    y: y / logical.1,
                },
            },
            Input::Button { code, pressed } => InputEvent::PointerButton {
                event: Button {
                    milliseconds,
                    code,
                    pressed,
                },
            },
            Input::Key { code, pressed } => InputEvent::Keyboard {
                event: Key {
                    milliseconds,
                    code,
                    pressed,
                },
            },
        }
    }
}
