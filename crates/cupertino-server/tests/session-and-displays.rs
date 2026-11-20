//! Public-boundary tests for W5.4:
//! - `org.cupertino.Session1` (D13): `LogOut`, `Restart`, `ShutDown`, `Sleep` orchestrating application quit
//!   sequences with real clients (one application at a time with the pause between them, the 45 s interrupt, retry,
//!   force quit, cancel, `Sleep` at once without quitting anything).
//! - `org.cupertino.Displays1` (D14): `ListDisplays`, `SetScale`, `SetRefreshRate`, `SetPosition`, `TestConfigure`,
//!   `Configure`, transform round-trip, validation errors, and caller authorization checks.

use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cupertino_assets::Assets;
use cupertino_peer::Caller;
use cupertino_server::backend::headless::{self, HeadlessDisplay};
use cupertino_server::displays::service::{parse_transform, transform_name, DisplaysCall};
use cupertino_server::quit::{
    BusOut, Call, Refusal, Reject, SessionAction, NEXT_APPLICATION_S, QUIT_TIMEOUT_S,
};
use cupertino_server::state::{insert_client, new_output, ClientState};
use cupertino_server::{Config, Cupertino};
use smithay::output::{Mode as OutputMode, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_protocols::xdg::shell::client::{
    xdg_surface, xdg_toplevel, xdg_wm_base,
};
use smithay::reexports::wayland_server::Display;
use smithay::utils::Transform;
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle};

fn make_server() -> (EventLoop<'static, Cupertino>, Cupertino) {
    let event_loop: EventLoop<'static, Cupertino> = EventLoop::try_new().unwrap();
    let display: Display<Cupertino> = Display::new().unwrap();
    let mut config = Config::default();
    config.shell_processes.clear();
    let state = Cupertino::new(
        display,
        event_loop.handle(),
        event_loop.get_signal(),
        config,
        false,
        &Assets::builtin(),
    )
    .expect("server initializes");
    (event_loop, state)
}

#[test]
fn session1_actions_dispatch_when_no_apps_open_and_reject_anonymous_callers() {
    let (_loop, mut state) = make_server();
    let fired = Arc::new(Mutex::new(Vec::<SessionAction>::new()));
    let sink_fired = Arc::clone(&fired);
    state.quit.action_sink = Some(Box::new(move |action| {
        sink_fired.lock().unwrap().push(action);
    }));

    // With no open application windows, all four D13 actions dispatch immediately.
    for action in [
        SessionAction::Sleep,
        SessionAction::LogOut,
        SessionAction::Restart,
        SessionAction::ShutDown,
    ] {
        state.request_session_action(action).unwrap();
    }
    assert_eq!(
        *fired.lock().unwrap(),
        vec![
            SessionAction::Sleep,
            SessionAction::LogOut,
            SessionAction::Restart,
            SessionAction::ShutDown,
        ]
    );
    assert!(state.pending_session_action().is_none());

    // Anonymous D-Bus caller is refused by `on_quit_call`.
    let (tx, rx) = async_channel::bounded(1);
    state.on_quit_call(Call::SessionAction(
        SessionAction::LogOut,
        Caller::Anonymous,
        tx,
    ));
    let res = rx.try_recv().unwrap();
    assert!(matches!(res, Err(Reject::Caller(_))));
}

#[test]
fn session1_quit_sequence_cancels_and_respects_timeout_constant() {
    let (_loop, mut state) = make_server();
    state.fixed_time = Some(10.0);
    assert_eq!(QUIT_TIMEOUT_S, 45.0);

    // Calling cancel/retry while idle is safe.
    state.cancel_session_action();
    state.retry_session_action();
    assert!(state.pending_session_action().is_none());
}

/// A client with one toplevel of its own application, counting the \`close\` events the server sends it.
#[derive(Default)]
struct AppClient {
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    wm_base: Option<xdg_wm_base::XdgWmBase>,
    configured: bool,
    closes: usize,
}

