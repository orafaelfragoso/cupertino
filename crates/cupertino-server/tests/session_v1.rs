//! `cupertino_shell_v1.get_session` end to end (version 1 and 2 observers): the compositor runs in-process on an offscreen EGL
//! device; session observers and a lock client connect through its socket. Tests skip (pass) when
//! no EGL device is available.

mod common;

use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cupertino_defaults::accessibility::DisplayOption;
use cupertino_protocols::client::cupertino_session_v1::{self, CupertinoSessionV1};
use cupertino_protocols::client::cupertino_shell_v1::CupertinoShellV1;
use cupertino_server::authorization::ShellRole;
use cupertino_server::backend::headless;
use cupertino_server::config::Config;
use cupertino_server::Cupertino;
use cupertino_test_bus::PrivateBus;
use cupertino_theme::Appearance;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use wayland_client::globals::{registry_queue_init, GlobalList, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_output, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use wayland_protocols::ext::session_lock::v1::client::{
    ext_session_lock_manager_v1, ext_session_lock_surface_v1, ext_session_lock_v1,
};

const OUTPUT: (i32, i32) = (1000, 700);

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

    fn wait_until(&mut self, what: &str, mut cond: impl FnMut(&Cupertino) -> bool) {
        let end = Instant::now() + common::HANG_GUARD;
        while Instant::now() < end {
            headless::step(
                &mut self.state,
                &mut self.event_loop,
                Duration::from_millis(5),
            )
            .unwrap();
            if cond(&self.state) {
                return;
            }
        }
        panic!("timed out waiting for {what}");
    }

    /// A connection of a shell process the server supervises, holding the Session role.
    fn shell_stream(&mut self) -> UnixStream {
        cupertino_server::shell_connection::shell_process_connection(
            &self.state.display_handle,
            &[ShellRole::Session],
        )
        .unwrap()
    }

    /// A connection of the login process: the session observer and the lock screen.
    fn login_stream(&mut self) -> UnixStream {
        cupertino_server::shell_connection::shell_process_connection(
            &self.state.display_handle,
            &[ShellRole::Session, ShellRole::SessionLock],
        )
        .unwrap()
    }

    /// A connection of the lock screen, holding the SessionLock role.
    fn lock_stream(&mut self) -> UnixStream {
        cupertino_server::shell_connection::shell_process_connection(
            &self.state.display_handle,
            &[ShellRole::SessionLock],
        )
        .unwrap()
    }
}

// ---- clients -------------------------------------------------------------------------------

/// What a session observer was told, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Told {
    Dark(bool),
    /// The `display_options` bits: 1 reduce motion, 2 reduce transparency, 4 increase contrast, 8 differentiate without colour.
    Options(u32),
    Locked,
    Unlocked,
    /// `lock_requested`, with `explicit`.
    LockRequested(bool),
    /// `log_out_requested`.
    LogOutRequested,
}

fn solid_buffer(
    shm: &wl_shm::WlShm,
    size: (i32, i32),
    qh: &QueueHandle<ClientState>,
) -> wl_buffer::WlBuffer {
    let (w, h) = size;
    let fd = rustix::fs::memfd_create("cupertino-test", rustix::fs::MemfdFlags::CLOEXEC).unwrap();
    let mut file = std::fs::File::from(fd);
    file.write_all(&vec![0x80; (w * h * 4) as usize]).unwrap();
    let pool = shm.create_pool(file.as_fd(), w * h * 4, qh, ());
    let buffer = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Argb8888, qh, ());
    pool.destroy();
    buffer
}

