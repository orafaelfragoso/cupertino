//! A transaction bound to a touchpad gesture, against the real server, headless (`docs/architecture/shell-host.md`,
//! Gestures): while the fingers move, every property the transaction animates shows its start plus the gesture's
//! progress times the way to its target, and released short of commit the transaction settles back to the start and
//! ends reverted. The server's supervisor starts this test binary as `cupertino-shell`: with [`host::MARK`] among its
//! arguments it runs a host with one `overview` component, which arms a transaction for the swipe up whose staged
//! window moves a surface layer [`TRAVEL`] points along a motion far longer than the test.

// shared with `shell-host.rs`, which uses what this test does not
#[allow(dead_code)]
#[path = "shell-host/rig.rs"]
mod rig;

use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;

use cupertino_input::gesture::GestureEvent;
use cupertino_kit::core::{
    Application, Change, Command, CommandRegistry, Element, Event as UiEvent, EventCx, Key,
    MeasureCx, PaintCx, Propagation, RendererClass, SemanticsCx, SurfaceLayerMark, Widget, Window,
    WindowBackground, WindowKey,
};
use cupertino_kit::geometry::{Rect, Size};
use cupertino_kit::layout::{Constraints, ContainerKind, LayoutSpec, Measured, SizeSpec};
use cupertino_kit::motion::{Easing, SpringPreset};
use cupertino_kit::platform::{
    DecorationMode, LayerChange, LayerProperty, SurfaceLayerLook, WindowRequest, WindowSpec,
};
use cupertino_kit::render::{Paint, PaintOp, Shape};
use cupertino_kit::theme::Color;
use cupertino_kit_shell::outputs::OutputId;
use cupertino_kit_shell::{
    transaction, Anchor, Component, ComponentName, Entry, Event, Keyboard, Layer, LayerSpec,
    Margin, Program, Role, Surface,
};

use rig::{count, primary, Area, Rig};

/// The host program of this test: `cupertino-shell`'s own (`cupertino_shell::run`) with [`Gestured`] as `overview`.
mod host {
    use std::path::PathBuf;

    use cupertino_kit_shell::Entry;

    /// The argument that makes this test binary the host: a filter after `--` that names no test.
    pub const MARK: &str = "cupertino-shell-gesture-host";

    /// The components this host was built with.
    static ENTRIES: [&Entry; 1] = [&super::OVERVIEW];

    /// The report file and the host's own arguments that follow [`MARK`], when this process is the host.
    pub fn arguments() -> Option<(PathBuf, Vec<String>)> {
        let args: Vec<String> = std::env::args().collect();
        let at = args.iter().position(|a| a == MARK)?;
        let report = PathBuf::from(args.get(at + 1)?);
        Some((report, args[at + 2..].to_vec()))
    }

    /// Be the host, the component reporting to `report`.
    pub fn run(report: PathBuf, args: Vec<String>) -> ! {
        super::report_to(report);
        cupertino_shell::run(&ENTRIES, args)
    }
}

/// The file the host's component reports to.
static REPORT: OnceLock<PathBuf> = OnceLock::new();

fn report_to(path: PathBuf) {
    let _ = REPORT.set(path);
}

/// Append `line` to the report: one write, so lines of several threads never mix.
fn report(line: impl AsRef<str>) {
    let Some(path) = REPORT.get() else {
        return;
    };
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("the report opens");
    file.write_all(format!("{}\n", line.as_ref()).as_bytes())
        .expect("the report is written");
}

/// The component's window: a strip across the output's top, all of it the surface layer [`LAYER`].
const WINDOW: &str = "gesture";
const LAYER: u64 = 1;
const STRIP: Size = Size::new(800.0, 100.0);
/// How far the transaction moves the layer to the right, in points.
const TRAVEL: f64 = 600.0;
/// The seconds of the layer's own motion, the longest the layer protocol takes: in the second or so the swipe lasts,
/// its own time would move the layer a tenth of the way, so only the gesture's progress moves it far.
const SLOW: f64 = 10.0;
/// The layer's colour.
const GREEN: Color = Color::new(0.0, 1.0, 0.0, 1.0);

/// A rectangle of one colour that is the surface layer [`LAYER`].
struct Block;

impl<M: 'static> Widget<M> for Block {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(STRIP.width),
            height: SizeSpec::Fixed(STRIP.height),
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: STRIP,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &UiEvent, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        cx.push(PaintOp::Fill {
            shape: Shape::Rect(Rect::new(0.0, 0.0, STRIP.width, STRIP.height)),
            paint: Paint::Solid(GREEN),
        });
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}

    fn surface_layer(&self, _: &()) -> Option<SurfaceLayerMark> {
        Some(SurfaceLayerMark {
            key: LAYER,
            look: SurfaceLayerLook::default(),
        })
    }
}

/// The `overview` of this test.
pub struct Gestured;

/// The output the transaction is armed on, and its handle.
#[derive(Default)]
pub struct Model {
    output: Option<OutputId>,
    armed: Option<transaction::Handle>,
}

#[derive(Clone, Debug)]
pub enum Message {
    Arm(OutputId),
    Heard(&'static str),
}

impl Application for Gestured {
    type Model = Model;
    type Message = Message;
    const RENDERER: RendererClass = RendererClass::Cpu;

