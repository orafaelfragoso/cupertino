//! `ListShellComponents` of `org.cupertino.Session1`: every component of the server's table, where it runs (the shared
//! host, a separate host, from the development folder, nowhere, or turned off) and whether a host
//! holds its connection, for the session's own user alone.

use std::os::fd::OwnedFd;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cupertino_assets::Assets;
use cupertino_peer::{Caller, ProcessHandle};
use cupertino_protocols::client::host::cupertino_host_component_v1::{
    self, CupertinoHostComponentV1,
};
use cupertino_protocols::client::host::cupertino_host_v1::{self, CupertinoHostV1};
use cupertino_server::authorization::{Holder, DETACHED_LIFETIME};
use cupertino_server::config::DevFolder;
use cupertino_server::host::HostKind;
use cupertino_server::quit::{Call, Reject, ShellComponentRecord};
use cupertino_server::supervisor::HostProgram;
use cupertino_server::{Config, Cupertino};
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use wayland_client::protocol::wl_registry;
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle};

/// The shell host's side: its global and the component connections it was handed.
#[derive(Default)]
struct HostClient {
    host: Option<CupertinoHostV1>,
    connections: Vec<OwnedFd>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for HostClient {
    fn event(
        state: &mut Self,
        reg: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name, interface, ..
        } = event
        {
            if interface == "cupertino_host_v1" {
                state.host = Some(reg.bind::<CupertinoHostV1, _, _>(name, 1, qh, ()));
            }
        }
    }
}

