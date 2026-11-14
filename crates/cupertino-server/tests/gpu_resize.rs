//! A toolkit window on the GPU through a live resize, end to end: the real Settings and Files processes on a headless
//! output of 3840 × 2160 at scale 1.5, resized a point at a time as a drag of the window's corner does, with and without
//! explicit sync. Every frame the server draws holds the buffers it was drawn from until the next, as a display's flip
//! does, so a window's buffers are back only a frame after it shows the next. Each size asked for must be shown within
//! a bound, and the last one soon after the drag ends. Tests that need a render node skip without one, unless
//! `CUPERTINO_GPU_REQUIRED=1` (the local GPU tier) makes that a failure.

use crate::common;

use std::path::Path;
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use cupertino_server::backend::headless;
use cupertino_server::config::Config;
use cupertino_server::frame::drawn_buffers;
use cupertino_server::Cupertino;
use cupertino_test_bus::PrivateBus;
use smithay::backend::renderer::utils::{Buffer, RendererSurfaceStateUserData};
use smithay::input::pointer::CursorImageStatus;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::Display;
use smithay::wayland::compositor::with_states;
use smithay::wayland::dmabuf::get_dmabuf;

const OUTPUT: (i32, i32) = (3840, 2160);
const SCALE: f64 = 1.5;
/// The longest a size asked for may wait to be shown while the window is resized.
const ANSWER_BOUND: Duration = Duration::from_millis(250);
/// How often the size changes, by a point on each axis: a pointer moving at 125 Hz. A buffer size step of 128 pixels
/// is crossed every 85 points at scale 1.5, about 0.7 s.
const MOTION: Duration = Duration::from_millis(8);
/// The points the window grows by and then shrinks back, crossing two buffer size steps each way.
const TRAVEL: i32 = 200;

fn gpu_required() -> bool {
    std::env::var("CUPERTINO_GPU_REQUIRED").as_deref() == Ok("1")
}

/// Skip (or, in the GPU tier, fail) a test whose machine cannot run it.
fn unavailable(why: &str) {
    assert!(!gpu_required(), "CUPERTINO_GPU_REQUIRED=1: {why}");
    eprintln!("SKIPPED: {why}; set CUPERTINO_GPU_REQUIRED=1 to fail instead");
}

struct Process(Child);

impl Drop for Process {
    fn drop(&mut self) {
        if self.0.try_wait().unwrap().is_none() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

struct Session {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
    home: tempfile::TempDir,
    bus: PrivateBus,
    /// The buffers the last frame was drawn from, held until the next as a flip holds them.
    held: Vec<Buffer>,
}

impl Session {
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
        if let Err(e) = headless::init(&mut state, OUTPUT, SCALE) {
            unavailable(&format!("no EGL device ({e:#})"));
            return None;
        }
        let renderer = state.backend.as_mut().unwrap().renderer();
        if cupertino_server::backend::render_node(renderer).is_none() {
            unavailable("the device has no render node");
            return None;
        }
        if explicit_sync && state.syncobj_state.is_none() {
            unavailable("the device has no render node with syncobj timelines");
            return None;
        }
        Some(Session {
            state,
            event_loop,
            home: tempfile::tempdir().unwrap(),
            bus: PrivateBus::start().expect("this test needs dbus-daemon"),
            held: Vec::new(),
        })
    }

    fn pump(&mut self) {
        headless::step(
            &mut self.state,
            &mut self.event_loop,
            Duration::from_millis(2),
        )
        .unwrap();
        let surfaces: Vec<_> = self
            .state
            .windows
            .iter()
            .filter(|w| w.mapped)
            .map(|w| w.toplevel.wl_surface().clone())
            .collect();
        self.held = drawn_buffers(&surfaces);
    }

    /// The real program `package`'s binary `name` on this session, drawing with the GPU renderer.
    fn spawn(&self, package: &str, name: &str, args: &[&str]) -> Process {
        let binary = common::fixtures::artifact(package, "bin", name);
        let home = self.home.path();
        let socket = Path::new(&std::env::var_os("XDG_RUNTIME_DIR").unwrap())
            .join(self.state.socket_name.as_ref().unwrap());
        let mut command = common::spawn::cleared(binary, home);
        command
            .args(args)
            .env("XDG_CONFIG_HOME", home.join("config"))
            .env("XDG_DATA_HOME", home.join("data"))
            .env("XDG_CACHE_HOME", home.join("cache"))
            .env("XDG_DATA_DIRS", home.join("share"))
            .env("XDG_RUNTIME_DIR", home)
            .env("WAYLAND_DISPLAY", socket)
            .env("DBUS_SESSION_BUS_ADDRESS", &self.bus.address)
            .env("DBUS_SYSTEM_BUS_ADDRESS", &self.bus.address)
            .env("CUPERTINO_RENDERER", "gpu")
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
        Process(command.spawn().expect("the program"))
    }

    fn window(&self) -> Option<&cupertino_server::state::WindowEntry> {
        self.state.windows.iter().find(|w| w.mapped)
    }

    /// Whether the window shows a dmabuf now.
    fn shows_dmabuf(&self) -> bool {
        let Some(window) = self.window() else {
            return false;
        };
        with_states(window.toplevel.wl_surface(), |states| {
            states
                .data_map
                .get::<RendererSurfaceStateUserData>()
                .and_then(|s| s.lock().unwrap().buffer().cloned())
                .is_some_and(|b| get_dmabuf(&b).is_ok())
        })
    }

    /// The size the window last committed an answer to, in points.
    fn shown_size(&self) -> Option<(i32, i32)> {
        let size = self.window()?.toplevel.current_state().size?;
        Some((size.w, size.h))
    }

    /// Ask the window for `size` (points), as an interactive resize does.
    fn resize(&self, size: (i32, i32)) {
        if let Some(window) = self.window() {
            window.toplevel.with_pending_state(|st| {
                st.size = Some(size.into());
                st.states.set(xdg_toplevel::State::Resizing);
            });
            window.toplevel.send_configure();
        }
    }

    fn end_resize(&self, size: (i32, i32)) {
        if let Some(window) = self.window() {
            window.toplevel.with_pending_state(|st| {
                st.size = Some(size.into());
                st.states.unset(xdg_toplevel::State::Resizing);
            });
            window.toplevel.send_configure();
        }
    }

    fn wait_until(&mut self, what: &str, mut cond: impl FnMut(&Session) -> bool) {
        let end = Instant::now() + common::HANG_GUARD;
        while !cond(self) {
            assert!(Instant::now() < end, "timed out waiting for {what}");
            self.pump();
        }
    }
}

/// What a resize saw: for each size asked for, how long until the window showed it or a later one.
struct Answers {
    asked: Vec<(Instant, (i32, i32))>,
    /// How many of `asked` were answered.
    answered: usize,
    longest: Duration,
    longest_at: (i32, i32),
}

impl Answers {
    fn new() -> Answers {
        Answers {
            asked: Vec::new(),
            answered: 0,
            longest: Duration::ZERO,
            longest_at: (0, 0),
        }
    }