struct ClientState {
    shm: wl_shm::WlShm,
    told: Arc<Mutex<Vec<Told>>>,
    locked: Arc<AtomicBool>,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for ClientState {
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
macro_rules! ignore_events {
    ($($t:ty),*) => {$(
        impl Dispatch<$t, ()> for ClientState {
            fn event(_: &mut Self, _: &$t, _: <$t as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    )*};
}
ignore_events!(
    wl_compositor::WlCompositor,
    wl_shm::WlShm,
    wl_shm_pool::WlShmPool,
    wl_buffer::WlBuffer,
    wl_surface::WlSurface,
    wl_output::WlOutput,
    CupertinoShellV1,
    ext_session_lock_manager_v1::ExtSessionLockManagerV1
);

impl Dispatch<CupertinoSessionV1, ()> for ClientState {
    fn event(
        state: &mut Self,
        _: &CupertinoSessionV1,
        event: cupertino_session_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let told = match event {
            cupertino_session_v1::Event::Appearance { dark } => Told::Dark(dark == 1),
            cupertino_session_v1::Event::DisplayOptions { options } => Told::Options(
                options
                    .into_result()
                    .expect("known display option bits")
                    .bits(),
            ),
            cupertino_session_v1::Event::Locked => Told::Locked,
            cupertino_session_v1::Event::Unlocked => Told::Unlocked,
            cupertino_session_v1::Event::LockRequested { explicit } => {
                Told::LockRequested(explicit == 1)
            }
            cupertino_session_v1::Event::LogOutRequested => Told::LogOutRequested,
            _ => return,
        };
        state.told.lock().unwrap().push(told);
    }
}

impl Dispatch<ext_session_lock_v1::ExtSessionLockV1, ()> for ClientState {
    fn event(
        state: &mut Self,
        _: &ext_session_lock_v1::ExtSessionLockV1,
        event: ext_session_lock_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ext_session_lock_v1::Event::Locked = event {
            state.locked.store(true, Ordering::SeqCst);
        }
    }
}

/// The surface is the user data, so the configure can attach a buffer of the configured size.
impl Dispatch<ext_session_lock_surface_v1::ExtSessionLockSurfaceV1, wl_surface::WlSurface>
    for ClientState
{
    fn event(
        state: &mut Self,
        lock_surface: &ext_session_lock_surface_v1::ExtSessionLockSurfaceV1,
        event: ext_session_lock_surface_v1::Event,
        surface: &wl_surface::WlSurface,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let ext_session_lock_surface_v1::Event::Configure {
            serial,
            width,
            height,
        } = event
        {
            lock_surface.ack_configure(serial);
            let size = (width as i32, height as i32);
            surface.attach(Some(&solid_buffer(&state.shm, size, qh)), 0, 0);
            surface.damage_buffer(0, 0, size.0, size.1);
            surface.commit();
        }
    }
}

/// A client thread; dropping the handle ends the thread and so disconnects the client.
struct Client {
    /// The client's end of the connection, to cut it without the client's cooperation.
    stream: UnixStream,
    stop: Arc<AtomicBool>,
    unlock: Arc<AtomicBool>,
    told: Arc<Mutex<Vec<Told>>>,
    locked: Arc<AtomicBool>,
}

impl Client {
    fn run(
        stream: UnixStream,
        setup: impl FnOnce(
                &mut EventQueue<ClientState>,
                &mut ClientState,
                &GlobalList,
            ) -> Option<ext_session_lock_v1::ExtSessionLockV1>
            + Send
            + 'static,
    ) -> Client {
        let (stop, unlock) = (
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
        );
        let (told, locked) = (
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(AtomicBool::new(false)),
        );
        let (stop2, unlock2, told2, locked2) =
            (stop.clone(), unlock.clone(), told.clone(), locked.clone());
        let thread_stream = stream.try_clone().unwrap();
        std::thread::spawn(move || {
            let conn = Connection::from_socket(thread_stream).unwrap();
            let (globals, mut queue) = registry_queue_init::<ClientState>(&conn).unwrap();
            let qh = queue.handle();
            let mut client = ClientState {
                shm: globals.bind(&qh, 1..=1, ()).unwrap(),
                told: told2,
                locked: locked2,
            };
            let mut lock = setup(&mut queue, &mut client, &globals);
            while !stop2.load(Ordering::SeqCst) {
                if unlock2.swap(false, Ordering::SeqCst) {
                    if let Some(lock) = lock.take() {
                        lock.unlock_and_destroy();
                    }
                }
                if queue.roundtrip(&mut client).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        Client {
            stream,
            stop,
            unlock,
            told,
            locked,
        }
    }

    /// A shell process that observes the session at `version` of the shell global.
    fn observer(stream: UnixStream, version: u32) -> Client {
        Self::run(stream, move |queue, client, globals| {
            let qh = queue.handle();
            let shell: CupertinoShellV1 = globals.bind(&qh, version..=version, ()).unwrap();
            // the observer lives as long as the thread: it is never destroyed
            std::mem::forget(shell.get_session(&qh, ()));
            queue.roundtrip(client).unwrap();
            None
        })
    }

    /// A client that locks the session and covers the output with a lock surface.
    fn locker(stream: UnixStream) -> Client {
        Self::run(stream, |queue, client, globals| {
            let qh = queue.handle();
            let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ()).unwrap();
            let manager: ext_session_lock_manager_v1::ExtSessionLockManagerV1 =
                globals.bind(&qh, 1..=1, ()).unwrap();
            let output: wl_output::WlOutput = globals.bind(&qh, 1..=4, ()).unwrap();
            let lock = manager.lock(&qh, ());
            let surface = compositor.create_surface(&qh, ());
            let lock_surface = lock.get_lock_surface(&surface, &output, &qh, surface.clone());
            queue.roundtrip(client).unwrap();
            std::mem::forget((manager, output, surface, lock_surface));
            Some(lock)
        })
    }

    fn told(&self) -> Vec<Told> {
        self.told.lock().unwrap().clone()
    }

    fn locked(&self) -> bool {
        self.locked.load(Ordering::SeqCst)
    }

    /// Send `unlock_and_destroy`.
    fn unlock(&self) {
        self.unlock.store(true, Ordering::SeqCst);
    }

    /// Close the connection at once, as a crash would.
    fn kill(&self) {
        self.stream.shutdown(std::net::Shutdown::Both).unwrap();
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

// ---- tests ---------------------------------------------------------------------------------

#[test]
fn binding_tells_the_current_appearance_display_options_and_lock_state() {
    let Some(mut h) = Harness::new() else { return };
    let before = Client::observer(h.shell_stream(), 2);
    h.wait_until("the first observer to be told", |_| {
        before.told().len() == 3
    });
    assert_eq!(
        before.told(),
        [Told::Dark(false), Told::Options(0), Told::Unlocked]
    );

    h.state.set_appearance(Appearance::Dark);
    h.state
        .set_display_option(DisplayOption::ReduceTransparency, true);
    h.state
        .set_display_option(DisplayOption::DifferentiateWithoutColor, true);
    let locker = Client::locker(h.lock_stream());
    h.wait_until("the session to be locked", |_| locker.locked());
    let after = Client::observer(h.shell_stream(), 2);
    h.wait_until("the second observer to be told", |_| {
        after.told().len() == 3
    });
    assert_eq!(
        after.told(),
        [Told::Dark(true), Told::Options(2 | 8), Told::Locked]
    );
}

#[test]
fn appearance_changes_and_the_lock_cycle_reach_an_observer_in_order() {
    let Some(mut h) = Harness::new() else { return };
    let observer = Client::observer(h.shell_stream(), 2);
    h.wait_until("the observer to be told", |_| observer.told().len() == 3);

    h.state.toggle_appearance();
    // the appearance it already has: nothing to tell
    h.state.set_appearance(Appearance::Dark);
    let locker = Client::locker(h.lock_stream());
    h.wait_until("the session to be locked", |_| locker.locked());
    locker.unlock();
    h.wait_until("the unlock to reach the observer", |_| {
        observer.told().len() == 6
    });
    h.state.set_appearance(Appearance::Light);
    h.wait_until("the second appearance change", |_| {
        observer.told().len() == 7
    });

    assert_eq!(
        observer.told(),
        [
            Told::Dark(false),
            Told::Options(0),
            Told::Unlocked,
            Told::Dark(true),
            Told::Locked,
            Told::Unlocked,
            Told::Dark(false)
        ]
    );
}

#[test]
fn every_display_option_change_reaches_a_version_two_observer_and_none_a_version_one_observer() {
    let Some(mut h) = Harness::new() else { return };
    let current = Client::observer(h.shell_stream(), 2);
    let old = Client::observer(h.shell_stream(), 1);
    h.wait_until("both observers to be told", |_| {
        current.told().len() == 3 && old.told().len() == 2
    });

    h.state.toggle_display_option(DisplayOption::ReduceMotion);
    // the value it already has: nothing to tell
    h.state
        .set_display_option(DisplayOption::ReduceMotion, true);
    h.state
        .toggle_display_option(DisplayOption::IncreaseContrast);
    h.state.toggle_display_option(DisplayOption::ReduceMotion);
    // a colour filter alone changes no display option
    let mut settings = h.state.accessibility.settings;
    settings.color_filter = Some(cupertino_defaults::accessibility::ColorFilter::Grayscale);
    h.state.set_accessibility(settings);
    h.state
        .toggle_display_option(DisplayOption::ReduceTransparency);
    h.wait_until("the changes to reach the observer", |_| {
        current.told().len() == 7
    });
    // the version 1 observer gets the appearance change sent after the options, and nothing before it
    h.state.toggle_appearance();
    h.wait_until("the appearance to reach both", |_| {
        current.told().len() == 8 && old.told().len() == 3
    });

    assert_eq!(
        current.told(),
        [
            Told::Dark(false),
            Told::Options(0),
            Told::Unlocked,
            Told::Options(1),
            Told::Options(1 | 4),
            Told::Options(4),
            Told::Options(4 | 2),
            Told::Dark(true)
        ]
    );
    assert_eq!(
        old.told(),
        [Told::Dark(false), Told::Unlocked, Told::Dark(true)]
    );
}

#[test]
fn a_lock_whose_client_crashes_stays_locked_and_a_takeover_tells_nothing_new() {
    let Some(mut h) = Harness::new() else { return };
    let observer = Client::observer(h.shell_stream(), 2);
    h.wait_until("the observer to be told", |_| observer.told().len() == 3);

    let holder = Client::locker(h.lock_stream());
    h.wait_until("the session to be locked", |_| holder.locked());
    holder.kill();
    h.wait_until("the lock to be orphaned", |s| {
        s.surface_under((500.0, 350.0).into()).is_none()
    });
    let successor = Client::locker(h.lock_stream());
    h.wait_until("a new client to take the lock", |_| successor.locked());
    successor.unlock();
    h.wait_until("the unlock to reach the observer", |_| {
        observer.told().len() == 5
    });

    assert_eq!(
        observer.told(),
        [
            Told::Dark(false),
            Told::Options(0),
            Told::Unlocked,
            Told::Locked,
            Told::Unlocked
        ]
    );
}

/// A process that took `org.cupertino.Login1`: counts the lock requests it receives.
struct Squatter(Arc<Mutex<u32>>);

#[zbus::interface(name = "org.cupertino.Login1")]
impl Squatter {
    fn lock(&self) {
        *self.0.lock().unwrap() += 1;
    }
    fn lock_now(&self) {
        *self.0.lock().unwrap() += 1;
    }
    fn request_log_out(&self) {
        *self.0.lock().unwrap() += 1;
    }
}

/// SEC-04: the lock goes to the login process over the server's own connection, so a process that owns the session bus name
/// before the login process does cannot make it go missing, and it receives nothing. Without a login process connected the
/// request falls back to the session bus, which only the verified owner of the name answers.
#[test]
fn a_lock_request_reaches_the_lock_screen_while_a_squatter_owns_the_login_name() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: no dbus-daemon");
        return;
    };
    let Some(mut h) = Harness::new() else { return };
    let handle = cupertino_server::dbus::DbusHandle::start_on(Some(bus.address.clone()), || {})
        .expect("worker thread");
    // the supervised login process is some other process than the squatter, which is this test
    handle.set_login(cupertino_server::dbus::LoginTrust::Agent(
        std::process::id() + 1,
    ));
    h.state.dbus = Some(handle);
    let received = Arc::new(Mutex::new(0));
    let _squatter = zbus::block_on(
        zbus::connection::Builder::address(bus.address.as_str())
            .unwrap()
            .name("org.cupertino.Login1")
            .unwrap()
            .serve_at("/org/cupertino/Login1", Squatter(received.clone()))
            .unwrap()
            .build(),
    )
    .expect("the squatter");

    // another shell process with the Session role only is not asked
    let other = Client::observer(h.shell_stream(), 9);
    let login = Client::observer(h.login_stream(), 9);
    h.wait_until("both observers to be told", |_| {
        other.told().len() == 3 && login.told().len() == 3
    });

    h.state.lock_session_now();
    h.state.lock_session();
    h.wait_until("the login process to be asked twice", |_| {
        login.told().len() == 5
    });
    assert_eq!(
        login.told()[3..],
        [Told::LockRequested(true), Told::LockRequested(false)]
    );
    assert_eq!(
        other.told().len(),
        3,
        "a process without the lock role is not asked"
    );

    // a log out goes the same way: the lock screen is asked over the private connection, and nothing ends the session
    h.state.log_out_session();
    h.wait_until("the login process to be asked to log out", |_| {
        login.told().len() == 6
    });
    assert_eq!(login.told()[5], Told::LogOutRequested);
    assert_eq!(other.told().len(), 3);
    assert!(h.state.running, "the session goes on");

    // the bus path would have been tried by now
    let end = Instant::now() + Duration::from_millis(500);
    while Instant::now() < end {
        h.wait_until("a step", |_| true);
    }
    assert_eq!(
        *received.lock().unwrap(),
        0,
        "the squatter got a lock request"
    );

    // no lock screen connected: the bus is the way, and the squatter is still refused; a log out it would answer for
    // neither reaches it nor ends the session
    drop(login);
    drop(other);
    h.state.sessions = Default::default();
    h.state.lock_session_now();
    h.state.log_out_session();
    let end = Instant::now() + Duration::from_millis(500);
    while Instant::now() < end {
        h.wait_until("a step", |_| true);
    }
    assert_eq!(
        *received.lock().unwrap(),
        0,
        "the squatter got a lock request"
    );
}

#[test]
fn a_session_the_system_sleeps_on_before_it_is_locked_asks_the_lock_screen_to_take_it_over() {
    let Some(mut h) = Harness::new() else { return };
    h.state
        .config
        .shell_processes
        .push(cupertino_server::config::ShellProcess {
            name: "login".into(),
            command: vec!["/usr/bin/false".into()],
            roles: vec![ShellRole::SessionLock],
        });
    let login = Client::observer(h.login_stream(), 9);
    h.wait_until("the observer to be told", |_| login.told().len() == 3);

    // the lock screen did not lock before the sleep: the session is covered and the lock screen asked to lock
    h.state.system_going_to_sleep();
    h.wait_until("the lock screen to be asked to lock", |_| {
        login.told().len() == 5
    });
    assert_eq!(
        login.told()[3..],
        [Told::Locked, Told::LockRequested(false)]
    );
    let successor = Client::locker(h.lock_stream());
    h.wait_until("the lock screen to take the lock", |_| successor.locked());
    successor.unlock();
    h.wait_until("the unlock to reach the observer", |_| {
        login.told().len() == 6
    });
    assert_eq!(login.told()[5], Told::Unlocked);
}
