//! A supervised legacy shell and an ordinary mapped client stop dispatching. A third connection must still
//! receive frame callbacks even while the consumer of either scripted output is not reading.

use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

use rustix::event::{poll, PollFd, PollFlags, Timespec};
use rustix::fs::inotify;
use serde_json::json;
use smithay::reexports::wayland_protocols::xdg::shell::client::{
    xdg_surface, xdg_toplevel, xdg_wm_base,
};
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_callback, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle};

use cupertino_protocols::server::cupertino_shell_v1::CupertinoShellV1;
use wayland_backend::{
    client::{Backend, ObjectData, ObjectId},
    protocol::{Argument, Message},
};
use wayland_server::Resource;

/// Bounds setup and teardown on a loaded builder.
const SETUP: Duration = Duration::from_secs(20);
/// A fresh frame has to arrive before a stalled write could time out.
const FRAME_WAIT: Duration = Duration::from_secs(2);
/// Seconds after the server starts at which it writes the window list into the blocked FIFO. The server's script runs
/// on the wall clock from its start, so this is the time the supervised shell and its two clients have to map on a
/// loaded builder (they took up to 2.6 s with the workspace's tests running at 64 jobs).
const LIST_AT: f64 = 6.0;
/// Seconds after the window list at which the server writes the screenshot into the other blocked FIFO.
const SHOT_AFTER_LIST: f64 = 0.5;
/// Seconds after the window list at which the server writes the second window list to a plain file: the time the test
/// has to rename the frame client once the first list's writer opened the FIFO.
const LATER_LIST_AFTER_LIST: f64 = 2.0;
/// Seconds after the second window list at which the server writes its last screenshot and exits.
const LAST_SHOT_AFTER_LATER_LIST: f64 = 0.5;

fn readable(fd: &impl AsFd, wait: Duration) -> bool {
    poll(
        &mut [PollFd::new(fd, PollFlags::IN)],
        Some(&Timespec::try_from(wait).unwrap()),
    )
    .unwrap()
        != 0
}

struct Session(Child);
impl Session {
    fn finish(&mut self) -> std::process::ExitStatus {
        let fd = rustix::process::pidfd_open(
            rustix::process::Pid::from_raw(self.0.id() as i32).unwrap(),
            rustix::process::PidfdFlags::empty(),
        )
        .unwrap();
        assert!(readable(&fd, SETUP), "server did not finish the snapshots");
        self.0.wait().unwrap()
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Home(PathBuf);
impl Home {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("cupertino-{label}-{}", std::process::id()));
        DirBuilder::new().mode(0o700).create(&dir).unwrap();
        Self(dir)
    }

    fn server(&self, shell: serde_json::Value) -> Command {
        let config = self.0.join("compositor.json");
        fs::write(
            &config,
            json!({"prepare_toolkit_programs":false, "shell":shell}).to_string(),
        )
        .unwrap();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_cupertino-server"));
        cmd.args([
            "--headless",
            "--dev-shell",
            "--size",
            "192x128",
            "--no-menu-bar",
        ])
        .env_clear()
        .env("HOME", &self.0)
        .env("XDG_RUNTIME_DIR", &self.0)
        .env("CUPERTINO_SERVER_CONFIG", &config)
        .env("LIBGL_ALWAYS_SOFTWARE", "1")
        .stdout(Stdio::null());
        cmd
    }
}
impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A full pipe whose reader is already present. IN_OPEN tells the test exactly when the compositor's writer opens
/// the path, without inferring it from a sleep or a file timestamp.
struct Pipe {
    path: PathBuf,
    read: File,
    opened: OwnedFd,
    prefix: usize,
}

impl Pipe {
    fn new(path: PathBuf) -> Self {
        rustix::fs::mknodat(
            rustix::fs::CWD,
            &path,
            rustix::fs::FileType::Fifo,
            rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
            0,
        )
        .unwrap();
        let read = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&path)
            .unwrap();
        let mut fill = OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&path)
            .unwrap();
        let capacity = rustix::pipe::fcntl_getpipe_size(&fill).unwrap();
        fill.write_all(&vec![0; capacity]).unwrap();
        drop(fill);
        let opened = inotify::init(inotify::CreateFlags::CLOEXEC).unwrap();
        inotify::add_watch(&opened, &path, inotify::WatchFlags::OPEN).unwrap();
        Self {
            path,
            read,
            opened,
            prefix: capacity,
        }
    }

    fn contents(mut self, peer: &mut UnixStream) -> Vec<u8> {
        assert!(
            readable(&self.opened, SETUP),
            "writer never opened {}",
            self.path.display()
        );
        // Each command asks for a fresh callback only after the preceding one arrived.
        for _ in 0..3 {
            peer.write_all(b"F").unwrap();
            let mut got = [0];
            peer.read_exact(&mut got)
                .expect("no independent frame while scripted output is blocked");
            assert_eq!(&got, b"F");
        }
        let mut bytes = Vec::new();
        let end = Instant::now() + SETUP;
        loop {
            assert!(readable(
                &self.read,
                end.checked_duration_since(Instant::now())
                    .expect("FIFO did not close")
            ));
            let mut buf = [0; 8192];
            match self.read.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => bytes.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => panic!("reading {}: {e}", self.path.display()),
            }
        }
        bytes.split_off(self.prefix)
    }
}

