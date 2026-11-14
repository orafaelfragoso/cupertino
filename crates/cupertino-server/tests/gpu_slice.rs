//! The GPU buffer handoff, end to end in a headless session: the slice client (`cupertino-gpu-slice`) draws with GLES
//! into dmabufs on the device the compositor's feedback names and attaches them with explicit sync, or without the
//! protocol waits for its own fence. Tests that need a render node with syncobj timelines skip without one, unless
//! `CUPERTINO_GPU_REQUIRED=1` (the local GPU tier) makes that a failure.

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
use smithay::reexports::wayland_protocols::wp::linux_drm_syncobj::v1::server::wp_linux_drm_syncobj_surface_v1::WpLinuxDrmSyncobjSurfaceV1;
use smithay::reexports::wayland_server::Display;
use smithay::wayland::compositor::with_states;
use smithay::wayland::dmabuf::get_dmabuf;
use smithay::wayland::shm::with_buffer_contents;

const OUTPUT: (i32, i32) = (1000, 700);
/// The slice's window, in buffer pixels (the output is at scale 1).
const WINDOW: (i32, i32) = (320, 240);
/// The colours the slice's frames end in (its two dmabuf colours and its shared-memory colour), as 8-bit RGBA.
const SLICE_COLOURS: [[u8; 4]; 3] = [[51, 115, 217, 255], [217, 89, 51, 255], [64, 179, 89, 255]];
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

struct Session {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
    home: tempfile::TempDir,
}

impl Session {
    fn start(withhold_explicit_sync: bool) -> Option<Session> {
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
        state.withhold_explicit_sync = withhold_explicit_sync;
        state.cursor_status = CursorImageStatus::Hidden;
        if let Err(e) = headless::init(&mut state, OUTPUT, 1.0) {
            unavailable(&format!("no EGL device ({e:#})"));
            return None;
        }
        Some(Session {
            state,
            event_loop,
            home: tempfile::tempdir().unwrap(),
        })
    }

    fn render_node(&mut self) -> bool {
        let renderer = self.state.backend.as_mut().unwrap().renderer();
        cupertino_server::backend::render_node(renderer).is_some()
    }

    fn pump(&mut self) {
        headless::step(
            &mut self.state,
            &mut self.event_loop,
            Duration::from_millis(2),
        )
        .unwrap();
    }

    /// The slice client on this session with `args`, its report written to a file in the session's home.
    fn slice(&self, args: &[&str]) -> (Child, PathBuf) {
        let report = self.home.path().join(format!("report-{}", args.join("_")));
        let binary =
            common::fixtures::artifact("cupertino-gpu-slice", "bin", "cupertino-gpu-slice");
        let mut command = common::spawn::cleared(binary, self.home.path());
        command
            .env(
                "XDG_RUNTIME_DIR",
                std::env::var_os("XDG_RUNTIME_DIR").unwrap(),
            )
            .env("WAYLAND_DISPLAY", self.state.socket_name.as_ref().unwrap())
            .args(["--size", &format!("{}x{}", WINDOW.0, WINDOW.1)])
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
        (command.spawn().expect("the slice client"), report)
    }

    /// Run the session until `child` ends, calling `each` every pass; its report.
    fn until_done(
        &mut self,
        mut child: Child,
        report: &Path,
        mut each: impl FnMut(&mut Session),
    ) -> BTreeMap<String, String> {
        let end = Instant::now() + common::HANG_GUARD;
        loop {
            self.pump();
            each(self);
            if let Some(status) = child.try_wait().unwrap() {
                // the client's last messages
                for _ in 0..10 {
                    self.pump();
                }
                let text = std::fs::read_to_string(report).unwrap_or_default();
                assert!(
                    status.success() || !text.is_empty(),
                    "the slice failed: {status}"
                );
                return text
                    .lines()
                    .filter_map(|l| l.split_once(' '))
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect();
            }
            if Instant::now() > end {
                let _ = child.kill();
                panic!("the slice did not finish");
            }
        }
    }

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

    /// The slice's window: the surface's current buffer is a dmabuf (with its modifier) or shared memory, and whether
    /// a syncobj surface object exists for it.
    fn window_buffer(&self) -> Option<(Option<u64>, bool)> {
        let window = self.state.windows.iter().find(|w| w.mapped)?;
        let surface = window.toplevel.wl_surface();
        with_states(surface, |states| {
            let buffer = states
                .data_map
                .get::<RendererSurfaceStateUserData>()
                .and_then(|s| s.lock().unwrap().buffer().cloned())?;
            let modifier = get_dmabuf(&buffer)
                .ok()
                .map(|d| u64::from(d.format().modifier));
            let shm = with_buffer_contents(&buffer, |_, _, _| ()).is_ok();
            let syncobj = states
                .data_map
                .get::<std::cell::RefCell<Option<WpLinuxDrmSyncobjSurfaceV1>>>()
                .is_some_and(|s| s.borrow().is_some());
            (modifier.is_some() || shm).then_some((modifier, syncobj))
        })
    }

