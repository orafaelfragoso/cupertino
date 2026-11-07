//! The lock screen as assistive technology sees it, on a private bus with a fake accessibility registry: the password
//! field is password text that says nothing of what is typed, no node of the tree, no message on the bus and no line of
//! the diagnostics holds the password while it is typed, and every answer to an attempt is announced once. The lock
//! screen is the Kit view of the lock screen ([`view::screen`]) on the headless window system with the lock screen's
//! AT-SPI adapter, in a copy of this test binary started with the bus addresses AccessKit reads from its environment.
//! A second window of that application announces a marker before and after its part, and AccessKit sends its events in
//! order, so what lies between the two markers is exactly what the part sent.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, Read, Write};
use std::num::NonZeroU32;
use std::process::{Child, ChildStdin, Command as Process, Stdio};
use std::sync::mpsc::{self, channel, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Wake, Waker};
use std::thread::Thread;
use std::time::{Duration, Instant};

use cupertino_assets::BUILTIN_FONT;
use cupertino_kit::core::{
    AccessibilityAdapter, Application, Command, Key as NodeKey, PlaceholderIcons, RuntimeOptions,
    Window, WindowKey,
};
use cupertino_kit::geometry::Size;
use cupertino_kit::layout::LayoutDirection;
use cupertino_kit::platform::{
    Capabilities, KeyEvent, Keysym, Mods, PlatformEvent, Timestamp, WindowEvent, WindowId,
    WindowStates,
};
use cupertino_kit::render::{CpuConfig, CpuRenderer};
use cupertino_kit::text::{FontIndex, GlyphRaster, TextSystem};
use cupertino_kit::theme::{Appearance, ThemePreferences, ThemeTokens};
use cupertino_login::model::{AuthOutcome, LockScreen};
use cupertino_login::user::User;
use cupertino_login::view::{
    self, Clock, Field, Input, Metrics, Screen, DENIED, LOCKED_TITLE, LOCK_APP_ID, PASSWORD_LABEL,
};
use cupertino_test_bus::PrivateBus;
use cupertino_ui_atspi::AtspiAdapter;
use cupertino_ui_test::{Inspector, ManualClock, TestDriver};
use zbus::blocking::{Connection, MessageIterator, Proxy};
use zbus::message::Type;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};
use zbus::Message;

/// Set in the copy of the test binary that plays the lock screen.
const LOCK_SCREEN: &str = "CUPERTINO_TEST_LOCK_SCREEN";
const ROOT: &str = "/org/a11y/atspi/accessible/root";
const SECRET: &str = "hunter2";
/// AT-SPI's role number of password text (`AtspiRole`).
const PASSWORD_TEXT: u32 = 40;
/// AT-SPI's politeness number of an assertive announcement (`AtspiLive`).
const ASSERTIVE: i32 = 2;
/// What the marker window announces just before and just after the part.
const READY: &str = "Test part starts";
const PLAYED: &str = "Test part ends";
const MARKERS_TITLE: &str = "Markers";
/// The diagnostics of the lock screen start with this line, so the test knows they were written.
const DIAGNOSTICS: &str = "diagnostics:";
/// How long one step may take on a heavily loaded machine; on an idle one each takes milliseconds. Below nextest's
/// 120 s budget for the whole test, so a hang fails with its own message.
const PATIENCE: Duration = Duration::from_secs(60);
const SCREEN: Size = Size::new(1280.0, 800.0);

const RETURN: u32 = 0xff0d;
const ESCAPE: u32 = 0xff1b;

/// The answers the lock screen gives to its attempts, in order; set before its application starts.
static OUTCOMES: Mutex<Vec<AuthOutcome>> = Mutex::new(Vec::new());

#[derive(Clone, Debug)]
enum Msg {
    Lock(Input),
    /// Escape on the marker window shows the next marker.
    Marker(Input),
}

#[derive(Debug)]
struct Model {
    screen: LockScreen,
    outcomes: VecDeque<AuthOutcome>,
    /// How many markers were shown.
    markers: usize,
    /// Each attempt's password, as the authenticator would take it.
    attempts: usize,
    now: f64,
    clock: Clock,
    user: User,
    metrics: Metrics,
}

/// The lock screen of the session process, answering each attempt at once with the next of [`OUTCOMES`].
struct Locker;

impl Application for Locker {
    type Model = Model;
    type Message = Msg;

