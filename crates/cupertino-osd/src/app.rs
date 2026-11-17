//! Glue between the overlay model and CupertinoKit: the overlay's window gets its layer surface from [`OsdRoles`], the
//! bus interface's commands arrive as messages, and timers pace the hold and the fade.

use cupertino_alsa::DEVICE_DIR;
use cupertino_kit::core::{Application, Command, Key, Window, WindowKey};
use cupertino_kit::platform::WindowSpec;
use cupertino_modifiers::Profile;
use cupertino_osd::backend::{
    AlsaVolume, MemoryBacklight, MemoryVolume, SessionMicrophone, SessionVolume, Volume,
};
use cupertino_osd::control;
use cupertino_osd::controller::{self, Controller};
use cupertino_osd::layout::{BOTTOM_MARGIN, WINDOW_SIZE};
use cupertino_osd::model::Phase;
use cupertino_osd::system::{DisplayBacklight, KeyboardBacklight};
use cupertino_osd::view::{self, APP_ID};
use cupertino_pipewire::Wpctl;
use cupertino_shell_client::{
    Anchor, KeyboardInteractivity, Layer, LayerShell, LayerSurface, LayerSurfaceConfig, Margin,
    ShellEvent,
};
use cupertino_ui_wayland::{RoleEvent, SurfaceRoles, WaylandWindowSystem};
use cupertino_wayland_client::{
    Connection, Error as WaylandError, ExtensionEvent, OutputId, SurfaceId,
};
use std::error::Error;
use std::path::PathBuf;
use std::time::{Duration, Instant};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// Role of the surface for the server (`wlr-layer-shell` namespace).
const NAMESPACE: &str = "cupertino-osd";

/// Levels of the in-memory backends of `--fake-backends` (arbitrary, for trying the overlay).
const FAKE_LEVEL: f64 = 0.5;

/// The fade's frame interval.
const FRAME: Duration = Duration::from_micros(16_667);

/// Command line of the process.
#[derive(Default)]
pub struct Options {
    /// Change in-memory levels instead of the machine's volume and backlights.
    pub fake_backends: bool,
    /// Sound card whose master volume is used instead of the default PipeWire sink.
    pub alsa_card: Option<u32>,
}

impl Options {
    /// Parse the arguments; the problem is returned as the error.
    pub fn parse(args: impl Iterator<Item = String>) -> std::result::Result<Self, String> {
        let mut options = Options::default();
        let mut args = args;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--fake-backends" => options.fake_backends = true,
                "--alsa-card" => {
                    let card = args.next().ok_or("--alsa-card needs a card number")?;
                    options.alsa_card = Some(
                        card.parse()
                            .map_err(|_| format!("{card:?} is not a card number"))?,
                    );
                }
                _ => return Err(format!("unknown argument {arg}")),
            }
        }
        Ok(options)
    }
}

fn controller(options: &Options) -> Controller {
    if options.fake_backends {
        return Controller::new(
            Box::new(MemoryVolume::new(FAKE_LEVEL, false)),
            Box::new(MemoryVolume::new(FAKE_LEVEL, false)),
            Box::new(MemoryBacklight::new(FAKE_LEVEL)),
            Box::new(MemoryBacklight::new(FAKE_LEVEL)),
        );
    }
    let alsa = AlsaVolume::new(PathBuf::from(DEVICE_DIR), options.alsa_card);
    let volume: Box<dyn Volume> = match options.alsa_card {
        Some(_) => Box::new(alsa),
        None => Box::new(SessionVolume::new(Wpctl::default(), alsa)),
    };
    Controller::new(
        volume,
        Box::new(SessionMicrophone::new(Wpctl::default())),
        Box::new(DisplayBacklight::new()),
        Box::new(KeyboardBacklight::new()),
    )
}

/// Gives the overlay's window its layer surface: on the overlay layer of the server's first output, anchored to the
/// bottom only, so it is centred horizontally `BOTTOM_MARGIN` above the edge, with no keyboard and an empty input
/// region. Which output that is goes to the model at each change, so the overlay follows it and has no window while
/// the server has no output.
struct OsdRoles {
    shell: LayerShell,
    layer: Option<LayerSurface>,
    outputs: async_channel::Sender<Option<OutputId>>,
}

