//! Public-boundary tests for `cupertino-kit-shell` (W5.10): transaction limits matching
//! `cupertino_protocols::transaction`, design values, the component table, entry registration, safe no-op requests
//! outside a component thread, and the runtime's loop against the stand-in server: a component and a program of its
//! own run on Kit's UI loop on their connection, draw their view on the role it names, carry out the requests their
//! commands queue, persist their surfaces and say `ready`; a component is told the session's display options at start
//! and on every change; a component needing a newer global is refused before it starts; a busy loop and a busy render
//! thread are reported.

mod support;

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Mutex;
use std::time::Duration;

use cupertino_kit::core::{
    Application, Command, CommandRegistry, Element, Key, Window, WindowBackground, WindowKey,
};
use cupertino_kit::geometry::Size;
use cupertino_kit::layout::{Align, Axis};
use cupertino_kit::platform::{DecorationMode, SystemAppearance, WindowSpec};
use cupertino_kit_shell::bus::{Message, OsdKind};
use cupertino_kit_shell::hosting::{Ended, Host, HostEvent, Kind, Runtime, EXIT_VERSION};
use cupertino_kit_shell::{
    design, holds, host, outputs, run, session, transaction, Anchor, Component, ComponentName,
    Entry, Error, Event, Keyboard, Layer, LayerSpec, Margin, Program, Role, Surface,
};
use support::display::Offer;
use support::{
    component_connection, connect_host, environment_lock, eventually, wait_for, DisplayOption,
    Seen, Server,
};

/// The namespace of the test component's layer surface.
const NAMESPACE: &str = "test-overlay";
/// The key the test component's surface is persisted under.
const PERSISTED: &str = "overlay";
/// The serial and application of the activation the test component's command asks for.
const ACTIVATION: (u32, &str) = (7, "org.example.App");

/// A layer window of 120 x 80 points, persisted under `persistent`.
fn overlay_window<M: Clone + 'static>(key: &'static str) -> Window<M> {
    Window {
        key: WindowKey(Key::from(key)),
        spec: WindowSpec {
            title: key.to_owned(),
            app_id: format!("org.cupertino.test.{key}"),
            size: Size::new(120.0, 80.0),
            min_size: None,
            max_size: None,
            decorations: DecorationMode::None,
            foreign_parent: None,
            input_region: None,
        },
        shortcuts: CommandRegistry::new(),
        on_close: None,
        background: WindowBackground::None,
        content: Element::new(cupertino_kit::controls::Stack {
            axis: Axis::Vertical,
            padding: Default::default(),
            spacing: 0.0,
            align: Align::Start,
            children: Vec::new(),
        }),
        popup: None,
        menus: None,
    }
}

fn overlay_surface(namespace: &'static str, persistent: Option<&'static str>) -> Surface {
    Surface {
        role: Role::Layer(LayerSpec {
            namespace,
            output: None,
            layer: Layer::Overlay,
            anchor: Anchor::default(),
            size: Size::new(120.0, 80.0),
            margin: Margin::default(),
            exclusive_zone: 0,
            keyboard: Keyboard::None,
        }),
        persistent,
        holes: Vec::new(),
    }
}

/// A component with one persistent layer window, whose first command asks the session to bring an app forward.
struct Overlay;

#[derive(Clone, Debug)]
enum OverlayMessage {
    Activate,
}

impl Application for Overlay {
    type Model = ();
    type Message = OverlayMessage;

    fn init() -> ((), Vec<Command<OverlayMessage>>) {
        let task = Command::Perform(Box::pin(async { OverlayMessage::Activate }));
        ((), vec![task])
    }

    fn update(_: &mut (), message: OverlayMessage) -> Vec<Command<OverlayMessage>> {
        match message {
            OverlayMessage::Activate => session::activate_app(ACTIVATION.0, ACTIVATION.1),
        }
        Vec::new()
    }

    fn view(_: &()) -> Vec<Window<OverlayMessage>> {
        vec![overlay_window("overlay")]
    }
}

impl Program for Overlay {
    fn surface(_: &(), _: &WindowKey) -> Surface {
        overlay_surface(NAMESPACE, Some(PERSISTED))
    }

    fn event(_: Event) -> Option<OverlayMessage> {
        None
    }
}

impl Component for Overlay {
    const NAME: ComponentName = ComponentName::OSD;
}

/// A dock that draws nothing: it needs `cupertino_shell_v1` 28.
struct Dock;

impl Application for Dock {
    type Model = ();
    type Message = ();

    fn init() -> ((), Vec<Command<()>>) {
        ((), Vec::new())
    }

    fn update(_: &mut (), _: ()) -> Vec<Command<()>> {
        Vec::new()
    }

    fn view(_: &()) -> Vec<Window<()>> {
        Vec::new()
    }
}

