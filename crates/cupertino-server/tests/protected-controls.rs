//! Desktop control keeps away from protected surfaces (`crates/cupertino-server/src/control`): every surface of a
//! screenshot program the server started (its capture overlay, its Share Screen chooser) is one, known by the process
//! the server holds, never by a title or a namespace. Against the headless server, a controller allowed by a remembered
//! answer and an ordinary window beside the program's:
//! - an observation lists the ordinary window with its published controls and nothing of the screenshot program;
//! - an action on the program's window is refused as outside the run's scope, while one on the ordinary window reaches
//!   its application;
//! - pointer input over the program's overlay, and keyboard input while the overlay holds the keyboard, are refused as
//!   blocked; pointer input over the ordinary window is carried out, and keyboard input once the program ended.
//!
//!
//! The consent panel is one too: the prompts' connection (the `consent` role) sees the consent authority and a lock
//! screen's does not; the panel, a prompt surface the authority names, pauses no active run, yet that run's input does
//! not reach it; a scripted key on the panel does not allow a run, the person's key does, and refuses one.
//!
//! This test binary stands in for the screenshot program: started with [`STAND_IN`] it maps the chooser, publishing
//! its controls, and the overlay on the connection the server made for it ([`screenshot_stand_in`]).

use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use cupertino_app_protocols::semantics as wire;
use cupertino_assets::Assets;
use cupertino_defaults::desktop_control::{App, Decision, DesktopControlConf, Service};
use cupertino_input::ScreenshotRequest;
use cupertino_peer::component::process_start_ticks;
use cupertino_peer::ProcessHandle;
use cupertino_protocols::client::control::cupertino_control_authority_v1::{
    self, CupertinoControlAuthorityV1,
};
use cupertino_protocols::client::control::cupertino_control_session_v1::{
    self, Capability, CupertinoControlSessionV1, KeyState, Phase, Refusal,
};
use cupertino_protocols::client::control::cupertino_control_v1::CupertinoControlV1;
use cupertino_protocols::client::semantics::cupertino_semantic_surface_v1::{
    self, CupertinoSemanticSurfaceV1,
};
use cupertino_protocols::client::semantics::cupertino_semantics_v1::CupertinoSemanticsV1;
use cupertino_server::authorization::ShellRole;
use cupertino_server::backend::headless;
use cupertino_server::capture::launch::OVERLAY_NAMESPACE;
use cupertino_server::control::protected_surface;
use cupertino_server::shell_connection::shell_process_connection;
use cupertino_server::state::{insert_client, ClientState};
use cupertino_server::{Config, Cupertino};
use smithay::backend::input::{
    Device, DeviceCapability, Event as DeviceEvent, InputBackend, InputEvent,
    KeyState as DeviceKey, KeyboardKeyEvent, Keycode, UnusedEvent,
};
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_protocols::xdg::shell::client::{
    xdg_surface, xdg_toplevel, xdg_wm_base,
};
use smithay::reexports::wayland_server::Display;
use wayland_client::protocol::{
    wl_buffer, wl_callback, wl_compositor, wl_keyboard, wl_output, wl_registry, wl_seat, wl_shm,
    wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

/// How long the test waits for the server, the stand-in and the clients.
const WAIT: Duration = Duration::from_secs(30);

/// The headless output's size in pixels, at scale 1.
const OUTPUT: (i32, i32) = (800, 600);

/// The argument that makes this test binary the screenshot program: a filter after `--` that names no test.
const STAND_IN: &str = "stand-in";

/// The side of the program's overlay, square at the output's top left corner, in points.
const OVERLAY_SIDE: i32 = 200;

/// The program's chooser and the ordinary window: title, width and height in points, and the name of the one button
/// each publishes.
const CHOOSER: (&str, i32, i32, &str) = ("Share Screen", 120, 80, "Share");
const ORDINARY: (&str, i32, i32, &str) = ("Notes", 600, 400, "Save");

/// How far inside its bottom right corner the pointer goes over the ordinary window: clear of the chooser, which the
/// server places over the middle of the ordinary window at most.
const CORNER_INSET: f64 = 40.0;

/// The evdev code of the A key.
const KEY_A: u32 = 30;

/// The roles the server gives the prompts, the program that holds the consent authority.
const PROMPTS: [ShellRole; 3] = [ShellRole::Session, ShellRole::Prompt, ShellRole::Consent];

/// The consent authority's global.
const AUTHORITY: &str = "cupertino_control_authority_v1";

/// The consent panel's surface, centred on the output, in points.
const PANEL: (i32, i32) = (260, 200);

/// The authority's answers: allow, and a refusal remembered for the app.
const ALLOW: u32 = 1;
const DENY: u32 = 2;

/// What a connection heard.
#[derive(Default)]
struct Heard {
    globals: Vec<(u32, String, u32)>,
    configured: bool,
    layer_configured: bool,
    performed: Vec<wire::ActionCall>,
    epoch: Option<(u32, u32)>,
    phase: Option<WEnum<Phase>>,
    revision: Option<u32>,
    completed: Vec<u32>,
    refused: Vec<(u32, WEnum<Refusal>)>,
    observations: Vec<(u32, wire::Observation)>,
    /// The runs the consent authority was asked for, by epoch.
    asked: Vec<(u32, u32)>,
    /// The serials of the keys pressed on the connection's keyboard focus.
    keys: Vec<u32>,
    /// The syncs answered.
    synced: u32,
}

impl Dispatch<wl_registry::WlRegistry, ()> for Heard {
    fn event(
        heard: &mut Self,
        _: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            heard.globals.push((name, interface, version));
        }
    }
}