impl Dispatch<CupertinoHostV1, ()> for HostClient {
    fn event(
        _: &mut Self,
        _: &CupertinoHostV1,
        _: cupertino_host_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<CupertinoHostComponentV1, ()> for HostClient {
    fn event(
        state: &mut Self,
        _: &CupertinoHostComponentV1,
        event: cupertino_host_component_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let cupertino_host_component_v1::Event::Connection { fd } = event {
            state.connections.push(fd);
        }
    }
}

/// A server with no output and its loop.
struct Rig {
    event_loop: EventLoop<'static, Cupertino>,
    state: Cupertino,
}

impl Rig {
    fn new(config: Config) -> Self {
        let event_loop: EventLoop<'static, Cupertino> = EventLoop::try_new().unwrap();
        let display: Display<Cupertino> = Display::new().unwrap();
        let state = Cupertino::new(
            display,
            event_loop.handle(),
            event_loop.get_signal(),
            config,
            false,
            &Assets::builtin(),
        )
        .unwrap();
        Self { event_loop, state }
    }

    /// A few turns of the server's loop, each with the exchange of the host's connection.
    fn settle(
        &mut self,
        conn: &Connection,
        queue: &mut EventQueue<HostClient>,
        host: &mut HostClient,
    ) {
        for _ in 0..8 {
            let _ = conn.flush();
            let _ = self
                .event_loop
                .dispatch(Some(Duration::from_millis(2)), &mut self.state);
            self.state.tick();
            let _ = self.state.display_handle.flush_clients();
            if let Some(guard) = conn.prepare_read() {
                let _ = guard.read();
            }
            let _ = queue.dispatch_pending(host);
        }
    }

    /// A shared host the server was handed, listing `menubar`, whose connection it asks for and holds.
    fn connect_menu_bar(&mut self) -> (Connection, OwnedFd) {
        let fd = self
            .state
            .attach_host_connection(
                HostKind::Shared,
                vec!["menubar"],
                Holder::Detached(Instant::now() + DETACHED_LIFETIME),
            )
            .unwrap();
        let conn = Connection::from_socket(UnixStream::from(fd)).unwrap();
        let mut queue = conn.new_event_queue();
        let mut host = HostClient::default();
        let _registry = conn.display().get_registry(&queue.handle(), ());
        self.settle(&conn, &mut queue, &mut host);
        host.host
            .clone()
            .expect("the host global")
            .get_component("menubar".into(), &queue.handle(), ())
            .connect();
        self.settle(&conn, &mut queue, &mut host);
        let component = host.connections.pop().expect("the component's connection");
        (conn, component)
    }

    /// The answer to `ListShellComponents` from `caller` of user `user`.
    fn list(&mut self, caller: Caller, user: u32) -> Result<Vec<ShellComponentRecord>, Reject> {
        let (tx, rx) = async_channel::bounded(1);
        self.state
            .on_quit_call(Call::ListShellComponents(caller, Some(user), tx));
        rx.try_recv().expect("an answer at once")
    }
}

/// This test's process as a caller of the bus, and its user.
fn this_process() -> (Caller, u32) {
    let caller = Caller::of_process(
        ProcessHandle::open(std::process::id()).unwrap(),
        Path::new("/proc"),
    );
    (caller, std::fs::metadata("/proc/self").unwrap().uid())
}

/// An empty directory of the test's own that only its user can write.
fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    dir
}

/// An executable file at `path` that only its user can write.
fn program(path: &Path) {
    std::fs::write(path, b"#!/bin/false\n").unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// `(name, runs_in, from_dev_folder, connected)` of each component, in the table's order.
fn rows(records: &[ShellComponentRecord]) -> Vec<(&str, &str, bool, bool)> {
    records
        .iter()
        .map(|(name, runs_in, dev, connected)| (name.as_str(), runs_in.as_str(), *dev, *connected))
        .collect()
}

#[test]
fn every_component_of_the_table_is_listed_with_where_it_runs_and_whether_it_is_connected() {
    let root = scratch("shell-components");
    let installed = root.join("installed");
    let folder = root.join("dev");
    for dir in [&installed, &folder] {
        std::fs::create_dir(dir).unwrap();
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let host_program = installed.join("cupertino-shell");
    program(&host_program);
    program(&folder.join("cupertino-shell"));

    // with the host: the shared host, a separate host from the development folder and one of `alone`, one turned off
    let config = Config {
        shell_off: vec!["osd".into()],
        shell_alone: vec!["launcher".into()],
        dev_folder: Some(DevFolder {
            path: folder.clone(),
            components: vec!["dock".into()],
        }),
        ..Config::default()
    };
    let mut rig = Rig::new(config);
    let (_host, _menu_bar) = rig.connect_menu_bar();
    rig.state.supervisor.set_host_program(Some(HostProgram {
        path: host_program.clone(),
        args: Vec::new(),
        checked: false,
    }));
    rig.state.apply_shell_plan();
    let (caller, user) = this_process();
    let listed = rig.list(caller, user).expect("the session's user may list");
    assert_eq!(
        rows(&listed),
        [
            ("menubar", "host", false, true),
            ("dock", "host-dock", true, false),
            ("desktop", "host", false, false),
            ("windowmanager", "host", false, false),
            ("overview", "host", false, false),
            ("launcher", "host-launcher", false, false),
            ("notifications", "host", false, false),
            ("controlcenter", "host", false, false),
            ("osd", "off", false, false),
            ("agent", "host", false, false),
            ("dialogs", "host", false, false),
        ]
    );

    // without the host: nothing runs
    let config = Config {
        shell_off: vec!["osd".into()],
        ..Config::default()
    };
    let mut rig = Rig::new(config);
    rig.state.apply_shell_plan();
    let (caller, user) = this_process();
    let listed = rig.list(caller, user).expect("the session's user may list");
    let places: Vec<(&str, &str)> = rows(&listed).iter().map(|r| (r.0, r.1)).collect();
    assert_eq!(places[8], ("osd", "off"));
    assert!(
        places
            .iter()
            .filter(|(name, _)| *name != "osd")
            .all(|(_, runs_in)| *runs_in == "none"),
        "{places:?}"
    );

    // another user is refused
    let (caller, user) = this_process();
    assert!(rig.list(caller, user + 1).is_err());
}