impl Program for Dock {
    fn surface(_: &(), _: &WindowKey) -> Surface {
        overlay_surface("test-dock", None)
    }

    fn event(_: Event) -> Option<()> {
        None
    }
}

impl Component for Dock {
    const NAME: ComponentName = ComponentName::DOCK;
}

/// What the stalling component's update waits for before it returns.
static GATE: Mutex<Option<Receiver<()>>> = Mutex::new(None);

/// A component whose first update waits for the test to let it go, as an update stuck on work.
struct Stalling;

impl Application for Stalling {
    type Model = ();
    type Message = ();

    fn init() -> ((), Vec<Command<()>>) {
        ((), vec![Command::Perform(Box::pin(async {}))])
    }

    fn update(_: &mut (), _: ()) -> Vec<Command<()>> {
        let gate = GATE.lock().unwrap().take();
        if let Some(gate) = gate {
            let _ = gate.recv_timeout(support::PATIENCE);
        }
        Vec::new()
    }

    fn view(_: &()) -> Vec<Window<()>> {
        Vec::new()
    }
}

impl Program for Stalling {
    fn surface(_: &(), _: &WindowKey) -> Surface {
        overlay_surface("test-stalling", None)
    }

    fn event(_: Event) -> Option<()> {
        None
    }
}

impl Component for Stalling {
    const NAME: ComponentName = ComponentName::NOTIFICATIONS;
}

/// What the following component was told of the session's appearance and display options, in order.
static TOLD: Mutex<Vec<SystemAppearance>> = Mutex::new(Vec::new());

/// A component with no window that keeps what it is told of the session's appearance and display options.
struct Following;

impl Application for Following {
    type Model = ();
    type Message = SystemAppearance;

    fn init() -> ((), Vec<Command<SystemAppearance>>) {
        ((), Vec::new())
    }

    fn update(_: &mut (), appearance: SystemAppearance) -> Vec<Command<SystemAppearance>> {
        TOLD.lock().unwrap().push(appearance);
        Vec::new()
    }

    fn view(_: &()) -> Vec<Window<SystemAppearance>> {
        Vec::new()
    }
}

impl Program for Following {
    fn surface(_: &(), _: &WindowKey) -> Surface {
        overlay_surface("test-following", None)
    }

    fn event(event: Event) -> Option<SystemAppearance> {
        match event {
            Event::Appearance(appearance) => Some(appearance),
            _ => None,
        }
    }
}

impl Component for Following {
    const NAME: ComponentName = ComponentName::LAUNCHER;
}

static FOLLOWING: Entry = Entry::of::<Following>();
static OVERLAY: Entry = Entry::of::<Overlay>();
static DOCK: Entry = Entry::of::<Dock>();
static STALLING: Entry = Entry::of::<Stalling>();

/// A host connected to `server` and its runtime.
fn host_and_runtime(host_end: std::os::unix::net::UnixStream) -> (Host, Runtime) {
    let mut host = connect_host(host_end);
    wait_for(&mut host, |event| matches!(event, HostEvent::Components(_)));
    let environment = cupertino_kit::app::environment().expect("toolkit environment");
    let runtime = Runtime::for_host(&mut host, Kind::Shared, environment).unwrap();
    (host, runtime)
}

fn position(seen: &[Seen], want: impl Fn(&Seen) -> bool) -> usize {
    seen.iter()
        .position(want)
        .unwrap_or_else(|| panic!("not seen in {seen:?}"))
}

#[test]
fn a_component_draws_its_view_runs_its_commands_and_says_ready() {
    let (server, host_end) = Server::start(Kind::Shared, &["osd"]);
    let (mut host, runtime) = host_and_runtime(host_end);
    let connection = component_connection(&mut host, "osd");
    let mut running = runtime.start(&OVERLAY, connection).unwrap();

    let osd = || "osd".to_owned();
    let seen = server.seen_until(|seen| {
        let ready = seen.contains(&Seen::Ready(osd()));
        let activated = seen.iter().any(|s| matches!(s, Seen::ActivateApp { .. }));
        ready && activated
    });
    let persisted = position(&seen, |s| {
        *s == Seen::Persisted {
            component: osd(),
            key: PERSISTED.into(),
        }
    });
    let drawn = position(&seen, |s| {
        *s == Seen::Drawn {
            component: osd(),
            namespace: NAMESPACE.into(),
        }
    });
    let ready = position(&seen, |s| *s == Seen::Ready(osd()));
    // persisted before its first buffer; ready once that frame was shown
    assert!(persisted < drawn && drawn < ready, "{seen:?}");
    assert!(seen.contains(&Seen::ActivateApp {
        component: osd(),
        serial: ACTIVATION.0,
        desktop_id: ACTIVATION.1.into(),
    }));
    // the loop waits for what comes next, out of its busy record
    eventually("the loop waits", || running.busy_for().is_none());

    running.stop();
    eventually("the component stops", || {
        running.ended() == Some(Ended::Stopped)
    });
    drop(server);
}