wayland_client::delegate_noop!(Heard: ignore wl_compositor::WlCompositor);
wayland_client::delegate_noop!(Heard: ignore wl_surface::WlSurface);
wayland_client::delegate_noop!(Heard: ignore wl_shm::WlShm);
wayland_client::delegate_noop!(Heard: ignore wl_shm_pool::WlShmPool);
wayland_client::delegate_noop!(Heard: ignore wl_buffer::WlBuffer);
wayland_client::delegate_noop!(Heard: ignore wl_output::WlOutput);
wayland_client::delegate_noop!(Heard: ignore xdg_toplevel::XdgToplevel);
wayland_client::delegate_noop!(Heard: ignore zwlr_layer_shell_v1::ZwlrLayerShellV1);
wayland_client::delegate_noop!(Heard: ignore CupertinoControlV1);
wayland_client::delegate_noop!(Heard: ignore CupertinoSemanticsV1);
wayland_client::delegate_noop!(Heard: ignore wl_seat::WlSeat);

impl Dispatch<wl_keyboard::WlKeyboard, ()> for Heard {
    fn event(
        heard: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_keyboard::Event::Key {
            serial,
            state: WEnum::Value(wl_keyboard::KeyState::Pressed),
            ..
        } = event
        {
            heard.keys.push(serial);
        }
    }
}

impl Dispatch<wl_callback::WlCallback, ()> for Heard {
    fn event(
        heard: &mut Self,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            heard.synced += 1;
        }
    }
}

