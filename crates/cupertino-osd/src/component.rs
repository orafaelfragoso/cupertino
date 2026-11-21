//! The on-screen displays as a component of the shell host (`cupertino-shell`): the overlay's window on a layer
//! surface, `org.cupertino.Osd1` on the component's own session bus connection, the commands carried out one at a
//! time through the services that own the levels, and the hold and the fade on the UI loop's clock and the display's
//! frames; and the keyboard's two panels, the input-source switcher and the accent picker, which the server sends
//! through `cupertino_kit_shell::keyboard_panels` and which never take the keyboard.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use cupertino_kit::bus::{self as kit_bus, Bus};
use cupertino_kit::core::{Application, Command as Do, Key, RendererClass, Window, WindowKey};
use cupertino_kit::geometry::{Rect, Size};
use cupertino_kit_shell::bus::{self, Message as Published, OsdKind, Topic};
use cupertino_kit_shell::keyboard_panels::{self, Update};
use cupertino_kit_shell::outputs::{self, OutputId};
use cupertino_kit_shell::{
    Anchor, Component, ComponentName, Event, Keyboard, Layer, LayerSpec, Margin, Program, Role,
    Surface,
};

use crate::controller::{Command, EffectError, Shown};
use crate::effects::{self, Buses};
use crate::layout::{BOTTOM_MARGIN, WINDOW_SIZE};
use crate::model::{Hud, Kind, Overlay, Phase};
use crate::panels::{Panels, LABEL_SIZE};
use crate::panels_view::{self, accents_key, sources_key};
use crate::sender::hold_seconds;
use crate::{control, view};

/// The layer surface's namespace, for the server.
const NAMESPACE: &str = "cupertino-osd";

/// The on-screen displays.
pub struct Osd;

/// What the component holds.
pub struct Model {
    hud: Hud,
    /// The start of the model's clock, which the hold and the fade are measured on.
    start: Instant,
    buses: Buses,
    /// The system bus connection is being made, for a backlight command waiting for it.
    connecting: bool,
    /// Where `org.cupertino.Osd1` hands the calls it accepted, and where they come out.
    calls: (
        async_channel::Sender<Command>,
        async_channel::Receiver<Command>,
    ),
    /// The commands not yet carried out, in the order they came.
    pending: VecDeque<Command>,
    /// A command is being carried out.
    busy: bool,
    /// The hold that counts; the end of an earlier one is ignored.
    hold: u64,
    /// The server's first output, which the overlay shows on; none while it has none.
    output: Option<OutputId>,
    /// The window's generation: a window the server closed, or one on an output that went, is made anew.
    window: u64,
    /// The keyboard's panels as the server last described them.
    panels: Panels,
    /// The switcher's panel's size, measured from its labels as its list came.
    sources_size: Size,
}

#[derive(Clone)]
pub enum Message {
    /// The component's session bus connection, or none.
    Session(Option<zbus::Connection>),
    SessionClosed,
    /// Whether `org.cupertino.Osd1` is served.
    Served(Result<(), String>),
    /// The system bus connection, or none.
    System(Option<zbus::Connection>),
    SystemClosed,
    Command(Command),
    /// A command was carried out.
    Done(Result<Shown, EffectError>),
    /// The hold numbered so ended.
    HoldEnded(u64),
    /// A frame of the display, while the overlay fades.
    Frame,
    /// The server's outputs changed.
    Outputs,
    /// The server closed the window of this key.
    Closed(WindowKey),
    /// What the server says of the keyboard's panels.
    Panels(Update),
    /// A frame of the display, while the switcher's panel waits for its first frame or fades.
    SourcesFrame,
    /// The pointer was released on candidate `index` of the picker of `serial`.
    Pick {
        serial: u32,
        index: usize,
    },
}

impl Model {
    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    /// The next command, when none is being carried out: a backlight one waits for the system bus connection, made
    /// for it.
    fn next(&mut self) -> Vec<Do<Message>> {
        let Some(&command) = self.pending.front().filter(|_| !self.busy) else {
            return Vec::new();
        };
        if command.backlight() && self.buses.system.is_none() {
            if self.connecting {
                return Vec::new();
            }
            self.connecting = true;
            return kit_bus::connect(
                Bus::System,
                |connection| Message::System(connection.ok()),
                Message::SystemClosed,
            );
        }
        self.pending.pop_front();
        self.busy = true;
        let buses = self.buses.clone();
        vec![Do::Perform(Box::pin(async move {
            Message::Done(effects::apply(command, buses).await)
        }))]
    }

    /// Show what a command did and time the end of its hold.
    fn show(&mut self, shown: Shown) -> Vec<Do<Message>> {
        let now = self.now();
        self.hud
            .show(now, shown.overlay, hold_seconds(shown.changed));
        self.hold += 1;
        match self.hud.until_fade(now) {
            Some(wait) => vec![Do::After {
                delay: Duration::from_secs_f64(wait),
                message: Message::HoldEnded(self.hold),
            }],
            None => Vec::new(),
        }
    }
}

