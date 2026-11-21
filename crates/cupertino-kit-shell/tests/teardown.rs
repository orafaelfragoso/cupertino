//! A component that fails reaches no one from that moment: after a panic of its loop, a panic of one of its helpers
//! or an exit its host did not ask for, its connection to the server carries nothing more, so its teardown destroys
//! no role object and the server holds its menu bar from the connection's end. Each case runs a menu bar against
//! the stand-in through a relay that counts what the component sent.

use crate::support;

use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, Once};
use std::time::Instant;

use cupertino_kit::core::{
    Application, Command, CommandRegistry, Element, Key, Window, WindowBackground, WindowKey,
};
use cupertino_kit::geometry::Size;
use cupertino_kit::layout::{Align, Axis};
use cupertino_kit::platform::{DecorationMode, WindowSpec};
use cupertino_kit_shell::hosting::{Ended, Failure, HostEvent, Kind, Runtime};
use cupertino_kit_shell::{
    host, Anchor, Component, ComponentName, Entry, Event, Keyboard, Layer, LayerSpec, Margin,
    Program, Role, Surface,
};
use support::relay::Relay;
use support::{component_connection, connect_host, wait_for, Seen, Server, PATIENCE};

/// The namespace of the bar's layer surface.
const NAMESPACE: &str = "test-menubar";

/// The relay of the case that runs, and what it counted as the component failed.
static RELAY: Mutex<Option<Arc<Relay>>> = Mutex::new(None);
static SENT_AT_FAILURE: Mutex<Option<u64>> = Mutex::new(None);
/// What the component's helper waits for before the case's failure.
static GATE: Mutex<Option<Receiver<()>>> = Mutex::new(None);

/// Records what the component sent so far, once per case.
fn record_failure() {
    let relay = RELAY.lock().unwrap().clone();
    let mut at = SENT_AT_FAILURE.lock().unwrap();
    if let (Some(relay), None) = (relay, *at) {
        *at = Some(relay.sent());
    }
}

/// Waits for the test to let the case's failure happen.
fn wait_for_gate() {
    let gate = GATE.lock().unwrap().take().expect("the case's gate");
    gate.recv_timeout(PATIENCE)
        .expect("the test opens the gate");
}

/// The menu bar's window: a stretched bar 24 points tall.
fn bar<M: Clone + 'static>() -> Window<M> {
    Window {
        key: WindowKey(Key::from("bar")),
        spec: WindowSpec {
            title: "bar".to_owned(),
            app_id: "org.cupertino.test.bar".to_owned(),
            size: Size::new(0.0, 24.0),
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

/// The bar's surface: the output's menu bar, persisted as the server holds it.
fn bar_surface() -> Surface {
    Surface {
        role: Role::MenuBar(LayerSpec {
            namespace: NAMESPACE,
            output: None,
            layer: Layer::Top,
            anchor: Anchor {
                top: true,
                left: true,
                right: true,
                bottom: false,
            },
            size: Size::new(0.0, 24.0),
            margin: Margin::default(),
            exclusive_zone: 24,
            keyboard: Keyboard::None,
        }),
        persistent: Some("bar"),
        holes: Vec::new(),
    }
}

/// A menu bar whose helper waits for the gate; `$helper` is its work, `$opened` its update once the gate opened.
macro_rules! failing_bar {
    ($ty:ident, $helper:expr, $opened:expr) => {
        struct $ty;

        impl Application for $ty {
            type Model = ();
            type Message = ();

            fn init() -> ((), Vec<Command<()>>) {
                ((), vec![host::thread("gate", $helper, |()| ())])
            }

            fn update(_: &mut (), (): ()) -> Vec<Command<()>> {
                $opened()
            }

            fn view(_: &()) -> Vec<Window<()>> {
                vec![bar()]
            }
        }

        impl Program for $ty {
            fn surface(_: &(), _: &WindowKey) -> Surface {
                bar_surface()
            }

            fn event(_: Event) -> Option<()> {
                None
            }
        }

        impl Component for $ty {
            const NAME: ComponentName = ComponentName::MENUBAR;
        }
    };
}

failing_bar!(
    LoopPanics,
    |_: &host::Stop| wait_for_gate(),
    || -> Vec<Command<()>> { panic!("the menu bar panics as it handles a message") }
);
failing_bar!(
    HelperPanics,
    |_: &host::Stop| {
        wait_for_gate();
        panic!("a helper of the menu bar panics")
    },
    Vec::new
);
failing_bar!(Exits, |_: &host::Stop| wait_for_gate(), || {
    record_failure();
    vec![Command::Exit]
});

static LOOP_PANICS: Entry = Entry::of::<LoopPanics>();
static HELPER_PANICS: Entry = Entry::of::<HelperPanics>();
static EXITS: Entry = Entry::of::<Exits>();

/// Chains a panic hook that records what the component sent as it panics. Installed before the host's runtime, whose
/// own hook then runs first.
fn record_panics() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            record_failure();
            previous(info);
        }));
    });
}

#[test]
fn a_failed_components_connection_sends_nothing_after_the_failure() {
    record_panics();
    let cases: [(&str, &'static Entry, Failure); 3] = [
        ("a panic of its loop", &LOOP_PANICS, Failure::Panic),
        ("a panic of its helper", &HELPER_PANICS, Failure::Panic),
        ("an exit", &EXITS, Failure::Exit),
    ];
    let mut carried = Vec::new();
    for (case, entry, cause) in cases {
        let (server, host_end) = Server::start(Kind::Shared, &["menubar"]);
        let mut host = connect_host(host_end);
        wait_for(&mut host, |event| matches!(event, HostEvent::Components(_)));
        let environment = cupertino_kit::app::environment().expect("toolkit environment");
        let runtime = Runtime::for_host(&mut host, Kind::Shared, environment).unwrap();
        let (open, gate) = mpsc::channel();
        *GATE.lock().unwrap() = Some(gate);
        *SENT_AT_FAILURE.lock().unwrap() = None;
        let relayed = server.relay("menubar");
        let connection = component_connection(&mut host, "menubar");
        let relay = relayed.recv_timeout(PATIENCE).expect("the relay");
        *RELAY.lock().unwrap() = Some(Arc::clone(&relay));
        let mut bar = runtime.start(entry, connection).unwrap();
        server.expect(|seen| {
            matches!(seen, Seen::Drawn { component, namespace }
                if component == "menubar" && namespace == NAMESPACE)
        });

        open.send(()).unwrap();
        let asked = Instant::now();
        let ended = loop {
            host.wait(Some(PATIENCE))
                .expect("the host's connection stays up");
            if let Some(ended) = bar.ended() {
                break ended;
            }
            assert!(asked.elapsed() < PATIENCE, "{case}: the bar did not end");
        };
        assert_eq!(ended, Ended::Failed(cause), "{case}");
        // as the host does once it read the failure
        bar.shut();
        let sent = relay.closed();
        let at_failure = SENT_AT_FAILURE
            .lock()
            .unwrap()
            .unwrap_or_else(|| panic!("{case}: the failure was recorded"));
        assert!(at_failure > 0, "{case}: the bar drew before it failed");
        if sent != at_failure {
            carried.push((case, sent - at_failure));
        }
        *RELAY.lock().unwrap() = None;
        drop(runtime);
        drop(server);
    }
    assert!(
        carried.is_empty(),
        "bytes the bar's connection carried after the failure: {carried:?}"
    );
}
