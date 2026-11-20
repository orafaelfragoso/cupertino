//! The test components the host of [`super::host`] runs, each a [`Component`] of the server's table on Kit's UI loop:
//! - `osd`: a persistent overlay layer window of [`GREEN`] ([`HELLO_KEY`]); it reports the outputs and holds it starts
//!   with, publishes [`dnd`] and reports what it hears on the panels topic;
//! - `menubar`: the menu bar role, a persistent bar of [`BLUE`] [`bar_height`] tall across the top of the output;
//! - `dock`: the dock role, a persistent dock of [`RED`] at the bottom; it publishes [`LAUNCHER_SHOWN`] and reports
//!   what it hears on the Do Not Disturb topic;
//! - `overview`: once the server tells it of the window of [`HOLE_APP_ID`], one transaction shows a layer window whose
//!   surface layer is a hole claiming that window;
//! - `notifications`: its loop stalls for [`STALL`], then the render thread does, then it starts a helper that ignores
//!   its stop.
//!
//! What a component reports goes to the host's report file ([`report`]) as one line.

use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use cupertino_kit::core::{
    Application, Change, Command, CommandRegistry, Element, Event as UiEvent, EventCx, Key,
    MeasureCx, PaintCx, Propagation, RendererClass, SemanticsCx, SurfaceLayerMark, Widget, Window,
    WindowBackground, WindowKey,
};
use cupertino_kit::geometry::{Rect, Size};
use cupertino_kit::layout::{Constraints, ContainerKind, LayoutSpec, Measured, SizeSpec};
use cupertino_kit::platform::{DecorationMode, SurfaceLayerLook, WindowSpec};
use cupertino_kit::render::{Paint, PaintOp, Shape};
use cupertino_kit::theme::Color;
use cupertino_kit_shell::bus::{self, Message, Panel, Sender, Topic};
use cupertino_kit_shell::outputs::{self, OutputId};
use cupertino_kit_shell::{
    holds, host, overview, transaction, Anchor, Component, ComponentName, Entry, Event, Hole,
    HoleSource, Keyboard, Layer, LayerSpec, Margin, Program, Role, ServerWindowId, Surface,
};

/// The colour of the `osd` window.
pub const GREEN: Color = Color::new(0.0, 1.0, 0.0, 1.0);
/// The colour of the menu bar.
pub const BLUE: Color = Color::new(0.0, 0.0, 1.0, 1.0);
/// The colour of the dock.
pub const RED: Color = Color::new(1.0, 0.0, 0.0, 1.0);

/// The size of the `osd` window, in points.
pub const HELLO_SIZE: Size = Size::new(120.0, 80.0);
/// The key the `osd` window is persisted under.
pub const HELLO_KEY: &str = "hello";
/// The layer-shell namespace of the `osd` window.
pub const HELLO_NAMESPACE: &str = "test-hello";

/// The height of the menu bar, in points, the theme's as the real bar's: the server reserves that much for a surface
/// of the menu bar role; its exclusive zone is as tall.
pub fn bar_height() -> f64 {
    cupertino_kit_shell::design::menu_bar().height
}
/// The key the menu bar is persisted under.
pub const BAR_KEY: &str = "bar";
/// The layer-shell namespace of the menu bar.
pub const BAR_NAMESPACE: &str = "test-menubar";

/// The size of the dock, in points.
pub const DOCK_SIZE: Size = Size::new(200.0, 60.0);
/// The key the dock is persisted under.
pub const DOCK_KEY: &str = "dock";
/// The layer-shell namespace of the dock.
pub const DOCK_NAMESPACE: &str = "test-dock";

/// The app id of the window the overview's hole claims.
pub const HOLE_APP_ID: &str = "org.cupertino.test.hole";
/// The size of the overview's window and of its hole, in points, at the output's top left.
pub const HOLE_SIZE: Size = Size::new(160.0, 120.0);
/// The layer-shell namespace of the overview's window.
pub const HOLE_NAMESPACE: &str = "test-overview";
/// The key of the overview window's surface layer that is the hole.
const HOLE_LAYER: u64 = 1;
/// The key of the overview's window.
const HOLE_WINDOW: &str = "holes";

/// How long the `notifications` loop, and then the render thread, stay out of their wait: long enough for the host's
/// checks to see them busy for most of a second, short of the time after which the host reports the component
/// unresponsive.
pub const STALL: Duration = Duration::from_millis(1800);
/// How long the `notifications` helper runs, ignoring its stop: well past `hosting`'s helper join timeout.
const STUBBORN: Duration = Duration::from_secs(4);

