//! The test components the host of [`super::host`] runs with `cupertino-shell`'s host program, each a [`Component`]
//! of the server's table on Kit's UI loop:
//! - `dock`: a persistent layer window at the output's bottom left whose colour turns from [`RED`] to [`BLUE`] and
//!   back every [`TICK`], so it is seen drawing; it reports what it hears from the host on the host bus;
//! - `notifications`: its first instance panics as it handles a message and again as its model is dropped in the
//!   teardown; its second panics on a helper thread; the instances after it report that they run;
//! - `controlcenter`: every instance panics as it handles its first message;
//! - `launcher`: its first instance in the session waits [`HANG_AFTER`], past the host's start-up grace, then its
//!   loop never returns to its wait; later instances report that they run;
//! - `menubar`: a persistent layer window at the output's top left whose colour turns from [`GREEN`] to [`YELLOW`]
//!   and back every [`TICK`]; it opens its session bus connection and reports its unique name with its instance.
//!
//! - `osd`: the real on-screen displays of `cupertino-osd`, as `cupertino-shell` runs them.
//!
//! What a component reports goes to the report file ([`report`]) as one line.

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use cupertino_kit::core::{
    Application, Change, Command, CommandRegistry, Element, Event as UiEvent, EventCx, Key,
    MeasureCx, PaintCx, Propagation, RendererClass, SemanticsCx, Widget, Window, WindowBackground,
    WindowKey,
};
use cupertino_kit::geometry::{Rect, Size};
use cupertino_kit::layout::{Constraints, ContainerKind, LayoutSpec, Measured, SizeSpec};
use cupertino_kit::platform::{DecorationMode, WindowSpec};
use cupertino_kit::render::{Paint, PaintOp, Shape};
use cupertino_kit::theme::Color;
use cupertino_kit_shell::bus::{self, Message, Sender, Topic};
use cupertino_kit_shell::{
    host, Anchor, Component, ComponentName, Entry, Event, Keyboard, Layer, LayerSpec, Margin,
    Program, Role, Surface,
};

/// The dock's two colours.
pub const RED: Color = Color::new(1.0, 0.0, 0.0, 1.0);
pub const BLUE: Color = Color::new(0.0, 0.0, 1.0, 1.0);
/// The size of the dock's window, in points, at the output's bottom left.
pub const DOCK_SIZE: Size = Size::new(160.0, 60.0);
/// How often the dock turns its colour.
pub const TICK: Duration = Duration::from_millis(150);
/// How long the launcher's first instance waits before its loop hangs: past the host's start-up grace.
pub const HANG_AFTER: Duration = Duration::from_millis(10_500);

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

