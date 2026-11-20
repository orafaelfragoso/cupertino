//! A host running a component records the frame timeline `CUPERTINO_PROFILE` asks for: one trace for the process,
//! holding the component's `ui.window` and `ui.turn` spans on the track of the component's own thread.

mod support;

use std::path::Path;

use cupertino_kit::core::{
    Application, Command, CommandRegistry, Element, Key, Window, WindowBackground, WindowKey,
};
use cupertino_kit::geometry::Size;
use cupertino_kit::layout::{Align, Axis};
use cupertino_kit::platform::{DecorationMode, WindowSpec};
use cupertino_kit_shell::hosting::{Host, HostEvent, Kind, Runtime};
use cupertino_kit_shell::{
    Anchor, Component, ComponentName, Event, Keyboard, Layer, LayerSpec, Margin, Program, Role,
    Surface,
};
use support::{
    component_connection, connect_host, environment_lock, eventually, wait_for, Seen, Server,
};

/// The namespace of the test component's layer surface.
const NAMESPACE: &str = "test-profiled";

/// A component with one layer window.
struct Profiled;

impl Application for Profiled {
    type Model = ();
    type Message = ();

    fn init() -> ((), Vec<Command<()>>) {
        ((), Vec::new())
    }

    fn update(_: &mut (), _: ()) -> Vec<Command<()>> {
        Vec::new()
    }

    fn view(_: &()) -> Vec<Window<()>> {
        vec![Window {
            key: WindowKey(Key::from("profiled")),
            spec: WindowSpec {
                title: "profiled".to_owned(),
                app_id: "org.cupertino.test.profiled".to_owned(),
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
        }]
    }
}

impl Program for Profiled {
    fn surface(_: &(), _: &WindowKey) -> Surface {
        Surface {
            role: Role::Layer(LayerSpec {
                namespace: NAMESPACE,
                output: None,
                layer: Layer::Overlay,
                anchor: Anchor::default(),
                size: Size::new(120.0, 80.0),
                margin: Margin::default(),
                exclusive_zone: 0,
                keyboard: Keyboard::None,
            }),
            persistent: None,
            holes: Vec::new(),
        }
    }

    fn event(_: Event) -> Option<()> {
        None
    }
}

impl Component for Profiled {
    const NAME: ComponentName = ComponentName::OSD;
}

static PROFILED: cupertino_kit_shell::Entry = cupertino_kit_shell::Entry::of::<Profiled>();

/// The events of the one trace file in `dir`, read as the viewers do: the closing bracket is optional.
fn trace_events(dir: &Path) -> Option<Vec<serde_json::Value>> {
    let mut files = std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path());
    let file = files.next()?;
    assert!(files.next().is_none(), "one trace for the process");
    let text = std::fs::read_to_string(file).ok()?;
    let text = text.trim_end().trim_end_matches(']');
    serde_json::from_str(&format!("{text}]")).ok()
}

/// The complete events named `name` on the track `tid`.
fn spans(events: &[serde_json::Value], name: &str, tid: i64) -> usize {
    events
        .iter()
        .filter(|e| {
            e["ph"] == "X"
                && e["tid"] == tid
                && e["name"].as_str().is_some_and(|n| n.starts_with(name))
        })
        .count()
}

#[test]
fn a_host_running_a_component_writes_a_trace_with_its_frame_spans() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("profile-timeline");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    {
        let _lock = environment_lock();
        std::env::set_var("CUPERTINO_PROFILE", format!("chrome:{}/", dir.display()));
    }

    let (server, host_end) = Server::start(Kind::Shared, &["osd"]);
    let mut host: Host = connect_host(host_end);
    wait_for(&mut host, |event| matches!(event, HostEvent::Components(_)));
    let environment = cupertino_kit::app::environment().expect("toolkit environment");
    let runtime = Runtime::for_host(&mut host, Kind::Shared, environment).unwrap();
    let connection = component_connection(&mut host, "osd");
    let running = runtime.start(&PROFILED, connection).unwrap();
    server.seen_until(|seen| seen.contains(&Seen::Ready("osd".to_owned())));

    // the track of the component's own thread, named after the component
    let mut found = None;
    eventually("the trace holds the component's frame spans", || {
        let Some(events) = trace_events(&dir) else {
            return false;
        };
        let tid = events
            .iter()
            .find(|e| e["name"] == "thread_name" && e["args"]["name"] == "shell-osd")
            .and_then(|e| e["tid"].as_i64());
        found = tid.map(|tid| (events.clone(), tid));
        found.as_ref().is_some_and(|(events, tid)| {
            spans(events, "ui.window", *tid) > 0 && spans(events, "ui.turn", *tid) > 0
        })
    });
    assert!(found.is_some());

    running.stop();
    drop(server);
}