impl Dispatch<CupertinoControlAuthorityV1, ()> for Heard {
    fn event(
        heard: &mut Self,
        _: &CupertinoControlAuthorityV1,
        event: cupertino_control_authority_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let cupertino_control_authority_v1::Event::Requested {
            epoch_hi, epoch_lo, ..
        } = event
        {
            heard.asked.push((epoch_hi, epoch_lo));
        }
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for Heard {
    fn event(
        _: &mut Self,
        wm: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            wm.pong(serial);
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, ()> for Heard {
    fn event(
        heard: &mut Self,
        xdg: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            xdg.ack_configure(serial);
            heard.configured = true;
        }
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for Heard {
    fn event(
        heard: &mut Self,
        layer: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_layer_surface_v1::Event::Configure { serial, .. } = event {
            layer.ack_configure(serial);
            heard.layer_configured = true;
        }
    }
}

/// One button named `name` that takes a click.
fn snapshot(name: &str) -> wire::Snapshot {
    wire::Snapshot {
        revision: 1,
        root: 1,
        focus: None,
        nodes: vec![wire::Node {
            truncated: false,
            position: wire::Position::default(),
            id: 1,
            role: "button".into(),
            name: Some(name.into()),
            description: None,
            value: None,
            states: wire::States::default(),
            bounds: wire::Bounds {
                x: 10.0,
                y: 10.0,
                width: 80.0,
                height: 24.0,
            },
            actions: vec![wire::Action::Click],
            children: Vec::new(),
        }],
    }
}

/// A publication publishes its button whenever a run sees it, and keeps the actions it is asked to perform.
impl Dispatch<CupertinoSemanticSurfaceV1, &'static str> for Heard {
    fn event(
        heard: &mut Self,
        publisher: &CupertinoSemanticSurfaceV1,
        event: cupertino_semantic_surface_v1::Event,
        name: &&'static str,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            cupertino_semantic_surface_v1::Event::Interest {
                epoch_hi,
                epoch_lo,
                active,
            } if active != 0 => {
                let (file, len) = wire::seal(&snapshot(name), wire::MAX_FRAME).unwrap();
                publisher.publish(epoch_hi, epoch_lo, file.as_fd(), len);
            }
            cupertino_semantic_surface_v1::Event::Perform { payload, length } => {
                heard
                    .performed
                    .push(wire::read(payload, length, wire::MAX_FRAME).unwrap());
            }
            _ => {}
        }
    }
}

impl Dispatch<CupertinoControlSessionV1, ()> for Heard {
    fn event(
        heard: &mut Self,
        _: &CupertinoControlSessionV1,
        event: cupertino_control_session_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use cupertino_control_session_v1::Event;
        match event {
            Event::Created {
                epoch_hi, epoch_lo, ..
            } => heard.epoch = Some((epoch_hi, epoch_lo)),
            Event::Status { phase, .. } => heard.phase = Some(phase),
            Event::Refused { serial, reason } => heard.refused.push((serial, reason)),
            Event::Geometry { revision, .. } => heard.revision = Some(revision),
            Event::Completed { serial } => heard.completed.push(serial),
            Event::Observation {
                serial,
                payload,
                length,
            } => heard.observations.push((
                serial,
                wire::read(payload, length, wire::MAX_OBSERVATION).unwrap(),
            )),
            _ => {}
        }
    }
}

/// A connection to the server, what it heard, and its registry.
struct Wl {
    conn: Connection,
    queue: EventQueue<Heard>,
    heard: Heard,
    registry: wl_registry::WlRegistry,
}

impl Wl {
    fn new(conn: Connection) -> Wl {
        let queue = conn.new_event_queue();
        let registry = conn.display().get_registry(&queue.handle(), ());
        Wl {
            conn,
            queue,
            heard: Heard::default(),
            registry,
        }
    }

    fn qh(&self) -> QueueHandle<Heard> {
        self.queue.handle()
    }

    /// Bind the global `I` the server offers, at `version` at most.
    fn bind<I>(&self, version: u32) -> I
    where
        I: Proxy + 'static,
        Heard: Dispatch<I, ()>,
    {
        let wanted = I::interface().name;
        let &(name, _, offered) = self
            .heard
            .globals
            .iter()
            .find(|(_, interface, _)| interface == wanted)
            .unwrap_or_else(|| panic!("the server offers {wanted}"));
        self.registry
            .bind::<I, (), Heard>(name, offered.min(version), &self.qh(), ())
    }

    /// Read and dispatch what came, without waiting.
    fn step(&mut self) {
        let _ = self.conn.flush();
        if let Some(guard) = self.conn.prepare_read() {
            let _ = guard.read();
        }
        self.queue.dispatch_pending(&mut self.heard).unwrap();
        let _ = self.conn.flush();
    }

    /// Whether the server offers the global `interface` to this connection.
    fn offers(&self, interface: &str) -> bool {
        self.heard.globals.iter().any(|(_, i, _)| i == interface)
    }

    /// Wait for what comes and dispatch it, in a process of its own.
    fn blocking_step(&mut self) {
        self.queue.blocking_dispatch(&mut self.heard).unwrap();
    }
}

/// A buffer of `width` by `height` pixels of one opaque grey.
fn buffer(wl: &Wl, shm: &wl_shm::WlShm, width: i32, height: i32) -> wl_buffer::WlBuffer {
    let len = width * height * 4;
    let fd =
        rustix::fs::memfd_create("protected-controls", rustix::fs::MemfdFlags::CLOEXEC).unwrap();
    let mut file = std::fs::File::from(fd);
    file.write_all(&vec![0x80; len as usize]).unwrap();
    let pool = shm.create_pool(file.as_fd(), len, &wl.qh(), ());
    let buffer = pool.create_buffer(
        0,
        width,
        height,
        width * 4,
        wl_shm::Format::Xrgb8888,
        &wl.qh(),
        (),
    );
    pool.destroy();
    buffer
}

/// The globals a window needs.
struct Globals {
    compositor: wl_compositor::WlCompositor,
    shm: wl_shm::WlShm,
    wm: xdg_wm_base::XdgWmBase,
    semantics: CupertinoSemanticsV1,
}

impl Globals {
    fn bind(wl: &Wl) -> Globals {
        Globals {
            compositor: wl.bind(4),
            shm: wl.bind(1),
            wm: wl.bind(1),
            semantics: wl.bind(2),
        }
    }
}

/// A toplevel titled `window.0`, of the size it names, asked for: committed without a buffer, which it gets once
/// configured ([`show`]), and a publication of its button.
fn open(
    wl: &Wl,
    globals: &Globals,
    window: (&'static str, i32, i32, &'static str),
) -> (
    wl_surface::WlSurface,
    xdg_toplevel::XdgToplevel,
    CupertinoSemanticSurfaceV1,
) {
    let qh = wl.qh();
    let surface = globals.compositor.create_surface(&qh, ());
    let xdg = globals.wm.get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg.get_toplevel(&qh, ());
    toplevel.set_title(window.0.into());
    toplevel.set_app_id(window.0.into());
    let publisher = globals.semantics.get_surface(&surface, &qh, window.3);
    surface.commit();
    (surface, toplevel, publisher)
}

/// Give `surface` its first buffer, of `width` by `height`.
fn show(wl: &Wl, globals: &Globals, surface: &wl_surface::WlSurface, width: i32, height: i32) {
    surface.attach(Some(&buffer(wl, &globals.shm, width, height)), 0, 0);
    surface.damage(0, 0, width, height);
    surface.commit();
}

/// The screenshot program, when the server started this binary with [`STAND_IN`]: the chooser, a window of its own
/// publishing its button, then the overlay on the overlay layer at the output's top left corner, taking the keyboard,
/// both on the connection the server made for it, until the server closes it. A test run does nothing here.
#[test]
fn screenshot_stand_in() {
    if !std::env::args().any(|arg| arg == STAND_IN) {
        return;
    }
    let mut wl = Wl::new(Connection::connect_to_env().expect("started with a WAYLAND_SOCKET"));
    wl.queue.roundtrip(&mut wl.heard).unwrap();
    let globals = Globals::bind(&wl);
    let layer_shell: zwlr_layer_shell_v1::ZwlrLayerShellV1 = wl.bind(4);
    let (chooser, _toplevel, _publisher) = open(&wl, &globals, CHOOSER);
    while !wl.heard.configured {
        wl.blocking_step();
    }
    show(&wl, &globals, &chooser, CHOOSER.1, CHOOSER.2);
    let overlay = globals.compositor.create_surface(&wl.qh(), ());
    let layer = layer_shell.get_layer_surface(
        &overlay,
        None,
        zwlr_layer_shell_v1::Layer::Overlay,
        OVERLAY_NAMESPACE.into(),
        &wl.qh(),
        (),
    );
    layer.set_size(OVERLAY_SIDE as u32, OVERLAY_SIDE as u32);
    layer.set_anchor(zwlr_layer_surface_v1::Anchor::Top | zwlr_layer_surface_v1::Anchor::Left);
    layer.set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive);
    overlay.commit();
    while !wl.heard.layer_configured {
        wl.blocking_step();
    }
    show(&wl, &globals, &overlay, OVERLAY_SIDE, OVERLAY_SIDE);
    while wl.queue.blocking_dispatch(&mut wl.heard).is_ok() {}
}

/// This test binary, running [`screenshot_stand_in`] alone.
fn screenshot_command() -> Vec<String> {
    let exe = std::env::current_exe().unwrap().display().to_string();
    [
        exe.as_str(),
        "--exact",
        "screenshot_stand_in",
        "--nocapture",
        "--",
        STAND_IN,
    ]
    .map(String::from)
    .to_vec()
}

/// The headless server, starting this binary as its screenshot program.
struct Rig {
    event_loop: EventLoop<'static, Cupertino>,
    state: Cupertino,
}

impl Rig {
    fn new() -> Rig {
        // the software rasteriser, as the program tests run the headless server
        std::env::set_var("LIBGL_ALWAYS_SOFTWARE", "1");
        let event_loop: EventLoop<'static, Cupertino> = EventLoop::try_new().unwrap();
        let display: Display<Cupertino> = Display::new().unwrap();
        let config = Config {
            screenshot_command: screenshot_command(),
            ..Config::default()
        };
        let mut state = Cupertino::new(
            display,
            event_loop.handle(),
            event_loop.get_signal(),
            config,
            false,
            &Assets::builtin(),
        )
        .expect("the server starts");
        headless::init(&mut state, OUTPUT, 1.0).expect("an offscreen renderer");
        Rig { event_loop, state }
    }

