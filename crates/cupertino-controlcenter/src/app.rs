//! Glue between the controller and CupertinoKit: the card's and the catcher's windows get their layer surfaces from
//! [`PanelRoles`], the bus interface's toggles and the session lock arrive as messages, and timers pace the backend
//! writes, the refresh and the fade.

use cupertino_alsa::DEVICE_DIR;
use cupertino_controlcenter::backend::{
    AlsaVolume, MemoryBattery, MemoryBluetooth, MemoryBrightness, MemoryFocusMode, MemoryVolume,
    MemoryWifi, RecordingOverlay, SessionVolume,
};
use cupertino_controlcenter::control;
use cupertino_controlcenter::controller::{Backends, Controller};
use cupertino_controlcenter::layout::EDGE_MARGIN;
use cupertino_controlcenter::lifecycle::Phase;
use cupertino_controlcenter::model::{Battery, VolumeState, WifiStatus};
use cupertino_controlcenter::system::{
    BtdBluetooth, NetdWifi, NotificationFocusMode, PowerdBattery, PowerdBrightness, SessionOverlay,
};
use cupertino_controlcenter::view::{self, Input, APP_ID, CATCHER_APP_ID};
use cupertino_kit::core::{Application, Command, Window};
use cupertino_kit::platform::WindowSpec;
use cupertino_modifiers::Profile;
use cupertino_pipewire::Wpctl;
use cupertino_protocol_app::AppEvent;
use cupertino_shell_client::{
    Anchor, KeyboardInteractivity, Layer, LayerShell, LayerSurface, LayerSurfaceConfig, Margin,
    ShellEvent,
};
use cupertino_theme::Theme;
use cupertino_ui_wayland::{RoleEvent, SurfaceRoles, WaylandWindowSystem};
use cupertino_wayland_client::{
    Connection, Error as WaylandError, ExtensionEvent, OutputId, SurfaceId,
};
use std::error::Error;
use std::path::PathBuf;
use std::time::{Duration, Instant};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// Role of the surfaces for the server (`wlr-layer-shell` namespace).
const NAMESPACE: &str = "cupertino-controlcenter";

/// Readings of the in-memory backends of `--fake-backends` (arbitrary, for trying the panel).
const FAKE_NETWORK: &str = "Home Network";
const FAKE_BRIGHTNESS: f64 = 0.6;
const FAKE_VOLUME: f64 = 0.4;
const FAKE_BATTERY_PERCENT: u8 = 80;

/// The fade's frame interval.
const FRAME: Duration = Duration::from_micros(16_667);

/// Command line of the process.
#[derive(Default)]
pub struct Options {
    /// Use in-memory backends instead of the machine's radios, volume and backlight.
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

fn backends(options: &Options) -> Backends {
    if options.fake_backends {
        return Backends {
            wifi: Box::new(MemoryWifi(WifiStatus {
                enabled: true,
                network: Some(FAKE_NETWORK.to_string()),
            })),
            bluetooth: Box::new(MemoryBluetooth(false)),
            focus_mode: Box::new(MemoryFocusMode(false)),
            brightness: Box::new(MemoryBrightness(FAKE_BRIGHTNESS)),
            volume: Box::new(MemoryVolume(VolumeState {
                level: FAKE_VOLUME,
                muted: false,
            })),
            battery: Box::new(MemoryBattery(Some(Battery {
                percent: FAKE_BATTERY_PERCENT,
                charging: false,
            }))),
            overlay: Box::new(RecordingOverlay::default()),
        };
    }
    Backends {
        wifi: Box::new(NetdWifi::new()),
        bluetooth: Box::new(BtdBluetooth::new()),
        focus_mode: Box::new(NotificationFocusMode::new()),
        brightness: Box::new(PowerdBrightness::new()),
        volume: match options.alsa_card {
            Some(card) => Box::new(AlsaVolume::new(PathBuf::from(DEVICE_DIR), Some(card))),
            None => Box::new(SessionVolume::new(
                Wpctl::default(),
                AlsaVolume::new(PathBuf::from(DEVICE_DIR), None),
            )),
        },
        battery: Box::new(PowerdBattery::new()),
        overlay: Box::new(SessionOverlay::new()),
    }
}

/// What reaches the panel from outside its windows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    /// The `Toggle` call of the bus interface.
    Toggle,
    /// The session locked: the server draws the lock screen, and only a shown panel has to go.
    Dismiss,
}

/// Gives the panel's windows their layer surfaces on `output`. The card: the overlay layer, top right below the menu
/// bar, with the exclusive keyboard while it is shown, so it is the key window as soon as it opens. The catcher: the
/// top layer under it, over the whole output. The card's window opens with the panel and closes after its fade, so
/// a hidden panel takes no input. Hears the session lock for the application.
struct PanelRoles {
    shell: LayerShell,
    output: OutputId,
    /// The menu bar's height, in whole points.
    menu_bar: i32,
    requests: async_channel::Sender<Request>,
    card: Option<LayerSurface>,
    catcher: Option<LayerSurface>,
}

