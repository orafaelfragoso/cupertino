//! The lock screen of a person's session is the built-in login the server supervises (`docs/architecture/shell-host.md`,
//! *Programs the server starts itself*), never a process entry of the configuration. With the configuration as it
//! comes, a server that supervises the built-in login:
//! - asks it to lock once the display slept for the password delay ("Require password after display sleep");
//! - covers a session that is not locked when the system is about to sleep, asks the login to take the lock, and holds
//!   the sleep back until every output shows a covered frame.
//!
//! A server without it (no login installed, or the login screen's own server) asks for no lock and lets the sleep go on
//! at once.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use cupertino_assets::Assets;
use cupertino_defaults::power::{PowerSettings, SourceTimers, Timer};
use cupertino_protocols::client::cupertino_session_v1::{self, CupertinoSessionV1};
use cupertino_protocols::client::cupertino_shell_v1::CupertinoShellV1;
use cupertino_server::authorization::ShellRole;
use cupertino_server::backend::headless;
use cupertino_server::config::BuiltIn;
use cupertino_server::shell_connection::shell_process_connection;
use cupertino_server::{Config, Cupertino};
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use wayland_client::protocol::wl_registry;
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle};

/// How long the test waits for the server and its client.
const WAIT: Duration = Duration::from_secs(30);

/// The display sleep timer of the test's power settings.
const DISPLAY_SLEEP: Duration = Duration::from_secs(60);

/// How long after display sleep the session locks in the test's power settings.
const PASSWORD_DELAY: Duration = Duration::from_secs(5);

/// The lowest `cupertino_shell_v1` version whose session object has `lock_requested`.
const LOCK_REQUESTED_VERSION: u32 = 7;

/// What the lock screen's connection heard.
#[derive(Default)]
struct Heard {
    shell: Option<CupertinoShellV1>,
    session: Option<CupertinoSessionV1>,
    /// The `explicit` argument of every `lock_requested`.
    lock_requested: Vec<u32>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for Heard {
    fn event(
        heard: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            if interface == "cupertino_shell_v1" {
                assert!(
                    version >= LOCK_REQUESTED_VERSION,
                    "the shell global is {version}"
                );
                let shell: CupertinoShellV1 = registry.bind(name, version, qh, ());
                heard.session = Some(shell.get_session(qh, ()));
                heard.shell = Some(shell);
            }
        }
    }
}

impl Dispatch<CupertinoSessionV1, ()> for Heard {
    fn event(
        heard: &mut Self,
        _: &CupertinoSessionV1,
        event: cupertino_session_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let cupertino_session_v1::Event::LockRequested { explicit } = event {
            heard.lock_requested.push(explicit);
        }
    }
}

wayland_client::delegate_noop!(Heard: ignore CupertinoShellV1);

/// A connection the server made with the login's roles: what the lock screen hears from the server.
struct LockScreen {
    conn: Connection,
    queue: EventQueue<Heard>,
    heard: Heard,
}

impl LockScreen {
    fn step(&mut self) {
        let _ = self.conn.flush();
        if let Some(guard) = self.conn.prepare_read() {
            let _ = guard.read();
        }
        self.queue.dispatch_pending(&mut self.heard).unwrap();
    }
}

/// Which server runs.
#[derive(Clone, Copy, Debug)]
enum Server {
    /// A person's session whose supervisor runs the built-in login.
    WithLogin,
    /// A person's session without the login program.
    WithoutLogin,
    /// The login screen's own server, which runs `cupertino-login --greeter`.
    LoginScreen,
}

/// The headless server with one output, the test's power settings and the configuration as it comes.
struct Rig {
    event_loop: EventLoop<'static, Cupertino>,
    state: Cupertino,
    lock_screen: LockScreen,
}