impl SurfaceRoles for OsdRoles {
    fn create(
        &mut self,
        conn: &mut Connection,
        spec: &WindowSpec,
    ) -> Option<std::result::Result<SurfaceId, WaylandError>> {
        if spec.app_id != APP_ID {
            return None;
        }
        let config = LayerSurfaceConfig {
            namespace: NAMESPACE.to_string(),
            // with no output known the server picks one, or closes the surface when it has none
            output: first_output(conn),
            layer: Layer::Overlay,
            anchor: Anchor::Bottom,
            size: (WINDOW_SIZE, WINDOW_SIZE),
            margin: Margin {
                bottom: BOTTOM_MARGIN,
                ..Margin::default()
            },
            exclusive_zone: -1,
            keyboard: KeyboardInteractivity::None,
        };
        let layer = match self.shell.layer_surface(conn, &config) {
            Ok(layer) => layer,
            Err(cupertino_shell_client::Error::Transport(e)) => return Some(Err(e)),
            Err(_) => return Some(Err(WaylandError::Protocol("the overlay's output is gone"))),
        };
        let surface = layer.surface();
        // the window ignores the mouse
        if let Err(e) = conn.set_input_region(surface, Some(&[])) {
            return Some(Err(e));
        }
        self.layer = Some(layer);
        Some(Ok(surface))
    }

    fn event(&mut self, event: &ExtensionEvent) -> Option<RoleEvent> {
        let layer = self.layer.as_ref()?;
        match *event.downcast_ref::<ShellEvent>()? {
            ShellEvent::Configure { surface, size } if surface == layer.surface() => {
                // a configure without a size closes the overlay, which opens again on an output still there
                Some(match layer.configured(size) {
                    Ok(size) => RoleEvent::Configure { surface, size },
                    Err(_) => RoleEvent::Closed(surface),
                })
            }
            ShellEvent::Closed(surface) if surface == layer.surface() => {
                Some(RoleEvent::Closed(surface))
            }
            _ => None,
        }
    }

    fn destroyed(&mut self, surface: SurfaceId) {
        if self.layer.as_ref().is_some_and(|l| l.surface() == surface) {
            self.layer = None;
        }
    }

    fn outputs_changed(&mut self, conn: &Connection) {
        let _ = self.outputs.try_send(first_output(conn));
    }
}

/// Where the overlay's window goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Placement {
    /// Wherever the window system puts a window (a toplevel, without the layer shell).
    Anywhere,
    /// On this output, the server's first.
    On(OutputId),
    /// Nowhere: the server has no output. The bus is still served, so the keys still change the volume and the
    /// brightness.
    Nowhere,
}

impl From<Option<OutputId>> for Placement {
    fn from(output: Option<OutputId>) -> Placement {
        output.map_or(Placement::Nowhere, Placement::On)
    }
}

pub struct Osd {
    controller: Controller,
    commands: async_channel::Receiver<controller::Command>,
    start: Instant,
    /// The timer that counts; one scheduled before it is ignored when it fires.
    timer: u64,
    /// The server's first output at each change, from the window system.
    outputs: async_channel::Receiver<Option<OutputId>>,
    placement: Placement,
    /// The window's generation: each one is a new window, so one the server closed, or one on an output that went, is
    /// not kept.
    window: u64,
}

#[derive(Clone, Debug)]
pub enum Message {
    Command(controller::Command),
    Timer(u64),
    /// The bus thread is gone.
    Unheard,
    /// The server's first output is now this one, or it has none.
    Output(Option<OutputId>),
    /// The server closed the window: its output went away.
    Closed,
}

impl Osd {
    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }
}

fn listen(commands: &async_channel::Receiver<controller::Command>) -> Command<Message> {
    let commands = commands.clone();
    Command::Perform(Box::pin(async move {
        commands
            .recv()
            .await
            .map_or(Message::Unheard, Message::Command)
    }))
}

/// The next change of the server's first output; a closed channel (no window system tells any) waits for ever.
fn follow(outputs: &async_channel::Receiver<Option<OutputId>>) -> Command<Message> {
    let outputs = outputs.clone();
    Command::Perform(Box::pin(async move {
        match outputs.recv().await {
            Ok(output) => Message::Output(output),
            Err(_) => std::future::pending().await,
        }
    }))
}

/// The timer for the next change of the lifetime: the end of the hold, or the next frame of the fade.
fn schedule(model: &mut Osd) -> Vec<Command<Message>> {
    model.timer += 1;
    let now = model.now();
    let wait = match model.controller.phase(now) {
        Phase::Hidden => return Vec::new(),
        Phase::Holding => model
            .controller
            .until_fade(now)
            .map_or(FRAME, Duration::from_secs_f64),
        Phase::Fading => FRAME,
    };
    let timer = model.timer;
    vec![Command::Perform(Box::pin(async move {
        async_io::Timer::after(wait).await;
        Message::Timer(timer)
    }))]
}

