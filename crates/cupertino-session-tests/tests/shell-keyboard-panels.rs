//! The input-source switcher and the accent picker drawn by the on-screen displays (\`cupertino-osd\`, W6.12), against
//! the real server, headless, with two input sources (\`us\` and \`de\`) and an accent table, on a session bus and in
//! a home of the test's own. The server's supervisor starts this test binary as \`cupertino-shell\`: with [\`host::MARK\`]
//! among its arguments it runs the host with the real on-screen displays (\`cupertino_shell::OSD\`), with
//! [\`host::STAND_IN\`] a stand-in \`osd\` that shows a panel for the switch and then hangs.
//! - The chord shows the switcher's panel; the release switches the source and the panel fades out, also when the
//!   release comes before the panel's first frame. A release while the displays restart switches at once, and a new
//!   instance during a switch shows its list. A hung displays component delays no switch and its panel goes.
//! - Without the displays the chord still switches and a held letter repeats in the application.
//! - A held letter in a Kit text field opens the picker at the caret: a digit inserts its candidate, Escape puts the
//!   letter back, a click on a candidate inserts it, a press off the picker closes it and reaches the window, and the
//!   end of the displays closes it with the letter back.
//! - Locking ends the switch and the picker.

use std::io::{BufRead, BufReader, Write};
use std::os::fd::IntoRawFd;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command as Process, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use calloop::EventLoop;
use cupertino_assets::Assets;
use cupertino_kit::controls::{TextChange, TextField};
use cupertino_kit::core::{
    Application, Change, Command, CommandRegistry, Element, Event as UiEvent, EventCx, Key,
    MeasureCx, PaintCx, Propagation, RendererClass, SemanticsCx, Widget, Window, WindowBackground,
    WindowKey,
};
use cupertino_kit::geometry::{Insets, Rect, Size};
use cupertino_kit::layout::{
    Align, Axis, Constraints, ContainerKind, Justify, LayoutSpec, Measured, SizeSpec,
};
use cupertino_kit::platform::{DecorationMode, PointerEvent, WindowSpec};
use cupertino_kit::render::{Paint, PaintOp, Shape};
use cupertino_kit::text::ContentPurpose;
use cupertino_kit::theme::Color;
use cupertino_osd::panels::AccentsPanel;
use cupertino_server::authorization::ShellRole;
use cupertino_server::backend::headless;
use cupertino_server::kbd::SourceSpec;
use cupertino_server::shell_connection::shell_process_connection;
use cupertino_server::state::{insert_client, ClientState};
use cupertino_server::supervisor::HostProgram;
use cupertino_server::{Config, Cupertino};
use wayland_client::protocol::wl_registry;
use wayland_protocols::ext::session_lock::v1::client::{
    ext_session_lock_manager_v1::ExtSessionLockManagerV1, ext_session_lock_v1::ExtSessionLockV1,
};
use wayland_server::Display;
use zbus::blocking::Connection as BusConnection;

/// The offscreen output's size in pixels, at scale 1.
const OUTPUT_SIZE: (i32, i32) = (800, 600);
/// How long a test waits for the server, its host and the application to do what it expects: a debug build drawing
/// with the software rasteriser on a loaded builder.
const WAIT: Duration = Duration::from_secs(60);
/// How long one turn of the server's loop waits for an event at most.
const TURN: Duration = Duration::from_millis(5);
/// The candidates of \`e\` in the test's accent table.
const ACCENT_TABLE: &str = r#"{"default":{"e":{"direction":"right","strings":["è","é","ê"]}}}"#;
const CANDIDATES: [&str; 3] = ["è", "é", "ê"];
/// The evdev codes of the keys the tests press.
const KEY_ESC: u32 = 1;
const KEY_2: u32 = 3;
const KEY_TAB: u32 = 15;
const KEY_E: u32 = 18;
const KEY_CTRL: u32 = 29;
const KEY_SPACE: u32 = 57;
/// The main pointer button.
const BTN_LEFT: u32 = 0x110;
/// The middle of the output, where the switcher's panel shows: smaller than the panel, so only the panel covers it.
const SOURCES_AREA: Area = Area::new(OUTPUT_SIZE.0 / 2 - 110, OUTPUT_SIZE.1 / 2 - 35, 220, 70);
/// How many pixels of an area must change for a panel to show there, and how few may for none to.
const SHOWN: usize = 1500;
const GONE: usize = 60;
/// The stand-in's panel: its size and its colour.
const STAND_IN_SIZE: Size = Size::new(240.0, 90.0);
const GREEN: Color = Color::new(0.0, 1.0, 0.0, 1.0);
/// How long the stand-in shows its panel before its loop hangs.
const STAND_IN_HANGS_AFTER: Duration = Duration::from_millis(300);
/// The text window's size, in points.
const TEXT_WINDOW: Size = Size::new(360.0, 240.0);
/// The space the text window keeps around its field, in points.
const TEXT_INSET: f64 = 20.0;