    fn init() -> (Model, Vec<Command<Message>>) {
        (Model::default(), Vec::new())
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        match message {
            Message::Arm(output) if model.armed.is_none() => {
                model.output = Some(output);
                // the window comes with this view: its first commit is kept for the gesture, with the layer's motion
                let key = WindowKey(Key::from(WINDOW));
                let mut part = transaction::create(output);
                part.bind(transaction::Gesture::SwipeUp, SpringPreset::DEFAULT);
                part.stage(&key);
                model.armed = Some(part.submit());
                report("gesture armed");
                vec![Command::Window(
                    key,
                    WindowRequest::Layer {
                        key: LAYER,
                        changes: vec![LayerChange::new(
                            LayerProperty::X,
                            TRAVEL,
                            Some(Easing::LINEAR.over(SLOW)),
                        )],
                    },
                )]
            }
            Message::Arm(_) => Vec::new(),
            Message::Heard(line) => {
                report(line);
                Vec::new()
            }
        }
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        if model.output.is_none() {
            return Vec::new();
        }
        vec![Window {
            key: WindowKey(Key::from(WINDOW)),
            spec: WindowSpec {
                title: WINDOW.to_owned(),
                app_id: format!("org.cupertino.test.{WINDOW}"),
                size: STRIP,
                min_size: None,
                max_size: None,
                decorations: DecorationMode::None,
                foreign_parent: None,
                input_region: None,
            },
            shortcuts: CommandRegistry::new(),
            on_close: None,
            background: WindowBackground::None,
            content: Element::new(Block),
            popup: None,
            menus: None,
        }]
    }
}

impl Program for Gestured {
    fn surface(model: &Model, _: &WindowKey) -> Surface {
        Surface {
            role: Role::Layer(LayerSpec {
                namespace: "test-gesture",
                output: model.output,
                layer: Layer::Overlay,
                anchor: Anchor {
                    top: true,
                    left: true,
                    ..Anchor::default()
                },
                size: STRIP,
                margin: Margin::default(),
                exclusive_zone: -1,
                keyboard: Keyboard::None,
            }),
            persistent: None,
            holes: Vec::new(),
        }
    }

    fn event(event: Event) -> Option<Message> {
        use transaction::{Event as Tx, Outcome};
        match event {
            Event::Overview(output, _) => Some(Message::Arm(output)),
            Event::Transaction(Tx::GestureBegan { .. }) => Some(Message::Heard("gesture began")),
            Event::Transaction(Tx::Finished {
                outcome: Outcome::Reverted,
                ..
            }) => Some(Message::Heard("gesture reverted")),
            Event::Transaction(Tx::Finished { .. }) => Some(Message::Heard("gesture finished")),
            _ => None,
        }
    }
}

impl Component for Gestured {
    const NAME: ComponentName = ComponentName::OVERVIEW;
}

pub static OVERVIEW: Entry = Entry::of::<Gestured>();

/// The host itself, when the supervisor started this binary with [`host::MARK`]; a test run does nothing here.
#[test]
fn host_program() {
    if let Some((report, args)) = host::arguments() {
        host::run(report, args);
    }
}

/// The fingers the swipe up uses, and its travel for the whole gesture (the recogniser's `overview_distance`).
const FINGERS: u32 = 3;
const SWIPE_DISTANCE: f64 = 250.0;
/// The seconds between two touchpad events.
const EVENT_STEP: f64 = 0.016;

/// Whether the strip from the output's left edge `to` points along shows the layer.
fn green_before(rig: &mut Rig, to: i32) -> usize {
    count(
        &rig.image(),
        Area::new(0, 0, to, STRIP.height as i32),
        primary(1),
    )
}

#[test]
fn a_bound_gesture_moves_its_layers_with_the_progress_and_settles_back_when_reverted() {
    let mut rig = Rig::new("gesture", &["overview"], &[]);
    rig.until_reported("gesture armed");
    for _ in 0..20 {
        rig.turn();
    }
    let strip = Area::new(0, 0, STRIP.width as i32, STRIP.height as i32);
    assert_eq!(
        count(&rig.image(), strip, primary(1)),
        0,
        "the armed part's window shows nothing before the gesture"
    );

    // a swipe up of half the gesture's travel: the layer is half way along its motion
    let mut time = 1.0;
    rig.state.gesture(GestureEvent::SwipeBegin {
        time,
        fingers: FINGERS,
    });
    let steps = 25;
    for _ in 0..steps {
        time += EVENT_STEP;
        rig.state.gesture(GestureEvent::SwipeUpdate {
            time,
            dx: 0.0,
            dy: -SWIPE_DISTANCE / 2.0 / f64::from(steps),
        });
        rig.turn();
    }
    rig.until_reported("gesture began");
    for _ in 0..10 {
        rig.turn();
    }
    let half = (TRAVEL / 2.0) as i32;
    let before = green_before(&mut rig, half - 20);
    assert_eq!(
        before, 0,
        "at progress 0.5 the layer stands {half} pt along, so the strip's first {} pt show none of it",
        half - 20
    );
    assert!(
        rig.shows(
            Area::new(
                half + 20,
                0,
                STRIP.width as i32 - half - 20,
                STRIP.height as i32
            ),
            primary(1),
            1
        ),
        "the layer shows past {half} pt"
    );

    // the fingers go back down and lift short of commit: the transaction settles back to its start and is reverted
    for _ in 0..steps {
        time += EVENT_STEP;
        rig.state.gesture(GestureEvent::SwipeUpdate {
            time,
            dx: 0.0,
            dy: SWIPE_DISTANCE / 2.0 / f64::from(steps),
        });
        rig.turn();
    }
    time += EVENT_STEP;
    rig.state.gesture(GestureEvent::SwipeEnd {
        time,
        cancelled: false,
    });
    rig.until_reported("gesture reverted");
    rig.until("the layer is back at its start", |rig| {
        green_before(rig, 40) > 0
    });
}