#[test]
fn a_component_is_told_the_display_options_at_start_and_on_every_change() {
    let (server, host_end) = Server::start(Kind::Shared, &["launcher"]);
    // the session starts with Reduce Motion on
    server.set_display_options(DisplayOption::ReduceMotion);
    let (mut host, runtime) = host_and_runtime(host_end);
    let connection = component_connection(&mut host, "launcher");
    let mut running = runtime.start(&FOLLOWING, connection).unwrap();
    let told = || TOLD.lock().unwrap().clone();
    let reduce_motion = || told().iter().map(|a| a.reduce_motion).collect::<Vec<_>>();

    eventually("the component is told Reduce Motion at start", || {
        reduce_motion() == [true]
    });
    assert!(
        !told()[0].dark && !told()[0].increase_contrast,
        "{:?}",
        told()
    );

    server.set_display_options(DisplayOption::empty());
    eventually("the component is told Reduce Motion went off", || {
        reduce_motion() == [true, false]
    });

    server.set_display_options(DisplayOption::ReduceMotion | DisplayOption::IncreaseContrast);
    eventually("the component is told Reduce Motion came back", || {
        reduce_motion() == [true, false, true]
    });
    assert!(told()[2].increase_contrast, "{:?}", told());

    // the same options again are no change: the next one told is the one after them
    server.set_display_options(DisplayOption::ReduceMotion | DisplayOption::IncreaseContrast);
    server.set_display_options(DisplayOption::ReduceTransparency);
    eventually("the component is told the next change", || {
        told().len() >= 4
    });
    let last = told()[3];
    assert!(
        !last.reduce_motion && !last.increase_contrast && last.reduce_transparency,
        "{:?}",
        told()
    );
    assert_eq!(told().len(), 4, "{:?}", told());

    running.stop();
    eventually("the component stops", || {
        running.ended() == Some(Ended::Stopped)
    });
    drop(server);
}

#[test]
fn a_component_needing_newer_globals_is_refused_before_it_starts() {
    let older_shell = Offer {
        shell: 27,
        ..Offer::CURRENT
    };
    let (server, host_end) = Server::start_offering(Kind::Shared, &["dock"], older_shell);
    let (mut host, runtime) = host_and_runtime(host_end);
    let connection = component_connection(&mut host, "dock");
    match runtime.start(&DOCK, connection) {
        Err(error) => assert_eq!(
            error,
            Error::Version {
                expected: 28,
                found: 27
            }
        ),
        Ok(_) => panic!("a dock started on a server whose shell is older than it needs"),
    }
    drop(server);

    let older_layer_shell = Offer {
        layer_shell: 3,
        ..Offer::CURRENT
    };
    let (server, host_end) = Server::start_offering(Kind::Shared, &["osd"], older_layer_shell);
    let (mut host, runtime) = host_and_runtime(host_end);
    let connection = component_connection(&mut host, "osd");
    match runtime.start(&OVERLAY, connection) {
        Err(error) => assert_eq!(
            error,
            Error::Version {
                expected: 4,
                found: 3
            }
        ),
        Ok(_) => panic!("an overlay started on a server whose layer shell is older than it needs"),
    }
    drop(server);
}

#[test]
fn a_busy_loop_and_a_busy_render_thread_are_reported() {
    let (server, host_end) = Server::start(Kind::Shared, &["notifications"]);
    let (mut host, runtime) = host_and_runtime(host_end);
    let (release, gate) = mpsc::channel();
    *GATE.lock().unwrap() = Some(gate);
    let connection = component_connection(&mut host, "notifications");
    let mut running = runtime.start(&STALLING, connection).unwrap();
    eventually("the stalled loop is busy", || {
        running
            .busy_for()
            .is_some_and(|busy| busy >= Duration::from_millis(50))
    });
    release.send(()).unwrap();
    eventually("the loop waits again", || running.busy_for().is_none());

    let (release, gate): (Sender<()>, Receiver<()>) = mpsc::channel();
    cupertino_ui_render::render_thread::hand_over(move || {
        let _ = gate.recv_timeout(support::PATIENCE);
    });
    eventually("the stuck render thread is busy", || {
        runtime
            .render_busy_for()
            .is_some_and(|busy| busy >= Duration::from_millis(50))
    });
    release.send(()).unwrap();

    running.stop();
    eventually("the component stops", || {
        running.ended() == Some(Ended::Stopped)
    });
    drop(server);
}

