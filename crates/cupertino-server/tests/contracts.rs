//! The Wayland globals the server advertises, against docs/architecture/contracts.yaml: an ordinary client and a
//! connection made for each shell role see exactly the registry entries of their scope. A global added in code without
//! an entry (or an entry for a global that is gone) fails here. Without an EGL device the tests
//! skip with a message, unless CUPERTINO_EGL_REQUIRED=1 (CI), which makes a missing device a failure. With it set, a
//! host whose /dev/dri has a renderD node while EGL runs on a device without one also fails, by design.

use crate::common;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use cupertino_server::shell_connection::{connect_client, shell_process_connection};
use cupertino_server::{
    authorization::ShellRole, backend::headless, config::Config, state::ClientState, Cupertino,
};
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::wl_registry;
use wayland_client::{Connection, Dispatch, QueueHandle};

struct RegistryClient;

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for RegistryClient {
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

fn registry() -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap();
    std::fs::read_to_string(root.join("docs/architecture/contracts.yaml"))
        .expect("the contracts registry")
}

const ROLES: [ShellRole; 9] = [
    ShellRole::Session,
    ShellRole::MenuBar,
    ShellRole::Dock,
    ShellRole::WindowManager,
    ShellRole::Capture,
    ShellRole::SessionLock,
    ShellRole::DataControl,
    ShellRole::Prompt,
    ShellRole::DesktopControl,
];

const DMABUF: &str = "zwp_linux_dmabuf_v1";

const SIZE: (i32, i32) = (1000, 700);

type Globals = BTreeMap<String, u32>;

/// Who is looking at the registry.
#[derive(Clone, Copy, Debug)]
enum Peer<'a> {
    Ordinary,
    /// A connection made through a security context.
    Sandboxed,
    Shell(&'a [ShellRole]),
    /// The compositor's own connection (the accent picker's input method).
    Internal,
}

impl Peer<'_> {
    fn sees(self, role: &str) -> bool {
        match self {
            Peer::Ordinary | Peer::Sandboxed => false,
            Peer::Shell(roles) => role
                .split(' ')
                .any(|r| roles.iter().any(|held| format!("{held:?}") == r)),
            Peer::Internal => role == "internal client",
        }
    }
}

/// The `wayland-global` entries of the registry that `peer` is offered; `accent_table` is whether the server was
/// started with an accent table, `render_node` whether the EGL display has a render node.
fn registered(registry: &str, peer: Peer, accent_table: bool, render_node: bool) -> Globals {
    let mut globals = Globals::new();
    let mut entry: BTreeMap<&str, &str> = BTreeMap::new();
    let mut flush = |entry: &mut BTreeMap<&str, &str>| {
        if entry.get("kind") == Some(&"wayland-global") {
            let present = match entry["advertised"] {
                "always" => true,
                "accent-table" => accent_table,
                other => panic!("{}: advertised {other}", entry["name"]),
            };
            let offered = match entry["scope"] {
                "public" => true,
                "shell-private" => peer.sees(entry["role"]),
                scope => panic!("{}: scope {scope} for a global", entry["name"]),
            };
            let offered = offered
                && !(matches!(peer, Peer::Sandboxed)
                    && entry.get("visible_to") == Some(&"unsandboxed"));
            if present && offered {
                let version = match entry.get("version_without_render_node") {
                    Some(version) if !render_node => version,
                    _ => entry["version"],
                };
                let version = version.parse().expect("a version number");
                globals.insert(entry["name"].to_string(), version);
            }
        }
        entry.clear();
    };
    for line in registry
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
    {
        if let Some(rest) = line.strip_prefix("- ") {
            flush(&mut entry);
            let (key, value) = rest.split_once(": ").expect("key: value");
            entry.insert(key, value);
        } else {
            let (key, value) = line.trim_start().split_once(": ").expect("key: value");
            entry.insert(key, value);
        }
    }
    flush(&mut entry);
    globals
}

fn differences(registered: &Globals, observed: &Globals) -> Vec<String> {
    let mut problems = Vec::new();
    for (name, version) in observed {
        match registered.get(name) {
            None => problems.push(format!(
                "{name} v{version} is advertised and has no registry entry"
            )),
            Some(v) if v != version => problems.push(format!(
                "{name} is advertised at v{version}, the registry says v{v}"
            )),
            Some(_) => {}
        }
    }
    for name in registered
        .keys()
        .filter(|name| !observed.contains_key(*name))
    {
        problems.push(format!("{name} has a registry entry and is not advertised"));
    }
    problems
}

struct Server {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
}

fn egl_required() -> bool {
    std::env::var("CUPERTINO_EGL_REQUIRED").as_deref() == Ok("1")
}

fn render_node_device_exists() -> bool {
    std::fs::read_dir("/dev/dri").is_ok_and(|entries| {
        entries
            .flatten()
            .any(|entry| entry.file_name().to_string_lossy().starts_with("renderD"))
    })
}

impl Server {
    /// A headless server, or None (with a message) when there is no EGL device; CUPERTINO_EGL_REQUIRED=1 makes that
    /// a failure.
    fn start(config: Config) -> Option<Server> {
        let event_loop = EventLoop::<Cupertino>::try_new().unwrap();
        let display = Display::<Cupertino>::new().unwrap();
        let mut state = Cupertino::new(
            display,
            event_loop.handle(),
            event_loop.get_signal(),
            config,
            false,
            &cupertino_assets::Assets::builtin(),
        )
        .unwrap();
        if let Err(e) = headless::init(&mut state, SIZE, 1.0) {
            assert!(
                !egl_required(),
                "no EGL device and CUPERTINO_EGL_REQUIRED=1: {e:#}"
            );
            eprintln!(
                "SKIPPED: no EGL device ({e:#}); set CUPERTINO_EGL_REQUIRED=1 to fail instead"
            );
            return None;
        }
        Some(Server { state, event_loop })
    }