    /// A connection of the server's making with `state`.
    fn connect(&mut self, state: ClientState) -> Wl {
        let (ours, theirs) = UnixStream::pair().unwrap();
        insert_client(&self.state.display_handle, ours, state).unwrap();
        Wl::new(Connection::from_socket(theirs).unwrap())
    }

    /// Turn the server's loop and read every client until `done` holds, which is asked before every turn.
    fn until(
        &mut self,
        clients: &mut [&mut Wl],
        what: &str,
        mut done: impl FnMut(&Rig, &[&mut Wl]) -> bool,
    ) {
        let deadline = Instant::now() + WAIT;
        loop {
            for wl in clients.iter_mut() {
                wl.step();
            }
            if done(self, clients) {
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting until {what}");
            headless::step(
                &mut self.state,
                &mut self.event_loop,
                Duration::from_millis(5),
            )
            .unwrap();
        }
    }

    /// The handle desktop control knows a window by: the screenshot program's window, or the other one.
    fn handle(&self, screenshot: bool) -> u64 {
        self.state
            .windows
            .iter()
            .find(|w| self.state.is_screenshot_surface(w.surface()) == screenshot)
            .map(|w| w.id.0 * 2)
            .expect("the window is mapped")
    }
}

/// The children of this process that run as the screenshot stand-in.
fn stand_ins() -> Vec<u32> {
    let mut pids = Vec::new();
    for task in std::fs::read_dir("/proc/self/task").unwrap().flatten() {
        let children = std::fs::read_to_string(task.path().join("children")).unwrap_or_default();
        pids.extend(
            children
                .split_whitespace()
                .filter_map(|p| p.parse::<u32>().ok()),
        );
    }
    pids.retain(|pid| {
        std::fs::read(format!("/proc/{pid}/cmdline"))
            .unwrap_or_default()
            .split(|b| *b == 0)
            .any(|arg| arg == STAND_IN.as_bytes())
    });
    pids
}

/// The app desktop control names a run of this process by: the executable of its parent, as a run under no window.
fn this_app() -> App {
    let parent = cupertino_peer::own_parent();
    let exe = std::fs::read_link(format!("/proc/{parent}/exe")).unwrap();
    let exe = exe.to_str().unwrap();
    App::Executable(exe.strip_suffix(" (deleted)").unwrap_or(exe).into())
}

/// A controller's run on a connection of its own, named as this process.
struct Run {
    wl: Wl,
    session: CupertinoControlSessionV1,
    epoch: (u32, u32),
    serial: u32,
}

impl Run {
    /// A run asking for `capabilities` on the output, its start sent as request 1.
    fn request(rig: &mut Rig, capabilities: Capability) -> Run {
        let stream =
            shell_process_connection(&rig.state.display_handle, &[ShellRole::DesktopControl])
                .unwrap();
        let mut wl = Wl::new(Connection::from_socket(stream).unwrap());
        rig.until(
            &mut [&mut wl],
            "the controller sees the globals",
            |_, wl| wl[0].offers("cupertino_control_v1"),
        );
        let output: wl_output::WlOutput = wl.bind(4);
        let control: CupertinoControlV1 = wl.bind(5);
        let session = control.get_session(&wl.qh(), ());
        rig.until(&mut [&mut wl], "the run is created", |_, wl| {
            wl[0].heard.epoch.is_some()
        });
        let epoch = wl.heard.epoch.unwrap();
        let own = ProcessHandle::open(std::process::id()).unwrap();
        let start = process_start_ticks(&own).unwrap();
        session.identify(std::process::id(), (start >> 32) as u32, start as u32);
        session.start(1, epoch.0, epoch.1, &output, capabilities);
        Run {
            wl,
            session,
            epoch,
            serial: 1,
        }
    }