/// A program of its own: it shows one layer window and exits once that window presented its first frame.
struct Standalone;

#[derive(Clone, Debug)]
enum StandaloneMessage {
    Shown,
}

/// The outputs the program saw when it started.
static STANDALONE_OUTPUTS: Mutex<Vec<String>> = Mutex::new(Vec::new());

impl Application for Standalone {
    type Model = ();
    type Message = StandaloneMessage;

    fn init() -> ((), Vec<Command<StandaloneMessage>>) {
        let _environment = cupertino_kit_shell::environment();
        assert!(!session::locked());
        assert!(holds().is_empty());
        *STANDALONE_OUTPUTS.lock().unwrap() =
            outputs::all().into_iter().filter_map(|o| o.name).collect();
        let shown = Command::OnPresented {
            window: WindowKey(Key::from("standalone")),
            message: StandaloneMessage::Shown,
        };
        ((), vec![shown])
    }

    fn update(_: &mut (), message: StandaloneMessage) -> Vec<Command<StandaloneMessage>> {
        match message {
            StandaloneMessage::Shown => vec![Command::Exit],
        }
    }

    fn view(_: &()) -> Vec<Window<StandaloneMessage>> {
        vec![overlay_window("standalone")]
    }
}

impl Program for Standalone {
    fn surface(_: &(), _: &WindowKey) -> Surface {
        overlay_surface("test-standalone", None)
    }

    fn event(_: Event) -> Option<StandaloneMessage> {
        None
    }
}

#[test]
fn a_program_of_its_own_runs_on_kit_s_loop_until_it_exits() {
    use std::os::fd::IntoRawFd;

    let (server, _host_end) = Server::start(Kind::Shared, &[]);
    let client = server.client();
    let environment = cupertino_kit::app::environment().expect("toolkit environment");
    {
        let _lock = environment_lock();
        std::env::set_var("WAYLAND_SOCKET", client.into_raw_fd().to_string());
        run::<Standalone>(environment).expect("the program runs and exits");
        std::env::remove_var("WAYLAND_SOCKET");
    }
    assert_eq!(
        *STANDALONE_OUTPUTS.lock().unwrap(),
        vec!["TEST-1".to_owned()]
    );
    server.seen_until(|seen| {
        seen.contains(&Seen::Drawn {
            component: String::new(),
            namespace: "test-standalone".into(),
        })
    });

    let mut cmd = std::process::Command::new("/bin/true");
    assert!(host::spawn(cmd));
    cmd = std::process::Command::new("/nonexistent/cupertino-test-binary");
    assert!(!host::spawn(cmd));
}

#[test]
fn design_limits_entries_and_outside_thread_safety() {
    assert_eq!(EXIT_VERSION, 76);
    assert_eq!(
        transaction::MAX_DEADLINE,
        Duration::from_millis(cupertino_protocols::transaction::MAX_DEADLINE_MS as u64)
    );
    // the menu bar's values are the theme data's
    let mb = design::menu_bar();
    let theme = cupertino_assets::theme::load().expect("theme data");
    let typography = theme.tokens.typography();
    assert_eq!(mb.height, theme.shapes.menu_bar_height);
    assert_eq!(
        mb.selection_radius,
        theme.shapes.menu_bar_selection_corner_radius
    );
    assert_eq!(mb.item.size, typography.menu_bar_item().size as f32);
    assert_eq!(
        (mb.item.weight.0, mb.app_name.weight.0),
        (
            typography.menu_bar_item().weight,
            typography.menu_bar_app_name().weight
        )
    );
    assert_eq!(
        mb.cap_height,
        typography.metrics.cap_height / typography.metrics.units_per_em
    );

    assert_eq!(OVERLAY.name(), ComponentName::OSD);

    // Every component in the canonical table round-trips through ComponentName::from_name.
    for name in [
        "menubar",
        "dock",
        "desktop",
        "windowmanager",
        "overview",
        "launcher",
        "notifications",
        "controlcenter",
        "osd",
        "agent",
        "dialogs",
    ] {
        let c = ComponentName::from_name(name).expect("known component");
        assert_eq!(c.as_str(), name);
    }
    assert!(ComponentName::from_name("unknown").is_none());

    // Calling request/query APIs outside a component thread does not panic.
    assert!(!session::locked());
    assert!(outputs::all().is_empty());
    assert!(holds().is_empty());
    session::activate_app(1, "org.example.App");
    cupertino_kit_shell::bus::publish(Message::ShowOsd {
        kind: OsdKind::Volume,
        level: 0.5,
    });

    // Host::connect fails cleanly when WAYLAND_SOCKET is unset.
    let _lock = environment_lock();
    std::env::remove_var("WAYLAND_SOCKET");
    assert!(matches!(Host::connect(), Err(Error::Protocol(_))));
}