/// What the dock publishes: retained, so the shared host's components get it once the bridge is up.
pub const LAUNCHER_SHOWN: Message = Message::PanelShown {
    panel: Panel::Launcher,
    shown: true,
    output: None,
};

/// What the `osd` publishes: retained, so a separate host's dock gets it once the bridge is up.
pub fn dnd() -> Message {
    Message::DoNotDisturbChanged {
        on: true,
        lifetimes: Vec::new(),
    }
}

/// The file the host's components report to.
static REPORT: OnceLock<PathBuf> = OnceLock::new();

/// Report to `path` from now on.
pub fn report_to(path: PathBuf) {
    let _ = REPORT.set(path);
}

/// Append `line` to the report: one write, so lines of several threads and processes never mix.
pub fn report(line: impl AsRef<str>) {
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

/// The outputs of the calling component as one report line: the name, size and scale of each.
fn outputs_line(component: &str) -> String {
    let mut line = format!("{component} outputs");
    for output in outputs::all() {
        let size = output
            .geometry
            .map(|g| format!("{}x{}", g.width(), g.height()))
            .unwrap_or_else(|| "none".into());
        line += &format!(
            " {}:{size}@{}",
            output.name.unwrap_or_default(),
            output.scale
        );
    }
    line
}

/// A rectangle of one colour; with `layer`, the surface layer of that key.
struct Block {
    size: Size,
    color: Color,
    layer: Option<u64>,
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

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if old.size != self.size || old.layer != self.layer {
            Change::Layout
        } else if old.color != self.color {
            Change::Paint
        } else {
            Change::None
        }
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

    fn surface_layer(&self, _: &()) -> Option<SurfaceLayerMark> {
        self.layer.map(|key| SurfaceLayerMark {
            key,
            look: SurfaceLayerLook::default(),
        })
    }
}

/// A window `key` of `size` points showing `block` all over.
fn window<M: Clone + 'static>(key: &'static str, size: Size, block: Block) -> Window<M> {
    Window {
        key: WindowKey(Key::from(key)),
        spec: WindowSpec {
            title: key.to_owned(),
            app_id: format!("org.cupertino.test.{key}"),
            size,
            min_size: None,
            max_size: None,
            decorations: DecorationMode::None,
            foreign_parent: None,
            input_region: None,
        },
        shortcuts: CommandRegistry::new(),
        on_close: None,
        background: WindowBackground::None,
        content: Element::new(block),
        popup: None,
        menus: None,
    }
}

/// A layer surface of `size` points on the server's choice of output.
fn layer(
    namespace: &'static str,
    layer: Layer,
    anchor: Anchor,
    size: Size,
    zone: i32,
) -> LayerSpec {
    LayerSpec {
        namespace,
        output: None,
        layer,
        anchor,
        size,
        margin: Margin::default(),
        exclusive_zone: zone,
        keyboard: Keyboard::None,
    }
}

/// What a component hears on the host bus, as one report line.
fn heard(component: &str, from: Sender, message: &Message) -> String {
    let from = match from {
        Sender::Host => "host",
        Sender::Component(name) => name.as_str(),
    };
    let kind = match message {
        Message::PanelShown { .. } => "PanelShown",
        Message::DoNotDisturbChanged { .. } => "DoNotDisturbChanged",
        Message::ComponentStopped { .. } => "ComponentStopped",
        _ => "other",
    };
    format!("{component} heard {from} {kind}")
}

/// The `osd` component.
pub struct Osd;

impl Application for Osd {
    type Model = ();
    type Message = String;
    const RENDERER: RendererClass = RendererClass::Cpu;

    fn init() -> ((), Vec<Command<String>>) {
        report(outputs_line("osd"));
        report(format!("osd holds {}", holds().join(",")));
        bus::follow(&[Topic::Panels]);
        bus::publish(dnd());
        ((), Vec::new())
    }

    fn update(_: &mut (), line: String) -> Vec<Command<String>> {
        report(line);
        Vec::new()
    }

    fn view(_: &()) -> Vec<Window<String>> {
        vec![window(
            HELLO_KEY,
            HELLO_SIZE,
            Block {
                size: HELLO_SIZE,
                color: GREEN,
                layer: None,
            },
        )]
    }
}

impl Program for Osd {
    fn surface(_: &(), _: &WindowKey) -> Surface {
        Surface {
            role: Role::Layer(layer(
                HELLO_NAMESPACE,
                Layer::Overlay,
                Anchor::default(),
                HELLO_SIZE,
                0,
            )),
            persistent: Some(HELLO_KEY),
            holes: Vec::new(),
        }
    }