    /// A run active on the output with every capability but capture, allowed by a remembered answer.
    fn start(rig: &mut Rig) -> Run {
        let mut store = DesktopControlConf::default();
        store
            .set(Service::Control, &this_app(), Decision::Allow)
            .unwrap();
        rig.state.apply_desktop_control_store(store);
        let mut run = Run::request(
            rig,
            Capability::Observe | Capability::Pointer | Capability::Keyboard | Capability::Semantic,
        );
        assert_eq!(run.answer(rig, 1), Ok(()), "the run starts");
        assert_eq!(run.wl.heard.phase, Some(WEnum::Value(Phase::Active)));
        run
    }

    fn next(&mut self) -> u32 {
        self.serial += 1;
        self.serial
    }

    /// The answer to request `serial`: `Ok` once it was carried out, the refusal's reason else.
    fn answer(&mut self, rig: &mut Rig, serial: u32) -> Result<(), WEnum<Refusal>> {
        rig.until(
            &mut [&mut self.wl],
            &format!("request {serial} is answered"),
            |_, wl| {
                let heard = &wl[0].heard;
                heard.completed.contains(&serial) || heard.refused.iter().any(|(s, _)| *s == serial)
            },
        );
        match self.wl.heard.refused.iter().find(|(s, _)| *s == serial) {
            Some((_, reason)) => Err(*reason),
            None => Ok(()),
        }
    }

    fn observe(&mut self, rig: &mut Rig) -> wire::Observation {
        let serial = self.next();
        self.session.observe(serial, self.epoch.0, self.epoch.1);
        rig.until(&mut [&mut self.wl], "the observation comes", |_, wl| {
            wl[0].heard.observations.iter().any(|(s, _)| *s == serial)
        });
        let at = self
            .wl
            .heard
            .observations
            .iter()
            .position(|(s, _)| *s == serial)
            .unwrap();
        self.wl.heard.observations.remove(at).1
    }

    /// Click the button of the window known by `handle`, at its publication's first revision.
    fn click(&mut self, handle: u64) -> u32 {
        let serial = self.next();
        let call = wire::Invocation {
            window: handle,
            revision: 1,
            node: 1,
            action: wire::Action::Click,
            data: None,
        };
        let (file, len) = wire::seal(&call, wire::MAX_FRAME).unwrap();
        self.session
            .perform(serial, self.epoch.0, self.epoch.1, file.as_fd(), len);
        serial
    }

    /// Send the input `send` makes of a serial and the run's geometry revision, sent again while windows still move
    /// into place (`stale_geometry`, as the protocol has a controller do); its answer.
    fn input(
        &mut self,
        rig: &mut Rig,
        send: impl Fn(&CupertinoControlSessionV1, u32, (u32, u32), u32),
    ) -> Result<(), WEnum<Refusal>> {
        let deadline = Instant::now() + WAIT;
        loop {
            let serial = self.next();
            let revision = self.wl.heard.revision.expect("the run's geometry");
            send(&self.session, serial, self.epoch, revision);
            match self.answer(rig, serial) {
                Err(WEnum::Value(Refusal::StaleGeometry)) => {
                    assert!(
                        Instant::now() < deadline,
                        "the run's geometry never settles"
                    );
                }
                answer => return answer,
            }
        }
    }

