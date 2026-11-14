//! The toolkit's GPU presenter, end to end in a headless session: the presenter's check window
//! (`cupertino-ui-wayland`'s `gpu-window`) draws its first frames into shared memory while its device warms up, then
//! uploads every frame into dmabufs and attaches them with syncobj points, or without the protocol after its own fence.
//! The losses, the per-surface fallback and the resize storm are driven through the window's options. Tests that need
//! a render node skip without one, unless `CUPERTINO_GPU_REQUIRED=1` (the local GPU tier) makes that a failure.

use crate::common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use cupertino_server::backend::headless;
use cupertino_server::config::Config;
use cupertino_server::Cupertino;
use smithay::backend::allocator::Buffer as _;
use smithay::backend::renderer::utils::RendererSurfaceStateUserData;
use smithay::input::pointer::CursorImageStatus;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use smithay::wayland::compositor::with_states;
use smithay::wayland::dmabuf::get_dmabuf;
use smithay::wayland::shm::with_buffer_contents;

const OUTPUT: (i32, i32) = (1400, 1000);
/// The modifier the target machine's driver picks for a rendered ARGB8888 buffer (its default tiled layout).
const NVIDIA_DEFAULT_MODIFIER: &str = "0x0300000000e08014";

fn gpu_required() -> bool {
    std::env::var("CUPERTINO_GPU_REQUIRED").as_deref() == Ok("1")
}

/// Skip (or, in the GPU tier, fail) a test whose machine cannot run it.
fn unavailable(why: &str) {
    assert!(!gpu_required(), "CUPERTINO_GPU_REQUIRED=1: {why}");
    eprintln!("SKIPPED: {why}; set CUPERTINO_GPU_REQUIRED=1 to fail instead");
}

/// What the session saw of the window's buffers. A shared-memory buffer still shown beside a syncobj object is no fault
/// (the first dmabuf commit after it is held for its acquire point); a shared-memory commit on a surface with one is,
/// and the session ends the client for it, which fails the test.
#[derive(Default, Debug)]
struct Shown {
    modifiers: Vec<u64>,
    dmabuf: u64,
    shm: u64,
}

struct Session {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
    home: tempfile::TempDir,
    /// The window runs in a sandbox that hides `/dev/dri` (a user and mount namespace of its own).
    sandboxed: bool,
}

impl Session {
    /// A headless session; with `explicit_sync`, one that must offer syncobj timelines.
    fn start(explicit_sync: bool) -> Option<Session> {
        let event_loop = EventLoop::try_new().unwrap();
        let mut state = Cupertino::new(
            Display::new().unwrap(),
            event_loop.handle(),
            event_loop.get_signal(),
            Config::default(),
            true,
            &cupertino_assets::Assets::builtin(),
        )
        .unwrap();
        state.withhold_explicit_sync = !explicit_sync;
        state.cursor_status = CursorImageStatus::Hidden;
        if let Err(e) = headless::init(&mut state, OUTPUT, 1.0) {
            unavailable(&format!("no EGL device ({e:#})"));
            return None;
        }
        let mut s = Session {
            state,
            event_loop,
            home: tempfile::tempdir().unwrap(),
            sandboxed: false,
        };
        let renderer = s.state.backend.as_mut().unwrap().renderer();
        if cupertino_server::backend::render_node(renderer).is_none() {
            unavailable("the device has no render node");
            return None;
        }
        if explicit_sync && s.state.syncobj_state.is_none() {
            unavailable("the device has no render node with syncobj timelines");
            return None;
        }
        Some(s)
    }

    fn pump(&mut self) {
        headless::step(
            &mut self.state,
            &mut self.event_loop,
            Duration::from_millis(2),
        )
        .unwrap();
    }