/// A rectangle of the output, in pixels (points at scale 1).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Area {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

impl Area {
    const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    fn of(rect: Rect) -> Self {
        Self::new(
            rect.x().round() as i32,
            rect.y().round() as i32,
            rect.width().round() as i32,
            rect.height().round() as i32,
        )
    }
}

/// What the hosts report, one line each: a file under the target's temporary directory.
static REPORT: OnceLock<PathBuf> = OnceLock::new();

fn report(line: &str) {
    if let Some(path) = REPORT.get() {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .expect("the report opens");
        file.write_all(format!("{line}\n").as_bytes())
            .expect("the report is written");
    }
}

/// The host programs of this test: \`cupertino-shell\`'s own (\`cupertino_shell::run\`) with the real on-screen
/// displays, or with a stand-in for them that hangs.
mod host {
    use std::path::PathBuf;

    use cupertino_kit_shell::Entry;

    use super::stand_in::STAND_IN_OSD;

    /// The argument that makes this test binary the host of the real on-screen displays.
    pub const MARK: &str = "cupertino-keyboard-panels-host";
    /// The argument that makes it the host of the stand-in.
    pub const STAND_IN: &str = "cupertino-keyboard-panels-stand-in";

    static ENTRIES: [&Entry; 1] = [&cupertino_shell::OSD];
    static STAND_IN_ENTRIES: [&Entry; 1] = [&STAND_IN_OSD];

    /// The mark, the report file and the host's own arguments that follow the mark, when this process is a host.
    pub fn arguments() -> Option<(&'static str, PathBuf, Vec<String>)> {
        let args: Vec<String> = std::env::args().collect();
        let (at, mark) = args.iter().enumerate().find_map(|(i, a)| {
            [MARK, STAND_IN]
                .into_iter()
                .find(|m| a == m)
                .map(|m| (i, m))
        })?;
        let report = PathBuf::from(args.get(at + 1)?);
        Some((mark, report, args[at + 2..].to_vec()))
    }

    /// Be the host named by \`mark\`.
    pub fn run(mark: &str, report: PathBuf, args: Vec<String>) -> ! {
        let _ = super::REPORT.set(report);
        if mark == STAND_IN {
            cupertino_shell::run(&STAND_IN_ENTRIES, args)
        } else {
            cupertino_shell::run(&ENTRIES, args)
        }
    }
}

/// The host itself, when the supervisor started this binary with a mark of [\`host\`]; a test run does nothing here.
#[test]
fn host_program() {
    if let Some((mark, report, args)) = host::arguments() {
        host::run(mark, report, args);
    }
}

/// A rectangle of one colour.
struct Block {
    size: Size,
    color: Color,
}

impl<M: 'static> Widget<M> for Block {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.size.width),
            height: SizeSpec::Fixed(self.size.height),
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: self.size,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &UiEvent, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        cx.push(PaintOp::Fill {
            shape: Shape::Rect(Rect::new(0.0, 0.0, self.size.width, self.size.height)),
            paint: Paint::Solid(self.color),
        });
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// The stand-in for the on-screen displays: given the switch's list it shows a green panel with the switcher's
/// namespace and, [\`STAND_IN_HANGS_AFTER\`] later, its loop hangs and never returns to its wait.
mod stand_in {
    use cupertino_kit::core::{
        Application, Command, CommandRegistry, Element, Key, RendererClass, Window,
        WindowBackground, WindowKey,
    };
    use cupertino_kit::platform::{DecorationMode, WindowSpec};
    use cupertino_kit_shell::keyboard_panels::{Update, SOURCES_NAMESPACE};
    use cupertino_kit_shell::{
        Anchor, Component, ComponentName, Entry, Event, Keyboard, Layer, LayerSpec, Margin,
        Program, Role, Surface,
    };

    use super::{report, Block, GREEN, STAND_IN_HANGS_AFTER, STAND_IN_SIZE};

    pub struct StandIn;

    #[derive(Clone)]
    pub enum Told {
        Sources,
        Hang,
    }

    impl Application for StandIn {
        /// Whether its panel shows.
        type Model = bool;
        type Message = Told;
        const RENDERER: RendererClass = RendererClass::Cpu;

        fn init() -> (bool, Vec<Command<Told>>) {
            report("stand-in started");
            (false, Vec::new())
        }

        fn update(shown: &mut bool, told: Told) -> Vec<Command<Told>> {
            match told {
                Told::Sources if !*shown => {
                    report("stand-in told the sources");
                    *shown = true;
                    vec![Command::After {
                        delay: STAND_IN_HANGS_AFTER,
                        message: Told::Hang,
                    }]
                }
                Told::Sources => Vec::new(),
                Told::Hang => {
                    report("stand-in hangs");
                    loop {
                        std::thread::park();
                    }
                }
            }
        }