    fn motion(&mut self, rig: &mut Rig, x: f64, y: f64) -> Result<(), WEnum<Refusal>> {
        self.input(rig, |session, serial, (hi, lo), revision| {
            session.motion(serial, hi, lo, revision, x, y)
        })
    }

    fn key(&mut self, rig: &mut Rig, state: KeyState) -> Result<(), WEnum<Refusal>> {
        self.input(rig, |session, serial, (hi, lo), revision| {
            session.key(serial, hi, lo, revision, KEY_A, state)
        })
    }
}

/// Kill `pid` and wait until it has exited as the server sees it: its pidfd readable.
fn end(pid: u32) {
    let process = ProcessHandle::open(pid).unwrap();
    process.send(libc::SIGKILL).unwrap();
    let deadline = Instant::now() + WAIT;
    while !process.has_exited() {
        assert!(Instant::now() < deadline, "the stand-in {pid} did not end");
        let fd = process.fd();
        let mut fds = [rustix::event::PollFd::new(
            &fd,
            rustix::event::PollFlags::IN,
        )];
        let left =
            rustix::event::Timespec::try_from(deadline.saturating_duration_since(Instant::now()))
                .unwrap();
        let _ = rustix::event::poll(&mut fds, Some(&left));
    }
}

#[test]
fn desktop_control_neither_sees_nor_reaches_the_screenshot_program() {
    let mut rig = Rig::new();
    let mut ordinary = rig.connect(ClientState::default());
    rig.until(
        &mut [&mut ordinary],
        "the ordinary client sees the globals",
        |_, wl| {
            wl[0]
                .heard
                .globals
                .iter()
                .any(|(_, i, _)| i == "cupertino_semantics_v1")
        },
    );
    let globals = Globals::bind(&ordinary);
    let (surface, _toplevel, _publisher) = open(&ordinary, &globals, ORDINARY);
    rig.until(
        &mut [&mut ordinary],
        "the ordinary window is configured",
        |_, wl| wl[0].heard.configured,
    );
    show(&ordinary, &globals, &surface, ORDINARY.1, ORDINARY.2);
    rig.until(
        &mut [&mut ordinary],
        "the ordinary window maps",
        |rig, _| rig.state.windows.len() == 1,
    );

    // the program the server starts for a screenshot shortcut maps its chooser and its overlay
    rig.state.start_screenshot(ScreenshotRequest::Selection);
    rig.until(
        &mut [&mut ordinary],
        "the screenshot program maps its surfaces",
        |rig, _| rig.state.screenshot_overlay_shown() && rig.state.windows.len() == 2,
    );
    let (chooser, notes) = (rig.handle(true), rig.handle(false));

    let mut run = Run::start(&mut rig);
    // the ordinary window's button is published once the run sees it
    let deadline = Instant::now() + WAIT;
    let observation = loop {
        let observation = run.observe(&mut rig);
        let published = observation
            .windows
            .iter()
            .any(|w| w.handle == notes && w.snapshot.is_some());
        if published {
            break observation;
        }
        assert!(
            Instant::now() < deadline,
            "the ordinary window's button is never observed"
        );
        rig.until(
            &mut [&mut ordinary],
            "the ordinary client publishes",
            |_, _| true,
        );
    };
    let listed: Vec<(u64, &str)> = observation
        .windows
        .iter()
        .map(|w| (w.handle, w.title.as_str()))
        .collect();
    assert_eq!(
        listed,
        [(notes, ORDINARY.0)],
        "only the ordinary window is observed"
    );
    assert!(
        observation
            .windows
            .iter()
            .all(|w| w.app_id != OVERLAY_NAMESPACE && w.app_id != CHOOSER.0),
        "{listed:?}"
    );
    let notes_origin = observation.windows[0].origin;

    // an action on the program's window is outside the run's scope; one on the ordinary window reaches its application
    let refused = run.click(chooser);
    assert_eq!(
        run.answer(&mut rig, refused),
        Err(WEnum::Value(Refusal::OutsideScope))
    );
    let clicked = run.click(notes);
    rig.until(
        &mut [&mut ordinary, &mut run.wl],
        "the click reaches the ordinary window",
        |_, wl| !wl[0].heard.performed.is_empty(),
    );
    assert_eq!(ordinary.heard.performed[0].node, 1);
    assert_eq!(ordinary.heard.performed[0].action, wire::Action::Click);
    assert!(run.wl.heard.refused.iter().all(|(s, _)| *s != clicked));

    // the pointer does not reach the overlay, nor the keyboard while the overlay holds it
    let middle = f64::from(OVERLAY_SIDE) / 2.0;
    assert_eq!(
        run.motion(&mut rig, middle, middle),
        Err(WEnum::Value(Refusal::Blocked))
    );
    let corner = (
        notes_origin[0] + f64::from(ORDINARY.1) - CORNER_INSET,
        notes_origin[1] + f64::from(ORDINARY.2) - CORNER_INSET,
    );
    assert_eq!(
        run.motion(&mut rig, corner.0, corner.1),
        Ok(()),
        "the ordinary window takes the pointer"
    );
    assert_eq!(
        run.key(&mut rig, KeyState::Pressed),
        Err(WEnum::Value(Refusal::Blocked))
    );

    // once the program ended, the keyboard is the ordinary window's business again
    for pid in stand_ins() {
        end(pid);
    }
    rig.until(
        &mut [&mut ordinary, &mut run.wl],
        "the screenshot program's surfaces go",
        |rig, _| !rig.state.screenshot_overlay_shown() && rig.state.windows.len() == 1,
    );
    assert_eq!(run.key(&mut rig, KeyState::Pressed), Ok(()));
    assert_eq!(run.key(&mut rig, KeyState::Released), Ok(()));
}

/// The person's keyboard, as a device the server reads.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Keyboard;

impl Device for Keyboard {
    fn id(&self) -> String {
        "person-keyboard".into()
    }

