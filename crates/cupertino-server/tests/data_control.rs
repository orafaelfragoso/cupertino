//! Who sees and binds the data control globals: a connection holding the data control role, and a client of a program
//! allowed in `data-control.conf`; every other client neither sees nor binds them. The file is read at start and on
//! each change, a file that is not the person's alone allows nothing, and a client that is no longer allowed after a
//! change is disconnected. Tests skip (pass) when no EGL device is available.

use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cupertino_defaults::data_control::{DataControlConf, Entry};
use cupertino_server::authorization::ShellRole;
use cupertino_server::backend::headless;
use cupertino_server::config::{Config, SettingsFiles};
use cupertino_server::live::SettingsPaths;
use cupertino_server::Cupertino;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{event_created_child, Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols_wlr::data_control::v1::client::{
    zwlr_data_control_device_v1::{self, ZwlrDataControlDeviceV1},
    zwlr_data_control_manager_v1::ZwlrDataControlManagerV1,
    zwlr_data_control_offer_v1::ZwlrDataControlOfferV1,
};

use crate::common;

const DATA_CONTROL: [&str; 2] = [
    "zwlr_data_control_manager_v1",
    "ext_data_control_manager_v1",
];

struct Harness {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
    dir: tempfile::TempDir,
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
        match headless::init(&mut state, (800, 600), 1.0) {
            Ok(()) => Some(Harness {
                state,
                event_loop,
                dir: tempfile::tempdir().unwrap(),
            }),
            Err(e) => {
                common::egl_unavailable(format!("{e:#}"));
                None
            }
        }
    }

    fn conf(&self) -> PathBuf {
        self.dir.path().join("data-control.conf")
    }

    /// Read `data-control.conf` of the test's directory now and on every change.
    fn watch(&mut self) {
        let paths = SettingsPaths {
            data_control: Some(self.conf()),
            ..SettingsPaths::default()
        };
        self.state
            .start_settings_watch(SettingsFiles::default(), paths);
    }

    fn step(&mut self) {
        headless::step(
            &mut self.state,
            &mut self.event_loop,
            Duration::from_millis(5),
        )
        .unwrap();
    }

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

    /// Run `change` on the file and step the server until it applied it.
    fn change(&mut self, change: impl FnOnce(&Path)) {
        let before = self.state.live.reloads;
        change(&self.conf());
        self.wait_until("the change of data-control.conf", |s| {
            s.live.reloads > before
        });
    }

    fn connect(&self) -> UnixStream {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
        UnixStream::connect(PathBuf::from(dir).join(self.state.socket_name.as_ref().unwrap()))
            .unwrap()
    }

    fn finish<T>(&mut self, thread: JoinHandle<T>) -> T {
        let end = Instant::now() + common::HANG_GUARD;
        while !thread.is_finished() {
            assert!(Instant::now() < end, "timed out waiting for the client");
            self.step();
        }
        thread.join().unwrap()
    }

    /// The data control globals a new client of the public socket sees.
    fn seen_by_a_client(&mut self) -> Vec<String> {
        let stream = self.connect();
        self.seen_on(stream)
    }

    fn seen_on(&mut self, stream: UnixStream) -> Vec<String> {
        let names = self.finish(std::thread::spawn(move || names_of(stream)));
        names
            .into_iter()
            .filter(|n| DATA_CONTROL.contains(&n.as_str()))
            .collect()
    }
}

/// This test program as `data-control.conf` allows it.
fn this_program() -> Entry {
    Entry::for_program(&std::env::current_exe().unwrap()).unwrap()
}

fn store(path: &Path, allowed: Vec<Entry>) {
    DataControlConf { allowed }.store(path).unwrap();
}

struct Probe;