impl PanelRoles {
    fn config(&self, spec: &WindowSpec) -> Option<LayerSurfaceConfig> {
        let namespace = NAMESPACE.to_string();
        let output = Some(self.output);
        Some(match spec.app_id.as_str() {
            APP_ID => LayerSurfaceConfig {
                namespace,
                output,
                layer: Layer::Overlay,
                // the bar reserves no layer zone (the server keeps windows clear of it), so the margin carries its
                // height
                anchor: Anchor::Top | Anchor::Right,
                size: (
                    spec.size.width.ceil() as u32,
                    spec.size.height.ceil() as u32,
                ),
                margin: Margin {
                    top: self.menu_bar + EDGE_MARGIN,
                    right: EDGE_MARGIN,
                    ..Margin::default()
                },
                exclusive_zone: -1,
                keyboard: KeyboardInteractivity::Exclusive,
            },
            CATCHER_APP_ID => LayerSurfaceConfig {
                namespace,
                output,
                layer: Layer::Top,
                anchor: Anchor::Top | Anchor::Right | Anchor::Bottom | Anchor::Left,
                size: (0, 0),
                margin: Margin::default(),
                exclusive_zone: -1,
                keyboard: KeyboardInteractivity::None,
            },
            _ => return None,
        })
    }

    fn layer(&self, surface: SurfaceId) -> Option<&LayerSurface> {
        [&self.card, &self.catcher]
            .into_iter()
            .flatten()
            .find(|l| l.surface() == surface)
    }
}

impl SurfaceRoles for PanelRoles {
    fn create(
        &mut self,
        conn: &mut Connection,
        spec: &WindowSpec,
    ) -> Option<std::result::Result<SurfaceId, WaylandError>> {
        let config = self.config(spec)?;
        let layer = match self.shell.layer_surface(conn, &config) {
            Ok(layer) => layer,
            Err(cupertino_shell_client::Error::Transport(e)) => return Some(Err(e)),
            Err(_) => {
                return Some(Err(WaylandError::Protocol(
                    "the control panel's output is gone",
                )))
            }
        };
        let surface = layer.surface();
        if spec.app_id == APP_ID {
            self.card = Some(layer);
        } else {
            self.catcher = Some(layer);
        }
        Some(Ok(surface))
    }

    fn event(&mut self, event: &ExtensionEvent) -> Option<RoleEvent> {
        if let Some(AppEvent::SessionLock { locked: true }) = event.downcast_ref::<AppEvent>() {
            let _ = self.requests.send_blocking(Request::Dismiss);
            // the window system follows the lock too
            return None;
        }
        match *event.downcast_ref::<ShellEvent>()? {
            ShellEvent::Configure { surface, size } => {
                let layer = self.layer(surface)?;
                // a configure without a size closes the window, which restarts the process
                Some(match layer.configured(size) {
                    Ok(size) => RoleEvent::Configure { surface, size },
                    Err(_) => RoleEvent::Closed(surface),
                })
            }
            ShellEvent::Closed(surface) => {
                self.layer(surface)?;
                Some(RoleEvent::Closed(surface))
            }
            _ => None,
        }
    }

    /// The catcher goes when the fade starts: the fading card gives the keyboard back at once.
    fn destroyed(&mut self, surface: SurfaceId) {
        if self.card.as_ref().is_some_and(|l| l.surface() == surface) {
            self.card = None;
        } else if self.catcher.as_ref().is_some_and(|l| l.surface() == surface) {
            self.catcher = None;
            if let Some(card) = &self.card {
                card.set_keyboard_interactivity(KeyboardInteractivity::None);
            }
        }
    }
}

pub struct Panel {
    controller: Controller,
    requests: async_channel::Receiver<Request>,
    start: Instant,
    /// The timer that counts; one scheduled before it is ignored when it fires.
    timer: u64,
}

#[derive(Clone, Debug)]
pub enum Message {
    Request(Request),
    Input(Input),
    Timer(u64),
    /// The bus thread and the window system's roles are gone.
    Unheard,
}

impl Panel {
    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    /// Carry out what the user did on the card; the card only acts while it is shown, not while it fades.
    fn input(&mut self, input: Input) {
        let now = self.now();
        if self.controller.phase(now) != Phase::Shown {
            return;
        }
        match input {
            Input::Press(module) => {
                self.controller.press(module);
            }
            Input::Slide(slider, fraction) => {
                self.controller.slide(now, slider, fraction);
            }
            Input::Release(..) => self.controller.release(now),
            Input::Dismiss => self.controller.dismiss(now),
        }
    }
}