    fn init() -> (Model, Vec<Command<Msg>>) {
        let mut screen = LockScreen::new(0.0, 3600.0);
        screen.settle_interface();
        let model = Model {
            screen,
            outcomes: OUTCOMES.lock().unwrap().drain(..).collect(),
            markers: 0,
            attempts: 0,
            now: 1.0,
            clock: Clock {
                date: "Monday, June 1".into(),
                time: "09:41".into(),
            },
            user: User {
                login: "ada".into(),
                display: "Ada".into(),
                picture: None,
                uid: None,
                logged_in: false,
            },
            metrics: Metrics {
                ascent: 0.97,
                descent: 0.24,
            },
        };
        (model, Vec::new())
    }

    fn update(model: &mut Model, message: Msg) -> Vec<Command<Msg>> {
        // the application's diagnostics: every message it handles
        eprintln!("cupertino-login test: {message:?}");
        model.now += 0.01;
        let now = model.now;
        match message {
            Msg::Marker(Input::Escape) => model.markers += 1,
            Msg::Marker(_) => {}
            Msg::Lock(Input::Edited) => model.screen.edit(now),
            Msg::Lock(Input::Escape) => model.screen.escape(now),
            Msg::Lock(Input::Activity) => model.screen.activity(now),
            Msg::Lock(Input::Submit(typed)) => {
                if model.screen.submit(now, typed.secret()).is_some() {
                    model.attempts += 1;
                    let outcome = model
                        .outcomes
                        .pop_front()
                        .expect("an answer for each attempt");
                    model.screen.finish_auth(now, outcome).unwrap();
                }
            }
            Msg::Lock(_) => {}
        }
        Vec::new()
    }

    fn view(model: &Model) -> Vec<Window<Msg>> {
        fn screen<'a>(
            model: &'a Model,
            field: u64,
            denied: bool,
            notice: Option<&'a str>,
        ) -> Screen<'a> {
            Screen {
                size: SCREEN,
                metrics: model.metrics,
                clock: &model.clock,
                user: Some(&model.user),
                field: Field::Password {
                    generation: field,
                    enabled: true,
                },
                caps_lock: false,
                spinner: None,
                denied,
                notice,
                help: false,
                opacity: 1.0,
                shake: 0.0,
                hidden: false,
                power: None,
            }
        }
        let s = &model.screen;
        let lock = view::screen(
            &screen(model, s.field(), s.failure_outline(), s.notice()),
            LOCKED_TITLE,
            Msg::Lock,
        );
        let marker = [None, Some(READY), Some(PLAYED)][model.markers.min(2)];
        let markers = view::screen(&screen(model, 0, false, marker), MARKERS_TITLE, Msg::Marker);
        vec![
            view::full_screen(
                WindowKey(NodeKey::from("lock")),
                format!("{LOCK_APP_ID}.0"),
                LOCKED_TITLE,
                SCREEN,
                lock,
                None,
            ),
            view::full_screen(
                WindowKey(NodeKey::from("markers")),
                "cupertino-login.test-markers".into(),
                MARKERS_TITLE,
                SCREEN,
                markers,
                None,
            ),
        ]
    }
}

struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

fn budget() -> NonZeroU32 {
    NonZeroU32::new(10_000).unwrap()
}

/// The lock screen running headless, publishing to AT-SPI.
struct Run {
    driver: TestDriver<Locker>,
    lock: WindowId,
    markers: WindowId,
}