    /// The window on this session with `args`, its report and its runtime directory (where the toolkit records its
    /// renderer) in `home`.
    fn window(&self, home: &Path, args: &[&str]) -> (Child, PathBuf) {
        let report = home.join(format!("report-{}", args.join("_")));
        let binary = common::fixtures::artifact("cupertino-ui-wayland", "example", "gpu-window");
        let runtime = home.join("runtime");
        std::fs::create_dir_all(&runtime).unwrap();
        let socket = Path::new(&std::env::var_os("XDG_RUNTIME_DIR").unwrap())
            .join(self.state.socket_name.as_ref().unwrap());
        let mut command = if self.sandboxed {
            let mut command = common::spawn::cleared("unshare", home);
            command
                .args(["--user", "--map-root-user", "--mount", "sh", "-c"])
                .arg(r#"mount -t tmpfs none /dev/dri && exec "$0" "$@""#)
                .arg(binary);
            command
        } else {
            common::spawn::cleared(binary, home)
        };
        command
            .env("XDG_RUNTIME_DIR", &runtime)
            .env("WAYLAND_DISPLAY", socket)
            .args(args)
            .arg("--report")
            .arg(&report)
            .stdout(Stdio::null());
        for var in [
            "LIBGL_ALWAYS_SOFTWARE",
            "__EGL_VENDOR_LIBRARY_FILENAMES",
            "GALLIUM_DRIVER",
        ] {
            if let Some(value) = std::env::var_os(var) {
                command.env(var, value);
            }
        }
        (command.spawn().expect("the gpu window"), report)
    }

    /// Run the window with `args` until it ends, calling `each` every pass; its report and what the session showed.
    fn run(
        &mut self,
        home: &Path,
        args: &[&str],
        mut each: impl FnMut(&mut Session, &Shown),
    ) -> (BTreeMap<String, String>, Shown) {
        let (mut child, report) = self.window(home, args);
        let mut shown = Shown::default();
        let end = Instant::now() + common::HANG_GUARD;
        loop {
            self.pump();
            self.look(&mut shown);
            each(self, &shown);
            if let Some(status) = child.try_wait().unwrap() {
                for _ in 0..10 {
                    self.pump();
                }
                let text = std::fs::read_to_string(&report).unwrap_or_default();
                assert!(
                    status.success(),
                    "the gpu window failed (a protocol error ends it): {status}"
                );
                let report = text
                    .lines()
                    .map(|l| l.split_once(' ').unwrap_or((l, "")))
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect();
                eprintln!("{args:?}: {report:#?} {shown:?}");
                return (report, shown);
            }
            if Instant::now() > end {
                let _ = child.kill();
                let _ = child.wait();
                panic!("the gpu window did not finish");
            }
        }
    }

    /// Configure the window to `size` (logical points), as a resize by the window manager does.
    fn resize(&self, size: (i32, i32)) {
        if let Some(window) = self.state.windows.iter().find(|w| w.mapped) {
            window
                .toplevel
                .with_pending_state(|st| st.size = Some(size.into()));
            window.toplevel.send_configure();
        }
    }

    /// Note the window's current buffer: a dmabuf (with its modifier) or shared memory.
    fn look(&self, shown: &mut Shown) {
        let Some(window) = self.state.windows.iter().find(|w| w.mapped) else {
            return;
        };
        let surface = window.toplevel.wl_surface();
        let seen = with_states(surface, |states| {
            let buffer = states
                .data_map
                .get::<RendererSurfaceStateUserData>()
                .and_then(|s| s.lock().unwrap().buffer().cloned())?;
            let modifier = get_dmabuf(&buffer)
                .ok()
                .map(|d| u64::from(d.format().modifier));
            let shm = with_buffer_contents(&buffer, |_, _, _| ()).is_ok();
            (modifier.is_some() || shm).then_some(modifier)
        });
        match seen {
            Some(Some(modifier)) => {
                shown.dmabuf += 1;
                if !shown.modifiers.contains(&modifier) {
                    shown.modifiers.push(modifier);
                }
            }
            Some(None) => shown.shm += 1,
            None => {}
        }
    }
}

fn number(report: &BTreeMap<String, String>, key: &str) -> u64 {
    report[key]
        .parse()
        .unwrap_or_else(|_| panic!("{key}: {}", report[key]))
}

fn modifier(report: &BTreeMap<String, String>) -> u64 {
    u64::from_str_radix(report["modifier"].trim_start_matches("0x"), 16).unwrap()
}

/// One warm-up of the report: the switch after the start, the context and the programs (in ms), and the programs
/// loaded and compiled.
fn warm_ups(report: &BTreeMap<String, String>) -> Vec<(f64, f64, f64, u64, u64)> {
    report["warm_ups"]
        .split(',')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let v: Vec<u64> = w.split(' ').map(|n| n.parse().unwrap()).collect();
            let ms = |us: u64| us as f64 / 1000.0;
            (ms(v[0]), ms(v[1]), ms(v[2]), v[3], v[4])
        })
        .collect()
}