        fn view(shown: &bool) -> Vec<Window<Told>> {
            if !*shown {
                return Vec::new();
            }
            vec![Window {
                key: WindowKey(Key::from("panel")),
                spec: WindowSpec {
                    title: "panel".to_owned(),
                    app_id: "org.cupertino.test.stand-in".to_owned(),
                    size: STAND_IN_SIZE,
                    min_size: None,
                    max_size: None,
                    decorations: DecorationMode::None,
                    foreign_parent: None,
                    input_region: Some(Vec::new()),
                },
                shortcuts: CommandRegistry::new(),
                on_close: None,
                background: WindowBackground::None,
                content: Element::new(Block {
                    size: STAND_IN_SIZE,
                    color: GREEN,
                }),
                popup: None,
                menus: None,
            }]
        }
    }

    impl Program for StandIn {
        fn surface(_: &bool, _: &WindowKey) -> Surface {
            Surface {
                role: Role::Layer(LayerSpec {
                    namespace: SOURCES_NAMESPACE,
                    output: None,
                    layer: Layer::Overlay,
                    anchor: Anchor::default(),
                    size: STAND_IN_SIZE,
                    margin: Margin::default(),
                    exclusive_zone: -1,
                    keyboard: Keyboard::None,
                }),
                persistent: None,
                holes: Vec::new(),
            }
        }

        fn event(event: Event) -> Option<Told> {
            match event {
                Event::KeyboardPanels(Update::Sources { .. }) => Some(Told::Sources),
                _ => None,
            }
        }
    }

    impl Component for StandIn {
        const NAME: ComponentName = ComponentName::OSD;
    }

    pub static STAND_IN_OSD: Entry = Entry::of::<StandIn>();
}

/// A session bus of the test's own, whose address the hosts inherit, and the address.
struct Bus(Child, String);

/// A session bus that starts no service: nothing a component calls reaches a program of the machine.
const BUS_CONFIG: &str = r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:tmpdir=/tmp</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#;