#[derive(Default)]
struct Client {
    configured: bool,
    frame: bool,
}
impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Client {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
wayland_client::delegate_noop!(Client: ignore wl_compositor::WlCompositor);
wayland_client::delegate_noop!(Client: ignore wl_surface::WlSurface);
wayland_client::delegate_noop!(Client: ignore wl_shm::WlShm);
wayland_client::delegate_noop!(Client: ignore wl_shm_pool::WlShmPool);
wayland_client::delegate_noop!(Client: ignore wl_buffer::WlBuffer);
wayland_client::delegate_noop!(Client: ignore xdg_toplevel::XdgToplevel);
/// Receives the initial session state on the negotiated version-one session object.
#[derive(Default)]
struct Observer(AtomicBool);
impl ObjectData for Observer {
    fn event(
        self: Arc<Self>,
        _: &Backend,
        msg: Message<ObjectId, OwnedFd>,
    ) -> Option<Arc<dyn ObjectData>> {
        if msg.sender_id.interface().name == "cupertino_session_v1" {
            self.0.store(true, Ordering::Release);
        }
        None
    }
    fn destroyed(&self, _: ObjectId) {}
}
impl Dispatch<xdg_wm_base::XdgWmBase, ()> for Client {
    fn event(
        _: &mut Self,
        wm: &xdg_wm_base::XdgWmBase,
        e: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = e {
            wm.pong(serial);
        }
    }
}
impl Dispatch<xdg_surface::XdgSurface, ()> for Client {
    fn event(
        s: &mut Self,
        xdg: &xdg_surface::XdgSurface,
        e: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = e {
            xdg.ack_configure(serial);
            s.configured = true;
        }
    }
}
impl Dispatch<wl_callback::WlCallback, ()> for Client {
    fn event(
        s: &mut Self,
        _: &wl_callback::WlCallback,
        _: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        s.frame = true;
    }
}

fn map(
    home: &Path,
    title: &str,
) -> (
    EventQueue<Client>,
    wl_surface::WlSurface,
    xdg_toplevel::XdgToplevel,
) {
    let socket = home.join(std::env::var_os("WAYLAND_DISPLAY").unwrap());
    let conn = Connection::from_socket(UnixStream::connect(socket).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<Client>(&conn).unwrap();
    let qh = queue.handle();
    assert!(globals
        .contents()
        .clone_list()
        .iter()
        .all(|g| g.interface != CupertinoShellV1::interface().name));
    let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 1..=4, ()).unwrap();
    let wm: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=1, ()).unwrap();
    let surface = compositor.create_surface(&qh, ());
    let xdg = wm.get_xdg_surface(&surface, &qh, ());
    let top = xdg.get_toplevel(&qh, ());
    top.set_title(title.into());
    top.set_app_id(title.into());
    surface.commit();
    let mut state = Client::default();
    while !state.configured {
        queue.blocking_dispatch(&mut state).unwrap();
    }
    let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let fd = rustix::fs::memfd_create("frame-test", rustix::fs::MemfdFlags::CLOEXEC).unwrap();
    rustix::fs::ftruncate(&fd, 256).unwrap();
    let pool = shm.create_pool(fd.as_fd(), 256, &qh, ());
    let buffer = pool.create_buffer(0, 8, 8, 32, wl_shm::Format::Xrgb8888, &qh, ());
    surface.attach(Some(&buffer), 0, 0);
    surface.commit();
    queue.roundtrip(&mut state).unwrap();
    (queue, surface, top)
}

