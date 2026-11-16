//! The real login screen of the greeter session (`cupertino-login --greeter`) under a headless server, beside a stand-in
//! session manager on its socket: a fast mouse over a screen that is busy drawing, the person's clicks and keys, a
//! password refused, a return from another console and a display sleep never end its connection. Tests skip (pass)
//! when no EGL device is available.

use crate::common;

use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cupertino_defaults::power::{PowerSettings, SourceTimers, Timer};
use cupertino_login_ipc::{
    decode, encode, Decoder, Event, Outcome, PowerMenu, Request, UserRow, MAX_EVENT_FRAME,
    MAX_REQUEST_FRAME, VERSION,
};
use cupertino_peer::ProcessHandle;
use cupertino_server::backend::headless;
use cupertino_server::cli::start_shell_supervision;
use cupertino_server::config::Config;
use cupertino_server::display_power::DisplayState;
use cupertino_server::events::BTN_LEFT;
use cupertino_server::Cupertino;
use smithay::backend::input::{ButtonState, KeyState};
use smithay::input::keyboard::Keycode;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::{Display, Resource};

/// The output in pixels and its scale: a fractional scale, whose buffers the screen draws at the next whole scale.
const OUTPUT: (i32, i32) = (1920, 1080);
const SCALE: f64 = 1.5;
/// The centre of the only user's tile, as a fraction of the output's logical size.
const TILE: (f64, f64) = (0.5, 0.61);
/// Evdev codes typed into the password field: letters, then Return.
const TYPED: [u32; 5] = [35, 18, 38, 38, 28];
/// Evdev codes of Escape, then the arrows on the picker.
const ESCAPE: u32 = 1;
const ARROWS: [u32; 2] = [106, 105];
/// How often the fast mouse reports its motion.
const MOUSE_HZ: u32 = 1000;

/// What the stand-in manager heard and of whom: the types of the requests in order, and the last screen's process.
#[derive(Default)]
struct Heard {
    requests: Vec<String>,
    screen: Option<ProcessHandle>,
}

type Shared = Arc<Mutex<Heard>>;

/// Serves `listener` as the session manager does for one login screen at a time: `Hello` and `Welcome` with one user and
/// every power item, a password prompt for each attempt and a refusal for each answer.
fn serve_manager(listener: UnixListener, heard: Shared) {
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { return };
            heard.lock().unwrap().screen = ProcessHandle::of_peer(&stream).ok();
            let heard = Arc::clone(&heard);
            std::thread::spawn(move || serve_screen(stream, &heard));
        }
    });
}

fn serve_screen(mut stream: UnixStream, heard: &Shared) {
    let mut decoder = Decoder::new(MAX_REQUEST_FRAME);
    let mut buffer = [0u8; 4096];
    let mut conversation = 0;
    loop {
        let wanted = decoder.wanted().min(buffer.len());
        let count = match stream.read(&mut buffer[..wanted]) {
            Ok(0) | Err(_) => return,
            Ok(count) => count,
        };
        let Some(body) = decoder.push(&buffer[..count]).unwrap() else {
            continue;
        };
        let request: Request = decode(&body).unwrap();
        let name = format!("{request:?}");
        heard
            .lock()
            .unwrap()
            .requests
            .push(name.split([' ', '{']).next().unwrap().to_owned());
        let answer = match request {
            Request::Hello { .. } => vec![
                Event::Hello { version: VERSION },
                Event::Welcome {
                    users: vec![UserRow {
                        uid: 1000,
                        login: "person".into(),
                        display_name: "A Person".into(),
                        picture: None,
                        logged_in: false,
                    }],
                    show_other: false,
                    power: PowerMenu {
                        restart: true,
                        shut_down: true,
                        sleep: true,
                    },
                    will_prompt: false,
                    retries_until_hint: 3,
                },
            ],
            Request::Authenticate { .. } => {
                conversation += 1;
                vec![Event::Prompt {
                    conversation,
                    echo: false,
                    text: "Password: ".into(),
                }]
            }
            Request::Answer { .. } => vec![Event::Result {
                outcome: Outcome::Denied,
            }],
            _ => Vec::new(),
        };
        for event in &answer {
            if stream
                .write_all(&encode(event, MAX_EVENT_FRAME).unwrap())
                .is_err()
            {
                return;
            }
        }
    }
}

struct Greeter {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
    time: u32,
    log: PathBuf,
    heard: Shared,
    _dir: tempfile::TempDir,
    _bus: cupertino_test_bus::PrivateBus,
}

fn log_text(log: &Path) -> String {
    std::fs::read_to_string(log).unwrap_or_default()
}

/// The greeter's surface has the keyboard and the server has not given its connection up: a connection the server
/// closes is left out of its clients at once, before its surfaces go.
fn greeter_up(state: &Cupertino) -> bool {
    let Some(client) = state
        .seat
        .get_keyboard()
        .unwrap()
        .current_focus()
        .filter(|surface| surface.is_alive())
        .and_then(|surface| surface.client())
    else {
        return false;
    };
    let mut connected = false;
    state
        .display_handle
        .backend_handle()
        .with_all_clients(|id| connected |= id == client.id());
    connected
}