impl Dispatch<wl_registry::WlRegistry, ()> for AppClient {
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
            match interface.as_str() {
                "wl_compositor" => state.compositor = Some(reg.bind(name, 4, qh, ())),
                "wl_shm" => state.shm = Some(reg.bind(name, 1, qh, ())),
                "xdg_wm_base" => state.wm_base = Some(reg.bind(name, 1, qh, ())),
                _ => {}
            }
        }
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for AppClient {
    fn event(
        _: &mut Self,
        base: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            base.pong(serial);
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, ()> for AppClient {
    fn event(
        state: &mut Self,
        surface: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            surface.ack_configure(serial);
            state.configured = true;
        }
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, ()> for AppClient {
    fn event(
        state: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        event: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_toplevel::Event::Close = event {
            state.closes += 1;
        }
    }
}

wayland_client::delegate_noop!(AppClient: ignore wl_compositor::WlCompositor);
wayland_client::delegate_noop!(AppClient: ignore wl_surface::WlSurface);
wayland_client::delegate_noop!(AppClient: ignore wl_shm::WlShm);
wayland_client::delegate_noop!(AppClient: ignore wl_shm_pool::WlShmPool);
wayland_client::delegate_noop!(AppClient: ignore wl_buffer::WlBuffer);

/// One application's window, as its client holds it.
struct AppWindow {
    conn: Connection,
    queue: EventQueue<AppClient>,
    client: AppClient,
    surface: wl_surface::WlSurface,
    xdg: xdg_surface::XdgSurface,
    toplevel: xdg_toplevel::XdgToplevel,
}

/// A server with one output, a clock the test sets, and the actions it would carry out recorded instead.
struct Session {
    event_loop: EventLoop<'static, Cupertino>,
    state: Cupertino,
    fired: Arc<Mutex<Vec<SessionAction>>>,
    signals: async_channel::Receiver<BusOut>,
}

impl Session {
    fn new() -> Self {
        let (event_loop, mut state) = make_server();
        state.add_output(new_output(
            "HEADLESS-1",
            PhysicalProperties {
                size: (400, 300).into(),
                subpixel: Subpixel::None,
                make: "Cupertino".into(),
                model: "Test".into(),
            },
            OutputMode {
                size: (1280, 800).into(),
                refresh: 60_000,
            },
            1.0,
            Transform::Normal,
            (0, 0),
        ));
        state.fixed_time = Some(10.0);
        let fired = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&fired);
        state.quit.action_sink = Some(Box::new(move |action| sink.lock().unwrap().push(action)));
        // nothing is killed: the test's clients are this process
        state.send_signal = Box::new(|_, _| {});
        let signals = state.quit.subscribe();
        Self {
            event_loop,
            state,
            fired,
            signals,
        }
    }

    /// One turn of the server's loop.
    fn turn(&mut self) {
        let _ = self
            .event_loop
            .dispatch(Some(Duration::from_millis(2)), &mut self.state);
        self.state.tick();
        let _ = self.state.display_handle.flush_clients();
    }

    /// Ten turns of the server and of every window's client.
    fn settle(&mut self, windows: &mut [&mut AppWindow]) {
        for _ in 0..10 {
            for w in windows.iter_mut() {
                let _ = w.conn.flush();
            }
            self.turn();
            for w in windows.iter_mut() {
                read(&w.conn, &mut w.queue, &mut w.client);
            }
        }
    }

    /// Move the clock on by \`seconds\` and let the server see it.
    fn wait(&mut self, seconds: f64, windows: &mut [&mut AppWindow]) {
        self.state.fixed_time = Some(self.state.fixed_time.unwrap() + seconds);
        self.settle(windows);
    }

    /// An application \`app_id\` with one mapped window.
    fn open(&mut self, app_id: &str) -> AppWindow {
        let (ours, theirs) = UnixStream::pair().unwrap();
        insert_client(&self.state.display_handle, ours, ClientState::default()).unwrap();
        let conn = Connection::from_socket(theirs).unwrap();
        let mut queue: EventQueue<AppClient> = conn.new_event_queue();
        let qh = queue.handle();
        let mut client = AppClient::default();
        let _registry = conn.display().get_registry(&qh, ());
        let step =
            |session: &mut Session, queue: &mut EventQueue<AppClient>, client: &mut AppClient| {
                let _ = conn.flush();
                session.turn();
                read(&conn, queue, client);
            };
        for _ in 0..6 {
            step(self, &mut queue, &mut client);
        }
        let surface = client.compositor.as_ref().unwrap().create_surface(&qh, ());
        let xdg = client
            .wm_base
            .as_ref()
            .unwrap()
            .get_xdg_surface(&surface, &qh, ());
        let toplevel = xdg.get_toplevel(&qh, ());
        toplevel.set_title(app_id.into());
        toplevel.set_app_id(app_id.into());
        surface.commit();
        for _ in 0..20 {
            step(self, &mut queue, &mut client);
            if client.configured {
                break;
            }
        }
        assert!(client.configured, "the window of {app_id} is configured");
        let buffer = shm_buffer(client.shm.as_ref().unwrap(), &qh, (200, 100));
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, 200, 100);
        surface.commit();
        for _ in 0..10 {
            step(self, &mut queue, &mut client);
        }
        AppWindow {
            conn,
            queue,
            client,
            surface,
            xdg,
            toplevel,
        }
    }

    /// What the session interface emitted since the last call, without the settings changes.
    fn emitted(&self) -> Vec<String> {
        std::iter::from_fn(|| self.signals.try_recv().ok())
            .filter_map(|out| match out {
                BusOut::ApplicationQuit(app) => Some(format!("ApplicationQuit {app}")),
                BusOut::QuitInterrupted { app_id, reason } => {
                    Some(format!("QuitInterrupted {app_id} {reason}"))
                }
                BusOut::QuitInterruptClosed => Some("QuitInterruptClosed".into()),
                BusOut::SessionActionFailed { action, reason } => {
                    Some(format!("SessionActionFailed {action} {reason}"))
                }
                BusOut::Settings(_) => None,
            })
            .collect()
    }

    fn fired(&self) -> Vec<SessionAction> {
        self.fired.lock().unwrap().clone()
    }
}

impl AppWindow {
    /// The application quits: its window goes away.
    fn close(&self) {
        self.toplevel.destroy();
        self.xdg.destroy();
        self.surface.destroy();
    }
}

/// Read and dispatch what the server sent the client.
fn read(conn: &Connection, queue: &mut EventQueue<AppClient>, client: &mut AppClient) {
    if let Some(guard) = conn.prepare_read() {
        let _ = guard.read();
    }
    let _ = queue.dispatch_pending(client);
}

/// A \`width\` × \`height\` buffer of opaque grey.
fn shm_buffer(
    shm: &wl_shm::WlShm,
    qh: &QueueHandle<AppClient>,
    (width, height): (i32, i32),
) -> wl_buffer::WlBuffer {
    let len = width * height * 4;
    let fd = rustix::fs::memfd_create("cupertino-test", rustix::fs::MemfdFlags::CLOEXEC).unwrap();
    let mut file = std::fs::File::from(fd);
    file.write_all(&vec![0x80; len as usize]).unwrap();
    let pool = shm.create_pool(file.as_fd(), len, qh, ());
    let buffer = pool.create_buffer(
        0,
        width,
        height,
        width * 4,
        wl_shm::Format::Xrgb8888,
        qh,
        (),
    );
    pool.destroy();
    buffer
}

#[test]
fn session1_refused_final_action_signals_once_and_leaves_the_session_idle() {
    let mut s = Session::new();
    let attempts = Arc::new(Mutex::new(Vec::<SessionAction>::new()));
    let seen = Arc::clone(&attempts);
    // a stand-in logind: the first request is refused, the later ones work
    s.state.quit.action_result_sink = Some(Box::new(move |action| {
        let mut seen = seen.lock().unwrap();
        seen.push(action);
        if seen.len() == 1 {
            Err("logind Reboot: another user is logged in".to_owned())
        } else {
            Ok(())
        }
    }));
    s.emitted();

    s.state
        .request_session_action(SessionAction::Restart)
        .unwrap();
    s.settle(&mut []);
    assert_eq!(
        s.emitted(),
        vec!["SessionActionFailed Restart logind Reboot: another user is logged in"],
        "the refusal is signalled once"
    );
    assert!(
        s.state.pending_session_action().is_none(),
        "Session1 is idle again"
    );

    s.state
        .request_session_action(SessionAction::ShutDown)
        .unwrap();
    s.settle(&mut []);
    assert_eq!(
        *attempts.lock().unwrap(),
        vec![SessionAction::Restart, SessionAction::ShutDown],
        "a later request is carried out"
    );
    assert!(s.emitted().is_empty(), "a success signals no failure");
}

#[test]
fn session1_log_out_quits_open_applications_one_at_a_time_then_acts() {
    let mut s = Session::new();
    let mut a = s.open("org.example.First");
    let mut b = s.open("org.example.Second");
    let open = s.state.open_applications();
    assert_eq!(open.len(), 2, "both applications are listed: {open:?}");
    s.emitted();

    s.state
        .request_session_action(SessionAction::LogOut)
        .unwrap();
    s.settle(&mut [&mut a, &mut b]);
    assert_eq!(
        s.state.pending_session_action(),
        Some(SessionAction::LogOut)
    );
    let (first, mut first_window, mut second_window) = if a.client.closes == 1 {
        (open[0].clone(), a, b)
    } else {
        (open[0].clone(), b, a)
    };
    assert_eq!(
        first_window.client.closes, 1,
        "the first application is asked to quit"
    );
    assert_eq!(second_window.client.closes, 0, "one application at a time");
    assert_eq!(
        s.state.quitting(),
        Some(first.as_str()),
        "the most recently used goes first"
    );

    first_window.close();
    s.settle(&mut [&mut first_window, &mut second_window]);
    assert_eq!(s.emitted(), vec![format!("ApplicationQuit {first}")]);
    assert_eq!(
        second_window.client.closes, 0,
        "the next one waits for the pause"
    );

    s.wait(
        NEXT_APPLICATION_S,
        &mut [&mut first_window, &mut second_window],
    );
    assert_eq!(
        second_window.client.closes, 1,
        "the next one is asked after the pause"
    );
    assert_eq!(s.state.quitting(), Some(open[1].as_str()));
    assert!(
        s.fired().is_empty(),
        "nothing is carried out while an application is open"
    );

    second_window.close();
    s.settle(&mut [&mut first_window, &mut second_window]);
    s.wait(
        NEXT_APPLICATION_S,
        &mut [&mut first_window, &mut second_window],
    );
    assert_eq!(s.emitted(), vec![format!("ApplicationQuit {}", open[1])]);
    assert_eq!(s.fired(), vec![SessionAction::LogOut]);
    assert!(s.state.pending_session_action().is_none());
}

#[test]
fn session1_interrupt_then_retry_force_quit_and_the_application_quitting() {
    let mut s = Session::new();
    let mut w = s.open("org.example.Keeper");
    let app = s.state.open_applications().remove(0);
    s.emitted();

    s.state
        .request_session_action(SessionAction::Restart)
        .unwrap();
    s.settle(&mut [&mut w]);
    assert_eq!(w.client.closes, 1);
    s.wait(QUIT_TIMEOUT_S - 1.0, &mut [&mut w]);
    assert!(s.emitted().is_empty(), "no interrupt before the timeout");
    s.wait(1.0, &mut [&mut w]);
    assert_eq!(
        s.emitted(),
        vec![format!("QuitInterrupted {app} kept-windows")]
    );
    assert!(s.fired().is_empty());

    // Try Again: the application is asked again, with a fresh timeout
    s.state.retry_session_action();
    s.settle(&mut [&mut w]);
    assert_eq!(w.client.closes, 2, "Try Again asks the application again");
    s.wait(QUIT_TIMEOUT_S, &mut [&mut w]);
    assert_eq!(
        s.emitted(),
        vec![format!("QuitInterrupted {app} kept-windows")]
    );

    // Force Quit: another full timeout for the application to go, then the interrupt again
    s.state.force_quit_application(&app);
    s.wait(QUIT_TIMEOUT_S - 1.0, &mut [&mut w]);
    assert!(s.emitted().is_empty(), "a force quit restarts the timeout");
    s.wait(1.0, &mut [&mut w]);
    assert_eq!(
        s.emitted(),
        vec![format!("QuitInterrupted {app} kept-windows")]
    );

    // it quits while its panel is up: the panel closes and the restart goes on
    w.close();
    s.settle(&mut [&mut w]);
    assert_eq!(
        s.emitted(),
        vec![
            format!("ApplicationQuit {app}"),
            "QuitInterruptClosed".to_string()
        ]
    );
    s.wait(NEXT_APPLICATION_S, &mut [&mut w]);
    assert_eq!(s.fired(), vec![SessionAction::Restart]);
    assert!(s.state.pending_session_action().is_none());
}

#[test]
fn session1_interrupt_then_cancel_keeps_the_session_and_sleep_quits_nothing() {
    let mut s = Session::new();
    let mut w = s.open("org.example.Keeper");
    let app = s.state.open_applications().remove(0);
    s.emitted();

    s.state
        .request_session_action(SessionAction::ShutDown)
        .unwrap();
    s.wait(QUIT_TIMEOUT_S, &mut [&mut w]);
    assert_eq!(
        s.emitted(),
        vec![format!("QuitInterrupted {app} kept-windows")]
    );

    s.state.cancel_session_action();
    assert!(s.state.pending_session_action().is_none());
    assert_eq!(s.state.quitting(), None);
    s.wait(QUIT_TIMEOUT_S * 2.0, &mut [&mut w]);
    assert!(
        s.emitted().is_empty(),
        "a cancelled sequence interrupts no more"
    );
    assert!(s.fired().is_empty(), "the session goes on");
    assert_eq!(
        s.state.open_applications(),
        vec![app.clone()],
        "the application keeps its window"
    );

    // sleep quits no application and is carried out at once
    s.state
        .request_session_action(SessionAction::Sleep)
        .unwrap();
    s.settle(&mut [&mut w]);
    assert_eq!(s.fired(), vec![SessionAction::Sleep]);
    assert_eq!(w.client.closes, 1, "sleep asks no window to close");
    assert_eq!(s.state.open_applications(), vec![app]);
}

#[test]
fn session1_pending_interrupt_names_the_action_application_and_reason_only_while_interrupted() {
    let mut s = Session::new();
    let mut w = s.open("org.example.Keeper");
    let app = s.state.open_applications().remove(0);
    s.emitted();
    assert_eq!(
        s.state.pending_interrupt(),
        None,
        "nothing is pending when idle"
    );

    s.state
        .request_session_action(SessionAction::Restart)
        .unwrap();
    s.settle(&mut [&mut w]);
    assert_eq!(
        s.state.pending_interrupt(),
        None,
        "asking is not an interrupt"
    );
    s.wait(QUIT_TIMEOUT_S, &mut [&mut w]);
    let pending = Some(("Restart", app.clone(), "kept-windows".to_owned()));
    assert_eq!(s.state.pending_interrupt(), pending);

    // the agent asks as the session agent does, through the call path
    let ask = |s: &mut Session, caller| {
        let (tx, rx) = async_channel::bounded(1);
        s.state.on_quit_call(Call::PendingInterrupt(caller, tx));
        rx.try_recv().unwrap()
    };
    assert!(matches!(
        ask(&mut s, Caller::Anonymous),
        Err(Reject::Caller(_))
    ));
    assert_eq!(
        s.state.pending_interrupt(),
        pending,
        "a refusal changes nothing"
    );

    // Try Again answers it; the next interrupt is pending again
    s.state.retry_session_action();
    s.settle(&mut [&mut w]);
    assert_eq!(
        s.state.pending_interrupt(),
        None,
        "retry answers the interrupt"
    );
    s.wait(QUIT_TIMEOUT_S, &mut [&mut w]);
    assert_eq!(s.state.pending_interrupt(), pending);

    // Cancel ends the sequence
    s.state.cancel_session_action();
    assert_eq!(
        s.state.pending_interrupt(),
        None,
        "cancel answers the interrupt"
    );
}

#[test]
fn displays1_lists_and_configures_scale_refresh_position_and_transform() {
    let (_loop, mut state) = make_server();
    headless::init_displays(
        &mut state,
        &[
            HeadlessDisplay {
                name: "DP-1".into(),
                size: (1920, 1080),
                physical_mm: (340, 220),
            },
            HeadlessDisplay {
                name: "DP-2".into(),
                size: (1920, 1080),
                physical_mm: (340, 220),
            },
        ],
        Some(1.0),
    )
    .expect("headless displays initialize");

    let list = state.list_displays();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].0, "DP-1");
    assert!(list[0].2);
    assert_eq!(list[0].5, 1920);
    assert_eq!(list[0].6, 1080);
    assert_eq!(list[0].7, 60_000);

    // SetScale, SetRefreshRate, and SetPosition apply through configure_heads.
    state.display_set_scale("DP-1", 2.0).unwrap();
    state.display_set_refresh_rate("DP-1", 120_000).unwrap();
    state.display_set_position("DP-2", 960, 0).unwrap();

    let updated = state.list_displays();
    assert_eq!(updated[0].7, 120_000);
    assert!((updated[0].8 - 2.0).abs() < 1e-6);
    assert_eq!((updated[1].3, updated[1].4), (960, 0));

    // TestConfigure validates without mutating state.
    state
        .display_configure("DP-1", true, 1280, 720, 60_000, 0, 0, 1.0, "90", true)
        .unwrap();
    let after_test = state.list_displays();
    assert_eq!(after_test[0].5, 1920);
    assert_eq!(after_test[0].9, "normal");

    // Configure applies full state including transform (DP-2 at x=960 avoids overlap with 720x1280).
    state
        .display_configure("DP-1", true, 1280, 720, 60_000, 0, 0, 1.0, "90", false)
        .unwrap();
    let after_apply = state.list_displays();
    assert_eq!((after_apply[0].5, after_apply[0].6), (1280, 720));
    assert_eq!(after_apply[0].9, "90");

    // Validation errors.
    assert!(matches!(
        state.display_set_scale("DP-1", 0.0),
        Err(Refusal::InvalidArgs(_))
    ));
    assert!(matches!(
        state.display_set_scale("NO-SUCH", 1.0),
        Err(Refusal::InvalidArgs(_))
    ));
    assert!(matches!(
        state.display_configure("DP-1", true, 1280, 720, 60_000, 0, 0, 1.0, "bogus", false),
        Err(Refusal::InvalidArgs(_))
    ));

    // Every Transform round-trips cleanly through transform_name / parse_transform.
    for t in [
        Transform::Normal,
        Transform::_90,
        Transform::_180,
        Transform::_270,
        Transform::Flipped,
        Transform::Flipped90,
        Transform::Flipped180,
        Transform::Flipped270,
    ] {
        assert_eq!(parse_transform(transform_name(t)), Some(t));
    }

    // D-Bus call path: ListDisplays is open, SetScale refuses anonymous callers.
    let (ltx, lrx) = async_channel::bounded(1);
    state.on_displays_call(DisplaysCall::List(ltx));
    assert_eq!(lrx.try_recv().unwrap().unwrap().len(), 2);

    let (stx, srx) = async_channel::bounded(1);
    state.on_displays_call(DisplaysCall::SetScale {
        connector: "DP-1".into(),
        scale: 1.5,
        caller: Caller::Anonymous,
        reply: stx,
    });
    assert!(matches!(srx.try_recv().unwrap(), Err(Reject::Caller(_))));
}
