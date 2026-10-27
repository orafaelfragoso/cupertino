//! The Wayland globals the server advertises, against docs/architecture/contracts.yaml: an ordinary client and a
//! connection made for each shell role see exactly the registry entries of their scope. A global added in code without
//! an entry (or an entry for a global that is gone) fails here. Tests skip (pass) when no EGL device is available.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

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

const ROLES: [ShellRole; 6] = [
    ShellRole::Session,
    ShellRole::MenuBar,
    ShellRole::Dock,
    ShellRole::WindowManager,
    ShellRole::Capture,
    ShellRole::SessionLock,
];

type Globals = BTreeMap<String, u32>;

/// The `wayland-global` entries of the registry that every connection holding `roles` is offered.
fn registered(registry: &str, roles: &[ShellRole]) -> Globals {
    let held: Vec<String> = roles.iter().map(|role| format!("{role:?}")).collect();
    let mut globals = Globals::new();
    let mut entry: BTreeMap<&str, &str> = BTreeMap::new();
    let mut flush = |entry: &mut BTreeMap<&str, &str>| {
        if entry.get("kind") == Some(&"wayland-global")
            && entry.get("advertised") == Some(&"always")
        {
            let offered = match entry["scope"] {
                "public" => true,
                "shell-private" => entry["role"]
                    .split(' ')
                    .any(|role| held.iter().any(|h| h == role)),
                scope => panic!("{}: scope {scope} for a global", entry["name"]),
            };
            if offered {
                let version = entry["version"].parse().expect("a version number");
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

fn advertised(
    server: &mut Cupertino,
    event_loop: &mut EventLoop<'static, Cupertino>,
    roles: &[ShellRole],
) -> Globals {
    let stream = if roles.is_empty() {
        cupertino_server::shell_connection::connect_client(
            &server.display_handle,
            ClientState::default(),
        )
    } else {
        cupertino_server::shell_connection::shell_process_connection(&server.display_handle, roles)
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
    let deadline = Instant::now() + Duration::from_secs(10);
    let values = loop {
        headless::step(server, event_loop, Duration::from_millis(5)).unwrap();
        match rx.try_recv() {
            Ok(values) => break values,
            Err(mpsc::TryRecvError::Disconnected) => panic!("registry client failed"),
            Err(mpsc::TryRecvError::Empty) => assert!(Instant::now() < deadline),
        }
    };
    client.join().unwrap();
    values
}

#[test]
fn every_connection_sees_the_registry_entries_of_its_scope() {
    let mut event_loop = EventLoop::<Cupertino>::try_new().unwrap();
    let display = Display::<Cupertino>::new().unwrap();
    let mut server = Cupertino::new(
        display,
        event_loop.handle(),
        event_loop.get_signal(),
        Config::default(),
        false,
        &cupertino_assets::Assets::builtin(),
    )
    .unwrap();
    if let Err(e) = headless::init(&mut server, (1000, 700), 1.0) {
        eprintln!("skipping: no EGL device ({e:#})");
        return;
    }

    let registry = registry();
    let mut cases: Vec<Vec<ShellRole>> = vec![vec![]];
    cases.extend(ROLES.iter().map(|role| vec![*role]));
    cases.push(ROLES.to_vec());
    for roles in cases {
        let observed = advertised(&mut server, &mut event_loop, &roles);
        let problems = differences(&registered(&registry, &roles), &observed);
        assert!(problems.is_empty(), "roles {roles:?}: {problems:#?}");
    }
}

#[test]
fn the_shell_global_is_registered_for_the_roles_of_a_shell_process() {
    let ordinary = registered(&registry(), &[]);
    let session = registered(&registry(), &[ShellRole::Session]);
    assert!(!ordinary.contains_key("cupertino_shell_v1"));
    assert!(session.contains_key("cupertino_shell_v1"));
    assert!(!session.contains_key("ext_session_lock_manager_v1"));
    assert!(ordinary.contains_key("cupertino_app_effects_v1"));
}

#[test]
fn a_global_without_an_entry_is_reported() {
    let registry = registry();
    let without: String = registry
        .split("\n- ")
        .filter(|entry| !entry.starts_with("name: wl_seat\n"))
        .collect::<Vec<_>>()
        .join("\n- ");
    let observed = registered(&registry, &[]);
    assert_eq!(
        differences(&registered(&without, &[]), &observed),
        ["wl_seat v9 is advertised and has no registry entry"]
    );
    assert_eq!(
        differences(&observed, &registered(&without, &[])),
        ["wl_seat has a registry entry and is not advertised"]
    );
}