    fn event(event: Event) -> Option<String> {
        match event {
            Event::Outputs => Some(outputs_line("osd")),
            Event::Bus { from, message } => Some(heard("osd", from, &message)),
            _ => None,
        }
    }
}

impl Component for Osd {
    const NAME: ComponentName = ComponentName::OSD;
}

/// The `menubar` component.
pub struct MenuBar;

impl Application for MenuBar {
    type Model = ();
    type Message = ();
    const RENDERER: RendererClass = RendererClass::Cpu;

    fn init() -> ((), Vec<Command<()>>) {
        report(format!("menubar holds {}", holds().join(",")));
        ((), Vec::new())
    }

    fn update(_: &mut (), _: ()) -> Vec<Command<()>> {
        Vec::new()
    }

    fn view(_: &()) -> Vec<Window<()>> {
        let size = Size::new(bar_width(), bar_height());
        vec![window(
            BAR_KEY,
            size,
            Block {
                size,
                color: BLUE,
                layer: None,
            },
        )]
    }
}

/// The width of the first output, which the bar spans, in points.
fn bar_width() -> f64 {
    outputs::all()
        .first()
        .and_then(|o| o.geometry)
        .map_or(800.0, |g| g.width())
}

impl Program for MenuBar {
    fn surface(_: &(), _: &WindowKey) -> Surface {
        let across = Anchor {
            top: true,
            left: true,
            right: true,
            bottom: false,
        };
        Surface {
            role: Role::MenuBar(layer(
                BAR_NAMESPACE,
                Layer::Top,
                across,
                Size::new(0.0, bar_height()),
                bar_height() as i32,
            )),
            persistent: Some(BAR_KEY),
            holes: Vec::new(),
        }
    }

    fn event(_: Event) -> Option<()> {
        None
    }
}

impl Component for MenuBar {
    const NAME: ComponentName = ComponentName::MENUBAR;
}

/// The `dock` component.
pub struct Dock;

impl Application for Dock {
    type Model = ();
    type Message = String;
    const RENDERER: RendererClass = RendererClass::Cpu;

    fn init() -> ((), Vec<Command<String>>) {
        report(format!("dock holds {}", holds().join(",")));
        bus::follow(&[Topic::DoNotDisturb]);
        bus::publish(LAUNCHER_SHOWN);
        ((), Vec::new())
    }

    fn update(_: &mut (), line: String) -> Vec<Command<String>> {
        report(line);
        Vec::new()
    }

    fn view(_: &()) -> Vec<Window<String>> {
        vec![window(
            DOCK_KEY,
            DOCK_SIZE,
            Block {
                size: DOCK_SIZE,
                color: RED,
                layer: None,
            },
        )]
    }
}

impl Program for Dock {
    fn surface(_: &(), _: &WindowKey) -> Surface {
        let bottom = Anchor {
            bottom: true,
            ..Anchor::default()
        };
        Surface {
            role: Role::Dock(layer(DOCK_NAMESPACE, Layer::Top, bottom, DOCK_SIZE, 0)),
            persistent: Some(DOCK_KEY),
            holes: Vec::new(),
        }
    }

    fn event(event: Event) -> Option<String> {
        match event {
            Event::Bus { from, message } => Some(heard("dock", from, &message)),
            _ => None,
        }
    }
}

impl Component for Dock {
    const NAME: ComponentName = ComponentName::DOCK;
}

/// The `overview` component.
pub struct Overview;

/// The window the overview claims, and the transaction that shows the claim.
#[derive(Default)]
pub struct OverviewModel {
    claimed: Option<(OutputId, ServerWindowId)>,
    transaction: Option<transaction::Handle>,
}

#[derive(Clone, Debug)]
pub enum OverviewMessage {
    Window(OutputId, ServerWindowId),
    Started,
}

impl Application for Overview {
    type Model = OverviewModel;
    type Message = OverviewMessage;
    const RENDERER: RendererClass = RendererClass::Cpu;

    fn init() -> (OverviewModel, Vec<Command<OverviewMessage>>) {
        (OverviewModel::default(), Vec::new())
    }

    fn update(
        model: &mut OverviewModel,
        message: OverviewMessage,
    ) -> Vec<Command<OverviewMessage>> {
        match message {
            OverviewMessage::Window(output, id) if model.claimed.is_none() => {
                model.claimed = Some((output, id));
                // the window that shows the hole comes with this view: its first commit is the transaction's
                let mut part = transaction::create(output);
                part.stage(&WindowKey(Key::from(HOLE_WINDOW)));
                model.transaction = Some(part.submit());
                report(format!("overview claims {}", id.0));
            }
            OverviewMessage::Window(..) => {}
            OverviewMessage::Started => report("overview started"),
        }
        Vec::new()
    }