fn start(
    controller: Controller,
    commands: async_channel::Receiver<controller::Command>,
    outputs: async_channel::Receiver<Option<OutputId>>,
    placement: Placement,
) -> (Osd, Vec<Command<Message>>) {
    let first = vec![listen(&commands), follow(&outputs)];
    let model = Osd {
        controller,
        commands,
        start: Instant::now(),
        timer: 0,
        outputs,
        placement,
        window: 0,
    };
    (model, first)
}

/// What the model does when the server's first output becomes `output`: the window goes onto it, made anew, or goes
/// while there is none; one already on it stays.
fn place(model: &mut Osd, output: Option<OutputId>) {
    let placement = Placement::from(output);
    if placement != model.placement {
        model.placement = placement;
        model.window += 1;
    }
}

impl Application for Osd {
    type Model = Osd;
    type Message = Message;
    const RESIDENT: bool = true;
    /// A small overlay shown now and then: a GPU context would only cost memory.
    const RENDERER: cupertino_kit::core::RendererClass = cupertino_kit::core::RendererClass::Cpu;

    /// An overlay on the machine's backends that no bus feeds; [`run`] starts the process's one.
    fn init() -> (Osd, Vec<Command<Message>>) {
        start(
            controller(&Options::default()),
            async_channel::unbounded().1,
            async_channel::unbounded().1,
            Placement::Anywhere,
        )
    }

    fn update(model: &mut Osd, message: Message) -> Vec<Command<Message>> {
        match message {
            Message::Command(command) => {
                // a failure shows nothing
                if let Err(e) = model.controller.apply(model.now(), command) {
                    eprintln!("cupertino-osd: {e}");
                }
                let mut commands = schedule(model);
                commands.push(listen(&model.commands));
                commands
            }
            Message::Timer(timer) if timer == model.timer => schedule(model),
            Message::Timer(_) => Vec::new(),
            Message::Unheard => {
                eprintln!("cupertino-osd: the bus interface stopped");
                vec![Command::Exit]
            }
            Message::Output(output) => {
                place(model, output);
                vec![follow(&model.outputs)]
            }
            Message::Closed => {
                // a new window, on the output the server still has; while it has none there is no window
                model.window += 1;
                Vec::new()
            }
        }
    }

    fn view(model: &Osd) -> Vec<Window<Message>> {
        if model.placement == Placement::Nowhere {
            return Vec::new();
        }
        let mut window = view::window(model.controller.view(model.now()));
        window.key = WindowKey(Key::from(format!("osd-{}", model.window)));
        // a toplevel closed by the person ends the process; a layer surface the server closed is made again
        if model.placement != Placement::Anywhere {
            window.on_close = Some(Message::Closed);
        }
        vec![window]
    }
}

/// The first output the server has described, if it has one.
fn first_output(conn: &Connection) -> Option<OutputId> {
    conn.outputs()
        .into_iter()
        .find(|o| o.geometry.is_some())
        .map(|o| o.id)
}

/// Connect, claim the bus name and serve until the connection or the bus goes. While the server has no output the
/// overlay has no window and the bus is still served, so the volume and brightness keys still work; the window comes
/// back with an output.
pub fn run(options: &Options) -> Result<std::convert::Infallible> {
    let mut system = WaylandWindowSystem::connect_as(Profile::cupertino(), Osd::RENDERER)
        .map_err(|e| format!("cannot connect to the Wayland display: {e:?}"))?;
    let conn = system.connection();
    let shell = LayerShell::bind(conn).ok_or("the server lacks zwlr_layer_shell_v1")?;
    let output = first_output(conn);
    let (sender, commands) = async_channel::unbounded();
    let (outputs_sender, outputs) = async_channel::unbounded();
    // the bus thread only queues; the application's loop carries the commands out, so the effects never block the bus
    let _control = control::serve(zbus::connection::Builder::session()?, move |command| {
        let _ = sender.send_blocking(command);
    })
    .map_err(|e| match e {
        zbus::Error::NameTaken => "another overlay owns org.cupertino.Osd1".into(),
        e => Box::new(e) as Box<dyn Error>,
    })?;
    let system = system.with_roles(OsdRoles {
        shell,
        layer: None,
        outputs: outputs_sender,
    });
    let model = start(controller(options), commands, outputs, output.into());
    cupertino_kit::app::run_on::<Osd>(system, model)?;
    Err("the overlay's window system stopped".into())
}