    /// Whether the window's content, inside its rounded corners, is one colour in every capture taken while it draws.
    fn uniform_captures(&mut self, captures: usize) -> Vec<[u8; 4]> {
        // past the window's opening animation: until the window shows one of the slice's colours at full opacity
        let end = Instant::now() + common::HANG_GUARD;
        loop {
            assert!(
                Instant::now() < end,
                "the window never showed a colour of its own at full opacity"
            );
            let shot = self.screenshot();
            let Some(window) = self.state.windows.iter().find(|w| w.mapped) else {
                continue;
            };
            let frame = self.state.wm.window(window.id).unwrap().geometry();
            let px = shot
                .get_pixel(
                    (frame.x + frame.w / 2.0) as u32,
                    (frame.y + frame.h / 2.0) as u32,
                )
                .0;
            if SLICE_COLOURS
                .iter()
                .any(|c| c.iter().zip(px).all(|(a, b)| a.abs_diff(b) <= 1))
            {
                break;
            }
        }
        let mut colours = Vec::new();
        for _ in 0..captures {
            for _ in 0..20 {
                self.pump();
            }
            let Some(window) = self.state.windows.iter().find(|w| w.mapped) else {
                continue;
            };
            let frame = self.state.wm.window(window.id).unwrap().geometry();
            let shot = self.screenshot();
            const INSET: f64 = 24.0;
            let (x0, y0) = ((frame.x + INSET) as u32, (frame.y + INSET) as u32);
            let (x1, y1) = (
                (frame.x + frame.w - INSET) as u32,
                (frame.y + frame.h - INSET) as u32,
            );
            let first = shot.get_pixel(x0, y0).0;
            let mut worst = 0;
            for y in y0..y1 {
                for x in x0..x1 {
                    let px = shot.get_pixel(x, y).0;
                    worst = worst.max(
                        px.iter()
                            .zip(first)
                            .map(|(a, b)| a.abs_diff(b))
                            .max()
                            .unwrap(),
                    );
                }
            }
            if worst > 1 {
                let path = std::env::temp_dir()
                    .join(format!("gpu-slice-capture-{}.png", std::process::id()));
                shot.save(&path).unwrap();
                panic!("a capture of the window {frame:?} is not one colour: {worst} apart from {first:?} ({})", path.display());
            }
            colours.push(first);
        }
        colours
    }
}

/// Whether the machine's device offers explicit sync to a headless session.
fn explicit_sync(s: &mut Session) -> bool {
    if s.state.syncobj_state.is_none() {
        unavailable("the device has no render node with syncobj timelines");
        return false;
    }
    true
}

#[test]
fn a_headless_session_offers_explicit_sync_with_a_render_node_and_withholds_it_when_told() {
    let Some(mut offered) = Session::start(false) else {
        return;
    };
    let node = offered.render_node();
    if !node {
        unavailable("the device has no render node");
    }
    if node && offered.state.syncobj_state.is_none() {
        unavailable("the kernel cannot wait on syncobj timelines through an eventfd");
    }
    assert!(node || offered.state.syncobj_state.is_none());
    drop(offered);
    let withheld = Session::start(true).expect("the session started once");
    assert!(
        withheld.state.syncobj_state.is_none(),
        "--no-explicit-sync withholds it"
    );
}

#[test]
fn a_headless_presentation_numbers_its_refresh() {
    use cupertino_server::frame::refresh_count;
    let period = Duration::from_nanos(16_666_667);
    assert_eq!(
        refresh_count(period * 100 + Duration::from_millis(1), Some(period)),
        100
    );
    assert_eq!(refresh_count(period * 101, Some(period)), 101);
    assert_eq!(
        refresh_count(Duration::from_secs(5), None),
        0,
        "no period, no counter"
    );
}

