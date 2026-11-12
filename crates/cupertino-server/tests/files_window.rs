//! The real `cupertino-files` binary against the in-process server: its window maps as a decorated
//! application window, lists the home folder, redraws when the folder changes on disk, goes to the
//! enclosing folder on Super+Up and to a sidebar place on a click, and exits when the close shortcut
//! closes it. The server runs on
//! an offscreen EGL device; the test skips (passes) when there is none.

use crate::common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cupertino_server::backend::headless;
use cupertino_server::cli::start_shell_supervision;
use cupertino_server::config::{Config, ShellProcess};
use cupertino_server::Cupertino;
use cupertino_test_bus::PrivateBus;
use smithay::backend::input::{ButtonState, KeyState};
use smithay::backend::renderer::utils::with_renderer_surface_state;
use smithay::input::keyboard::Keycode;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;
use smithay::wayland::shm::with_buffer_contents;

const OUTPUT: (i32, i32) = (1400, 900);
/// Evdev codes plus the xkb offset of 8.
const SUPER: u32 = 125 + 8;
const UP: u32 = 103 + 8;
const W: u32 = 17 + 8;
/// Evdev code of the left button (`linux/input-event-codes.h`).
const BTN_LEFT: u32 = 0x110;

struct Server {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
}

impl Server {
    /// The server with `procs` supervised on the headless output; `None` without an EGL device.
    fn start(procs: Vec<ShellProcess>) -> Option<Server> {
        let event_loop: EventLoop<'static, Cupertino> = EventLoop::try_new().unwrap();
        let display: Display<Cupertino> = Display::new().unwrap();
        let cfg = Config {
            shell_processes: procs,
            ..Config::default()
        };
        let mut state = Cupertino::new(
            display,
            event_loop.handle(),
            event_loop.get_signal(),
            cfg,
            true,
            &cupertino_assets::Assets::builtin(),
        )
        .unwrap();
        if let Err(e) = headless::init(&mut state, OUTPUT, 1.0) {
            common::egl_unavailable(format!("{e:#}"));
            return None;
        }
        start_shell_supervision(&state, &event_loop).expect("the supervision timer is inserted");
        Some(Server { state, event_loop })
    }

    fn step(&mut self) {
        headless::step(
            &mut self.state,
            &mut self.event_loop,
            Duration::from_millis(5),
        )
        .unwrap();
    }

    /// Step the server until `cond` holds; panics after [`common::HANG_GUARD`].
    fn wait_until(&mut self, what: &str, mut cond: impl FnMut(&Cupertino) -> bool) {
        let end = Instant::now() + common::HANG_GUARD;
        while Instant::now() < end {
            self.step();
            if cond(&self.state) {
                return;
            }
        }
        panic!("timed out waiting for {what}");
    }

    /// [`common::ping_barrier`] on the file manager: it has handled what it was sent, and the server has what it did
    /// with it, the frames it drew included.
    fn settle(&mut self) {
        let client = self.state.windows[0].toplevel.client();
        let event_loop = &mut self.event_loop;
        common::ping_barrier(&mut self.state, &client, |state| {
            headless::step(state, event_loop, Duration::from_millis(5)).unwrap();
        });
    }

    fn key(&mut self, code: u32, pressed: bool) {
        self.state.keyboard_event_now(
            Keycode::new(code),
            if pressed {
                KeyState::Pressed
            } else {
                KeyState::Released
            },
            0,
        );
        self.step();
    }
}