/// Whether the report already holds `line`, from this host process or another.
fn reported(line: &str) -> bool {
    REPORT.get().is_some_and(|path| {
        std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .any(|l| l == line)
    })
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

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if old.size != self.size {
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
}

/// A component with no window of its own.
fn windowless() -> Surface {
    Surface {
        role: Role::Window,
        persistent: None,
        holes: Vec::new(),
    }
}

/// The `dock` component.
pub struct Dock;

#[derive(Clone, Debug)]
pub enum DockMessage {
    Tick,
    Heard(Sender, Message),
}

impl Application for Dock {
    /// Whether it shows red.
    type Model = bool;
    type Message = DockMessage;
    const RENDERER: RendererClass = RendererClass::Cpu;

    fn init() -> (bool, Vec<Command<DockMessage>>) {
        bus::follow(&[Topic::Host]);
        report("dock started");
        (
            true,
            vec![Command::After {
                delay: TICK,
                message: DockMessage::Tick,
            }],
        )
    }

    fn update(red: &mut bool, message: DockMessage) -> Vec<Command<DockMessage>> {
        match message {
            DockMessage::Tick => {
                *red = !*red;
                vec![Command::After {
                    delay: TICK,
                    message: DockMessage::Tick,
                }]
            }
            DockMessage::Heard(Sender::Host, Message::ComponentStopped { component }) => {
                report(format!(
                    "dock heard host ComponentStopped {}",
                    component.as_str()
                ));
                Vec::new()
            }
            DockMessage::Heard(..) => Vec::new(),
        }
    }

    fn view(red: &bool) -> Vec<Window<DockMessage>> {
        vec![Window {
            key: WindowKey(Key::from("dock")),
            spec: WindowSpec {
                title: "dock".to_owned(),
                app_id: "org.cupertino.test.dock".to_owned(),
                size: DOCK_SIZE,
                min_size: None,
                max_size: None,
                decorations: DecorationMode::None,
                foreign_parent: None,
                input_region: None,
            },
            shortcuts: CommandRegistry::new(),
            on_close: None,
            background: WindowBackground::None,
            content: Element::new(Block {
                size: DOCK_SIZE,
                color: if *red { RED } else { BLUE },
            }),
            popup: None,
            menus: None,
        }]
    }
}

impl Program for Dock {
    fn surface(_: &bool, _: &WindowKey) -> Surface {
        Surface {
            role: Role::Layer(LayerSpec {
                namespace: "test-dock",
                output: None,
                layer: Layer::Top,
                anchor: Anchor {
                    bottom: true,
                    left: true,
                    ..Anchor::default()
                },
                size: DOCK_SIZE,
                margin: Margin::default(),
                exclusive_zone: 0,
                keyboard: Keyboard::None,
            }),
            persistent: Some("dock"),
            holes: Vec::new(),
        }
    }

    fn event(event: Event) -> Option<DockMessage> {
        match event {
            Event::Bus { from, message } => Some(DockMessage::Heard(from, message)),
            _ => None,
        }
    }
}

impl Component for Dock {
    const NAME: ComponentName = ComponentName::DOCK;
}

/// How many instances of `notifications` this host process started.
static NOTIFICATIONS_STARTED: AtomicU32 = AtomicU32::new(0);

/// The `notifications` component.
pub struct Notifications;

/// A model whose drop panics: the teardown of the instance that holds it panics a second time, which would abort the
/// host were the model dropped while the loop's panic unwinds.
pub struct Brittle(bool);

impl Drop for Brittle {
    fn drop(&mut self) {
        if self.0 {
            report("notifications teardown panics");
            panic!("the notifications' model panics as it is dropped");
        }
    }
}

impl Application for Notifications {
    type Model = Brittle;
    type Message = ();
    const RENDERER: RendererClass = RendererClass::Cpu;

    fn init() -> (Brittle, Vec<Command<()>>) {
        let instance = NOTIFICATIONS_STARTED.fetch_add(1, Ordering::SeqCst) + 1;
        report(format!("notifications started {instance}"));
        match instance {
            1 => (Brittle(true), vec![Command::Perform(Box::pin(async {}))]),
            2 => {
                let _task = host::thread(
                    "fragile",
                    |_| panic!("a helper of the notifications"),
                    |()| (),
                );
                (Brittle(false), Vec::new())
            }
            _ => {
                report("notifications steady");
                (Brittle(false), Vec::new())
            }
        }
    }

    fn update(_: &mut Brittle, (): ()) -> Vec<Command<()>> {
        report("notifications panics");
        panic!("the notifications panic as they handle a message");
    }

    fn view(_: &Brittle) -> Vec<Window<()>> {
        Vec::new()
    }
}

impl Program for Notifications {
    fn surface(_: &Brittle, _: &WindowKey) -> Surface {
        windowless()
    }

    fn event(_: Event) -> Option<()> {
        None
    }
}

impl Component for Notifications {
    const NAME: ComponentName = ComponentName::NOTIFICATIONS;
}

/// The `controlcenter` component.
pub struct ControlCenter;

impl Application for ControlCenter {
    type Model = ();
    type Message = ();
    const RENDERER: RendererClass = RendererClass::Cpu;

    fn init() -> ((), Vec<Command<()>>) {
        report("controlcenter started");
        ((), vec![Command::Perform(Box::pin(async {}))])
    }

    fn update(_: &mut (), (): ()) -> Vec<Command<()>> {
        panic!("the control panel panics at every start");
    }

    fn view(_: &()) -> Vec<Window<()>> {
        Vec::new()
    }
}

impl Program for ControlCenter {
    fn surface(_: &(), _: &WindowKey) -> Surface {
        windowless()
    }

    fn event(_: Event) -> Option<()> {
        None
    }
}

impl Component for ControlCenter {
    const NAME: ComponentName = ComponentName::CONTROLCENTER;
}

/// The `launcher` component.
pub struct Launcher;

impl Application for Launcher {
    type Model = ();
    type Message = ();
    const RENDERER: RendererClass = RendererClass::Cpu;

    fn init() -> ((), Vec<Command<()>>) {
        if reported("launcher started") {
            report("launcher runs again");
            return ((), Vec::new());
        }
        report("launcher started");
        (
            (),
            vec![Command::After {
                delay: HANG_AFTER,
                message: (),
            }],
        )
    }

    fn update(_: &mut (), (): ()) -> Vec<Command<()>> {
        report("launcher hangs");
        loop {
            std::thread::park();
        }
    }

    fn view(_: &()) -> Vec<Window<()>> {
        Vec::new()
    }
}

impl Program for Launcher {
    fn surface(_: &(), _: &WindowKey) -> Surface {
        windowless()
    }

    fn event(_: Event) -> Option<()> {
        None
    }
}

impl Component for Launcher {
    const NAME: ComponentName = ComponentName::LAUNCHER;
}

/// The menu bar's two colours.
pub const GREEN: Color = Color::new(0.0, 1.0, 0.0, 1.0);
pub const YELLOW: Color = Color::new(1.0, 1.0, 0.0, 1.0);
/// The size of the menu bar's window, in points, at the output's top left.
pub const BAR_SIZE: Size = Size::new(160.0, 40.0);

/// How many instances of `menubar` this host process started.
static MENUBAR_STARTED: AtomicU32 = AtomicU32::new(0);

/// The `menubar` component.
pub struct MenuBar;

#[derive(Clone, Debug)]
pub enum BarMessage {
    Tick,
    Connected(Option<zbus::Connection>),
    Closed,
}

/// The menu bar's model: its instance, whether it shows green, and its bus connection, kept open.
pub struct Bar {
    instance: u32,
    green: bool,
    bus: Option<zbus::Connection>,
}

impl Application for MenuBar {
    type Model = Bar;
    type Message = BarMessage;
    const RENDERER: RendererClass = RendererClass::Cpu;

    fn init() -> (Bar, Vec<Command<BarMessage>>) {
        let instance = MENUBAR_STARTED.fetch_add(1, Ordering::SeqCst) + 1;
        let mut commands = bus::session(
            |connection| BarMessage::Connected(connection.ok()),
            BarMessage::Closed,
        );
        commands.push(Command::After {
            delay: TICK,
            message: BarMessage::Tick,
        });
        (
            Bar {
                instance,
                green: true,
                bus: None,
            },
            commands,
        )
    }

    fn update(bar: &mut Bar, message: BarMessage) -> Vec<Command<BarMessage>> {
        match message {
            BarMessage::Tick => {
                bar.green = !bar.green;
                vec![Command::After {
                    delay: TICK,
                    message: BarMessage::Tick,
                }]
            }
            BarMessage::Connected(connection) => {
                let name = connection
                    .as_ref()
                    .and_then(|c| c.unique_name().map(|n| n.to_string()));
                report(format!(
                    "menubar {} bus {}",
                    bar.instance,
                    name.as_deref().unwrap_or("none")
                ));
                bar.bus = connection;
                Vec::new()
            }
            BarMessage::Closed => {
                bar.bus = None;
                Vec::new()
            }
        }
    }

    fn view(bar: &Bar) -> Vec<Window<BarMessage>> {
        vec![Window {
            key: WindowKey(Key::from("bar")),
            spec: WindowSpec {
                title: "bar".to_owned(),
                app_id: "org.cupertino.test.bar".to_owned(),
                size: BAR_SIZE,
                min_size: None,
                max_size: None,
                decorations: DecorationMode::None,
                foreign_parent: None,
                input_region: None,
            },
            shortcuts: CommandRegistry::new(),
            on_close: None,
            background: WindowBackground::None,
            content: Element::new(Block {
                size: BAR_SIZE,
                color: if bar.green { GREEN } else { YELLOW },
            }),
            popup: None,
            menus: None,
        }]
    }
}

impl Program for MenuBar {
    fn surface(_: &Bar, _: &WindowKey) -> Surface {
        Surface {
            role: Role::Layer(LayerSpec {
                namespace: "test-bar",
                output: None,
                layer: Layer::Top,
                anchor: Anchor {
                    top: true,
                    left: true,
                    ..Anchor::default()
                },
                size: BAR_SIZE,
                margin: Margin::default(),
                exclusive_zone: 0,
                keyboard: Keyboard::None,
            }),
            persistent: Some("bar"),
            holes: Vec::new(),
        }
    }

    fn event(_: Event) -> Option<BarMessage> {
        None
    }
}

impl Component for MenuBar {
    const NAME: ComponentName = ComponentName::MENUBAR;
}

pub static MENUBAR: Entry = Entry::of::<MenuBar>();
pub static DOCK: Entry = Entry::of::<Dock>();
pub static NOTIFICATIONS: Entry = Entry::of::<Notifications>();
pub static CONTROLCENTER: Entry = Entry::of::<ControlCenter>();
pub static LAUNCHER: Entry = Entry::of::<Launcher>();

/// The components this host was built with.
/// The test components and the real on-screen displays (`cupertino_shell::OSD`).
pub static ENTRIES: [&Entry; 6] = [
    &MENUBAR,
    &DOCK,
    &NOTIFICATIONS,
    &CONTROLCENTER,
    &LAUNCHER,
    &cupertino_shell::OSD,
];