#[test]
fn the_presenter_warms_up_beside_its_first_frames_then_attaches_dmabufs_with_syncobj_points() {
    let Some(mut s) = Session::start(true) else {
        return;
    };
    let home = s.home.path().to_path_buf();
    let (report, shown) = s.run(&home, &["--frames", "180"], |_, _| {});
    assert_eq!(report["renderer"], "gpu");
    assert_eq!(report["reason"], "asked for the GPU");
    assert_eq!(report["to_gpu"], "1");
    assert_eq!(report["explicit_sync"], "1");
    assert_eq!(report["buffers"], "2");
    assert_eq!(report["allocations"], "2");
    assert_eq!(
        report["fence_waits"], "0",
        "explicit sync never waits on the CPU"
    );
    let gpu_frames = number(&report, "gpu_frames");
    assert!(gpu_frames > 100, "{gpu_frames} frames from dmabufs");
    assert!(
        number(&report, "release_waits") + 2 >= gpu_frames,
        "every buffer but the last two came back through its release point"
    );
    assert_eq!(report["syncobj_surfaces_created"], "1");
    assert_eq!(report["shm_with_syncobj"], "0");
    assert_eq!(
        shown.modifiers,
        [modifier(&report)],
        "the compositor imported the buffers in the layout the client drew"
    );
    if report["device"].contains("NVIDIA") {
        assert_eq!(report["modifier"], NVIDIA_DEFAULT_MODIFIER);
    }
    // the frames before the switch went out through shared memory
    let shm_frames = number(&report, "shm_frames");
    if shm_frames > 0 {
        assert_eq!(report["first_frame_renderer"], "cpu");
        assert!(shown.shm > 0);
    }
    let record = std::fs::read_to_string(
        home.join("runtime/cupertino/renderers")
            .read_dir()
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path(),
    )
    .unwrap();
    assert_eq!(
        record,
        "program=gpu-window\nrenderer=gpu\nreason=asked for the GPU\n"
    );
}

/// The first start compiles the programs and saves them; the next start of the same user loads them. Each start runs
/// long enough for its warm-up, which makes the presenter's programs and the GPU renderer's, to end.
#[test]
fn the_presenter_loads_the_programs_it_saved_on_its_next_start() {
    let Some(mut s) = Session::start(true) else {
        return;
    };
    let home = s.home.path().to_path_buf();
    let (cold, _) = s.run(&home, &["--frames", "90"], |_, _| {});
    let (warm, _) = s.run(&home, &["--frames", "90"], |_, _| {});
    let (cold_up, warm_up) = (warm_ups(&cold)[0], warm_ups(&warm)[0]);
    eprintln!(
        "first frame {} ms ({}) and {} ms ({}); cold warm-up: GPU after {:.1} ms (context {:.1} ms, programs {:.1} ms); \
         with the cache: after {:.1} ms (context {:.1} ms, programs {:.1} ms)",
        cold["first_frame_ms"],
        cold["first_frame_renderer"],
        warm["first_frame_ms"],
        warm["first_frame_renderer"],
        cold_up.0,
        cold_up.1,
        cold_up.2,
        warm_up.0,
        warm_up.1,
        warm_up.2,
    );
    assert!(cold_up.4 > 0, "the first start compiles: {cold_up:?}");
    // the first start loads only what it saved itself a moment before: the fill program made a second time, without
    // blending
    assert!(
        cold_up.3 < cold_up.4,
        "the first start compiles what it has not saved: {cold_up:?}"
    );
    assert_eq!(
        (warm_up.3, warm_up.4),
        (cold_up.3 + cold_up.4, 0),
        "the next start loads every program and compiles none"
    );
    assert!(home.join(".cache/cupertino/programs").is_dir());
}

#[test]
fn the_presenter_without_explicit_sync_waits_for_its_own_fence_and_keeps_three_buffers() {
    let Some(mut s) = Session::start(false) else {
        return;
    };
    let home = s.home.path().to_path_buf();
    let (report, shown) = s.run(&home, &["--frames", "180"], |_, _| {});
    assert_eq!(report["renderer"], "gpu");
    assert_eq!(report["explicit_sync"], "0");
    assert_eq!(report["buffers"], "3");
    assert_eq!(report["allocations"], "3");
    let gpu_frames = number(&report, "gpu_frames");
    assert!(gpu_frames > 100);
    assert_eq!(
        number(&report, "fence_waits"),
        gpu_frames,
        "every commit waited for the frame's fence"
    );
    assert_eq!(report["release_waits"], "0");
    assert_eq!(report["syncobj_surfaces_created"], "0");
    assert_eq!(shown.modifiers, [modifier(&report)]);
}