impl Greeter {
    /// The server with the login screen of the greeter session, once the screen is up; `None` without an EGL device.
    fn start(power: PowerSettings) -> Option<Greeter> {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let runtime = root.join("runtime");
        std::fs::create_dir(&runtime).unwrap();
        std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700)).unwrap();
        for sub in ["home", "config", "data", "cache"] {
            std::fs::create_dir(root.join(sub)).unwrap();
        }
        let socket = root.join("greeter.sock");
        let heard = Shared::default();
        serve_manager(UnixListener::bind(&socket).unwrap(), Arc::clone(&heard));
        let log = root.join("greeter.log");
        let bus = common::shell::private_bus();
        let mut login = common::shell::process("cupertino-login", &bus.address, &["--greeter"]);
        let program = login.command.split_off(login.command.len() - 2);
        // the login screen's own class picks its renderer, the processor, as installed
        login
            .command
            .retain(|word| !word.starts_with("CUPERTINO_RENDERER="));
        let path = |sub: &str| root.join(sub).display().to_string();
        login.command.extend([
            format!("CUPERTINO_LOGIN_SOCK={}", socket.display()),
            format!("HOME={}", path("home")),
            format!("XDG_CONFIG_HOME={}", path("config")),
            format!("XDG_DATA_HOME={}", path("data")),
            format!("XDG_CACHE_HOME={}", path("cache")),
            format!("XDG_RUNTIME_DIR={}", runtime.display()),
            format!("CUPERTINO_SPEC_DIR={}", common::spawn::spec_dir().display()),
            "sh".into(),
            "-c".into(),
            "exec \"$0\" \"$1\" 2>>\"$2\"".into(),
        ]);
        login.command.extend(program);
        login.command.push(log.display().to_string());
        // the greeter session's configuration gives the login screen no role
        login.roles = Vec::new();
        let event_loop: EventLoop<'static, Cupertino> = EventLoop::try_new().unwrap();
        let mut state = Cupertino::new(
            Display::new().unwrap(),
            event_loop.handle(),
            event_loop.get_signal(),
            Config {
                shell_processes: vec![login],
                power,
                ..Config::default()
            },
            true,
            &cupertino_assets::Assets::builtin(),
        )
        .unwrap();
        state.login_screen = true;
        if let Err(e) = headless::init(&mut state, OUTPUT, SCALE) {
            common::egl_unavailable(format!("{e:#}"));
            return None;
        }
        start_shell_supervision(&state, &event_loop).expect("the supervision timer is inserted");
        let mut greeter = Greeter {
            state,
            event_loop,
            time: 0,
            log,
            heard,
            _dir: dir,
            _bus: bus,
        };
        // drawn and mapped: the pointer finds it
        greeter.wait_until("the greeter to be shown with the keyboard", |s, log| {
            log.contains("greeter ready")
                && greeter_up(s)
                && s.surface_under(tile().into()).map(|(surface, _)| surface)
                    == s.seat.get_keyboard().unwrap().current_focus()
        });
        Some(greeter)
    }

    /// One turn of the server's loop; the greeter has not stopped.
    fn step(&mut self) {
        headless::step(
            &mut self.state,
            &mut self.event_loop,
            Duration::from_millis(5),
        )
        .unwrap();
        let log = log_text(&self.log);
        assert!(
            !log.contains("the application stopped"),
            "the greeter stopped:\n{log}"
        );
    }

    fn wait_until(&mut self, what: &str, mut condition: impl FnMut(&Cupertino, &str) -> bool) {
        let end = Instant::now() + common::HANG_GUARD;
        while !condition(&self.state, &log_text(&self.log)) {
            assert!(
                Instant::now() < end,
                "timed out waiting for {what}; the greeter said:\n{}",
                log_text(&self.log)
            );
            self.step();
        }
    }

    fn run_for(&mut self, time: Duration) {
        let end = Instant::now() + time;
        while Instant::now() < end {
            self.step();
        }
    }

    fn key(&mut self, code: u32) {
        for state in [KeyState::Pressed, KeyState::Released] {
            self.time += 10;
            // as each event of a device does
            self.state.input_activity();
            self.state
                .key_from_device(Keycode::new(code + 8), state, self.time);
            self.step();
        }
    }

    fn click(&mut self, at: (f64, f64)) {
        self.time += 1;
        self.state.input_activity();
        self.state.pointer_moved_to(at.into(), self.time, None);
        self.step();
        for state in [ButtonState::Pressed, ButtonState::Released] {
            self.time += 10;
            self.state.input_activity();
            self.state.pointer_button(BTN_LEFT, state, self.time);
            self.step();
        }
    }

    /// The pointer moves over the screen as a mouse reporting [`MOUSE_HZ`] times a second does for `time`: each motion
    /// flushed to the clients as the server does after each input event, the loop turned in between.
    fn sweep(&mut self, time: Duration) {
        let (w, h) = logical();
        let start = Instant::now();
        let count = (time.as_secs_f64() * f64::from(MOUSE_HZ)) as u32;
        for i in 0..count {
            let t = f64::from(i) / f64::from(count);
            let x = w * (0.5 + 0.45 * (t * 12.0 * std::f64::consts::PI).sin());
            let y = h * (0.5 + 0.45 * (t * 7.0 * std::f64::consts::PI).cos());
            self.time += 1;
            self.state.input_activity();
            self.state.pointer_moved_to((x, y).into(), self.time, None);
            let _ = self.state.display_handle.flush_clients();
            headless::step(&mut self.state, &mut self.event_loop, Duration::ZERO).unwrap();
            let due = start + time.mul_f64(f64::from(i + 1) / f64::from(count));
            std::thread::sleep(due.saturating_duration_since(Instant::now()));
        }
    }

    /// How many requests of `kind` the manager heard.
    fn heard(&self, kind: &str) -> usize {
        let heard = self.heard.lock().unwrap();
        heard.requests.iter().filter(|r| *r == kind).count()
    }

    /// Type [`TYPED`] on the password screen; returns once the manager refused the answer and the greeter said so.
    fn type_wrong_password(&mut self) {
        let answers = self.heard("Answer");
        let failures = log_text(&self.log).matches("authentication failed").count();
        for code in TYPED {
            self.key(code);
        }
        let heard = Arc::clone(&self.heard);
        self.wait_until("the password to be refused", |_, log| {
            let heard = heard.lock().unwrap();
            heard.requests.iter().filter(|r| *r == "Answer").count() > answers
                && log.matches("authentication failed").count() > failures
        });
    }
}