impl Run {
    fn start(adapter: Box<dyn AccessibilityAdapter>) -> Run {
        let mut fonts = FontIndex::new();
        fonts.add_font(Arc::new(BUILTIN_FONT.to_vec())).unwrap();
        let text = Arc::new(TextSystem::new(fonts, "Inter Variable"));
        let glyphs = Arc::clone(&text);
        let data = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/theme");
        let preferences = ThemePreferences {
            appearance: Appearance::Dark,
            reduce_transparency: false,
            reduce_motion: false,
        };
        let theme = ThemeTokens::cupertino(
            &cupertino_theme::Theme::from_dir(data).unwrap(),
            preferences,
        )
        .unwrap();
        let options = RuntimeOptions {
            theme: Arc::new(theme),
            text,
            icons: Arc::new(PlaceholderIcons::default()),
            direction: LayoutDirection::Ltr,
            node_limit: 1000,
            renderers: Box::new(move || {
                Box::new(CpuRenderer::new(
                    CpuConfig {
                        max_pixels: 1 << 22,
                        glyph_cache_bytes: 1 << 20,
                        image_cache_bytes: 1 << 20,
                        font_smoothing: true,
                    },
                    GlyphRaster::new(Arc::clone(&glyphs)),
                ))
            }),
            accessibility: Some(adapter),
            clock: Arc::new(ManualClock::new(Timestamp(0))),
            on_window_error: None,
        };
        let mut driver = TestDriver::new(
            Capabilities::default(),
            options,
            ManualClock::new(Timestamp(0)),
        );
        driver.run_until_idle(budget()).unwrap();
        let windows: HashMap<String, WindowId> = driver
            .runtime()
            .windows()
            .into_iter()
            .map(|(key, id)| (format!("{:?}", key.0), id))
            .collect();
        let find = |name: &str| {
            *windows
                .iter()
                .find(|(key, _)| key.contains(name))
                .unwrap_or_else(|| panic!("no {name} window in {windows:?}"))
                .1
        };
        let (lock, markers) = (find("lock"), find("markers"));
        for window in [markers, lock] {
            let active = WindowStates {
                activated: window == lock,
                ..WindowStates::default()
            };
            driver
                .window_system_mut()
                .configure(window, None, active)
                .unwrap();
        }
        let mut run = Run {
            driver,
            lock,
            markers,
        };
        // the adapter makes a window known to assistive technology as it learns its focus
        run.driver
            .window_system_mut()
            .enqueue_window(lock, WindowEvent::Focus(true))
            .unwrap();
        run.settle();
        run
    }

    /// Let the runtime do what is ready, the adapter's wake-ups included, and present the frames it asks for.
    fn settle(&mut self) {
        self.driver
            .window_system_mut()
            .enqueue(PlatformEvent::Wake)
            .unwrap();
        for _ in 0..16 {
            self.driver.run_until_idle(budget()).unwrap();
            let system = self.driver.window_system_mut();
            let mut asked = false;
            for window in [self.lock, self.markers] {
                if system.frame_requested(window).unwrap() {
                    system.frame_ready(window, Timestamp(0)).unwrap();
                    asked = true;
                }
            }
            if !asked {
                return;
            }
        }
        panic!("the windows keep asking for frames");
    }

    /// Press and release a key on `window`; a character's key types it.
    fn key(&mut self, window: WindowId, keysym: u32, text: Option<char>) {
        let code = match keysym {
            RETURN => 28,
            ESCAPE => 1,
            _ => 0,
        };
        for pressed in [true, false] {
            self.driver
                .window_system_mut()
                .enqueue_window(
                    window,
                    WindowEvent::Key(
                        KeyEvent {
                            code,
                            keysym: Keysym(keysym),
                            text: text.filter(|_| pressed),
                            pressed,
                            repeat: false,
                            mods: Mods::NONE,
                        },
                        Timestamp(0),
                    ),
                )
                .unwrap();
            self.settle();
        }
    }

    fn type_text(&mut self, text: &str) {
        for c in text.chars() {
            self.key(self.lock, c as u32, Some(c));
        }
    }

    fn marker(&mut self) {
        self.key(self.markers, ESCAPE, None);
    }

    /// The diagnostics a person could be shown: the Inspector dump of each window and the model.
    fn diagnostics(&self) {
        eprintln!("{DIAGNOSTICS}");
        let runtime = self.driver.runtime();
        for window in [self.lock, self.markers] {
            eprintln!(
                "{}",
                Inspector::default().dump(&runtime.snapshot(window).unwrap())
            );
        }
        eprintln!("{:?}", runtime.model());
    }
}

/// The lock screen: it answers its attempts with `outcomes`, publishes its tree once accessibility is active, and once
/// its input says go shows [`READY`], plays `part`, shows [`PLAYED`], writes its diagnostics, and keeps running until
/// its input closes.
fn lock_screen(outcomes: Vec<AuthOutcome>, part: impl FnOnce(&mut Run)) {
    *OUTCOMES.lock().unwrap() = outcomes;
    let adapter = AtspiAdapter::lock_screen(Waker::from(Arc::new(Unpark(std::thread::current()))));
    let mut run = Run::start(Box::new(adapter));
    let (said, input) = mpsc::channel();
    let main = std::thread::current();
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin().lock();
        let mut line = String::new();
        stdin.read_line(&mut line).unwrap();
        let _ = said.send(());
        main.unpark();
        stdin.read_to_end(&mut Vec::new()).unwrap();
        let _ = said.send(());
        main.unpark();
    });
    let wait = |run: &mut Run| loop {
        run.settle();
        if input.try_recv().is_ok() {
            return;
        }
        std::thread::park_timeout(Duration::from_millis(20));
    };
    wait(&mut run);
    run.marker();
    part(&mut run);
    run.marker();
    run.diagnostics();
    wait(&mut run);
}