    fn name(&self) -> String {
        "The person's keyboard".into()
    }

    fn has_capability(&self, capability: DeviceCapability) -> bool {
        capability == DeviceCapability::Keyboard
    }

    fn usb_id(&self) -> Option<(u32, u32)> {
        None
    }

    fn syspath(&self) -> Option<std::path::PathBuf> {
        None
    }
}

/// A key the person presses or releases, by its evdev code.
struct PersonKey {
    code: u32,
    state: DeviceKey,
}

impl DeviceEvent<Person> for PersonKey {
    fn time(&self) -> u64 {
        0
    }

    fn device(&self) -> Keyboard {
        Keyboard
    }
}

impl KeyboardKeyEvent<Person> for PersonKey {
    fn key_code(&self) -> Keycode {
        // xkb's codes are evdev's plus 8
        Keycode::new(self.code + 8)
    }

    fn state(&self) -> DeviceKey {
        self.state
    }

    fn count(&self) -> u32 {
        u32::from(self.state == DeviceKey::Pressed)
    }
}

/// The person at the keyboard: input the server takes from a device, as a backend delivers it.
struct Person;

impl InputBackend for Person {
    type Device = Keyboard;
    type KeyboardKeyEvent = PersonKey;
    type PointerAxisEvent = UnusedEvent;
    type PointerButtonEvent = UnusedEvent;
    type PointerMotionEvent = UnusedEvent;
    type PointerMotionAbsoluteEvent = UnusedEvent;
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
    type SpecialEvent = ();
}

/// The person types the A key on the keyboard.
fn person_types(rig: &mut Rig) {
    for state in [DeviceKey::Pressed, DeviceKey::Released] {
        rig.state
            .process_input_event(InputEvent::<Person>::Keyboard {
                event: PersonKey { code: KEY_A, state },
            });
    }
}

/// The prompts' stand-in: a connection with their roles, holding the consent authority, and the panel on which the
/// person answers, a layer surface on the overlay layer centred on the output that takes the keyboard.
struct Prompts {
    wl: Wl,
    authority: CupertinoControlAuthorityV1,
}

impl Prompts {
    fn start(rig: &mut Rig) -> Prompts {
        let stream = shell_process_connection(&rig.state.display_handle, &PROMPTS).unwrap();
        let mut wl = Wl::new(Connection::from_socket(stream).unwrap());
        rig.until(&mut [&mut wl], "the prompts see the globals", |_, wl| {
            wl[0].offers("wl_compositor")
        });
        let compositor: wl_compositor::WlCompositor = wl.bind(4);
        let shm: wl_shm::WlShm = wl.bind(1);
        let layer_shell: zwlr_layer_shell_v1::ZwlrLayerShellV1 = wl.bind(4);
        let seat: wl_seat::WlSeat = wl.bind(1);
        let _keyboard = seat.get_keyboard(&wl.qh(), ());
        let authority: CupertinoControlAuthorityV1 = wl.bind(2);
        let panel = compositor.create_surface(&wl.qh(), ());
        let layer = layer_shell.get_layer_surface(
            &panel,
            None,
            zwlr_layer_shell_v1::Layer::Overlay,
            "consent".into(),
            &wl.qh(),
            (),
        );
        layer.set_size(PANEL.0 as u32, PANEL.1 as u32);
        layer.set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive);
        // the panel is the authority's before it shows, as the prompts name their alert's surface
        authority.set_surface(&panel);
        panel.commit();
        rig.until(&mut [&mut wl], "the panel is configured", |_, wl| {
            wl[0].heard.layer_configured
        });
        panel.attach(Some(&buffer(&wl, &shm, PANEL.0, PANEL.1)), 0, 0);
        panel.damage(0, 0, PANEL.0, PANEL.1);
        panel.commit();
        rig.until(&mut [&mut wl], "the panel is shown", |rig, _| {
            rig.state.prompt_shown()
        });
        Prompts { wl, authority }
    }