impl Rig {
    fn new(server: Server) -> Rig {
        // the software rasteriser, as the program tests run the headless server
        std::env::set_var("LIBGL_ALWAYS_SOFTWARE", "1");
        let event_loop: EventLoop<'static, Cupertino> = EventLoop::try_new().unwrap();
        let display: Display<Cupertino> = Display::new().unwrap();
        let config = Config {
            power: PowerSettings {
                ac: SourceTimers {
                    display_sleep: Timer::After(DISPLAY_SLEEP),
                    system_sleep: Timer::Never,
                },
                dim_before_display_sleep: false,
                require_password_after: Timer::After(PASSWORD_DELAY),
                ..PowerSettings::default()
            },
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
        headless::init(&mut state, (320, 240), 1.0).expect("an offscreen renderer");
        // the login program has to be there to be supervised; it waits for its first start, which this test never runs
        let program: PathBuf = std::env::current_exe().unwrap();
        match server {
            Server::WithLogin => state
                .supervisor
                .add_built_ins([(BuiltIn::Login, Some(program))], Instant::now()),
            Server::WithoutLogin => state
                .supervisor
                .add_built_ins([(BuiltIn::Login, None)], Instant::now()),
            Server::LoginScreen => {
                state.login_screen = true;
                state
                    .supervisor
                    .add_login_greeter(Some(program), Instant::now());
            }
        }
        // the clock stands still: the test moves it
        state.fixed_time = Some(state.now());
        let stream = shell_process_connection(
            &state.display_handle,
            &[ShellRole::Session, ShellRole::SessionLock],
        )
        .unwrap();
        let conn = Connection::from_socket(stream).unwrap();
        let queue = conn.new_event_queue();
        let _registry = conn.display().get_registry(&queue.handle(), ());
        let mut rig = Rig {
            event_loop,
            state,
            lock_screen: LockScreen {
                conn,
                queue,
                heard: Heard::default(),
            },
        };
        rig.until("the lock screen follows the session", |rig| {
            rig.lock_screen.heard.session.is_some()
        });
        rig.turns(4);
        rig
    }

    /// Turn the server's loop and read the lock screen's connection until `done` holds, which is asked before every turn.
    fn until(&mut self, what: &str, mut done: impl FnMut(&Rig) -> bool) {
        let deadline = Instant::now() + WAIT;
        loop {
            self.lock_screen.step();
            if done(self) {
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting until {what}");
            self.turn();
        }
    }

    fn turn(&mut self) {
        headless::step(
            &mut self.state,
            &mut self.event_loop,
            Duration::from_millis(5),
        )
        .unwrap();
        self.lock_screen.step();
    }

    fn turns(&mut self, n: usize) {
        for _ in 0..n {
            self.turn();
        }
    }

    /// Move the model clock on by `by` and let the server act on it.
    fn advance(&mut self, by: Duration) {
        let now = self.state.fixed_time.unwrap();
        self.state.fixed_time = Some(now + by.as_secs_f64());
        self.turns(4);
    }
}

#[test]
fn display_sleep_asks_the_built_in_login_to_lock_after_the_password_delay() {
    for (server, locks) in [
        (Server::WithLogin, true),
        (Server::WithoutLogin, false),
        (Server::LoginScreen, false),
    ] {
        let mut rig = Rig::new(server);
        rig.advance(DISPLAY_SLEEP + Duration::from_secs(1));
        assert!(rig.state.power.asleep(), "{server:?}: the display sleeps");
        // the lock after display sleep is the next thing the idle policy has to do, only when a lock screen can lock
        assert_eq!(
            rig.state.display_power_deadline().is_some(),
            locks,
            "{server:?}: the password delay counts down"
        );
        assert!(
            rig.lock_screen.heard.lock_requested.is_empty(),
            "{server:?}"
        );
        rig.advance(PASSWORD_DELAY);
        let expected: &[u32] = if locks { &[0] } else { &[] };
        assert_eq!(
            rig.lock_screen.heard.lock_requested, expected,
            "{server:?}: the lock screen is asked to lock, as the display slept"
        );
    }
}

#[test]
fn a_system_sleep_before_the_lock_covers_the_session_until_every_output_shows_it_covered() {
    let mut rig = Rig::new(Server::WithLogin);
    rig.state.system_going_to_sleep();
    assert!(
        rig.state.session_lock.covers(),
        "the session is covered at once"
    );
    assert!(
        rig.state.session_lock.holds_sleep(),
        "the sleep waits for a covered frame"
    );
    rig.until("the lock screen is asked to take the lock", |rig| {
        rig.lock_screen.heard.lock_requested == [0]
    });
    rig.until(
        "the sleep goes on once the output shows a covered frame",
        |rig| !rig.state.session_lock.holds_sleep(),
    );
    assert!(
        rig.state.sleep_covered(),
        "every output shows a covered frame"
    );
    assert!(rig.state.session_lock.covers(), "the session stays covered");

    for server in [Server::WithoutLogin, Server::LoginScreen] {
        let mut rig = Rig::new(server);
        rig.state.system_going_to_sleep();
        assert!(
            !rig.state.session_lock.is_active() && !rig.state.session_lock.holds_sleep(),
            "{server:?}: without a lock screen to take it over the session is not covered and the sleep goes on"
        );
        rig.turns(4);
        assert!(
            rig.lock_screen.heard.lock_requested.is_empty(),
            "{server:?}"
        );
    }
}