struct Status;

#[zbus::interface(name = "org.a11y.Status")]
impl Status {
    #[zbus(property)]
    fn is_enabled(&self) -> bool {
        true
    }
}

struct Socket(Sender<String>);

#[zbus::interface(name = "org.a11y.atspi.Socket")]
impl Socket {
    fn embed(
        &self,
        plug: (String, OwnedObjectPath),
        #[zbus(connection)] conn: &zbus::Connection,
    ) -> (String, OwnedObjectPath) {
        let _ = self.0.send(plug.0);
        let registry = conn
            .unique_name()
            .map(|n| n.to_string())
            .unwrap_or_default();
        (registry, OwnedObjectPath::try_from(ROOT).unwrap())
    }
}

fn proxy<'a>(conn: &'a Connection, peer: &str, path: &str, interface: &'static str) -> Proxy<'a> {
    Proxy::new(conn, peer.to_owned(), path.to_owned(), interface).unwrap()
}

fn accessible<'a>(conn: &'a Connection, peer: &str, path: &str) -> Proxy<'a> {
    proxy(conn, peer, path, "org.a11y.atspi.Accessible")
}

fn children(conn: &Connection, peer: &str, path: &str) -> Vec<OwnedObjectPath> {
    let children: Vec<(String, OwnedObjectPath)> = accessible(conn, peer, path)
        .call("GetChildren", &())
        .unwrap_or_default();
    children.into_iter().map(|(_, path)| path).collect()
}

/// One node as assistive technology reads it: role, name, description and, for text, its whole text.
#[derive(Debug)]
struct NodeRead {
    path: String,
    role: u32,
    name: String,
    description: String,
    text: Option<String>,
}

/// Every node under `path`, depth first.
fn walk(conn: &Connection, peer: &str, path: &str, nodes: &mut Vec<NodeRead>) {
    let node = accessible(conn, peer, path);
    let text: zbus::Result<String> =
        proxy(conn, peer, path, "org.a11y.atspi.Text").call("GetText", &(0i32, -1i32));
    nodes.push(NodeRead {
        path: path.to_owned(),
        role: node.call("GetRole", &()).unwrap_or(0),
        name: node.get_property("Name").unwrap_or_default(),
        description: node.get_property("Description").unwrap_or_default(),
        text: text.ok(),
    });
    for child in children(conn, peer, path) {
        walk(conn, peer, child.as_str(), nodes);
    }
}

/// The text and politeness of an announcement.
fn announcement(message: &Message) -> Option<(String, i32)> {
    type Event = (String, i32, i32, OwnedValue, HashMap<String, OwnedValue>);
    let header = message.header();
    if header.interface()? != "org.a11y.atspi.Event.Object" || header.member()? != "Announcement" {
        return None;
    }
    let (_, politeness, _, said, _): Event = message.body().deserialize().unwrap();
    Some((String::try_from(said).unwrap(), politeness))
}

/// What the lock screen sent while it played its part: the messages between its two markers.
struct Part {
    sent: Vec<Message>,
    /// The object paths of the markers' announcements.
    markers: [String; 2],
}

impl Part {
    /// The signals of the part that are not about the marker window.
    fn signals(&self) -> Vec<&Message> {
        self.sent
            .iter()
            .filter(|m| m.message_type() == Type::Signal)
            .filter(|m| {
                let path = m.header().path().map(|p| p.to_string()).unwrap_or_default();
                !self.markers.contains(&path)
            })
            .collect()
    }
}

/// A lock screen started by test `name` in a copy of this binary, embedded in a fake registry, with every message it
/// sends from then on recorded and its standard error kept.
struct Locked {
    app: Child,
    stdin: ChildStdin,
    peer: String,
    sent: Arc<(Mutex<Vec<Message>>, Condvar)>,
    conn: Connection,
    _services: (Connection, Connection),
    _bus: PrivateBus,
}