    fn has_render_node(&mut self) -> bool {
        let renderer = self.state.backend.as_mut().unwrap().renderer();
        cupertino_server::backend::render_node(renderer).is_some()
    }

    fn advertised(&mut self, peer: Peer) -> Globals {
        let display = &self.state.display_handle;
        let stream = match peer {
            Peer::Ordinary => connect_client(display, ClientState::default()),
            Peer::Sandboxed => connect_client(
                display,
                ClientState {
                    sandboxed: true,
                    ..ClientState::default()
                },
            ),
            Peer::Internal => connect_client(
                display,
                ClientState {
                    internal: true,
                    ..ClientState::default()
                },
            ),
            Peer::Shell(roles) => shell_process_connection(display, roles),
        }
        .unwrap();
        let (tx, rx) = mpsc::channel();
        let client = thread::spawn(move || {
            let connection = Connection::from_socket(stream).unwrap();
            let (globals, _) = registry_queue_init::<RegistryClient>(&connection).unwrap();
            let values: Globals = globals
                .contents()
                .clone_list()
                .into_iter()
                .map(|global| (global.interface, global.version))
                .collect();
            tx.send(values).unwrap();
        });
        let deadline = Instant::now() + common::HANG_GUARD;
        let values = loop {
            headless::step(
                &mut self.state,
                &mut self.event_loop,
                Duration::from_millis(5),
            )
            .unwrap();
            match rx.try_recv() {
                Ok(values) => break values,
                Err(mpsc::TryRecvError::Disconnected) => panic!("registry client failed"),
                Err(mpsc::TryRecvError::Empty) => assert!(Instant::now() < deadline),
            }
        };
        client.join().unwrap();
        values
    }
}

#[test]
fn every_connection_sees_the_registry_entries_of_its_scope() {
    let registry = registry();
    let table = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/accents.json");
    for table in [None, Some(table)] {
        let accent_table = table.is_some();
        let mut config = Config::default();
        config.keyboard.accent.table = table;
        let Some(mut server) = Server::start(config) else {
            return;
        };
        // The dmabuf global has default feedback (a higher version) only when the EGL display has a render node: the
        // registry holds both versions, and the renderer decides which one this machine must advertise.
        let render_node = server.has_render_node();
        eprintln!("render node: {render_node}");
        assert!(
            render_node || !egl_required() || !render_node_device_exists(),
            "CUPERTINO_EGL_REQUIRED=1 and /dev/dri has a renderD node, but the EGL display reports no render node"
        );
        let mut peers = vec![Peer::Ordinary, Peer::Sandboxed, Peer::Internal];
        peers.extend(
            ROLES
                .iter()
                .map(|role| Peer::Shell(std::slice::from_ref(role))),
        );
        peers.push(Peer::Shell(&ROLES));
        for peer in peers {
            let problems = differences(
                &registered(&registry, peer, accent_table, render_node),
                &server.advertised(peer),
            );
            assert!(
                problems.is_empty(),
                "{peer:?}, accent table {accent_table}, render node {render_node}: {problems:#?}"
            );
        }
    }
}

#[test]
fn the_shell_global_is_registered_for_the_roles_of_a_shell_process() {
    let registry = registry();
    let ordinary = registered(&registry, Peer::Ordinary, false, true);
    let session = registered(&registry, Peer::Shell(&[ShellRole::Session]), false, true);
    assert!(!ordinary.contains_key("cupertino_shell_v1"));
    assert!(session.contains_key("cupertino_shell_v1"));
    assert!(!session.contains_key("ext_session_lock_manager_v1"));
    assert!(ordinary.contains_key("cupertino_app_effects_v1"));
}

#[test]
fn a_sandboxed_connection_is_not_offered_the_globals_that_reach_other_clients() {
    let registry = registry();
    let ordinary = registered(&registry, Peer::Ordinary, false, true);
    let sandboxed = registered(&registry, Peer::Sandboxed, false, true);
    for name in [
        "zwlr_output_manager_v1",
        "zwlr_output_power_manager_v1",
        "wp_security_context_manager_v1",
        "zwlr_layer_shell_v1",
        "ext_foreign_toplevel_list_v1",
    ] {
        assert!(ordinary.contains_key(name), "{name}");
        assert!(!sandboxed.contains_key(name), "{name}");
    }
    assert!(sandboxed.contains_key("wl_compositor"));
}

#[test]
fn the_dmabuf_version_depends_on_the_render_node() {
    let registry = registry();
    let version = |render_node| registered(&registry, Peer::Ordinary, false, render_node)[DMABUF];
    assert_eq!((version(true), version(false)), (5, 3));
}

#[test]
fn a_global_without_an_entry_is_reported() {
    let registry = registry();
    let without: String = registry
        .split("\n- ")
        .filter(|entry| !entry.starts_with("name: wl_seat\n"))
        .collect::<Vec<_>>()
        .join("\n- ");
    let observed = registered(&registry, Peer::Ordinary, false, true);
    assert_eq!(
        differences(
            &registered(&without, Peer::Ordinary, false, true),
            &observed
        ),
        ["wl_seat v9 is advertised and has no registry entry"]
    );
    assert_eq!(
        differences(
            &observed,
            &registered(&without, Peer::Ordinary, false, true)
        ),
        ["wl_seat has a registry entry and is not advertised"]
    );
}