impl Bus {
    fn start() -> Bus {
        let config = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "shell-keyboard-panels-bus-{}.conf",
            std::process::id()
        ));
        std::fs::write(&config, BUS_CONFIG).expect("the bus's configuration");
        let mut daemon = Process::new("dbus-daemon")
            .arg(format!("--config-file={}", config.display()))
            .args(["--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .spawn()
            .expect("dbus-daemon runs");
        let mut address = String::new();
        BufReader::new(daemon.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        assert!(!address.trim().is_empty(), "dbus-daemon gives its address");
        let address = address.trim().to_owned();
        std::env::set_var("DBUS_SESSION_BUS_ADDRESS", &address);
        Bus(daemon, address)
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A directory of the test's own, new and empty, of mode 0700.
fn private_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "shell-keyboard-panels-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .expect("a private directory");
    dir
}

/// A home, settings, data, state, cache and runtime directories of the test's own: nothing of the person's is read.
fn private_home(name: &str) -> PathBuf {
    let home = private_dir(&format!("{name}-home"));
    for (variable, dir) in [
        ("XDG_CONFIG_HOME", ".config"),
        ("XDG_DATA_HOME", ".local/share"),
        ("XDG_STATE_HOME", ".local/state"),
        ("XDG_CACHE_HOME", ".cache"),
    ] {
        let dir = home.join(dir);
        std::fs::create_dir_all(&dir).expect("a folder of the home");
        std::env::set_var(variable, dir);
    }
    std::env::set_var("HOME", &home);
    std::env::set_var("XDG_RUNTIME_DIR", private_dir(&format!("{name}-runtime")));
    home
}

/// The server with its session bus, drawing one offscreen output, its shell \`components\` run by this test binary as
/// the host of \`mark\`.
struct Rig {
    event_loop: EventLoop<'static, Cupertino>,
    state: Cupertino,
    report: PathBuf,
    _bus: Bus,
}

impl Rig {
    fn new(name: &str, mark: &str, components: &[&str]) -> Self {
        let home = private_home(name);
        let bus = Bus::start();
        // the software rasteriser, as the server's program tests run the headless server
        std::env::set_var("LIBGL_ALWAYS_SOFTWARE", "1");
        let report = home.join("hosts.report");
        let _ = REPORT.set(report.clone());
        let table = home.join("accents.json");
        std::fs::write(&table, ACCENT_TABLE).expect("the accent table");

        let event_loop: EventLoop<'static, Cupertino> = EventLoop::try_new().unwrap();
        let display: Display<Cupertino> = Display::new().unwrap();
        let mut config = Config {
            shell_components: components.iter().map(|c| c.to_string()).collect(),
            ..Config::default()
        };
        config.keyboard.sources = ["us", "de"]
            .map(|s| SourceSpec::parse(s).expect("an xkb layout"))
            .to_vec();
        config.keyboard.accent.table = Some(table);
        let mut state = Cupertino::new(
            display,
            event_loop.handle(),
            event_loop.get_signal(),
            config,
            false,
            &Assets::builtin(),
        )
        .expect("the server starts");
        headless::init(&mut state, OUTPUT_SIZE, 1.0).expect("an offscreen renderer");
        assert!(
            state.connect_input_method(),
            "the server's input method connects"
        );
        cupertino_server::quit::start_session_service_on(&mut state, Some(bus.1.clone()));
        // `host_program` by its name in the test binary, which starts with this file's
        let file = module_path!().split_once("::").map_or("", |(_, file)| file);
        let test = format!("{file}::host_program");
        let mut args: Vec<String> = ["--exact", &test, "--nocapture", "--", mark]
            .map(String::from)
            .to_vec();
        args.push(report.display().to_string());
        state.supervisor.set_host_program(Some(HostProgram {
            path: std::env::current_exe().unwrap(),
            args,
            checked: false,
        }));
        state.apply_shell_plan();
        state.supervise_shell(Instant::now());
        Self {
            event_loop,
            state,
            report,
            _bus: bus,
        }
    }

    /// One turn of the server's loop.
    fn turn(&mut self) {
        headless::step(&mut self.state, &mut self.event_loop, TURN).expect("the loop turns");
    }

    /// Turn until \`done\` holds, which is asked before every turn.
    fn until(&mut self, what: &str, mut done: impl FnMut(&mut Self) -> bool) {
        let deadline = Instant::now() + WAIT;
        while !done(self) {
            assert!(
                Instant::now() < deadline,
                "timed out waiting until {what} (instance of osd {}, {:?}, panels bound {}, switch open {}); the hosts reported:\n{}",
                self.state.hosts.component_instance("osd"),
                self.state.hosts.component_liveness("osd"),
                self.state.overlays.panels.bound(),
                self.state.overlays.inputs.switch_open(),
                std::fs::read_to_string(&self.report).unwrap_or_default()
            );
            self.turn();
        }
    }

    /// Turn the loop for \`time\`: what the server and its components do meanwhile, nothing should change.
    fn run_for(&mut self, time: Duration) {
        let end = Instant::now() + time;
        while Instant::now() < end {
            self.turn();
        }
    }

    fn reported(&self, line: &str) -> bool {
        std::fs::read_to_string(&self.report)
            .unwrap_or_default()
            .lines()
            .any(|l| l == line)
    }

    /// The output as it shows now, without the pointer.
    fn image(&mut self) -> image::RgbaImage {
        let mut backend = self.state.backend.take().unwrap();
        let image = self
            .state
            .draw_output_image(backend.renderer(), 0, false, false)
            .unwrap();
        self.state.backend = Some(backend);
        image
    }

    /// How many pixels of \`area\` differ from \`before\` by more than a step of rounding.
    fn changed(&mut self, before: &image::RgbaImage, area: Area) -> usize {
        let now = self.image();
        let mut n = 0;
        for y in area.y.max(0)..(area.y + area.h).min(OUTPUT_SIZE.1) {
            for x in area.x.max(0)..(area.x + area.w).min(OUTPUT_SIZE.0) {
                let (a, b) = (
                    now.get_pixel(x as u32, y as u32),
                    before.get_pixel(x as u32, y as u32),
                );
                if (0..3).any(|c| a[c].abs_diff(b[c]) > 2) {
                    n += 1;
                }
            }
        }
        n
    }

    /// How many pixels of \`area\` show the stand-in's green.
    fn green(&mut self, area: Area) -> usize {
        let image = self.image();
        let mut n = 0;
        for y in area.y..area.y + area.h {
            for x in area.x..area.x + area.w {
                let p = image.get_pixel(x as u32, y as u32);
                if p[1] > 200 && p[0] < 60 && p[2] < 60 {
                    n += 1;
                }
            }
        }
        n
    }

    /// Turn until the on-screen displays hold their panels' object.
    fn until_panels(&mut self) {
        self.until("the on-screen displays bind their panels", |rig| {
            rig.state.overlays.panels.bound()
        });
    }

    /// A key going down or up, as a device delivers it.
    fn key(&mut self, evdev: u32, pressed: bool) {
        self.state.scripted_evdev_key(evdev, pressed);
    }

    /// Control held, the space bar pressed and let go: the switch opens.
    fn chord(&mut self) {
        self.key(KEY_CTRL, true);
        self.key(KEY_SPACE, true);
        self.key(KEY_SPACE, false);
    }

    /// The current input source.
    fn source(&self) -> Option<String> {
        self.state.overlays.inputs.current().map(str::to_owned)
    }

    /// Crash the on-screen displays' instance at \`at\` (\`dispatch\`, \`paint\` or \`helper\`), through
    /// \`org.cupertino.ShellComponentDebug1\` on its own bus connection, and turn until its panels' object went.
    fn crash_osd(&mut self, at: &'static str) {
        let session = BusConnection::session().expect("the test's bus");
        let owner = {
            let dbus = zbus::blocking::fdo::DBusProxy::new(&session).unwrap();
            let mut owner = None;
            self.until("the on-screen displays own their name", |_| {
                owner = dbus
                    .get_name_owner(
                        zbus::names::BusName::try_from(cupertino_osd::control::BUS_NAME).unwrap(),
                    )
                    .ok()
                    .map(|o| o.to_string());
                owner.is_some()
            });
            owner.unwrap()
        };
        let crash = std::thread::spawn(move || {
            let _ = session.call_method(
                Some(owner.as_str()),
                "/org/cupertino/ShellComponentDebug1",
                Some("org.cupertino.ShellComponentDebug1"),
                "Crash",
                &(at,),
            );
        });
        self.until("the crashed instance's panels went", |rig| {
            crash.is_finished() && !rig.state.overlays.panels.bound()
        });
    }
}

/// What the text field holds now, as the application last heard it.
static FIELD: Mutex<String> = Mutex::new(String::new());
/// The presses the text window had off its field.
static PRESSES: AtomicU32 = AtomicU32::new(0);

fn field() -> String {
    FIELD.lock().unwrap().clone()
}

/// An ordinary Kit application: one window with a text field at its top that takes the focus when it opens.
struct TextApp;

#[derive(Clone)]
enum Typed {
    Changed(TextChange),
    Pressed,
}

/// The field's text and the revision of it the application accepted.
struct Text {
    value: String,
    revision: u64,
}

/// The window's content: its field at the top, and the rest of the window, which counts the presses on it.
struct Pad {
    children: Vec<Element<Typed>>,
}

impl Widget<Typed> for Pad {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(TEXT_WINDOW.width),
            height: SizeSpec::Fixed(TEXT_WINDOW.height),
            insets: Insets {
                top: TEXT_INSET,
                left: TEXT_INSET,
                bottom: TEXT_INSET,
                right: TEXT_INSET,
            },
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Vertical,
                spacing: 0.0,
                align: Align::Stretch,
                justify: Justify::Start,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<Typed>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: TEXT_WINDOW,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &UiEvent, cx: &mut EventCx<'_, Typed>) -> Propagation {
        if let UiEvent::Pointer(PointerEvent::Button { pressed: true, .. }, _) = event {
            cx.emit(Typed::Pressed);
        }
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

impl Application for TextApp {
    type Model = Text;
    type Message = Typed;
    const RENDERER: RendererClass = RendererClass::Cpu;

    fn init() -> (Text, Vec<Command<Typed>>) {
        (
            Text {
                value: String::new(),
                revision: 0,
            },
            Vec::new(),
        )
    }

    fn update(text: &mut Text, typed: Typed) -> Vec<Command<Typed>> {
        match typed {
            Typed::Changed(change) => {
                text.value = change.text.clone();
                text.revision = change.revision;
                *FIELD.lock().unwrap() = change.text;
            }
            Typed::Pressed => {
                PRESSES.fetch_add(1, Ordering::SeqCst);
            }
        }
        Vec::new()
    }

    fn view(text: &Text) -> Vec<Window<Typed>> {
        vec![Window {
            key: WindowKey(Key::from("text")),
            spec: WindowSpec {
                title: "Text".to_owned(),
                app_id: "org.cupertino.test.text".to_owned(),
                size: TEXT_WINDOW,
                min_size: None,
                max_size: None,
                decorations: DecorationMode::None,
                foreign_parent: None,
                input_region: None,
            },
            shortcuts: CommandRegistry::new(),
            on_close: None,
            background: WindowBackground::Theme,
            content: Element::new(Pad {
                children: vec![Element::new(TextField {
                    label: "Text".to_owned(),
                    placeholder: String::new(),
                    value: text.value.clone(),
                    revision: text.revision,
                    purpose: ContentPurpose::Normal,
                    enabled: true,
                    on_change: Some(Box::new(Typed::Changed)),
                    on_submit: None,
                    selection: None,
                    initial: true,
                })],
            }),
            popup: None,
            menus: None,
        }]
    }
}

/// Run [\`TextApp\`] on a connection of an ordinary client and turn until its field has the keyboard and the server's
/// input method hears it, with the text cursor known.
fn text_window(rig: &mut Rig) -> std::thread::JoinHandle<()> {
    let (ours, theirs) = UnixStream::pair().unwrap();
    insert_client(&rig.state.display_handle, ours, ClientState::default()).unwrap();
    std::env::set_var("WAYLAND_SOCKET", theirs.into_raw_fd().to_string());
    let app = std::thread::spawn(|| {
        let _ = cupertino_kit::app::run::<TextApp>();
    });
    rig.until("the field's text input is active with its caret", |rig| {
        rig.state.overlays.accent.bridge && rig.state.text_cursor_global().is_some()
    });
    std::env::remove_var("WAYLAND_SOCKET");
    // the window's opening runs its course: the picture holds still but for the caret's blink
    let mut last = rig.image();
    rig.until("the window's opening ends", |rig| {
        let now = rig.image();
        let moved = now
            .pixels()
            .zip(last.pixels())
            .filter(|(a, b)| (0..3).any(|c| a[c].abs_diff(b[c]) > 2))
            .count();
        last = now;
        moved <= GONE
    });
    app
}

/// Hold \`e\` down until the picker opens at its first repeat, then let it go.
fn hold_e(rig: &mut Rig) {
    rig.key(KEY_E, true);
    rig.until("the held letter opens the picker", |rig| {
        rig.state.overlays.accent.is_open()
    });
    rig.key(KEY_E, false);
}

/// Where the on-screen displays lay the open picker out on the output, by the server's text cursor.
fn picker_layout(rig: &Rig) -> cupertino_osd::panels::PickerLayout {
    let caret = rig.state.text_cursor_global().expect("the text cursor");
    let shell = &rig.state.config.shell;
    AccentsPanel {
        serial: 0,
        output: None,
        anchor: Rect::new(caret.x(), caret.y(), caret.width(), caret.height()),
        top: if shell.show_menu_bar {
            shell.menu_bar_height
        } else {
            0.0
        },
        candidates: CANDIDATES.map(str::to_owned).to_vec(),
        focused: None,
    }
    .layout(Size::new(
        f64::from(OUTPUT_SIZE.0),
        f64::from(OUTPUT_SIZE.1),
    ))
}

/// Turn until the picker's panel shows where the displays lay it out, and return its layout.
fn until_picker_shows(
    rig: &mut Rig,
    before: &image::RgbaImage,
) -> cupertino_osd::panels::PickerLayout {
    let layout = picker_layout(rig);
    let area = Area::of(layout.rect);
    let enough = (area.w * area.h) as usize / 4;
    // shown, and drawn to the end: the same in two pictures running
    let mut last = before.clone();
    rig.until("the picker's panel shows at the caret", |rig| {
        let now = rig.image();
        let shown = rig.changed(before, area) >= enough && rig.changed(&last, area) == 0;
        last = now;
        shown
    });
    layout
}

/// The global point at the middle of candidate \`index\` of the picker laid out as \`layout\`.
fn candidate(layout: &cupertino_osd::panels::PickerLayout, index: usize) -> (f64, f64) {
    let cell = layout.cells[index];
    (
        layout.rect.x() + cell.x() + cell.width() / 2.0,
        layout.rect.y() + cell.y() + cell.height() / 2.0,
    )
}

/// The session lock of a connection that holds the \`session_lock\` role.
#[derive(Default)]
struct Locker {
    manager: Option<ExtSessionLockManagerV1>,
}

impl wayland_client::Dispatch<wl_registry::WlRegistry, ()> for Locker {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &wayland_client::Connection,
        qh: &wayland_client::QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name, interface, ..
        } = event
        {
            if interface == "ext_session_lock_manager_v1" {
                state.manager = Some(registry.bind(name, 1, qh, ()));
            }
        }
    }
}

wayland_client::delegate_noop!(Locker: ignore ExtSessionLockManagerV1);
wayland_client::delegate_noop!(Locker: ignore ExtSessionLockV1);

/// Lock the session as a lock screen does, and turn until the server holds the lock.
fn lock(rig: &mut Rig) -> (wayland_client::Connection, ExtSessionLockV1) {
    let stream = shell_process_connection(&rig.state.display_handle, &[ShellRole::SessionLock])
        .expect("a lock screen's connection");
    let conn = wayland_client::Connection::from_socket(stream).unwrap();
    let mut queue = conn.new_event_queue();
    let _registry = conn.display().get_registry(&queue.handle(), ());
    let mut locker = Locker::default();
    rig.until("the lock manager is offered", |rig| {
        let _ = conn.flush();
        if let Some(guard) = conn.prepare_read() {
            let _ = guard.read();
        }
        let _ = queue.dispatch_pending(&mut locker);
        rig.state.display_handle.flush_clients().ok();
        locker.manager.is_some()
    });
    let lock = locker.manager.as_ref().unwrap().lock(&queue.handle(), ());
    let _ = conn.flush();
    rig.until("the session is locked", |rig| {
        rig.state.session_lock.is_active()
    });
    (conn, lock)
}

#[test]
fn the_chord_shows_the_sources_panel_and_the_release_switches_the_source() {
    let mut rig = Rig::new("chord", host::MARK, &["osd"]);
    rig.until_panels();
    let bare = rig.image();
    assert_eq!(rig.source().as_deref(), Some("us"));

    rig.chord();
    rig.until("the switcher's panel shows", |rig| {
        rig.changed(&bare, SOURCES_AREA) >= SHOWN
    });
    assert_eq!(
        rig.source().as_deref(),
        Some("us"),
        "nothing switches while Control is held"
    );
    rig.key(KEY_CTRL, false);
    assert_eq!(
        rig.source().as_deref(),
        Some("de"),
        "the release switches at once"
    );
    rig.until("the panel fades out", |rig| {
        rig.changed(&bare, SOURCES_AREA) <= GONE
    });
}

#[test]
fn a_quick_tap_switches_and_still_shows_the_choice_fading() {
    let mut rig = Rig::new("tap", host::MARK, &["osd"]);
    rig.until_panels();
    let bare = rig.image();

    // the whole chord before a turn of the loop: the close reaches the displays with the list
    rig.chord();
    rig.key(KEY_CTRL, false);
    assert_eq!(rig.source().as_deref(), Some("de"));
    rig.until("the choice shows", |rig| {
        rig.changed(&bare, SOURCES_AREA) >= SHOWN
    });
    rig.until("and fades out", |rig| {
        rig.changed(&bare, SOURCES_AREA) <= GONE
    });
}

#[test]
fn the_release_while_the_panel_component_restarts_switches_the_source() {
    let mut rig = Rig::new("restart-release", host::MARK, &["osd"]);
    rig.until_panels();
    let bare = rig.image();
    let first = rig.state.hosts.component_instance("osd");

    rig.chord();
    rig.until("the switcher's panel shows", |rig| {
        rig.changed(&bare, SOURCES_AREA) >= SHOWN
    });
    rig.crash_osd("dispatch");
    rig.key(KEY_CTRL, false);
    assert_eq!(
        rig.source().as_deref(),
        Some("de"),
        "the release switches without the displays"
    );
    rig.until("the panel of the ended instance goes", |rig| {
        rig.changed(&bare, SOURCES_AREA) <= GONE
    });
    rig.until_panels();
    assert!(
        rig.state.hosts.component_instance("osd") > first,
        "a new instance runs"
    );
    // the new instance is sent nothing: for a while after it binds, no panel shows
    rig.run_for(Duration::from_secs(1));
    assert!(
        rig.changed(&bare, SOURCES_AREA) <= GONE,
        "the new instance shows no list"
    );
}

#[test]
fn a_panel_component_that_starts_during_a_switch_shows_its_list() {
    let mut rig = Rig::new("restart-held", host::MARK, &["osd"]);
    rig.until_panels();
    let bare = rig.image();

    rig.chord();
    rig.until("the switcher's panel shows", |rig| {
        rig.changed(&bare, SOURCES_AREA) >= SHOWN
    });
    rig.crash_osd("dispatch");
    rig.until("the panel of the ended instance goes", |rig| {
        rig.changed(&bare, SOURCES_AREA) <= GONE
    });
    assert!(
        rig.state.overlays.inputs.switch_open(),
        "the switch goes on without the displays"
    );
    rig.until_panels();
    rig.until("the new instance shows the list", |rig| {
        rig.changed(&bare, SOURCES_AREA) >= SHOWN
    });
    rig.key(KEY_CTRL, false);
    assert_eq!(rig.source().as_deref(), Some("de"));
    rig.until("the panel fades out", |rig| {
        rig.changed(&bare, SOURCES_AREA) <= GONE
    });
}

#[test]
fn a_hung_panel_component_delays_no_switch_and_its_panel_goes() {
    let mut rig = Rig::new("hung", host::STAND_IN, &["osd"]);
    rig.until_panels();
    let (w, h) = (STAND_IN_SIZE.width as i32, STAND_IN_SIZE.height as i32);
    let panel = Area::new(
        (OUTPUT_SIZE.0 - w) / 2 + 10,
        (OUTPUT_SIZE.1 - h) / 2 + 10,
        w - 20,
        h - 20,
    );
    let covered = (panel.w * panel.h) as usize * 9 / 10;

    rig.chord();
    rig.until("the stand-in shows its panel", |rig| {
        rig.green(panel) >= covered
    });
    rig.until("the stand-in hangs", |rig| rig.reported("stand-in hangs"));
    rig.key(KEY_CTRL, false);
    assert_eq!(
        rig.source().as_deref(),
        Some("de"),
        "the release switches without waiting"
    );
    rig.until("the hung component's panel goes", |rig| {
        rig.green(panel) == 0
    });
    assert!(
        !rig.state.panels_can_open_picker(),
        "a hung component arms no picker"
    );
}

#[test]
fn without_the_panel_component_the_chord_switches_and_a_held_letter_repeats() {
    let mut rig = Rig::new("alone", host::MARK, &[]);
    let _app = text_window(&mut rig);
    assert!(!rig.state.overlays.panels.bound());

    rig.chord();
    rig.key(KEY_CTRL, false);
    assert_eq!(rig.source().as_deref(), Some("de"));
    // German and US keyboards type e at the same key
    rig.key(KEY_E, true);
    rig.until("the held letter repeats in the field", |_| {
        field().starts_with("eee")
    });
    rig.key(KEY_E, false);
    assert!(
        !rig.state.overlays.accent.is_open(),
        "no picker without its panel"
    );
}

#[test]
fn a_held_letter_opens_the_picker_at_the_caret_and_a_digit_inserts_its_candidate() {
    let mut rig = Rig::new("digit", host::MARK, &["osd"]);
    rig.until_panels();
    let _app = text_window(&mut rig);
    let before = rig.image();

    hold_e(&mut rig);
    until_picker_shows(&mut rig, &before);
    rig.until("the letter is in the field", |_| field() == "e");
    rig.key(KEY_2, true);
    rig.key(KEY_2, false);
    rig.until("the digit's candidate replaces the letter", |_| {
        field() == "é"
    });
    assert!(!rig.state.overlays.accent.is_open());
    // what changed of the picker's place once it went is the field's new text under it at most
    let area = Area::of(picker_layout(&rig).rect);
    let little = (area.w * area.h) as usize / 20;
    rig.until("the picker's panel goes", |rig| {
        rig.changed(&before, area) <= little
    });
}

#[test]
fn escape_puts_the_letter_back_and_a_click_on_a_candidate_inserts_it() {
    let mut rig = Rig::new("escape", host::MARK, &["osd"]);
    rig.until_panels();
    let _app = text_window(&mut rig);
    let before = rig.image();

    hold_e(&mut rig);
    until_picker_shows(&mut rig, &before);
    // Tab previews the first candidate in place of the letter; Escape puts the letter back
    rig.key(KEY_TAB, true);
    rig.key(KEY_TAB, false);
    rig.until("the preview replaced the letter", |_| field().is_empty());
    rig.key(KEY_ESC, true);
    rig.key(KEY_ESC, false);
    rig.until("the letter is back", |_| field() == "e");
    assert!(!rig.state.overlays.accent.is_open());

    // the picker's place shows what it showed before the first picker: the field's text lies below it
    hold_e(&mut rig);
    let layout = until_picker_shows(&mut rig, &before);
    rig.until("the second letter is in the field", |_| field() == "ee");
    rig.state.scripted_click(candidate(&layout, 2), BTN_LEFT);
    rig.until("the clicked candidate replaces the letter", |_| {
        field() == "eê"
    });
    assert!(!rig.state.overlays.accent.is_open());
}

#[test]
fn a_press_off_the_picker_closes_it_and_reaches_the_window_under_it() {
    let mut rig = Rig::new("press-off", host::MARK, &["osd"]);
    rig.until_panels();
    let _app = text_window(&mut rig);
    let before = rig.image();

    hold_e(&mut rig);
    until_picker_shows(&mut rig, &before);
    let window = rig
        .state
        .wm
        .key_window()
        .expect("the text window is in front");
    let frame = rig.state.wm.window(window).unwrap().geometry();
    let presses = PRESSES.load(Ordering::SeqCst);
    // the bottom of the window, under its field and away from the picker
    rig.state.scripted_click(
        (frame.x + frame.w / 2.0, frame.y + frame.h - TEXT_INSET),
        BTN_LEFT,
    );
    rig.until("the window got the press", |_| {
        PRESSES.load(Ordering::SeqCst) > presses
    });
    assert!(
        !rig.state.overlays.accent.is_open(),
        "the press closed the picker"
    );
    assert_eq!(field(), "e", "the letter stays");
}

#[test]
fn the_picker_closes_and_the_letter_returns_when_the_panel_component_ends() {
    let mut rig = Rig::new("ends", host::MARK, &["osd"]);
    rig.until_panels();
    let _app = text_window(&mut rig);
    let before = rig.image();

    hold_e(&mut rig);
    until_picker_shows(&mut rig, &before);
    rig.key(KEY_TAB, true);
    rig.key(KEY_TAB, false);
    rig.until("the preview replaced the letter", |_| field().is_empty());
    rig.crash_osd("dispatch");
    assert!(
        !rig.state.overlays.accent.is_open(),
        "the picker closed with its panel"
    );
    rig.until("the letter is back", |_| field() == "e");
}

#[test]
fn locking_ends_the_switch_and_the_picker() {
    let mut rig = Rig::new("lock", host::MARK, &["osd"]);
    rig.until_panels();
    let bare = rig.image();

    rig.chord();
    rig.until("the switcher's panel shows", |rig| {
        rig.changed(&bare, SOURCES_AREA) >= SHOWN
    });
    // the scripted picker of \`--accent\`, with Control still held
    assert!(rig.state.accent_open('e'));
    let (_conn, _lock) = lock(&mut rig);
    assert!(
        !rig.state.overlays.inputs.switch_open(),
        "the lock cancelled the switch"
    );
    assert!(
        !rig.state.overlays.accent.is_open(),
        "the lock closed the picker"
    );
    rig.key(KEY_CTRL, false);
    assert_eq!(
        rig.source().as_deref(),
        Some("us"),
        "the cancelled switch changed nothing"
    );
}