    /// The window shows `shown` at `now`: the latest size asked for that equals it, and every one before it, are
    /// answered.
    fn shown(&mut self, shown: Option<(i32, i32)>, now: Instant) {
        let Some(shown) = shown else { return };
        let Some(latest) = self.asked[self.answered..]
            .iter()
            .rposition(|(_, size)| *size == shown)
        else {
            return;
        };
        for (at, size) in &self.asked[self.answered..=self.answered + latest] {
            let waited = now - *at;
            if waited > self.longest {
                self.longest = waited;
                self.longest_at = *size;
            }
        }
        self.answered += latest + 1;
    }

    /// The longest any size has waited so far, answered or not.
    fn waiting(&self, now: Instant) -> Duration {
        let open = self.asked[self.answered..]
            .first()
            .map_or(Duration::ZERO, |(at, _)| now - *at);
        open.max(self.longest)
    }
}

/// Resize the program's window a point at a time out by [`TRAVEL`] and back, then end the resize; every size asked
/// for is shown within [`ANSWER_BOUND`].
fn resized_within_bound(explicit_sync: bool, package: &str, name: &str, args: &[&str]) {
    let Some(mut s) = Session::start(explicit_sync) else {
        return;
    };
    let _app = s.spawn(package, name, args);
    s.wait_until("the window on the GPU", |s| s.shows_dmabuf());
    let start = s.shown_size().expect("a configured window");
    // the window settles before the drag
    let settle = Instant::now() + Duration::from_millis(500);
    while Instant::now() < settle {
        s.pump();
    }
    let mut answers = Answers::new();
    let path: Vec<(i32, i32)> = (1..=TRAVEL)
        .chain((0..TRAVEL).rev())
        .map(|d| (start.0 + d, start.1 + d / 2))
        .collect();
    let mut next = Instant::now();
    for (i, &size) in path.iter().enumerate() {
        let last = i + 1 == path.len();
        if last {
            s.end_resize(size);
        } else {
            s.resize(size);
        }
        answers.asked.push((Instant::now(), size));
        next += MOTION;
        while Instant::now() < next {
            s.pump();
            answers.shown(s.shown_size(), Instant::now());
        }
        let now = Instant::now();
        assert!(
            answers.waiting(now) <= ANSWER_BOUND,
            "{name}: a size waited {:?} to be shown (the longest answered: {:?} for {:?}); asked {} sizes, {} shown, \
             the window shows {:?}",
            answers.waiting(now),
            answers.longest,
            answers.longest_at,
            answers.asked.len(),
            answers.answered,
            s.shown_size(),
        );
    }
    let end = Instant::now() + ANSWER_BOUND;
    while answers.answered < answers.asked.len() && Instant::now() < end {
        s.pump();
        answers.shown(s.shown_size(), Instant::now());
    }
    assert_eq!(
        answers.answered,
        answers.asked.len(),
        "{name}: the last size was not shown within {ANSWER_BOUND:?} of the drag's end: it shows {:?}",
        s.shown_size()
    );
    assert!(
        answers.longest <= ANSWER_BOUND,
        "{name}: {:?} waited {:?}",
        answers.longest_at,
        answers.longest
    );
    assert!(s.shows_dmabuf(), "{name} still draws on the GPU");
    eprintln!(
        "{name}: {} sizes, the longest waited {:?} ({:?})",
        answers.asked.len(),
        answers.longest,
        answers.longest_at
    );
}

fn settings() -> (&'static str, &'static str, &'static [&'static str]) {
    ("cupertino-settings", "cupertino-settings", &[])
}

fn files() -> (&'static str, &'static str, &'static [&'static str]) {
    ("cupertino-files", "cupertino-files", &[])
}

#[test]
fn settings_shows_every_size_of_a_live_resize_on_the_gpu_with_release_points() {
    let (package, name, args) = settings();
    resized_within_bound(true, package, name, args);
}

#[test]
fn settings_shows_every_size_of_a_live_resize_on_the_gpu_without_explicit_sync() {
    let (package, name, args) = settings();
    resized_within_bound(false, package, name, args);
}

#[test]
fn files_shows_every_size_of_a_live_resize_on_the_gpu_with_release_points() {
    let (package, name, args) = files();
    resized_within_bound(true, package, name, args);
}

#[test]
fn files_shows_every_size_of_a_live_resize_on_the_gpu_without_explicit_sync() {
    let (package, name, args) = files();
    resized_within_bound(false, package, name, args);
}