/// Three resets within a minute leave the process on the CPU; each before it made a new device and drew on it again.
/// Frames go out through shared memory meanwhile, never on a surface that still has a syncobj object.
#[test]
fn the_presenter_makes_a_new_device_after_a_reset_and_stays_on_the_cpu_after_the_third() {
    let Some(mut s) = Session::start(true) else {
        return;
    };
    let home = s.home.path().to_path_buf();
    let (report, shown) = s.run(
        &home,
        &["--frames", "240", "--reset-every", "20", "--losses", "3"],
        |_, _| {},
    );
    assert_eq!(report["losses"], "3");
    assert_eq!(
        (report["to_gpu"].as_str(), report["to_cpu"].as_str()),
        ("3", "3")
    );
    assert_eq!(report["renderer"], "cpu");
    assert_eq!(report["reason"], "the context was lost 3 times within 60 s");
    assert_eq!(warm_ups(&report).len(), 3);
    assert!(number(&report, "gpu_frames") >= 60);
    assert!(number(&report, "shm_frames") > 0);
    assert_eq!(report["syncobj_surfaces_created"], "3");
    assert_eq!(
        report["syncobj_surfaces_destroyed"], "3",
        "each loss destroyed the syncobj object before the next shared-memory frame"
    );
    assert_eq!(report["shm_with_syncobj"], "0");
    assert!(shown.dmabuf > 0 && shown.shm > 0);
    let record = std::fs::read_to_string(
        home.join("runtime/cupertino/renderers")
            .read_dir()
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path(),
    )
    .unwrap();
    assert!(
        record.contains("renderer=cpu\nreason=the context was lost"),
        "{record}"
    );
    let events = recorded_events(&home);
    let fallbacks = |scope: &str| {
        events
            .lines()
            .filter(|l| l.contains("\"toolkit.renderer.fallback\"") && l.contains(scope))
            .count()
    };
    assert_eq!(fallbacks("\"scope\":\"loss\""), 3, "each reset is recorded");
    assert_eq!(
        fallbacks("\"scope\":\"process\""),
        1,
        "and the CPU for good"
    );
}

/// A purge is not the work's fault: the process makes a new device every time and stays on the GPU.
#[test]
fn the_presenter_never_counts_a_purge_toward_the_cpu() {
    let Some(mut s) = Session::start(true) else {
        return;
    };
    let home = s.home.path().to_path_buf();
    let (report, shown) = s.run(
        &home,
        &["--frames", "240", "--purge-every", "20", "--losses", "4"],
        |_, _| {},
    );
    assert_eq!(report["losses"], "4");
    assert_eq!(
        (report["to_gpu"].as_str(), report["to_cpu"].as_str()),
        ("5", "4")
    );
    assert_eq!(report["renderer"], "gpu");
    assert!(
        !recorded_events(&home).contains("toolkit.renderer.fallback"),
        "a purge is no fallback"
    );
    let ups = warm_ups(&report);
    assert_eq!(ups.len(), 5);
    assert!(
        ups[1..].iter().all(|u| u.4 == 0),
        "a new device after a purge loads the programs saved by the first: {ups:?}"
    );
    assert_eq!(report["shm_with_syncobj"], "0");
    assert!(shown.dmabuf > 0 && shown.shm > 0, "{shown:?}");
}