impl Locked {
    fn start(name: &str) -> Option<Locked> {
        let bus = PrivateBus::start()?;
        let (embedded, embeds) = channel();
        let status = bus
            .blocking_builder()
            .name("org.a11y.Bus")
            .unwrap()
            .serve_at("/org/a11y/bus", Status)
            .unwrap()
            .build()
            .unwrap();
        let registry = bus
            .blocking_builder()
            .name("org.a11y.atspi.Registry")
            .unwrap()
            .serve_at(ROOT, Socket(embedded))
            .unwrap()
            .build()
            .unwrap();
        let mut app = Process::new(std::env::current_exe().unwrap())
            .args([name, "--exact", "--nocapture"])
            .env(LOCK_SCREEN, "1")
            .env("DBUS_SESSION_BUS_ADDRESS", &bus.address)
            .env("AT_SPI_BUS_ADDRESS", &bus.address)
            .env_remove("CUPERTINO_ASSET_DIR")
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let peer = embeds.recv_timeout(PATIENCE).expect("the root embedded");
        let monitor = bus.connect();
        // zbus drops what arrives before a stream subscribes: subscribe before the lock screen can send
        let messages = MessageIterator::from(&monitor);
        monitor
            .call_method(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus.Monitoring"),
                "BecomeMonitor",
                &(vec![format!("sender='{peer}'")], 0u32),
            )
            .unwrap();
        let sent = Arc::new((Mutex::new(Vec::new()), Condvar::new()));
        let record = Arc::clone(&sent);
        std::thread::spawn(move || {
            for message in messages.flatten() {
                record.0.lock().unwrap().push(message);
                record.1.notify_all();
            }
        });
        let stdin = app.stdin.take().unwrap();
        Some(Locked {
            app,
            stdin,
            peer,
            sent,
            conn: bus.connect(),
            _services: (status, registry),
            _bus: bus,
        })
    }

    /// The windows of the lock screen by name, once both are there.
    fn windows(&self) -> HashMap<String, String> {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let windows: HashMap<String, String> = children(&self.conn, &self.peer, ROOT)
                .into_iter()
                .map(|path| {
                    let name: String = accessible(&self.conn, &self.peer, path.as_str())
                        .get_property("Name")
                        .unwrap_or_default();
                    (name, path.to_string())
                })
                .collect();
            if windows.contains_key(LOCKED_TITLE) && windows.contains_key(MARKERS_TITLE) {
                return windows;
            }
            assert!(Instant::now() < deadline, "the lock screen never appeared");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Tells the lock screen to play its part and waits until it has announced both markers.
    fn play(&mut self) -> Part {
        writeln!(self.stdin).unwrap();
        self.stdin.flush().unwrap();
        let marker = |sent: &[Message], text: &str| {
            sent.iter()
                .position(|m| announcement(m).is_some_and(|(said, _)| said == text))
        };
        let deadline = Instant::now() + PATIENCE;
        let (lock, recorded) = &*self.sent;
        let mut sent = lock.lock().unwrap();
        loop {
            if let (Some(ready), Some(played)) = (marker(&sent, READY), marker(&sent, PLAYED)) {
                let path = |i: usize| sent[i].header().path().unwrap().to_string();
                return Part {
                    markers: [path(ready), path(played)],
                    sent: sent[ready + 1..played].to_vec(),
                };
            }
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "the lock screen never played its part");
            sent = recorded.wait_timeout(sent, left).unwrap().0;
        }
    }

    /// Everything the lock screen sent until now.
    fn sent(&self) -> Vec<Message> {
        self.sent.0.lock().unwrap().clone()
    }

    /// Closes the lock screen's input and returns what it wrote to standard error.
    fn finish(mut self) -> String {
        drop(self.stdin);
        let mut stderr = String::new();
        self.app
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut stderr)
            .unwrap();
        assert!(
            self.app.wait().unwrap().success(),
            "the lock screen failed: {stderr}"
        );
        stderr
    }
}