/// The logical size of the output.
fn logical() -> (f64, f64) {
    (f64::from(OUTPUT.0) / SCALE, f64::from(OUTPUT.1) / SCALE)
}

fn tile() -> (f64, f64) {
    let (w, h) = logical();
    (w * TILE.0, h * TILE.1)
}

/// The greeter is busy for a second, as when it draws a whole screen on the processor (stopped here), while a mouse
/// moves over it: the server keeps what it sends meanwhile, the greeter reads it all once it goes on, and the person
/// can still sign in.
#[test]
fn a_busy_greeter_keeps_its_connection_under_a_fast_mouse() {
    let Some(mut greeter) = Greeter::start(PowerSettings::default()) else {
        return;
    };
    let screen = greeter
        .heard
        .lock()
        .unwrap()
        .screen
        .take()
        .expect("the greeter's process");
    screen.send(libc::SIGSTOP).unwrap();
    greeter.sweep(Duration::from_secs(1));
    let up = greeter_up(&greeter.state);
    screen.send(libc::SIGCONT).unwrap();
    assert!(up, "the server closed the connection of the busy greeter");
    greeter.run_for(Duration::from_millis(300));
    greeter.click(tile());
    greeter.type_wrong_password();
    assert!(greeter_up(&greeter.state));
}

/// The pointer swept across the screen, the user's tile clicked, a password typed and refused, Escape and the arrows on
/// the picker, a return from another console and a display that slept and woke: the greeter keeps its connection all
/// along, and its server never asks for a lock.
#[test]
fn the_greeter_keeps_its_connection_through_the_persons_input() {
    // the display sleeps two seconds after the last input, and a lock would follow at once
    let timers = SourceTimers {
        display_sleep: Timer::After(Duration::from_secs(2)),
        system_sleep: Timer::Never,
    };
    let power = PowerSettings {
        ac: timers,
        battery: timers,
        dim_before_display_sleep: false,
        require_password_after: Timer::After(Duration::ZERO),
        ..PowerSettings::default()
    };
    let Some(mut greeter) = Greeter::start(power) else {
        return;
    };
    greeter.sweep(Duration::from_secs(1));
    greeter.click(tile());
    greeter.type_wrong_password();
    greeter.key(ESCAPE);
    for code in ARROWS {
        greeter.key(code);
    }

    // another console has the seat, then this one again
    greeter.state.session_deactivated();
    greeter.run_for(Duration::from_millis(2500));
    assert_eq!(
        greeter.state.power.policy().state(),
        DisplayState::On,
        "the display slept in the background"
    );
    greeter.state.session_activated();
    greeter.run_for(Duration::from_millis(200));

    // the display sleeps, a key wakes it, and the person signs in
    greeter.wait_until("the display to sleep", |s, _| {
        s.power.policy().state() == DisplayState::Asleep
    });
    greeter.run_for(Duration::from_millis(300));
    greeter.key(ARROWS[0]);
    assert_eq!(greeter.state.power.policy().state(), DisplayState::On);
    greeter.click(tile());
    greeter.type_wrong_password();
    assert!(greeter_up(&greeter.state));
    assert!(!greeter.state.session_lock.is_active());
    assert_eq!(greeter.heard("Hello"), 1, "the greeter was started again");
}