#[test]
fn the_slice_attaches_dmabufs_with_acquire_and_release_points_in_the_driver_s_layout() {
    let Some(mut s) = Session::start(false) else {
        return;
    };
    if !explicit_sync(&mut s) {
        return;
    }
    let (child, report) = s.slice(&["--frames", "180"]);
    let mut modifiers = Vec::new();
    let mut captured = false;
    let report = s.until_done(child, &report, |s| {
        if let Some((Some(modifier), _)) = s.window_buffer() {
            if !modifiers.contains(&modifier) {
                modifiers.push(modifier);
            }
            if !captured {
                captured = true;
                let colours = s.uniform_captures(3);
                assert!(!colours.is_empty());
            }
        }
    });
    eprintln!("{report:#?}");
    assert_eq!(report["fallback"], "none");
    assert_eq!(report["explicit_sync"], "1");
    assert_eq!(report["allocator"], "gbm");
    assert_eq!(report["buffers"], "2");
    assert_eq!(report["dmabuf_frames"], "180");
    assert_eq!(
        report["fence_waits"], "0",
        "explicit sync never waits on the CPU"
    );
    let releases: u64 = report["release_waits"].parse().unwrap();
    assert!(
        releases >= 178,
        "every buffer but the last two came back through its release point: {releases}"
    );
    assert_eq!(report["syncobj_surfaces_created"], "1");
    let modifier = u64::from_str_radix(report["modifier"].trim_start_matches("0x"), 16).unwrap();
    assert_eq!(
        modifiers,
        [modifier],
        "the compositor imported the buffers in the layout the client drew"
    );
    if report["software"] == "0" {
        assert_ne!(
            modifier, 0,
            "a hardware buffer is never linear unless both sides ask for it"
        );
    }
    if report["renderer"].contains("NVIDIA") {
        assert_eq!(report["modifier"], NVIDIA_DEFAULT_MODIFIER);
    }
    assert!(report["presented"].parse::<u64>().unwrap() >= 170);
}

#[test]
fn the_slice_destroys_its_syncobj_surface_before_a_shared_memory_frame_and_makes_it_again_after() {
    let Some(mut s) = Session::start(false) else {
        return;
    };
    if !explicit_sync(&mut s) {
        return;
    }
    let (child, report) = s.slice(&["--frames", "60", "--shm-every", "10"]);
    let (mut shm_seen, mut dmabuf_seen) = (0, 0);
    let report = s.until_done(child, &report, |s| match s.window_buffer() {
        Some((None, syncobj)) => {
            assert!(
                !syncobj,
                "a shared-memory buffer is shown on a surface that still has a syncobj object"
            );
            shm_seen += 1;
        }
        // a dmabuf frame still shown after the object is gone is one whose commit was held for its acquire point
        Some((Some(_), _)) => dmabuf_seen += 1,
        None => {}
    });
    eprintln!("{report:#?}");
    assert_eq!(
        (
            report["shm_frames"].as_str(),
            report["dmabuf_frames"].as_str()
        ),
        ("6", "54")
    );
    assert_eq!(report["syncobj_surfaces_destroyed"], "6");
    assert_eq!(
        report["syncobj_surfaces_created"], "6",
        "made again after each shared-memory frame but the last"
    );
    assert!(
        shm_seen > 0 && dmabuf_seen > 0,
        "the session saw both kinds ({shm_seen}, {dmabuf_seen})"
    );
}

#[test]
fn the_slice_without_explicit_sync_waits_for_its_own_fence_and_keeps_three_buffers() {
    let Some(mut s) = Session::start(true) else {
        return;
    };
    if !s.render_node() {
        unavailable("the device has no render node");
        return;
    }
    let (child, report) = s.slice(&["--frames", "180"]);
    let mut captured = false;
    let report = s.until_done(child, &report, |s| {
        if !captured && matches!(s.window_buffer(), Some((Some(_), _))) {
            captured = true;
            s.uniform_captures(3);
        }
    });
    eprintln!("{report:#?}");
    assert_eq!(report["fallback"], "none");
    assert_eq!(report["explicit_sync"], "0");
    assert_eq!(report["buffers"], "3");
    assert_eq!(
        report["fence_waits"], "180",
        "every commit waited for the frame's fence"
    );
    assert_eq!(report["release_waits"], "0");
    assert_eq!(report["syncobj_surfaces_created"], "0");
}

#[test]
fn the_slice_counts_no_missed_refresh_when_the_session_keeps_up_and_every_overdraw_capture_is_flat()
{
    let Some(mut s) = Session::start(false) else {
        return;
    };
    if !s.render_node() {
        unavailable("the device has no render node");
        return;
    }
    s.state.fixed_time = None;
    let (child, report) = s.slice(&["--seconds", "3"]);
    let mut colours = Vec::new();
    let report = s.until_done(child, &report, |s| {
        if colours.is_empty() && matches!(s.window_buffer(), Some((Some(_), _))) {
            colours = s.uniform_captures(8);
        }
    });
    eprintln!("{report:#?} captures {colours:?}");
    assert_eq!(colours.len(), 8);
    let presented: u64 = report["presented"].parse().unwrap();
    let missed: u64 = report["missed"].parse().unwrap();
    assert!(presented > 60, "{presented} frames presented in 3 s");
    // the captures themselves hold the loop up for a few refreshes each
    assert!(missed <= 8 * 6, "{missed} refreshes missed");
}