#[test]
fn neither_the_tree_nor_the_bus_nor_the_diagnostics_hold_a_password_being_typed() {
    const NAME: &str =
        "neither_the_tree_nor_the_bus_nor_the_diagnostics_hold_a_password_being_typed";
    if std::env::var_os(LOCK_SCREEN).is_some() {
        return lock_screen(Vec::new(), |run| run.type_text(SECRET));
    }
    let Some(mut locked) = Locked::start(NAME) else {
        return;
    };
    let windows = locked.windows();
    let lock_window = windows[LOCKED_TITLE].clone();
    let part = locked.play();

    // the whole tree, read while the password sits in the field
    let mut nodes = Vec::new();
    walk(&locked.conn, &locked.peer, &lock_window, &mut nodes);
    let field = nodes
        .iter()
        .find(|node| node.role == PASSWORD_TEXT)
        .unwrap_or_else(|| panic!("no password text in {nodes:#?}"));
    assert_eq!(field.name, PASSWORD_LABEL);
    assert!(
        field.text.as_deref().is_none_or(str::is_empty),
        "the password field reads {:?}",
        field.text
    );
    let hold = |text: &str| text.contains(SECRET);
    for node in &nodes {
        assert!(
            !hold(&node.name)
                && !hold(&node.description)
                && !node.text.as_deref().is_some_and(hold),
            "a node holds the password: {node:?}"
        );
    }
    let sent = locked.sent();
    let stderr = locked.finish();

    let typed: Vec<_> = part
        .signals()
        .iter()
        .map(|m| {
            let header = m.header();
            (
                header.path().map(|p| p.to_string()),
                header.member().map(|m| m.to_string()),
            )
        })
        .filter(|(_, member)| {
            // typing may move the caret's state or the field's emptiness, never a text or value change
            member
                .as_deref()
                .is_some_and(|m| m.contains("TextChanged") || m.contains("PropertyChange"))
        })
        .filter(|(path, _)| path.as_deref() == Some(field.path.as_str()))
        .collect();
    assert!(
        typed.is_empty(),
        "typing sent text to assistive technology: {typed:?}"
    );
    let holds = |text: &[u8]| {
        sent.iter()
            .any(|m| m.data().windows(text.len()).any(|w| w == text))
    };
    assert!(
        holds(PASSWORD_LABEL.as_bytes()),
        "the monitor reads what the lock screen sends"
    );
    let keys: Vec<_> = sent
        .iter()
        .map(|m| {
            let header = m.header();
            (
                header
                    .interface()
                    .map(|i| i.to_string())
                    .unwrap_or_default(),
                header.member().map(|m| m.to_string()).unwrap_or_default(),
            )
        })
        .filter(|(interface, member)| {
            interface.contains("DeviceEventController") || member.contains("Key")
        })
        .collect();
    assert!(
        keys.is_empty(),
        "keyboard events went to the registry: {keys:?}"
    );
    assert!(!holds(SECRET.as_bytes()), "a message held the password");

    let (_, diagnostics) = stderr
        .split_once(DIAGNOSTICS)
        .unwrap_or_else(|| panic!("the lock screen wrote no diagnostics: {stderr}"));
    assert!(
        diagnostics.contains("SecureField") || diagnostics.contains(PASSWORD_LABEL),
        "the diagnostics show the field: {diagnostics}"
    );
    assert!(
        stderr.contains("Lock(Edited)"),
        "the diagnostics list the messages: {stderr}"
    );
    assert!(
        !stderr.contains(SECRET),
        "the diagnostics hold the password"
    );
}

#[test]
fn every_answer_to_an_attempt_is_announced_once_and_assertively() {
    const NAME: &str = "every_answer_to_an_attempt_is_announced_once_and_assertively";
    const LOCKED_OUT: &str = "The account is locked.";
    if std::env::var_os(LOCK_SCREEN).is_some() {
        let outcomes = vec![
            AuthOutcome::Denied,
            AuthOutcome::Denied,
            AuthOutcome::Error(LOCKED_OUT.into()),
        ];
        return lock_screen(outcomes, |run| {
            for _ in 0..3 {
                run.type_text("x");
                run.key(run.lock, RETURN, None);
                // frames with nothing new to say
                run.settle();
                run.settle();
            }
        });
    }
    let Some(mut locked) = Locked::start(NAME) else {
        return;
    };
    locked.windows();
    let part = locked.play();
    let stderr = locked.finish();

    let announced: Vec<_> = part.sent.iter().filter_map(announcement).collect();
    assert_eq!(
        announced,
        [
            (DENIED.to_owned(), ASSERTIVE),
            (DENIED.to_owned(), ASSERTIVE),
            (LOCKED_OUT.to_owned(), ASSERTIVE),
        ],
        "{stderr}"
    );
}