    fn view(model: &OverviewModel) -> Vec<Window<OverviewMessage>> {
        if model.claimed.is_none() {
            return Vec::new();
        }
        vec![window(
            HOLE_WINDOW,
            HOLE_SIZE,
            Block {
                size: HOLE_SIZE,
                color: Color::new(0.0, 0.0, 0.0, 0.0),
                layer: Some(HOLE_LAYER),
            },
        )]
    }
}

impl Program for Overview {
    fn surface(model: &OverviewModel, _: &WindowKey) -> Surface {
        let top_left = Anchor {
            top: true,
            left: true,
            ..Anchor::default()
        };
        let (output, holes) = match model.claimed {
            Some((output, id)) => (
                Some(output),
                vec![Hole {
                    layer: HOLE_LAYER,
                    source: HoleSource::Window(id),
                    claim: true,
                }],
            ),
            None => (None, Vec::new()),
        };
        Surface {
            role: Role::Layer(LayerSpec {
                output,
                ..layer(HOLE_NAMESPACE, Layer::Overlay, top_left, HOLE_SIZE, 0)
            }),
            persistent: None,
            holes,
        }
    }

    fn event(event: Event) -> Option<OverviewMessage> {
        match event {
            Event::Overview(output, overview::Update { windows, .. }) => windows
                .iter()
                .find(|w| w.app_id == HOLE_APP_ID)
                .map(|w| OverviewMessage::Window(output, w.id)),
            Event::Transaction(transaction::Event::Started { .. }) => {
                Some(OverviewMessage::Started)
            }
            _ => None,
        }
    }
}

impl Component for Overview {
    const NAME: ComponentName = ComponentName::OVERVIEW;
}

/// The `notifications` component, through the stages of its stalls.
pub struct Notifications;

#[derive(Clone, Debug)]
pub enum Stage {
    /// The loop stalls.
    Loop,
    /// The render thread stalls.
    Render,
    /// A helper that ignores its stop starts.
    Stubborn,
    /// The helper ended, which the host never waits for.
    Done,
}

impl Application for Notifications {
    type Model = ();
    type Message = Stage;
    const RENDERER: RendererClass = RendererClass::Cpu;

    fn init() -> ((), Vec<Command<Stage>>) {
        report("notifications started");
        ((), vec![Command::Perform(Box::pin(async { Stage::Loop }))])
    }

    fn update(_: &mut (), stage: Stage) -> Vec<Command<Stage>> {
        match stage {
            Stage::Loop => {
                std::thread::sleep(STALL);
                report("notifications loop stalled");
                // a pause the host sees the loop wait in, before the render thread stalls
                vec![host::thread(
                    "pause",
                    |_| std::thread::sleep(STALL),
                    |()| Stage::Render,
                )]
            }
            Stage::Render => {
                cupertino_ui_render::render_thread::hand_over(|| std::thread::sleep(STALL));
                report("notifications render stalls");
                vec![host::thread(
                    "pause",
                    |_| std::thread::sleep(STALL * 2),
                    |()| Stage::Stubborn,
                )]
            }
            Stage::Stubborn => {
                report("notifications helper ignores its stop");
                vec![host::thread(
                    "stubborn",
                    |_| std::thread::sleep(STUBBORN),
                    |()| Stage::Done,
                )]
            }
            Stage::Done => Vec::new(),
        }
    }

    fn view(_: &()) -> Vec<Window<Stage>> {
        Vec::new()
    }
}

impl Program for Notifications {
    fn surface(_: &(), _: &WindowKey) -> Surface {
        Surface {
            role: Role::Window,
            persistent: None,
            holes: Vec::new(),
        }
    }

    fn event(_: Event) -> Option<Stage> {
        None
    }
}

impl Component for Notifications {
    const NAME: ComponentName = ComponentName::NOTIFICATIONS;
}

pub static OSD: Entry = Entry::of::<Osd>();
pub static MENUBAR: Entry = Entry::of::<MenuBar>();
pub static DOCK: Entry = Entry::of::<Dock>();
pub static OVERVIEW: Entry = Entry::of::<Overview>();
pub static NOTIFICATIONS: Entry = Entry::of::<Notifications>();

/// The components this host was built with.
pub static ENTRIES: [&Entry; 5] = [&OSD, &MENUBAR, &DOCK, &OVERVIEW, &NOTIFICATIONS];