/// The supervisor runs this test binary as the legacy shell. After registration, its private session observer and
/// the ordinary client's mapped toplevel keep their connections open but never dispatch another event.
fn clients(home: &Path) {
    let shell = Connection::connect_to_env().unwrap();
    let (globals, mut shell_queue) = registry_queue_init::<Client>(&shell).unwrap();
    // Use the server crate's wire descriptions. No client-only shell dependency is needed for these two requests.
    let iface = CupertinoShellV1::interface();
    let name = globals
        .contents()
        .clone_list()
        .into_iter()
        .find(|g| g.interface == iface.name)
        .unwrap()
        .name;
    let observer = Arc::new(Observer::default());
    let role = shell
        .send_request(
            globals.registry(),
            wl_registry::Request::Bind {
                name,
                id: (iface, 1),
            },
            Some(observer.clone()),
        )
        .unwrap();
    let session = iface
        .requests
        .iter()
        .position(|r| r.name == "get_session")
        .unwrap();
    shell
        .backend()
        .send_request(
            Message {
                sender_id: role,
                opcode: session as u16,
                args: [Argument::NewId(ObjectId::null())].into_iter().collect(),
            },
            Some(observer.clone()),
            None,
        )
        .unwrap();
    let mut state = Client::default();
    shell_queue.roundtrip(&mut state).unwrap();
    assert!(observer.0.load(Ordering::Acquire));
    let _stalled = map(home, "stalled-client");
    let (mut queue, surface, top) = map(home, "frame-client");
    let mut control = UnixStream::connect(home.join("probe")).unwrap();
    let mut cmd = [0];
    while control.read_exact(&mut cmd).is_ok() {
        state.frame = false;
        top.set_title("renamed-after-sample".into());
        surface.frame(&queue.handle(), ());
        surface.damage(0, 0, 8, 8);
        surface.commit();
        while !state.frame {
            queue.blocking_dispatch(&mut state).unwrap();
        }
        control.write_all(b"F").unwrap();
    }
}

#[test]
fn unresponsive_peers_and_scripted_outputs_do_not_stop_frames() {
    if let Some(home) = std::env::var_os("CUPERTINO_FRAME_TEST_HOME") {
        clients(Path::new(&home));
        return;
    }
    let home = Home::new("frame-test");
    let dir = &home.0;
    let probe = UnixListener::bind(dir.join("probe")).unwrap();
    let list = Pipe::new(dir.join("windows.fifo"));
    let shot = Pipe::new(dir.join("frame.png"));
    let shell = json!([{"name":"test-shell", "roles":["session"], "command":[std::env::current_exe().unwrap(),
        "--exact", "unresponsive_peers_and_scripted_outputs_do_not_stop_frames", "--nocapture"]}]);
    let later = LIST_AT + LATER_LIST_AFTER_LIST;
    let mut server = Session(
        home.server(shell)
            .arg("--window-list")
            .arg(format!("{LIST_AT}:{}", list.path.display()))
            .arg("--window-list")
            .arg(format!("{later}:{}", dir.join("later").display()))
            .arg("--shot")
            .arg(format!(
                "{}:{}",
                LIST_AT + SHOT_AFTER_LIST,
                shot.path.display()
            ))
            .arg("--screenshot-after")
            .arg((later + LAST_SHOT_AFTER_LATER_LIST).to_string())
            .args(["--exit-after-screenshot", "--screenshot"])
            .arg(dir.join("last.png"))
            .env("CUPERTINO_FRAME_TEST_HOME", dir)
            .spawn()
            .unwrap(),
    );
    assert!(readable(&probe, SETUP), "test clients did not connect");
    let (mut peer, _) = probe.accept().unwrap();
    peer.set_read_timeout(Some(FRAME_WAIT)).unwrap();
    let listed = String::from_utf8(list.contents(&mut peer)).unwrap();
    for title in ["stalled-client", "frame-client"] {
        assert!(
            listed.lines().any(
                |line| serde_json::from_str::<serde_json::Value>(line).unwrap()["title"] == title
            ),
            "{listed}"
        );
    }
    let captured = shot.contents(&mut peer);
    assert!(server.finish().success());
    assert!(fs::read_to_string(dir.join("later"))
        .unwrap()
        .contains("renamed-after-sample"));
    for bytes in [captured, fs::read(dir.join("last.png")).unwrap()] {
        let image = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png).unwrap();
        assert_eq!((image.width(), image.height()), (192, 128));
    }
}

#[test]
fn a_stalled_screenshot_fails_the_run_after_writing_the_other_output() {
    let home = Home::new("snapshot-failure");
    let stalled = Pipe::new(home.0.join("shot.png"));
    let mut server = Session(
        home.server(json!([]))
            .args([
                "--extra-output",
                "second:96x64",
                "--screenshot-after",
                "0",
                "--exit-after-screenshot",
                "--screenshot",
            ])
            .arg(&stalled.path)
            .spawn()
            .unwrap(),
    );
    assert!(readable(&stalled.opened, SETUP));
    assert!(
        !server.finish().success(),
        "unwritten screenshot reported as a successful exit"
    );
    let other = image::open(home.0.join("shot-second.png")).unwrap();
    assert_eq!((other.width(), other.height()), (96, 64));
}