/// A buffer the compositor refuses to import moves the surface to shared memory for good, its syncobj object destroyed
/// before the first shared-memory attach; the process stays on the GPU.
#[test]
fn the_presenter_falls_back_to_shared_memory_for_a_surface_whose_buffer_is_refused() {
    let Some(mut s) = Session::start(true) else {
        return;
    };
    let home = s.home.path().to_path_buf();
    let mut grown = false;
    let (report, shown) = s.run(&home, &["--frames", "90"], |s, shown| {
        if shown.dmabuf > 20 && !grown {
            grown = true;
            s.state.refuse_dmabuf_imports = true;
            s.resize((900, 700));
        }
    });
    assert_eq!(report["fell_back"], "1");
    assert_eq!(report["renderer"], "gpu", "the process stays on the GPU");
    assert!(
        recorded_events(&home)
            .lines()
            .any(|l| l.contains("\"toolkit.renderer.fallback\"")
                && l.contains("\"scope\":\"surface\"")),
        "the surface's fallback is recorded"
    );
    assert!(number(&report, "gpu_frames") >= 1);
    assert!(number(&report, "shm_frames") > 0);
    assert_eq!(report["syncobj_surfaces_created"], "1");
    assert_eq!(report["syncobj_surfaces_destroyed"], "1");
    assert_eq!(report["shm_with_syncobj"], "0");
    assert!(shown.dmabuf > 0 && shown.shm > 0, "{shown:?}");
}

/// A window that asks for another size every frame allocates buffers only when a frame leaves its size step, and
/// shows every size cropped from them.
#[test]
fn the_presenter_allocates_in_size_steps_through_a_resize_storm() {
    let Some(mut s) = Session::start(true) else {
        return;
    };
    let home = s.home.path().to_path_buf();
    let mut pass = 0i32;
    let (report, shown) = s.run(&home, &["--frames", "240"], |s, shown| {
        if shown.dmabuf > 0 {
            pass += 1;
            // a drag: a point bigger every four passes up to two size steps more, then smaller again
            let t = (pass / 4) % 512;
            let grown = if t < 256 { t } else { 512 - t };
            s.resize((480 + grown, 360 + grown));
        }
    });
    assert_eq!(report["renderer"], "gpu");
    let (sizes, allocations, gpu_frames) = (
        number(&report, "sizes"),
        number(&report, "allocations"),
        number(&report, "gpu_frames"),
    );
    assert!(sizes > 50, "{sizes} sizes");
    // two buffers each time a frame leaves its step: about one step in 128 pixels of travel
    assert!(
        allocations <= 24 && allocations * 10 <= gpu_frames,
        "{allocations} buffers for {gpu_frames} frames of {sizes} sizes"
    );
    assert_eq!(shown.modifiers, [modifier(&report)]);
}

/// A short soak: many frames, every one from a dmabuf after the switch, each buffer back through its release point.
#[test]
fn the_presenter_keeps_its_fences_in_step_through_a_short_soak() {
    let Some(mut s) = Session::start(true) else {
        return;
    };
    let home = s.home.path().to_path_buf();
    let (report, shown) = s.run(&home, &["--frames", "900"], |_, _| {});
    let gpu_frames = number(&report, "gpu_frames");
    // the frames drawn while the device warmed up come on top of the 900
    assert_eq!(
        number(&report, "frames") - number(&report, "shm_frames_while_warming"),
        900
    );
    assert!(gpu_frames >= 850, "{gpu_frames}");
    assert!(number(&report, "release_waits") + 2 >= gpu_frames);
    assert_eq!(report["allocations"], "2");
    assert_eq!(report["fence_waits"], "0");
    assert_eq!(shown.modifiers, [modifier(&report)]);
}

/// The colours and the mark the window draws with `--renderer` (`gpu-window`'s constants), at scale 1.
const RENDERER_COLOUR: [u8; 4] = [51, 115, 217, 255];
const MARK_COLOUR: [u8; 4] = [230, 64, 26, 255];
const MARK_AT: (u32, u32) = (16, 24);
const MARK_SIZE: (u32, u32) = (40, 20);

impl Session {
    fn screenshot(&mut self) -> image::RgbaImage {
        let path = self.home.path().join("shot.png");
        self.state.screenshot_requests.push(path.clone());
        self.state.request_redraw();
        let end = Instant::now() + common::HANG_GUARD;
        while !path.exists() {
            self.pump();
            assert!(Instant::now() < end, "the screenshot was not taken");
        }
        let img = image::open(&path).unwrap().to_rgba8();
        let _ = std::fs::remove_file(path);
        img
    }