/// The next call `org.cupertino.Osd1` accepted.
fn listen(calls: &async_channel::Receiver<Command>) -> Do<Message> {
    let calls = calls.clone();
    Do::Perform(Box::pin(async move {
        match calls.recv().await {
            Ok(command) => Message::Command(command),
            // the model holds the sender, so the channel never closes while it lives
            Err(_) => std::future::pending().await,
        }
    }))
}

/// The server's first output, if it has one.
fn first_output() -> Option<OutputId> {
    outputs::all()
        .into_iter()
        .find(|o| o.geometry.is_some())
        .map(|o| o.id)
}

/// The overlay a level published on the host bus shows.
fn published(kind: OsdKind, level: f64) -> Option<Overlay> {
    let kind = match kind {
        OsdKind::Volume => Kind::Volume,
        OsdKind::Mute => Kind::Mute,
        OsdKind::Brightness => Kind::Brightness,
        OsdKind::KeyboardBrightness => Kind::KeyboardBrightness,
    };
    Overlay::new(kind, level).ok()
}

impl Application for Osd {
    type Model = Model;
    type Message = Message;
    const RESIDENT: bool = true;
    /// A small overlay shown now and then: a GPU context would only cost memory.
    const RENDERER: RendererClass = RendererClass::Cpu;

    fn init() -> (Model, Vec<Do<Message>>) {
        let calls = async_channel::unbounded();
        bus::follow(&[Topic::Osd]);
        let mut commands = bus::session(
            |connection| Message::Session(connection.ok()),
            Message::SessionClosed,
        );
        commands.push(listen(&calls.1));
        let model = Model {
            hud: Hud::new(),
            start: Instant::now(),
            buses: Buses::default(),
            connecting: false,
            calls,
            pending: VecDeque::new(),
            busy: false,
            hold: 0,
            output: first_output(),
            window: 0,
            panels: Panels::default(),
            sources_size: Size::ZERO,
        };
        (model, commands)
    }

    fn update(model: &mut Model, message: Message) -> Vec<Do<Message>> {
        match message {
            Message::Session(Some(connection)) => {
                model.buses.session = Some(connection.clone());
                let calls = model.calls.0.clone();
                let mut commands = vec![Do::Perform(Box::pin(async move {
                    Message::Served(
                        control::serve(&connection, calls)
                            .await
                            .map_err(|e| e.to_string()),
                    )
                }))];
                commands.extend(model.next());
                commands
            }
            Message::Session(None) | Message::SessionClosed => {
                // without its bus connection the component serves nobody: the host starts it again
                eprintln!("cupertino-osd: no session bus connection");
                vec![Do::Exit]
            }
            Message::Served(Ok(())) => Vec::new(),
            Message::Served(Err(e)) => {
                eprintln!("cupertino-osd: {} is not served: {e}", control::BUS_NAME);
                vec![Do::Exit]
            }
            Message::System(connection) => {
                model.connecting = false;
                model.buses.system = connection;
                if model.buses.system.is_none() {
                    // the backlight command that waited for it fails; the next one tries again
                    if let Some(command) = model.pending.pop_front() {
                        eprintln!(
                            "cupertino-osd: {} failed, nothing shown: no system bus connection",
                            command.name()
                        );
                    }
                }
                model.next()
            }
            Message::SystemClosed => {
                model.buses.system = None;
                Vec::new()
            }
            Message::Command(command) => {
                model.pending.push_back(command);
                let mut commands = model.next();
                commands.push(listen(&model.calls.1));
                commands
            }
            Message::Done(result) => {
                model.busy = false;
                let mut commands = match result {
                    Ok(shown) => model.show(shown),
                    // a failure shows nothing
                    Err(e) => {
                        eprintln!("cupertino-osd: {e}");
                        Vec::new()
                    }
                };
                commands.extend(model.next());
                commands
            }
            // the view draws the fade from the model's clock; these only make it look again
            Message::HoldEnded(_) | Message::Frame => Vec::new(),
            Message::Outputs => {
                let output = first_output();
                if output != model.output {
                    model.output = output;
                    model.window += 1;
                }
                Vec::new()
            }
            Message::Closed(key) => {
                // a panel the server dismissed is gone until the server sends it again
                if model
                    .panels
                    .sources
                    .as_ref()
                    .is_some_and(|p| key.0 == Key::from(sources_key(p)))
                {
                    model.panels.sources = None;
                } else if model
                    .panels
                    .accents
                    .as_ref()
                    .is_some_and(|p| key.0 == Key::from(accents_key(p)))
                {
                    model.panels.accents = None;
                } else {
                    // a new window, on the output the server still has; while it has none there is no window
                    model.window += 1;
                }
                Vec::new()
            }
            Message::Panels(update) => {
                let sources = matches!(update, Update::Sources { .. });
                let now = model.now();
                model.panels.apply(update, now);
                if let (true, Some(panel)) = (sources, model.panels.sources.as_ref()) {
                    let widest = panel
                        .sources
                        .iter()
                        .map(|s| panels_view::text_width(&s.name, LABEL_SIZE, 400))
                        .fold(0.0, f64::max);
                    model.sources_size = panel.size(widest);
                }
                Vec::new()
            }
            Message::SourcesFrame => {
                let now = model.now();
                model.panels.sources_frame(now);
                Vec::new()
            }
            Message::Pick { serial, index } => {
                keyboard_panels::accent_pick(serial, index as u32);
                Vec::new()
            }
        }
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        let mut windows = Vec::new();
        let now = model.now();
        if model.output.is_some() {
            let fading = model.hud.phase(now) == Phase::Fading;
            let mut window = view::window(model.hud.view(now), fading.then_some(Message::Frame));
            window.key = WindowKey(Key::from(format!("osd-{}", model.window)));
            windows.push(window);
        }
        if let Some(panel) = &model.panels.sources {
            let frame = panel.wants_frames().then_some(Message::SourcesFrame);
            windows.push(panels_view::sources_window(
                panel,
                model.sources_size,
                panel.alpha(now),
                frame,
            ));
        }
        if let Some((panel, layout)) = model.panels.accents.as_ref().and_then(|panel| {
            let size = output_size(panel.output)?;
            Some((panel, panel.layout(size)))
        }) {
            let serial = panel.serial;
            windows.push(panels_view::accents_window(panel, &layout, |index| {
                Message::Pick { serial, index }
            }));
        }
        windows
    }
}