macro_rules! ignore_events {
    ($($t:ty),*) => {$(
        impl Dispatch<$t, ()> for Probe {
            fn event(_: &mut Self, _: &$t, _: <$t as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    )*};
}
ignore_events!(
    wl_seat::WlSeat,
    wl_registry::WlRegistry,
    ZwlrDataControlManagerV1,
    ZwlrDataControlOfferV1
);

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Probe {
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

impl Dispatch<ZwlrDataControlDeviceV1, ()> for Probe {
    fn event(
        _: &mut Self,
        _: &ZwlrDataControlDeviceV1,
        _: zwlr_data_control_device_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }

    event_created_child!(Probe, ZwlrDataControlDeviceV1, [
        zwlr_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (ZwlrDataControlOfferV1, ()),
    ]);
}

fn names_of(stream: UnixStream) -> Vec<String> {
    let conn = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<Probe>(&conn).unwrap();
    queue.roundtrip(&mut Probe).unwrap();
    globals
        .contents()
        .with_list(|list| list.iter().map(|g| g.interface.clone()).collect())
}

/// A clipboard manager: it binds the manager and makes a device, then stays until `stop`. Gives whether the server
/// disconnected it.
fn clipboard_manager(stream: UnixStream, stop: Arc<AtomicBool>) -> JoinHandle<bool> {
    std::thread::spawn(move || {
        let conn = Connection::from_socket(stream).unwrap();
        let (globals, mut queue) = registry_queue_init::<Probe>(&conn).unwrap();
        let qh = queue.handle();
        let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=1, ()).unwrap();
        let manager: ZwlrDataControlManagerV1 = globals.bind(&qh, 1..=2, ()).unwrap();
        let _device = manager.get_data_device(&seat, &qh, ());
        while !stop.load(Ordering::SeqCst) {
            if queue.roundtrip(&mut Probe).is_err() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        false
    })
}

/// A client that never asks for the globals: it only syncs until `stop`. Gives whether the server disconnected it.
fn bystander(stream: UnixStream, stop: Arc<AtomicBool>) -> JoinHandle<bool> {
    std::thread::spawn(move || {
        let conn = Connection::from_socket(stream).unwrap();
        let mut queue = conn.new_event_queue::<Probe>();
        while !stop.load(Ordering::SeqCst) {
            if queue.roundtrip(&mut Probe).is_err() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        false
    })
}

#[test]
fn a_client_of_a_program_nobody_allowed_sees_no_data_control_and_one_of_an_allowed_program_does() {
    let Some(mut h) = Harness::new() else { return };
    h.watch();
    assert_eq!(h.seen_by_a_client(), Vec::<String>::new());

    h.change(|path| store(path, vec![this_program()]));
    assert_eq!(h.seen_by_a_client().len(), 2);
}

#[test]
fn a_connection_with_the_data_control_role_sees_data_control_without_any_file() {
    let Some(mut h) = Harness::new() else { return };
    let stream = cupertino_server::shell_connection::shell_process_connection(
        &h.state.display_handle,
        &[ShellRole::DataControl],
    )
    .unwrap();
    assert_eq!(h.seen_on(stream).len(), 2);
    let stream = cupertino_server::shell_connection::shell_process_connection(
        &h.state.display_handle,
        &[ShellRole::Session],
    )
    .unwrap();
    assert_eq!(h.seen_on(stream), Vec::<String>::new());
}

#[test]
fn a_refused_client_that_binds_the_global_by_its_name_is_disconnected() {
    let Some(mut h) = Harness::new() else { return };
    common::allow_data_control(&mut h.state);
    let stream = h.connect();
    let name = h.finish(std::thread::spawn(move || {
        let conn = Connection::from_socket(stream).unwrap();
        let (globals, _queue) = registry_queue_init::<Probe>(&conn).unwrap();
        globals.contents().with_list(|list| {
            list.iter()
                .find(|g| g.interface == DATA_CONTROL[0])
                .map(|g| g.name)
                .unwrap()
        })
    }));
    h.state.set_data_control_allowed(Vec::new());

    let stream = h.connect();
    let bound = h.finish(std::thread::spawn(move || {
        let conn = Connection::from_socket(stream).unwrap();
        let mut queue = conn.new_event_queue::<Probe>();
        let qh = queue.handle();
        let registry = conn.display().get_registry(&qh, ());
        let _manager: ZwlrDataControlManagerV1 = registry.bind(name, 1, &qh, ());
        queue.roundtrip(&mut Probe)
    }));
    assert!(
        bound.is_err(),
        "the bind of a refused client is a protocol error: {bound:?}"
    );
}

#[test]
fn a_change_that_no_longer_allows_a_clipboard_manager_disconnects_it_and_only_it() {
    let Some(mut h) = Harness::new() else { return };
    store(&h.conf(), vec![this_program()]);
    h.watch();
    let stop = Arc::new(AtomicBool::new(false));
    let manager = clipboard_manager(h.connect(), stop.clone());
    let other = bystander(h.connect(), stop.clone());
    for _ in 0..40 {
        h.step();
    }

    // the program's pin no longer matches: as if the file had been replaced since the person allowed it
    let mut changed = this_program();
    changed.pin.as_mut().unwrap().mtime += 1;
    h.change(|path| store(path, vec![changed]));
    h.wait_until("the clipboard manager to be disconnected", |_| {
        manager.is_finished()
    });
    assert!(h.finish(manager), "the clipboard manager was disconnected");
    assert_eq!(h.seen_by_a_client(), Vec::<String>::new());

    stop.store(true, Ordering::SeqCst);
    assert!(
        !h.finish(other),
        "a client never offered data control stays"
    );
}

#[test]
fn a_file_that_is_not_the_persons_alone_allows_nothing() {
    use std::os::unix::fs::PermissionsExt;
    let Some(mut h) = Harness::new() else { return };
    // the watch reports a change of the contents, so each step writes other entries
    let ls = || Entry::for_program(&std::fs::canonicalize("/usr/bin/ls").unwrap()).unwrap();
    store(&h.conf(), vec![this_program(), ls()]);
    std::fs::set_permissions(h.conf(), std::fs::Permissions::from_mode(0o666)).unwrap();
    h.watch();
    assert_eq!(
        h.seen_by_a_client(),
        Vec::<String>::new(),
        "writable by all"
    );

    h.change(|path| store(path, vec![this_program()]));
    assert_eq!(h.seen_by_a_client().len(), 2);

    let elsewhere = h.dir.path().join("elsewhere.conf");
    store(&elsewhere, vec![ls(), this_program()]);
    h.change(|path| {
        std::fs::remove_file(path).unwrap();
        std::os::unix::fs::symlink(&elsewhere, path).unwrap();
    });
    assert_eq!(h.seen_by_a_client(), Vec::<String>::new(), "a link");
}