    /// The window's mark on screen once the window shows it at its size (past the opening animation): its first
    /// pixel relative to the window's frame, and its size; `None` while it is not yet shown so.
    fn mark(&mut self) -> Option<((i64, i64), (u32, u32))> {
        let shot = self.screenshot();
        let window = self.state.windows.iter().find(|w| w.mapped)?;
        let frame = self.state.wm.window(window.id)?.geometry();
        let near = |px: [u8; 4], c: [u8; 4]| px.iter().zip(c).all(|(a, b)| a.abs_diff(b) <= 1);
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
        for (x, y, px) in shot.enumerate_pixels() {
            if near(px.0, MARK_COLOUR) {
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
            }
        }
        if x0 >= x1 || (x1 - x0, y1 - y0) != MARK_SIZE {
            return None;
        }
        // the window's own colour all around the mark, so the mark lies where it was drawn, the right way up
        for (x, y) in [
            (x0 - 4, y0 + 4),
            (x1 + 4, y0 + 4),
            (x0 + 4, y0 - 4),
            (x0 + 4, y1 + 4),
        ] {
            if !near(shot.get_pixel(x, y).0, RENDERER_COLOUR) {
                return None;
            }
        }
        Some((
            (
                i64::from(x0) - frame.x as i64,
                i64::from(y0) - frame.y as i64,
            ),
            (x1 - x0, y1 - y0),
        ))
    }
}

/// A window on the GPU is drawn by the toolkit's GPU renderer on the presenter's device: every frame after the switch
/// is copied from the renderer's texture into a dmabuf, nothing uploaded, and the screen shows what it drew where it
/// drew it, also at a size between the buffers' size steps (the buffer larger than the frame, cropped).
#[test]
fn the_presenter_shows_what_the_gpu_renderer_drew_from_its_texture_in_dmabufs() {
    let Some(mut s) = Session::start(true) else {
        return;
    };
    let home = s.home.path().to_path_buf();
    let mut marks = Vec::new();
    let mut resized = false;
    let end = Instant::now() + common::HANG_GUARD;
    let (report, shown) = s.run(&home, &["--frames", "600", "--renderer"], |s, shown| {
        if shown.dmabuf < 30 || marks.len() >= 2 || Instant::now() > end {
            return;
        }
        if let Some(mark) = s.mark() {
            marks.push(mark);
            if !resized {
                resized = true;
                s.resize((333, 251));
            }
        }
    });
    assert_eq!(report["renderer"], "gpu");
    let gpu_frames = number(&report, "gpu_frames");
    assert!(gpu_frames > 100, "{gpu_frames} frames from dmabufs");
    assert_eq!(
        number(&report, "from_texture"),
        gpu_frames,
        "every frame on the GPU came from the renderer's texture"
    );
    assert!(number(&report, "renderer_frames") >= gpu_frames);
    assert_eq!(shown.modifiers, [modifier(&report)]);
    assert_eq!(
        marks.len(),
        2,
        "the mark at the window's two sizes: {marks:?}"
    );
    // the mark keeps its place in the window at both sizes
    assert_eq!(marks[0], marks[1]);
    let (at, size) = marks[0];
    assert_eq!(size, MARK_SIZE);
    assert!(
        at.0 >= i64::from(MARK_AT.0) - 1 && at.1 >= i64::from(MARK_AT.1) - 1,
        "the mark at {at:?} of the window's frame"
    );
}

/// The diagnostic events the window recorded in its store (`XDG_STATE_HOME` under `home`), every line of every segment.
fn recorded_events(home: &Path) -> String {
    let mut text = String::new();
    let mut dirs = vec![home.join(".local/state/cupertino/events")];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            } else {
                text += &std::fs::read_to_string(&path).unwrap_or_default();
            }
        }
    }
    text
}

/// The renderer record the window left in its runtime directory under `home`.
fn renderer_record(home: &Path) -> String {
    std::fs::read_to_string(
        home.join("runtime/cupertino/renderers")
            .read_dir()
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path(),
    )
    .unwrap()
}

/// A process of the CPU's class draws every frame into shared memory and makes no device, however large its window.
#[test]
fn a_cpu_class_draws_on_the_cpu_however_large_its_window() {
    let Some(mut s) = Session::start(true) else {
        return;
    };
    let home = s.home.path().to_path_buf();
    let (report, shown) = s.run(
        &home,
        &["--class", "cpu", "--size", "1300x900", "--frames", "150"],
        |_, _| {},
    );
    assert_eq!(report["class"], "cpu");
    assert_eq!(report["renderer"], "cpu");
    assert_eq!(report["reason"], "its class draws on the CPU");
    assert_eq!(
        (report["to_gpu"].as_str(), report["gpu_frames"].as_str()),
        ("0", "0")
    );
    assert_eq!(report["warm_ups"], "");
    assert_eq!(shown.dmabuf, 0);
    assert!(!recorded_events(&home).contains("toolkit.renderer.fallback"));
}