/// The output a panel shows on: the one the server named, else the first one.
fn panel_output(output: Option<OutputId>) -> Option<OutputId> {
    output.or_else(first_output)
}

/// The size in points of the output a panel shows on.
fn output_size(output: Option<OutputId>) -> Option<Size> {
    let geometry = outputs::get(panel_output(output)?)?.geometry?;
    Some(geometry.size)
}

/// A panel's layer surface: on the overlay layer of `output`, never taking the keyboard.
fn panel_surface(
    namespace: &'static str,
    output: Option<OutputId>,
    anchor: Anchor,
    rect: Rect,
) -> Surface {
    Surface {
        role: Role::Layer(LayerSpec {
            namespace,
            output: panel_output(output),
            layer: Layer::Overlay,
            anchor,
            size: rect.size,
            margin: Margin {
                top: rect.y().round() as i32,
                left: rect.x().round() as i32,
                ..Margin::default()
            },
            exclusive_zone: -1,
            keyboard: Keyboard::None,
        }),
        persistent: None,
        holes: Vec::new(),
    }
}

impl Program for Osd {
    /// The overlay: on the overlay layer of the server's first output, anchored to the bottom only, so it is centred
    /// horizontally `BOTTOM_MARGIN` above the edge, with no keyboard. The switcher's panel: centred on its output (no
    /// anchor). The picker's panel: anchored top and left at the origin of its layout's rectangle.
    fn surface(model: &Model, key: &WindowKey) -> Surface {
        if let Some(panel) = model
            .panels
            .sources
            .as_ref()
            .filter(|p| key.0 == Key::from(sources_key(p)))
        {
            let rect = Rect::new(
                0.0,
                0.0,
                model.sources_size.width,
                model.sources_size.height,
            );
            return panel_surface(
                keyboard_panels::SOURCES_NAMESPACE,
                panel.output,
                Anchor::default(),
                rect,
            );
        }
        if let Some(panel) = model
            .panels
            .accents
            .as_ref()
            .filter(|p| key.0 == Key::from(accents_key(p)))
        {
            let size = output_size(panel.output).unwrap_or(Size::ZERO);
            return panel_surface(
                keyboard_panels::ACCENTS_NAMESPACE,
                panel.output,
                Anchor {
                    top: true,
                    left: true,
                    ..Anchor::default()
                },
                panel.layout(size).rect,
            );
        }
        let side = f64::from(WINDOW_SIZE);
        Surface {
            role: Role::Layer(LayerSpec {
                namespace: NAMESPACE,
                output: model.output,
                layer: Layer::Overlay,
                anchor: Anchor {
                    bottom: true,
                    ..Anchor::default()
                },
                size: Size::new(side, side),
                margin: Margin {
                    bottom: BOTTOM_MARGIN,
                    ..Margin::default()
                },
                exclusive_zone: -1,
                keyboard: Keyboard::None,
            }),
            persistent: None,
            holes: Vec::new(),
        }
    }

    fn event(event: Event) -> Option<Message> {
        match event {
            Event::Bus {
                message: Published::ShowOsd { kind, level },
                ..
            } => published(kind, level).map(|overlay| Message::Command(Command::Show(overlay))),
            Event::Outputs => Some(Message::Outputs),
            Event::Closed(key) => Some(Message::Closed(key)),
            Event::KeyboardPanels(update) => Some(Message::Panels(update)),
            _ => None,
        }
    }
}

impl Component for Osd {
    const NAME: ComponentName = ComponentName::OSD;
}