/// A scratch directory removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> TempDir {
        let dir = std::env::temp_dir().join(format!(
            "cupertino-files-window-{name}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

/// The file manager artifact prepared before the test runner starts.
fn files_binary() -> PathBuf {
    common::fixtures::artifact("cupertino-files", "bin", "cupertino-files")
}

/// The file manager with `home` as its home folder and its configuration, data and system data directories inside it,
/// so that its sidebar does not depend on the host: `data_dirs` stands for `$XDG_DATA_DIRS`. Its session bus is the
/// test's own `bus`: Files serves one process per session bus name, and another test's Files on a shared bus would
/// take its launch.
fn files_process(home: &Path, data_dirs: &Path, bus: &PrivateBus) -> ShellProcess {
    let env = |name: &str, value: &Path| format!("{name}={}", value.display());
    // the test's diagnostic journal; the store is the one under `home` below
    let command = vec![
        "env".into(),
        common::events::isolated()[1].clone(),
        "CUPERTINO_ASSET_SEARCH_PATH=".into(),
        format!("DBUS_SESSION_BUS_ADDRESS={}", bus.address),
        env("HOME", home),
        env("XDG_CONFIG_HOME", &home.join(".config")),
        env("XDG_DATA_HOME", &home.join(".local/share")),
        env("XDG_STATE_HOME", &home.join(".local/state")),
        env("XDG_DATA_DIRS", data_dirs),
        files_binary().display().to_string(),
    ];
    ShellProcess {
        name: "cupertino-files".into(),
        command,
        roles: Vec::new(),
    }
}

/// The file manager's sidebar metrics in points: the sidebar runs the window's full height, its list starting below
/// the window control buttons' row of 52, then the space above the first group, a group title row, a medium row and
/// the space between groups.
const SIDEBAR_TOP: f64 = 52.0 + 10.0;
const GROUP_ROW: f64 = 19.0;
const ROW: f64 = 28.0;
const INTERGROUP: f64 = 13.0;

/// The vertical centre, in window points, of row `row` of the second sidebar group when the first group has
/// `favourites` rows: the sidebar stacks its groups the same way for any number of places.
fn second_group_row_centre(favourites: usize, row: usize) -> f64 {
    SIDEBAR_TOP
        + GROUP_ROW
        + favourites as f64 * ROW
        + INTERGROUP
        + GROUP_ROW
        + row as f64 * ROW
        + ROW / 2.0
}

/// What the server holds of the file manager's window.
struct Shown {
    /// The server draws the title bar and the window control buttons.
    server_side: bool,
    title: String,
    /// A copy of the committed buffer's pixels.
    pixels: Vec<u8>,
    /// The committed buffer's width and height.
    size: (i32, i32),
}

/// The file manager's window; `None` until it has committed a buffer.
fn window(state: &Cupertino) -> Option<Shown> {
    let entry = state.windows.iter().find(|w| w.mapped)?;
    let surface = entry.surface();
    let title = with_states(surface, |s| {
        s.data_map
            .get::<XdgToplevelSurfaceData>()
            .unwrap()
            .lock()
            .unwrap()
            .title
            .clone()
    })
    .unwrap_or_default();
    let buffer = with_renderer_surface_state(surface, |s| s.buffer().cloned())??;
    let (pixels, size) = with_buffer_contents(&buffer, |ptr, len, data| {
        // the pool holds both buffers of the window: copy this one's rows only
        let (start, count) = (data.offset as usize, (data.stride * data.height) as usize);
        assert!(start + count <= len, "the buffer lies inside its pool");
        let mut copy = vec![0u8; count];
        // SAFETY: `ptr` is valid for `len` bytes while the callback runs and the range was checked; the client may
        // write the pool, so the bytes are copied out instead of borrowed
        unsafe { std::ptr::copy_nonoverlapping(ptr.add(start), copy.as_mut_ptr(), count) };
        (copy, (data.width, data.height))
    })
    .expect("the file manager commits shm buffers");
    Some(Shown {
        server_side: entry.server_side,
        title,
        pixels,
        size,
    })
}

#[test]
fn the_file_manager_window_maps_lists_refreshes_navigates_and_closes() {
    let scratch = TempDir::new("home");
    let home = scratch.0.join("home");
    std::fs::create_dir_all(home.join("Documents")).unwrap();
    std::fs::write(home.join("notes.txt"), b"x").unwrap();
    // one system data directory with an applications folder: the sidebar's Favorites are Recents and Applications (the
    // default Favorites that exist here,: no Desktop folder, Documents not assigned in user-dirs.dirs)
    let data_dirs = scratch.0.join("share");
    std::fs::create_dir_all(data_dirs.join("applications")).unwrap();
    let Some(bus) = PrivateBus::start() else {
        return;
    };
    let Some(mut server) = Server::start(vec![files_process(&home, &data_dirs, &bus)]) else {
        return;
    };

    // a decorated window at the default content size of 920 x 436 pt, titled with the folder's name
    server.wait_until("the window to map", |s| window(s).is_some());
    let shown = window(&server.state).unwrap();
    assert!(
        shown.server_side,
        "the server draws the title bar and the window control buttons"
    );
    assert_eq!(shown.title, "home");
    assert_eq!(shown.size, (920, 436));

    // a file created in the folder appears without any input: the window draws a different frame, once it has drawn
    // the folder as it is, a frame that a barrier on the client leaves as it was
    let end = Instant::now() + common::HANG_GUARD;
    let mut before = window(&server.state).unwrap().pixels;
    loop {
        server.settle();
        let now = window(&server.state).unwrap().pixels;
        if now == before {
            break;
        }
        assert!(Instant::now() < end, "the window never stopped drawing");
        before = now;
    }
    std::fs::write(home.join("added.txt"), b"x").unwrap();
    server.wait_until("the window to draw the new file", |s| {
        window(s).is_some_and(|w| w.pixels != before)
    });

    // Super+Up goes to the enclosing folder, which the title bar names
    server.key(SUPER, true);
    server.key(UP, true);
    server.key(UP, false);
    server.key(SUPER, false);
    let parent = scratch
        .0
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    server.wait_until("the title of the enclosing folder", |s| {
        window(s).is_some_and(|w| w.title == parent)
    });

    // a click on the sidebar's computer row, the first of Locations under the two Favorites, shows the root
    let id = server.state.windows[0].id;
    let f = server.state.wm.window(id).unwrap().geometry();
    let (_, _, origin) = server
        .state
        .window_surface_under((f.x + f.w / 2.0, f.y + f.h / 2.0).into())
        .expect("the window's content is under its centre");
    server.state.pointer_moved_to(
        (origin.x + 100.0, origin.y + second_group_row_centre(2, 0)).into(),
        0,
        None,
    );
    server
        .state
        .pointer_button(BTN_LEFT, ButtonState::Pressed, 0);
    server.step();
    server
        .state
        .pointer_button(BTN_LEFT, ButtonState::Released, 0);
    server.wait_until("the title of the root", |s| {
        window(s).is_some_and(|w| w.title == "/")
    });

    // the close shortcut asks the window to close; the process exits with success, so the supervisor does not
    // start it again after its throttle (a failure would bring the window back)
    let pid = server.state.supervisor.pid_of("cupertino-files");
    assert!(pid.is_some());
    server.key(SUPER, true);
    server.key(W, true);
    server.key(W, false);
    server.key(SUPER, false);
    server.wait_until("the window to go away", |s| s.windows.is_empty());
    server.wait_until("the supervisor to see the process exit", |s| {
        s.supervisor.pid_of("cupertino-files") != pid
    });
    assert_eq!(
        (
            server.state.supervisor.pid_of("cupertino-files"),
            server.state.supervisor.next_deadline()
        ),
        (None, None),
        "the file manager was started again or waits to be: it did not exit with success"
    );
    assert!(server.state.windows.is_empty());
}