    /// Wait until the server has handled every request the prompts sent so far.
    fn sync(&mut self, rig: &mut Rig) {
        let synced = self.wl.heard.synced;
        self.wl.conn.display().sync(&self.wl.qh(), ());
        rig.until(
            &mut [&mut self.wl],
            "the server answers the sync",
            |_, wl| wl[0].heard.synced > synced,
        );
    }

    /// Answer the request of `run` with `allow`, quoting the last key pressed on the panel; once the server handled it.
    fn answer(&mut self, rig: &mut Rig, run: &Run, allow: u32) {
        let serial = *self.wl.heard.keys.last().expect("a key reached the panel");
        self.authority
            .answer(run.epoch.0, run.epoch.1, allow, serial);
        self.sync(rig);
    }

    /// Wait until a key pressed after `before` keys reached the panel.
    fn key_after(&mut self, rig: &mut Rig, before: usize) {
        rig.until(&mut [&mut self.wl], "the key reaches the panel", |_, wl| {
            wl[0].heard.keys.len() > before
        });
    }
}

/// A run that asks for capture, which the person has not decided for this process: the consent authority asks.
fn asking(rig: &mut Rig, prompts: &mut Prompts) -> Run {
    let mut run = Run::request(rig, Capability::Observe | Capability::Capture);
    assert_eq!(run.answer(rig, 1), Ok(()), "the run asks");
    assert_eq!(
        run.wl.heard.phase,
        Some(WEnum::Value(Phase::AwaitingConsent))
    );
    let epoch = run.epoch;
    rig.until(&mut [&mut prompts.wl], "the panel is asked", |_, wl| {
        wl[0].heard.asked.contains(&epoch)
    });
    run
}

#[test]
fn the_consent_panel_takes_the_persons_answer_and_never_desktop_controls() {
    let mut rig = Rig::new();

    // the prompts' connection sees the consent authority, the lock screen's does not
    let stream = shell_process_connection(
        &rig.state.display_handle,
        &[ShellRole::Session, ShellRole::SessionLock],
    )
    .unwrap();
    let mut lock = Wl::new(Connection::from_socket(stream).unwrap());
    rig.until(
        &mut [&mut lock],
        "the lock screen sees the globals",
        |_, wl| wl[0].offers("ext_session_lock_manager_v1"),
    );
    assert!(
        !lock.offers(AUTHORITY),
        "the session lock role holds no consent authority"
    );

    let mut active = Run::start(&mut rig);
    let mut prompts = Prompts::start(&mut rig);
    assert!(
        prompts.wl.offers(AUTHORITY),
        "the consent role sees the consent authority"
    );
    rig.until(
        &mut [&mut prompts.wl, &mut active.wl],
        "the panel takes the keyboard",
        |rig, _| {
            let focus = rig
                .state
                .seat
                .get_keyboard()
                .and_then(|k| k.current_focus());
            focus.is_some_and(|surface| protected_surface(&surface))
        },
    );

    // the panel pauses no run, and an active run's input does not reach it
    let mut run = asking(&mut rig, &mut prompts);
    active.wl.step();
    assert_eq!(active.wl.heard.phase, Some(WEnum::Value(Phase::Active)));
    assert_eq!(
        active.key(&mut rig, KeyState::Pressed),
        Err(WEnum::Value(Refusal::Blocked))
    );
    let centre = (f64::from(OUTPUT.0) / 2.0, f64::from(OUTPUT.1) / 2.0);
    assert_eq!(
        active.motion(&mut rig, centre.0, centre.1),
        Err(WEnum::Value(Refusal::Blocked))
    );

    // a scripted key on the panel allows nothing
    let keys = prompts.wl.heard.keys.len();
    rig.state.scripted_key("a").unwrap();
    prompts.key_after(&mut rig, keys);
    prompts.answer(&mut rig, &run, ALLOW);
    run.wl.step();
    assert_eq!(
        run.wl.heard.phase,
        Some(WEnum::Value(Phase::AwaitingConsent)),
        "a scripted key does not allow the run"
    );

    // the person's key on the panel does
    let keys = prompts.wl.heard.keys.len();
    person_types(&mut rig);
    prompts.key_after(&mut rig, keys);
    prompts.answer(&mut rig, &run, ALLOW);
    rig.until(
        &mut [&mut run.wl],
        "the person's Allow starts the run",
        |_, wl| wl[0].heard.phase == Some(WEnum::Value(Phase::Active)),
    );

    // and refuses another
    let mut refused = asking(&mut rig, &mut prompts);
    let keys = prompts.wl.heard.keys.len();
    person_types(&mut rig);
    prompts.key_after(&mut rig, keys);
    prompts.answer(&mut rig, &refused, DENY);
    rig.until(
        &mut [&mut refused.wl],
        "the person's Don't Allow refuses the run",
        |_, wl| wl[0].heard.phase == Some(WEnum::Value(Phase::Inactive)),
    );
    active.wl.step();
    assert_eq!(active.wl.heard.phase, Some(WEnum::Value(Phase::Active)));
}
