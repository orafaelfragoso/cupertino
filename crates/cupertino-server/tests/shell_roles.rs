//! Who may use the private shell roles, over the wire: the compositor runs in-process on an offscreen EGL device and
//! the clients speak raw Wayland, so a test can send what a hostile client would (a request on a global it was never
//! offered, on an object id it guessed, on a surface of another client). The shell roles are `cupertino_shell_v1` and
//! the objects it creates; `cupertino_app_effects_v1` is the public protocol. Tests skip (pass) when no EGL device is
//! available.

use crate::common;

use std::io::{ErrorKind, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use cupertino_server::authorization::ShellRole;
use cupertino_server::backend::headless;
use cupertino_server::config::Config;
use cupertino_server::shell_connection::{
    bind_holder, connect_bound_client, shell_process_connection, shell_process_state,
};
use cupertino_server::Cupertino;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;

const OUTPUT: (i32, i32) = (1000, 700);

/// `wl_display` is object 1; the first object a client allocates is 2.
const DISPLAY: u32 = 1;
const DISPLAY_GET_REGISTRY: u16 = 1;
const DISPLAY_ERROR: u16 = 0;
const REGISTRY_GLOBAL: u16 = 0;
const REGISTRY_BIND: u16 = 0;
const COMPOSITOR_CREATE_SURFACE: u16 = 0;

const SHELL_GET_MENU_BAR: u16 = 1;
const SHELL_GET_DOCK: u16 = 2;
const SHELL_GET_SESSION: u16 = 3;
const SHELL_GET_WINDOW_MANAGER: u16 = 4;
const SESSION_ACTIVATE_APP: u16 = 1;
const EFFECTS_GET_BACKDROP: u16 = 1;
const EFFECTS_SET_TITLEBAR_STYLE: u16 = 2;
const EFFECTS_SET_DISABLED_CONTROLS: u16 = 5;
const BACKDROP_SET_MATERIAL: u16 = 1;

/// `wl_display.error` code of a global the client is not offered; the server reports it on `wl_display` itself. A
/// request on an object id the server does not know ends the connection without an error event.
const INVALID_OBJECT: u32 = 0;
/// `cupertino_shell_v1.error.role`.
const ROLE: u32 = 0;
/// `cupertino_shell_v1.error.permission`.
const PERMISSION: u32 = 2;

struct Harness {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
}

impl Harness {
    fn new() -> Option<Harness> {
        let event_loop: EventLoop<'static, Cupertino> =
            EventLoop::try_new().expect("create test event loop");
        let display: Display<Cupertino> = Display::new().expect("create test display");
        let mut state = Cupertino::new(
            display,
            event_loop.handle(),
            event_loop.get_signal(),
            Config::default(),
            true,
            &cupertino_assets::Assets::builtin(),
        )
        .expect("create test compositor");
        match headless::init(&mut state, OUTPUT, 1.0) {
            Ok(()) => Some(Harness { state, event_loop }),
            Err(e) => {
                common::egl_unavailable(format!("{e:#}"));
                None
            }
        }
    }

    fn step(&mut self) {
        headless::step(
            &mut self.state,
            &mut self.event_loop,
            Duration::from_millis(5),
        )
        .unwrap();
    }

    fn socket_path(&self) -> PathBuf {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
        PathBuf::from(dir).join(self.state.socket_name.as_ref().unwrap())
    }

    /// A client of the public socket.
    fn public(&self) -> Wire {
        Wire::new(UnixStream::connect(self.socket_path()).expect("connecting to the compositor"))
    }

    /// A connection the server made for a shell process holding `roles`.
    fn shell(&mut self, roles: &[ShellRole]) -> Wire {
        Wire::new(shell_process_connection(&self.state.display_handle, roles).unwrap())
    }

    /// A connection made for `process`, the way the supervisor makes one: it holds `roles` until the process exits.
    fn held_shell(&mut self, roles: &[ShellRole], process: &Child) -> Wire {
        let (stream, state) =
            connect_bound_client(&self.state.display_handle, shell_process_state(roles)).unwrap();
        bind_holder(&state, process.id()).unwrap();
        Wire::new(stream)
    }

    /// Run the server and read what every wire got, long enough for a round trip.
    fn settle(&mut self, wires: &mut [&mut Wire]) {
        for _ in 0..8 {
            self.step();
            for wire in wires.iter_mut() {
                wire.pump();
            }
        }
    }
}

#[derive(Clone, Debug)]
struct Event {
    object: u32,
    opcode: u16,
    payload: Vec<u8>,
}

/// A hand-written Wayland client: requests go out as bytes and events come back parsed.
struct Wire {
    stream: UnixStream,
    unread: Vec<u8>,
    events: Vec<Event>,
    closed: bool,
    next_id: u32,
}

enum Arg<'a> {
    Uint(u32),
    Str(&'a str),
}

impl Wire {
    fn new(stream: UnixStream) -> Wire {
        stream.set_nonblocking(true).unwrap();
        Wire {
            stream,
            unread: Vec::new(),
            events: Vec::new(),
            closed: false,
            next_id: 2,
        }
    }

    /// The next object id of this client.
    fn new_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn send(&mut self, object: u32, opcode: u16, args: &[Arg]) {
        let mut body = Vec::new();
        for arg in args {
            match arg {
                Arg::Uint(v) => body.extend_from_slice(&v.to_ne_bytes()),
                Arg::Str(s) => {
                    body.extend_from_slice(&(s.len() as u32 + 1).to_ne_bytes());
                    body.extend_from_slice(s.as_bytes());
                    body.push(0);
                    while body.len() % 4 != 0 {
                        body.push(0);
                    }
                }
            }
        }
        let mut message = object.to_ne_bytes().to_vec();
        message.extend_from_slice(
            &(((body.len() as u32 + 8) << 16) | u32::from(opcode)).to_ne_bytes(),
        );
        message.extend_from_slice(&body);
        // a closed connection is the point of some tests: the write may fail
        let _ = self.stream.write_all(&message);
    }

    fn pump(&mut self) {
        let mut chunk = [0u8; 4096];
        loop {
            match self.stream.read(&mut chunk) {
                Ok(0) => {
                    self.closed = true;
                    break;
                }
                Ok(n) => self.unread.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(_) => {
                    self.closed = true;
                    break;
                }
            }
        }
        while self.unread.len() >= 8 {
            let word = u32::from_ne_bytes(self.unread[4..8].try_into().unwrap());
            let size = (word >> 16) as usize;
            if self.unread.len() < size {
                break;
            }
            self.events.push(Event {
                object: u32::from_ne_bytes(self.unread[0..4].try_into().unwrap()),
                opcode: (word & 0xffff) as u16,
                payload: self.unread[8..size].to_vec(),
            });
            self.unread.drain(..size);
        }
    }

    /// `wl_display.get_registry`; returns the registry's id.
    fn get_registry(&mut self) -> u32 {
        let registry = self.new_id();
        self.send(DISPLAY, DISPLAY_GET_REGISTRY, &[Arg::Uint(registry)]);
        registry
    }

    /// The globals of `registry` announced so far: (name, interface, version).
    fn globals(&self, registry: u32) -> Vec<(u32, String, u32)> {
        self.events
            .iter()
            .filter(|e| e.object == registry && e.opcode == REGISTRY_GLOBAL)
            .map(|e| {
                let name = u32::from_ne_bytes(e.payload[0..4].try_into().unwrap());
                let len = u32::from_ne_bytes(e.payload[4..8].try_into().unwrap()) as usize;
                let interface = String::from_utf8(e.payload[8..8 + len - 1].to_vec()).unwrap();
                let version_at = 8 + len.next_multiple_of(4);
                let version =
                    u32::from_ne_bytes(e.payload[version_at..version_at + 4].try_into().unwrap());
                (name, interface, version)
            })
            .collect()
    }

    /// `wl_registry.bind` of the global announced under `interface`, at `version`; returns the new object's id.
    fn bind(&mut self, registry: u32, name: u32, interface: &str, version: u32) -> u32 {
        let id = self.new_id();
        self.send(
            registry,
            REGISTRY_BIND,
            &[
                Arg::Uint(name),
                Arg::Str(interface),
                Arg::Uint(version),
                Arg::Uint(id),
            ],
        );
        id
    }

    /// The protocol error the server posted, as (object, code): `wl_display.error`.
    fn error(&self) -> Option<(u32, u32)> {
        self.events
            .iter()
            .find(|e| e.object == DISPLAY && e.opcode == DISPLAY_ERROR)
            .map(|e| {
                (
                    u32::from_ne_bytes(e.payload[0..4].try_into().unwrap()),
                    u32::from_ne_bytes(e.payload[4..8].try_into().unwrap()),
                )
            })
    }

    /// Events other than the registry's announcements and `wl_display` ones, from objects the client created.
    fn events_of(&self, object: u32) -> usize {
        self.events.iter().filter(|e| e.object == object).count()
    }
}

/// The global of `interface` that `wire` was offered, with its version.
fn offered(wire: &Wire, registry: u32, interface: &str) -> Option<(u32, u32)> {
    wire.globals(registry)
        .into_iter()
        .find(|(_, i, _)| i == interface)
        .map(|(name, _, version)| (name, version))
}

/// A client of `h` with an output, a surface and the shell global bound (at its version 9).
struct ShellClient {
    wire: Wire,
    shell: u32,
    output: u32,
    surface: u32,
}

impl ShellClient {
    fn connect(h: &mut Harness, roles: &[ShellRole]) -> Option<ShellClient> {
        let wire = h.shell(roles);
        ShellClient::over(h, wire)
    }

    fn over(h: &mut Harness, mut wire: Wire) -> Option<ShellClient> {
        let registry = wire.get_registry();
        h.settle(&mut [&mut wire]);
        let (shell_name, _) = offered(&wire, registry, "cupertino_shell_v1")?;
        let (output_name, output_version) = offered(&wire, registry, "wl_output").unwrap();
        let (compositor_name, compositor_version) =
            offered(&wire, registry, "wl_compositor").unwrap();
        let shell = wire.bind(registry, shell_name, "cupertino_shell_v1", 9);
        let output = wire.bind(registry, output_name, "wl_output", output_version.min(4));
        let compositor = wire.bind(
            registry,
            compositor_name,
            "wl_compositor",
            compositor_version.min(4),
        );
        let surface = wire.new_id();
        wire.send(compositor, COMPOSITOR_CREATE_SURFACE, &[Arg::Uint(surface)]);
        h.settle(&mut [&mut wire]);
        assert_eq!(wire.error(), None, "binding the shell global");
        Some(ShellClient {
            wire,
            shell,
            output,
            surface,
        })
    }
}

/// The controller's ninth capability is separate from every existing shell and capture role.
#[test]
fn desktop_control_is_private_and_grants_no_other_role() {
    let Some(mut h) = Harness::new() else { return };
    let mut controller = h.shell(&[ShellRole::DesktopControl]);
    let cr = controller.get_registry();
    let mut public = h.public();
    let pr = public.get_registry();
    let mut dock = h.shell(&[ShellRole::Dock]);
    let dr = dock.get_registry();
    h.settle(&mut [&mut controller, &mut public, &mut dock]);
    let (name, version) = offered(&controller, cr, "cupertino_control_v1").unwrap();
    assert_eq!(version, 5);
    assert!(offered(&controller, cr, "ext_image_copy_capture_manager_v1").is_some());
    assert!(offered(
        &controller,
        cr,
        "ext_output_image_capture_source_manager_v1"
    )
    .is_some());
    for interface in [
        "cupertino_shell_v1",
        "cupertino_control_authority_v1",
        "ext_foreign_toplevel_image_capture_source_manager_v1",
        "ext_session_lock_manager_v1",
        "cupertino_screenshot_v1",
    ] {
        assert_eq!(offered(&controller, cr, interface), None, "{interface}");
    }
    assert_eq!(offered(&public, pr, "cupertino_control_v1"), None);
    assert_eq!(offered(&dock, dr, "cupertino_control_v1"), None);
    public.bind(pr, name, "cupertino_control_v1", 1);
    dock.bind(dr, name, "cupertino_control_v1", 1);
    h.settle(&mut [&mut public, &mut dock]);
    assert_eq!(public.error(), Some((DISPLAY, INVALID_OBJECT)));
    assert_eq!(dock.error(), Some((DISPLAY, INVALID_OBJECT)));
    assert!(public.closed && dock.closed);
    assert_eq!(controller.error(), None);
}

fn control_manager(h: &mut Harness, wire: &mut Wire) -> u32 {
    let registry = wire.get_registry();
    h.settle(&mut [&mut *wire]);
    let (name, _) = offered(wire, registry, "cupertino_control_v1").unwrap();
    let manager = wire.bind(registry, name, "cupertino_control_v1", 1);
    h.settle(&mut [&mut *wire]);
    assert_eq!(
        wire.error(),
        None,
        "the controller bound while its holder was alive"
    );
    manager
}

fn control_session(h: &mut Harness, wire: &mut Wire, manager: u32) -> (u32, [u32; 2]) {
    let session = wire.new_id();
    wire.send(manager, 1, &[Arg::Uint(session)]);
    h.settle(&mut [&mut *wire]);
    let created = wire
        .events
        .iter()
        .find(|e| e.object == session && e.opcode == 0)
        .expect("created event for the inactive session");
    let epoch = [
        u32::from_ne_bytes(created.payload[0..4].try_into().unwrap()),
        u32::from_ne_bytes(created.payload[4..8].try_into().unwrap()),
    ];
    assert_ne!(epoch, [0, 0]);
    assert_eq!(
        &created.payload[8..12],
        &0u32.to_ne_bytes(),
        "no available operations"
    );
    let status = wire
        .events
        .iter()
        .find(|e| e.object == session && e.opcode == 1)
        .unwrap();
    assert_eq!(
        status.payload,
        [0u32.to_ne_bytes(), 0u32.to_ne_bytes()].concat()
    );
    (session, epoch)
}

#[test]
fn control_discovery_is_inactive_and_refuses_another_runs_epoch() {
    let Some(mut h) = Harness::new() else { return };
    let mut wire = h.shell(&[ShellRole::DesktopControl]);
    let manager = control_manager(&mut h, &mut wire);
    let (session, epoch) = control_session(&mut h, &mut wire, manager);
    wire.send(
        session,
        1,
        &[Arg::Uint(11), Arg::Uint(epoch[0]), Arg::Uint(epoch[1])],
    );
    wire.send(
        session,
        1,
        &[Arg::Uint(12), Arg::Uint(epoch[0] ^ 1), Arg::Uint(epoch[1])],
    );
    h.settle(&mut [&mut wire]);
    let replies: Vec<_> = wire.events.iter().filter(|e| e.object == session).collect();
    assert_eq!(replies.len(), 4);
    assert_eq!(replies[2].opcode, 1, "the current run receives status");
    assert_eq!(
        replies[2].payload,
        [11u32.to_ne_bytes(), 0u32.to_ne_bytes()].concat()
    );
    assert_eq!(
        replies[3].opcode, 2,
        "another epoch receives only a refusal"
    );
    assert_eq!(
        replies[3].payload,
        [12u32.to_ne_bytes(), 0u32.to_ne_bytes()].concat()
    );
    assert_eq!(wire.error(), None);
}

#[test]
fn destroying_a_control_session_releases_its_slot_but_not_its_epoch() {
    let Some(mut h) = Harness::new() else { return };
    let mut wire = h.shell(&[ShellRole::DesktopControl]);
    let manager = control_manager(&mut h, &mut wire);
    let (first, old_epoch) = control_session(&mut h, &mut wire, manager);
    wire.send(first, 0, &[]);
    let (second, epoch) = control_session(&mut h, &mut wire, manager);
    assert_ne!(old_epoch, epoch);
    wire.send(
        second,
        1,
        &[
            Arg::Uint(1),
            Arg::Uint(old_epoch[0]),
            Arg::Uint(old_epoch[1]),
        ],
    );
    h.settle(&mut [&mut wire]);
    assert!(wire
        .events
        .iter()
        .any(|e| e.object == second && e.opcode == 2));
    assert_eq!(wire.error(), None);
}

#[test]
fn control_session_limit_applies_across_manager_bindings() {
    let Some(mut h) = Harness::new() else { return };
    let mut wire = h.shell(&[ShellRole::DesktopControl]);
    let first_manager = control_manager(&mut h, &mut wire);
    let second_manager = control_manager(&mut h, &mut wire);
    let _ = control_session(&mut h, &mut wire, first_manager);
    let extra = wire.new_id();
    wire.send(second_manager, 1, &[Arg::Uint(extra)]);
    h.settle(&mut [&mut wire]);
    assert_eq!(wire.error(), Some((second_manager, 1)));
    assert_eq!(wire.events_of(extra), 0);
    assert!(wire.closed);
}

#[test]
fn control_requests_recheck_the_live_holder_on_managers_and_sessions() {
    for query in [false, true] {
        let Some(mut h) = Harness::new() else { return };
        let mut child = Command::new("sleep").arg("30").spawn().unwrap();
        let mut wire = h.held_shell(&[ShellRole::DesktopControl], &child);
        let manager = control_manager(&mut h, &mut wire);
        let (session, epoch) = if query {
            control_session(&mut h, &mut wire, manager)
        } else {
            (0, [0, 0])
        };
        child.kill().unwrap();
        child.wait().unwrap();
        let target = if query {
            wire.send(
                session,
                1,
                &[Arg::Uint(1), Arg::Uint(epoch[0]), Arg::Uint(epoch[1])],
            );
            session
        } else {
            let session = wire.new_id();
            wire.send(manager, 1, &[Arg::Uint(session)]);
            manager
        };
        h.settle(&mut [&mut wire]);
        assert_eq!(wire.error(), Some((target, PERMISSION)));
        assert!(wire.closed);
        if query {
            assert_eq!(
                wire.events_of(session),
                2,
                "no status after the holder exits"
            );
        }
    }
}

#[test]
fn control_query_serial_zero_cannot_be_confused_with_initial_status() {
    let Some(mut h) = Harness::new() else { return };
    let mut wire = h.shell(&[ShellRole::DesktopControl]);
    let manager = control_manager(&mut h, &mut wire);
    let (session, epoch) = control_session(&mut h, &mut wire, manager);
    wire.send(
        session,
        1,
        &[Arg::Uint(0), Arg::Uint(epoch[0]), Arg::Uint(epoch[1])],
    );
    h.settle(&mut [&mut wire]);
    assert_eq!(wire.error(), Some((session, 1)));
    assert!(wire.closed);
}

/// SEC-01: a client of the public socket is not offered the shell global, cannot bind it by the name another client
/// sees, and a request on an object id it guessed is a protocol error that ends its connection.
#[test]
fn a_public_client_cannot_see_bind_or_use_a_role_global() {
    let Some(mut h) = Harness::new() else { return };
    let mut holder = h.shell(&[ShellRole::Session, ShellRole::Dock]);
    let holder_registry = holder.get_registry();
    let mut public = h.public();
    let registry = public.get_registry();
    h.settle(&mut [&mut holder, &mut public]);

    let (shell_name, shell_version) = offered(&holder, holder_registry, "cupertino_shell_v1")
        .expect("a shell process is offered cupertino_shell_v1");
    assert_eq!(shell_version, 22);
    let public_interfaces: Vec<String> = public
        .globals(registry)
        .into_iter()
        .map(|(_, i, _)| i)
        .collect();
    assert!(
        !public_interfaces.contains(&"cupertino_shell_v1".to_string()),
        "the public client is offered the shell global: {public_interfaces:?}"
    );
    assert!(
        public_interfaces.contains(&"cupertino_app_effects_v1".to_string()),
        "the public client is not offered the app effects: {public_interfaces:?}"
    );

    let guessed = public.bind(registry, shell_name, "cupertino_shell_v1", 9);
    h.settle(&mut [&mut public]);
    assert_eq!(
        public.error(),
        Some((DISPLAY, INVALID_OBJECT)),
        "binding a global the client is not offered"
    );
    assert!(public.closed, "the connection survived the forged bind");
    assert_eq!(public.events_of(guessed), 0);

    let mut forger = h.public();
    let _ = forger.get_registry();
    let forged_object = 40;
    forger.send(
        forged_object,
        SHELL_GET_SESSION,
        &[Arg::Uint(forger.next_id)],
    );
    h.settle(&mut [&mut forger]);
    assert!(
        forger.closed,
        "a request on a guessed object id: the connection survived"
    );

    // the launcher's `activate_app` on a session object id it guessed: no answer, and the connection ends
    let mut opener = h.public();
    let _ = opener.get_registry();
    opener.send(
        forged_object,
        SESSION_ACTIVATE_APP,
        &[Arg::Uint(1), Arg::Str("org.example.a.desktop")],
    );
    h.settle(&mut [&mut opener]);
    assert!(
        opener.closed,
        "activate_app from a public client: the connection survived"
    );
    assert_eq!(opener.events_of(forged_object), 0);
}

/// A capture role alone does not make the shell global visible.
#[test]
fn the_capture_role_alone_is_not_offered_the_shell_global() {
    let Some(mut h) = Harness::new() else { return };
    let mut screenshot = h.shell(&[ShellRole::Capture]);
    let registry = screenshot.get_registry();
    h.settle(&mut [&mut screenshot]);
    assert!(offered(&screenshot, registry, "cupertino_shell_v1").is_none());
    assert!(offered(&screenshot, registry, "ext_image_copy_capture_manager_v1").is_some());
}

/// SEC-02: a role belongs to the connection the server made for the process, nothing else: not to the process id the
/// test shares with every client, not to another role, and not to a later connection.
#[test]
fn a_role_does_not_carry_to_another_connection_or_another_role() {
    let Some(mut h) = Harness::new() else { return };

    let mut wm = ShellClient::connect(&mut h, &[ShellRole::WindowManager]).unwrap();
    let wm_object = wm.wire.new_id();
    wm.wire.send(
        wm.shell,
        SHELL_GET_WINDOW_MANAGER,
        &[Arg::Uint(wm_object), Arg::Uint(wm.output)],
    );
    h.settle(&mut [&mut wm.wire]);
    assert_eq!(wm.wire.error(), None, "the window manager role was refused");
    assert!(
        wm.wire.events_of(wm_object) > 0,
        "the window manager got no first group"
    );

    // the same process, a second connection without the role
    let mut other = h.public();
    let registry = other.get_registry();
    h.settle(&mut [&mut other]);
    assert!(offered(&other, registry, "cupertino_shell_v1").is_none());

    // the same connection asking for a role it was not given
    let id = wm.wire.new_id();
    wm.wire.send(wm.shell, SHELL_GET_SESSION, &[Arg::Uint(id)]);
    h.settle(&mut [&mut wm.wire]);
    assert_eq!(
        wm.wire.error(),
        Some((wm.shell, PERMISSION)),
        "the session role came with the window manager role"
    );

    // a connection that holds the dock role, not the window manager role
    let mut dock = ShellClient::connect(&mut h, &[ShellRole::Dock]).unwrap();
    let id = dock.wire.new_id();
    dock.wire.send(
        dock.shell,
        SHELL_GET_WINDOW_MANAGER,
        &[Arg::Uint(id), Arg::Uint(dock.output)],
    );
    h.settle(&mut [&mut dock.wire]);
    assert_eq!(
        dock.wire.error(),
        Some((dock.shell, PERMISSION)),
        "the dock role came with the window manager role"
    );
}

/// Each `get_*` of the shell global needs the role it creates.
#[test]
fn every_role_request_needs_its_own_role() {
    common::events::memory();
    let Some(mut h) = Harness::new() else { return };
    let requests: [(&str, u16, ShellRole); 3] = [
        ("get_menu_bar", SHELL_GET_MENU_BAR, ShellRole::MenuBar),
        ("get_dock", SHELL_GET_DOCK, ShellRole::Dock),
        ("get_session", SHELL_GET_SESSION, ShellRole::Session),
    ];
    for (name, opcode, needed) in requests {
        let held: Vec<ShellRole> = [
            ShellRole::Session,
            ShellRole::MenuBar,
            ShellRole::Dock,
            ShellRole::WindowManager,
        ]
        .into_iter()
        .filter(|r| *r != needed)
        .collect();
        let mut client = ShellClient::connect(&mut h, &held).unwrap();
        let id = client.wire.new_id();
        let args = if opcode == SHELL_GET_SESSION {
            vec![Arg::Uint(id)]
        } else {
            vec![
                Arg::Uint(id),
                Arg::Uint(client.surface),
                Arg::Uint(client.output),
            ]
        };
        client.wire.send(client.shell, opcode, &args);
        h.settle(&mut [&mut client.wire]);
        assert_eq!(
            client.wire.error(),
            Some((client.shell, PERMISSION)),
            "{name} without {needed:?}"
        );
    }
    // each refusal is an event with the interface, the client's program and the role it lacked
    let refused = common::events::recorded("server.authorization.refused");
    let roles: Vec<&str> = refused
        .iter()
        .map(|r| r.fields.get("role_required").unwrap().as_str().unwrap())
        .collect();
    assert_eq!(roles, ["MenuBar", "Dock", "Session"]);
    // the connections are made in the test's own process, which stands for the shell process
    let program = std::env::current_exe().unwrap();
    let program = program.file_name().unwrap().to_str().unwrap();
    for event in &refused {
        assert_eq!(event.severity, cupertino_diagnostic_events::Severity::Warn);
        assert_eq!(
            event.message,
            "a request needs a role the connection does not hold"
        );
        assert_eq!(event.fields.get("interface").unwrap(), "cupertino_shell_v1");
        assert_eq!(event.fields.get("app_id").unwrap(), program);
        assert!(event.redacted.is_empty(), "{event:?}");
    }
}

/// SEC-03: a client names only its own surfaces. A request on a surface or a backdrop of another client is an unknown
/// object to the server and ends the connection; the client's own surfaces work.
#[test]
fn an_app_cannot_claim_effects_on_another_clients_surface() {
    let Some(mut h) = Harness::new() else { return };

    let mut owner = h.public();
    let owner_registry = owner.get_registry();
    h.settle(&mut [&mut owner]);
    let (compositor_name, compositor_version) =
        offered(&owner, owner_registry, "wl_compositor").unwrap();
    let (effects_name, _) = offered(&owner, owner_registry, "cupertino_app_effects_v1").unwrap();
    let compositor = owner.bind(
        owner_registry,
        compositor_name,
        "wl_compositor",
        compositor_version.min(4),
    );
    let effects = owner.bind(owner_registry, effects_name, "cupertino_app_effects_v1", 1);
    let surface = owner.new_id();
    owner.send(compositor, COMPOSITOR_CREATE_SURFACE, &[Arg::Uint(surface)]);
    let backdrop = owner.new_id();
    owner.send(
        effects,
        EFFECTS_GET_BACKDROP,
        &[Arg::Uint(backdrop), Arg::Uint(surface)],
    );
    owner.send(backdrop, BACKDROP_SET_MATERIAL, &[Arg::Str("Menu")]);
    h.settle(&mut [&mut owner]);
    assert_eq!(owner.error(), None, "the owner's own backdrop was refused");

    // the intruder allocates ids the same way, so the owner's object ids are ids it has not created
    let mut intruder = h.public();
    let registry = intruder.get_registry();
    h.settle(&mut [&mut intruder]);
    let (effects_name, _) = offered(&intruder, registry, "cupertino_app_effects_v1").unwrap();
    let intruder_effects = intruder.bind(registry, effects_name, "cupertino_app_effects_v1", 1);
    let claimed = intruder.new_id();
    intruder.send(
        intruder_effects,
        EFFECTS_GET_BACKDROP,
        &[Arg::Uint(claimed), Arg::Uint(surface)],
    );
    h.settle(&mut [&mut intruder]);
    assert!(
        intruder.closed,
        "a backdrop on a surface the client never created: the connection survived"
    );

    let mut intruder = h.public();
    let _ = intruder.get_registry();
    h.settle(&mut [&mut intruder]);
    intruder.send(backdrop, BACKDROP_SET_MATERIAL, &[Arg::Str("Sidebar")]);
    h.settle(&mut [&mut intruder]);
    assert!(
        intruder.closed,
        "a material on another client's backdrop: the connection survived"
    );

    let mut intruder = h.public();
    let registry = intruder.get_registry();
    h.settle(&mut [&mut intruder]);
    let (effects_name, _) = offered(&intruder, registry, "cupertino_app_effects_v1").unwrap();
    let intruder_effects = intruder.bind(registry, effects_name, "cupertino_app_effects_v1", 1);
    intruder.send(
        intruder_effects,
        EFFECTS_SET_TITLEBAR_STYLE,
        &[Arg::Uint(surface), Arg::Uint(1)],
    );
    h.settle(&mut [&mut intruder]);
    assert!(
        intruder.closed,
        "a title bar style on a surface the client never created: the connection survived"
    );

    let mut intruder = h.public();
    let registry = intruder.get_registry();
    h.settle(&mut [&mut intruder]);
    let (effects_name, _) = offered(&intruder, registry, "cupertino_app_effects_v1").unwrap();
    let intruder_effects = intruder.bind(registry, effects_name, "cupertino_app_effects_v1", 4);
    intruder.send(
        intruder_effects,
        EFFECTS_SET_DISABLED_CONTROLS,
        &[Arg::Uint(surface), Arg::Uint(1)],
    );
    h.settle(&mut [&mut intruder]);
    assert!(
        intruder.closed,
        "disabled controls on a surface the client never created: the connection survived"
    );

    owner.send(backdrop, BACKDROP_SET_MATERIAL, &[Arg::Str("Menu")]);
    h.settle(&mut [&mut owner]);
    assert_eq!(owner.error(), None, "the owner was disturbed");
    assert!(!owner.closed);
}

/// A process that stays until it is killed.
fn sleeper() -> Child {
    Command::new("sleep")
        .arg("60")
        .stdout(Stdio::null())
        .spawn()
        .unwrap()
}

/// Ends `process` and reaps it, as the supervisor does when it sees the exit.
fn end(mut process: Child) {
    process.kill().unwrap();
    process.wait().unwrap();
}

/// One holder per role, and a restarted process gets its role as soon as the old process is gone, even when the server
/// has not yet seen the old connection close (the old connection is kept open here, as a leftover child would).
#[test]
fn a_restarted_process_gets_the_role_once_the_old_process_is_gone() {
    let Some(mut h) = Harness::new() else { return };
    let roles = [ShellRole::Session, ShellRole::WindowManager];

    let old_process = sleeper();
    let wire = h.held_shell(&roles, &old_process);
    let mut old = ShellClient::over(&mut h, wire).unwrap();
    let old_object = old.wire.new_id();
    old.wire.send(
        old.shell,
        SHELL_GET_WINDOW_MANAGER,
        &[Arg::Uint(old_object), Arg::Uint(old.output)],
    );
    h.settle(&mut [&mut old.wire]);
    assert_eq!(old.wire.error(), None, "the first holder was refused");
    assert!(old.wire.events_of(old_object) > 0);

    let early_process = sleeper();
    let wire = h.held_shell(&roles, &early_process);
    let mut early = ShellClient::over(&mut h, wire).unwrap();
    let id = early.wire.new_id();
    early.wire.send(
        early.shell,
        SHELL_GET_WINDOW_MANAGER,
        &[Arg::Uint(id), Arg::Uint(early.output)],
    );
    h.settle(&mut [&mut early.wire]);
    assert_eq!(
        early.wire.error(),
        Some((early.shell, ROLE)),
        "a second holder of the role while the first process is alive"
    );
    end(early_process);

    end(old_process);
    let new_process = sleeper();
    let wire = h.held_shell(&roles, &new_process);
    let mut new = ShellClient::over(&mut h, wire).unwrap();
    let new_object = new.wire.new_id();
    new.wire.send(
        new.shell,
        SHELL_GET_WINDOW_MANAGER,
        &[Arg::Uint(new_object), Arg::Uint(new.output)],
    );
    h.settle(&mut [&mut new.wire, &mut old.wire]);
    assert_eq!(
        new.wire.error(),
        None,
        "the restarted process was refused the role"
    );
    assert!(
        new.wire.events_of(new_object) > 0,
        "the restarted process got no first group"
    );
    end(new_process);
}

/// SEC-02: the roles of a connection end with the process it was made for. A connection that outlives its process
/// (a descriptor passed on, a leftover child) is refused every request, also after the role went to a new process.
#[test]
fn a_connection_replayed_after_its_process_exited_holds_nothing() {
    let Some(mut h) = Harness::new() else { return };
    let roles = [ShellRole::Session, ShellRole::WindowManager];

    let process = sleeper();
    let wire = h.held_shell(&roles, &process);
    let mut replayed = ShellClient::over(&mut h, wire).unwrap();
    end(process);

    let successor = sleeper();
    let wire = h.held_shell(&roles, &successor);
    let mut current = ShellClient::over(&mut h, wire).unwrap();
    let id = current.wire.new_id();
    current.wire.send(
        current.shell,
        SHELL_GET_WINDOW_MANAGER,
        &[Arg::Uint(id), Arg::Uint(current.output)],
    );
    h.settle(&mut [&mut current.wire, &mut replayed.wire]);
    assert_eq!(current.wire.error(), None);

    let id = replayed.wire.new_id();
    replayed
        .wire
        .send(replayed.shell, SHELL_GET_SESSION, &[Arg::Uint(id)]);
    h.settle(&mut [&mut replayed.wire, &mut current.wire]);
    assert_eq!(
        replayed.wire.error(),
        Some((replayed.shell, PERMISSION)),
        "a connection whose process is gone still held the session role"
    );

    let mut fresh = h.public();
    let registry = fresh.get_registry();
    h.settle(&mut [&mut fresh]);
    assert!(offered(&fresh, registry, "cupertino_shell_v1").is_none());
    end(successor);
}

/// Only the lock screen is offered `ext_session_lock_manager_v1`: not a public client, and not a shell process that
/// holds other roles.
#[test]
fn the_session_lock_is_offered_to_the_lock_screen_alone() {
    let Some(mut h) = Harness::new() else { return };
    let mut public = h.public();
    let public_registry = public.get_registry();
    let mut observer = h.shell(&[
        ShellRole::Session,
        ShellRole::Dock,
        ShellRole::WindowManager,
    ]);
    let observer_registry = observer.get_registry();
    let mut lock_screen = h.shell(&[ShellRole::Session, ShellRole::SessionLock]);
    let lock_registry = lock_screen.get_registry();
    h.settle(&mut [&mut public, &mut observer, &mut lock_screen]);

    let lock = "ext_session_lock_manager_v1";
    assert!(offered(&public, public_registry, lock).is_none());
    assert!(offered(&observer, observer_registry, lock).is_none());
    let (name, version) = offered(&lock_screen, lock_registry, lock).expect("the lock screen");

    let guessed = public.bind(public_registry, name, lock, version);
    h.settle(&mut [&mut public]);
    assert_eq!(public.error(), Some((DISPLAY, INVALID_OBJECT)));
    assert!(public.closed);
    assert_eq!(public.events_of(guessed), 0);
}

/// A connection whose process was never bound holds nothing, whatever roles it was made with.
#[test]
fn a_connection_without_a_bound_holder_holds_nothing() {
    let Some(mut h) = Harness::new() else { return };
    let (stream, _state) = connect_bound_client(
        &h.state.display_handle,
        shell_process_state(&[ShellRole::Session, ShellRole::Dock, ShellRole::Capture]),
    )
    .unwrap();
    let mut wire = Wire::new(stream);
    let registry = wire.get_registry();
    h.settle(&mut [&mut wire]);
    assert!(offered(&wire, registry, "cupertino_shell_v1").is_none());
    assert!(offered(&wire, registry, "ext_image_copy_capture_manager_v1").is_none());
}

/// An observer whose process exited is told nothing more, though its connection is still open.
#[test]
fn a_session_observer_of_an_exited_process_hears_no_more() {
    let Some(mut h) = Harness::new() else { return };
    let process = sleeper();
    let wire = h.held_shell(&[ShellRole::Session], &process);
    let mut observer = ShellClient::over(&mut h, wire).unwrap();
    let id = observer.wire.new_id();
    observer
        .wire
        .send(observer.shell, SHELL_GET_SESSION, &[Arg::Uint(id)]);
    h.settle(&mut [&mut observer.wire]);
    assert_eq!(observer.wire.error(), None);
    let before = observer.wire.events_of(id);
    assert!(before > 0, "the observer is told the state at once");

    h.state.sessions.lock(true);
    h.settle(&mut [&mut observer.wire]);
    assert!(
        observer.wire.events_of(id) > before,
        "a live observer hears the lock"
    );

    let heard = observer.wire.events_of(id);
    end(process);
    h.state.sessions.lock(false);
    h.settle(&mut [&mut observer.wire]);
    assert_eq!(observer.wire.events_of(id), heard);
}