fn listen(requests: &async_channel::Receiver<Request>) -> Command<Message> {
    let requests = requests.clone();
    Command::Perform(Box::pin(async move {
        requests
            .recv()
            .await
            .map_or(Message::Unheard, Message::Request)
    }))
}

/// The timer for the controller's next write or refresh, the end of the fade, or the next frame of the fade.
fn schedule(model: &mut Panel) -> Vec<Command<Message>> {
    model.timer += 1;
    let now = model.now();
    let next = model
        .controller
        .next_wakeup(now)
        .map(Duration::from_secs_f64);
    let wait = match model.controller.phase(now) {
        Phase::Hiding => Some(next.map_or(FRAME, |next| next.min(FRAME))),
        Phase::Shown | Phase::Hidden => next,
    };
    let Some(wait) = wait else {
        return Vec::new();
    };
    let timer = model.timer;
    vec![Command::Perform(Box::pin(async move {
        async_io::Timer::after(wait).await;
        Message::Timer(timer)
    }))]
}

fn start(
    controller: Controller,
    requests: async_channel::Receiver<Request>,
) -> (Panel, Vec<Command<Message>>) {
    let first = listen(&requests);
    let model = Panel {
        controller,
        requests,
        start: Instant::now(),
        timer: 0,
    };
    (model, vec![first])
}

fn input(input: Input) -> Message {
    Message::Input(input)
}

impl Application for Panel {
    type Model = Panel;
    type Message = Message;
    const RESIDENT: bool = true;

    /// A panel on the machine's backends that no bus feeds; [`run`] starts the process's one.
    fn init() -> (Panel, Vec<Command<Message>>) {
        start(
            Controller::new(backends(&Options::default())),
            async_channel::unbounded().1,
        )
    }

    fn update(model: &mut Panel, message: Message) -> Vec<Command<Message>> {
        match message {
            Message::Request(request) => {
                let now = model.now();
                match request {
                    Request::Toggle => model.controller.toggle(now),
                    Request::Dismiss => model.controller.dismiss(now),
                }
                let mut commands = schedule(model);
                commands.push(listen(&model.requests));
                commands
            }
            Message::Input(event) => {
                model.input(event);
                schedule(model)
            }
            Message::Timer(timer) if timer == model.timer => {
                model.controller.tick(model.now());
                schedule(model)
            }
            Message::Timer(_) => Vec::new(),
            Message::Unheard => {
                eprintln!("cupertino-controlcenter: the bus interface stopped");
                vec![Command::Exit]
            }
        }
    }

    fn view(model: &Panel) -> Vec<Window<Message>> {
        let now = model.now();
        let controls = model.controller.controls();
        match model.controller.phase(now) {
            Phase::Hidden => Vec::new(),
            Phase::Shown => vec![
                view::catcher(input),
                view::panel(controls, model.controller.opacity(now), input),
            ],
            Phase::Hiding => vec![view::panel(
                controls,
                model.controller.opacity(now),
                input,
            )],
        }
    }
}

/// The first output the server has described.
fn first_output(conn: &Connection) -> Result<OutputId> {
    Ok(conn
        .outputs()
        .into_iter()
        .find(|o| o.geometry.is_some())
        .ok_or("the server has no output")?
        .id)
}

/// Connect, claim the bus name and serve until the connection, the bus or a surface of the panel goes.
pub fn run(options: &Options) -> Result<std::convert::Infallible> {
    let menu_bar = Theme::load()?.shapes.menu_bar_height.ceil() as i32;
    let mut system = WaylandWindowSystem::connect(Profile::cupertino())
        .map_err(|e| format!("cannot connect to the Wayland display: {e:?}"))?;
    let conn = system.connection();
    let shell = LayerShell::bind(conn).ok_or("the server lacks zwlr_layer_shell_v1")?;
    let output = first_output(conn)?;
    let (sender, requests) = async_channel::unbounded();
    let bus = sender.clone();
    // the bus thread only queues; the application's loop carries the toggles out, so the effects never block the bus
    let _control = control::serve(zbus::connection::Builder::session()?, move || {
        let _ = bus.send_blocking(Request::Toggle);
    })
    .map_err(|e| match e {
        zbus::Error::NameTaken => "another control panel owns org.cupertino.ControlPanel1".into(),
        e => Box::new(e) as Box<dyn Error>,
    })?;
    let system = system.with_roles(PanelRoles {
        shell,
        output,
        menu_bar,
        requests: sender,
        card: None,
        catcher: None,
    });
    cupertino_kit::app::run_on::<Panel>(
        system,
        start(Controller::new(backends(options)), requests),
    )?;
    Err("a surface of the control panel was closed".into())
}