/// A process of the size and repaint rule's class starts on the CPU; a window over a quarter of its output moves it to
/// the GPU with its first frame, whose successors are drawn on the CPU only while the device warms up.
#[test]
fn the_rule_moves_a_process_to_the_gpu_once_its_window_covers_more_than_a_quarter_of_its_output() {
    let Some(mut s) = Session::start(true) else {
        return;
    };
    let home = s.home.path().to_path_buf();
    // 1000 x 800 of the 1400 x 1000 output
    let (report, shown) = s.run(
        &home,
        &["--class", "by-use", "--size", "1000x800", "--frames", "120"],
        |_, _| {},
    );
    assert_eq!(report["class"], "by use");
    assert_eq!(
        report["shm_frames_before_switch"], "1",
        "the first frame was large"
    );
    assert_eq!(report["still_waiting"], "0");
    assert_eq!(report["renderer"], "gpu");
    assert_eq!(report["reason"], "the GPU");
    assert_eq!(report["to_gpu"], "1");
    assert!(number(&report, "gpu_frames") > 60);
    assert!(shown.dmabuf > 0);
}

/// A small window of the rule's class stays on the CPU while it presents at most 30 frames a second; one that presents
/// faster for two whole seconds moves its process to the GPU.
#[test]
fn the_rule_moves_a_process_to_the_gpu_once_a_small_window_presented_fast_for_two_seconds() {
    let Some(mut s) = Session::start(true) else {
        return;
    };
    let home = s.home.path().to_path_buf();
    let started = Instant::now();
    let (report, _) = s.run(
        &home,
        &["--class", "by-use", "--size", "320x240", "--frames", "600"],
        |_, _| {},
    );
    let before = number(&report, "shm_frames_before_switch");
    assert!(
        before > 2 * 30,
        "{before} frames on the CPU before the switch, two seconds of more than 30"
    );
    assert!(started.elapsed() >= Duration::from_secs(2));
    assert_eq!(report["still_waiting"], "0");
    assert_eq!(report["renderer"], "gpu");
    assert_eq!(report["to_gpu"], "1");
}

/// A process of the GPU's class in a sandbox that hides `/dev/dri` cannot reach the device the compositor names: it
/// draws on the CPU from its first frame to its last, says why in its record, and records the fallback as a
/// diagnostic event.
#[test]
fn a_sandboxed_process_without_the_render_node_draws_on_the_cpu_and_records_the_fallback() {
    let Some(mut s) = Session::start(true) else {
        return;
    };
    let probe = std::process::Command::new("unshare")
        .args(["--user", "--map-root-user", "--mount", "sh", "-c"])
        .arg("mount -t tmpfs none /dev/dri && test -z \"$(ls /dev/dri)\"")
        .status();
    if !probe.is_ok_and(|s| s.success()) {
        unavailable("no user and mount namespaces to hide /dev/dri in");
        return;
    }
    s.sandboxed = true;
    let home = s.home.path().to_path_buf();
    let (report, shown) = s.run(&home, &["--class", "gpu", "--frames", "60"], |_, _| {});
    assert_eq!(report["renderer"], "cpu");
    assert_eq!(report["reason"], "no render node for the dmabuf device");
    assert_eq!(
        (report["to_gpu"].as_str(), report["gpu_frames"].as_str()),
        ("0", "0")
    );
    assert_eq!(report["first_frame_renderer"], "cpu");
    assert!(shown.shm > 0 && shown.dmabuf == 0);
    assert_eq!(
        renderer_record(&home),
        "program=gpu-window\nrenderer=cpu\nreason=no render node for the dmabuf device\n"
    );
    let events = recorded_events(&home);
    let fallback = events
        .lines()
        .find(|l| l.contains("\"toolkit.renderer.fallback\""))
        .unwrap_or_else(|| panic!("no fallback event in {events}"));
    assert!(
        fallback.contains("\"scope\":\"process\"")
            && fallback.contains("no render node for the dmabuf device"),
        "{fallback}"
    );
}
