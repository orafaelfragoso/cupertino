//! Glue between the controller and CupertinoKit: the panel's and the catcher's windows get their layer surfaces from
//! [`PanelRoles`], the bus interface's toggles, the session lock, the session's appearance and its Reduce Motion
//! arrive as messages, and timers pace the backend writes, the refresh, the fade and the motion of a detail view.

use cupertino_alsa::DEVICE_DIR;
use cupertino_controlcenter::backend::{
    AlsaVolume, DockHiding, MemoryBluetooth, MemoryBrightness, MemoryMedia, MemorySwitch,
    MemoryVolume, MemoryWifi, PipewireMicrophone, RecordingLauncher, RecordingLock,
    RecordingOverlay, SessionVolume, SettingsLauncher,
};
use cupertino_controlcenter::control;
use cupertino_controlcenter::controller::{Backends, Controller};
use cupertino_controlcenter::layout::{BAR_MARGIN, SHADOW_TOP};
use cupertino_controlcenter::lifecycle::{Phase, CLOSE_CURVE, OPEN_CURVE};
use cupertino_controlcenter::model::{Device, Network, Output, VolumeState, WifiStatus};
use cupertino_controlcenter::motion::Expansion;
use cupertino_controlcenter::system::{
    BtdBluetooth, LogindLock, MprisPlayer, NetdWifi, NotificationFocusMode, PowerdBrightness,
    SessionOverlay, UpowerKeyboard,
};
use cupertino_controlcenter::view::{
    self, focus_key, Input, Opened, Presentation, Via, APP_ID, CATCHER_APP_ID, DETAIL_FOCUS,
};
use cupertino_kit::controls::FrameClock;
use cupertino_kit::core::{Application, Command, Element, Key, Window, WindowKey};
use cupertino_kit::platform::{
    PresentChange, PresentProperty, SystemAppearance, WindowRequest, WindowSpec, WindowSystem,
};
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
const FAKE_OTHER_NETWORK: &str = "Guest";
const FAKE_DEVICE: (&str, &str) = ("00:11:22:33:44:55", "Headphones");
const FAKE_OUTPUTS: [(u32, &str); 2] = [(31, "Speakers"), (47, "Headphones")];
const FAKE_BRIGHTNESS: f64 = 0.6;
const FAKE_VOLUME: f64 = 0.4;

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
        let network = |ssid: &str, connected: bool, secured: bool| Network {
            ssid: ssid.to_string(),
            bars: Some(3),
            secured,
            known: connected,
            connected,
        };
        return Backends {
            wifi: Box::new(MemoryWifi(
                WifiStatus {
                    enabled: true,
                    network: Some(FAKE_NETWORK.to_string()),
                },
                vec![
                    network(FAKE_NETWORK, true, true),
                    network(FAKE_OTHER_NETWORK, false, false),
                ],
            )),
            bluetooth: Box::new(MemoryBluetooth(
                false,
                vec![Device {
                    id: FAKE_DEVICE.0.to_string(),
                    name: FAKE_DEVICE.1.to_string(),
                    connected: false,
                }],
            )),
            microphone: Box::new(MemorySwitch(true)),
            dock_hiding: Box::new(MemorySwitch(false)),
            keyboard: Box::new(MemorySwitch(false)),
            focus_mode: Box::new(MemorySwitch(false)),
            brightness: Box::new(MemoryBrightness(FAKE_BRIGHTNESS)),
            volume: Box::new(MemoryVolume(
                VolumeState {
                    level: FAKE_VOLUME,
                    muted: false,
                },
                FAKE_OUTPUTS
                    .iter()
                    .map(|&(id, name)| Output {
                        id,
                        name: name.to_string(),
                        current: id == FAKE_OUTPUTS[0].0,
                    })
                    .collect(),
            )),
            media: Box::new(MemoryMedia(None)),
            lock: Box::new(RecordingLock::default()),
            launcher: Box::new(RecordingLauncher::default()),
            overlay: Box::new(RecordingOverlay::default()),
        };
    }
    Backends {
        wifi: Box::new(NetdWifi::new()),
        bluetooth: Box::new(BtdBluetooth::new()),
        microphone: Box::new(PipewireMicrophone::new(Wpctl::default())),
        dock_hiding: Box::new(DockHiding::default()),
        keyboard: Box::new(UpowerKeyboard::new()),
        focus_mode: Box::new(NotificationFocusMode::new()),
        brightness: Box::new(PowerdBrightness::new()),
        volume: match options.alsa_card {
            Some(card) => Box::new(AlsaVolume::new(PathBuf::from(DEVICE_DIR), Some(card))),
            None => Box::new(SessionVolume::new(
                Wpctl::default(),
                AlsaVolume::new(PathBuf::from(DEVICE_DIR), None),
            )),
        },
        media: Box::new(MprisPlayer::new()),
        lock: Box::new(LogindLock::new()),
        launcher: Box::new(SettingsLauncher::default()),
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
    /// The output the panel's surfaces are on went away: the panel is hidden at once.
    OutputGone,
    /// The session's appearance: whether it is dark; the panel's theme follows it.
    Appearance { dark: bool },
    /// The session's Reduce Motion: a detail view then cross-fades in place instead of growing out of its tile.
    ReduceMotion(bool),
}

/// Who holds the card's keyboard. The card takes the exclusive keyboard whenever the panel is shown (the catcher
/// exists), gives it back when the fade starts, and a shown card that loses the keyboard to another surface, such as
/// the launcher, dismisses the panel. A leave that answers the card giving the keyboard back is not a dismissal.
#[derive(Debug, Default)]
struct CardKeyboard {
    /// The card heard the keyboard enter since it last asked for it.
    entered: bool,
}

impl CardKeyboard {
    /// The catcher opened: the panel is shown again, so the card wants the keyboard.
    fn shown(&mut self) -> KeyboardInteractivity {
        self.entered = false;
        KeyboardInteractivity::Exclusive
    }

    /// The catcher went with the start of the fade: the card gives the keyboard back.
    fn hiding(&mut self) -> KeyboardInteractivity {
        self.entered = false;
        KeyboardInteractivity::None
    }

    /// The card took or lost the keyboard; what the panel has to do about it.
    fn focus(&mut self, focused: bool) -> Option<Request> {
        if focused {
            self.entered = true;
            return None;
        }
        std::mem::take(&mut self.entered).then_some(Request::Dismiss)
    }
}

/// Gives the panel's windows their layer surfaces on one output. The card: the overlay layer, top right below the menu
/// bar, with the exclusive keyboard while it is shown, so it is the key window as soon as it opens. The catcher: the
/// top layer under it, over the whole output. The card's window opens with the panel and closes after its fade, so
/// a hidden panel takes no input. Hears the session lock, the card losing the keyboard and the panel's output going
/// away for the application.
///
/// The card and the catcher are always on the same output ([`SharedOutput`]): the server's active output when the panel
/// opens (the one the person works on, where the menu bar was clicked), kept for a toggle during the fade. When that
/// output goes the server closes them, which dismisses the panel at once (it does not move to another output); the
/// next toggle opens it on an output that exists.
struct PanelRoles {
    shell: LayerShell,
    /// The output the panel's surfaces share.
    output: SharedOutput<OutputId>,
    /// The menu bar's height, in whole points.
    menu_bar: i32,
    requests: async_channel::Sender<Request>,
    card: Option<LayerSurface>,
    catcher: Option<LayerSurface>,
    keyboard: CardKeyboard,
    /// The bus interface, whose properties say whether the panel is shown and on which output; `None` serves nothing.
    control: Option<control::Served>,
}

/// The output the panel's two surfaces share. A panel that opens with no surface open names no output, so the server
/// opens the catcher and the card on its active output, the one the person works on, in the same pass. The surfaces are
/// then told that output (`wl_surface.enter`), and a surface that opens while the other is still open names it: the
/// catcher of a toggle during the fade opens on the fading card's output, even when the active output changed
/// meanwhile. Until an open surface was told its output, the next one names none, as the first did.
#[derive(Debug)]
struct SharedOutput<O> {
    output: Option<O>,
}

impl<O> Default for SharedOutput<O> {
    fn default() -> Self {
        Self { output: None }
    }
}

impl<O: Copy> SharedOutput<O> {
    /// The output a surface about to open names; `open` holds the outputs each of the panel's open surfaces is on.
    fn for_new_surface(&mut self, open: &[Vec<O>]) -> Option<O> {
        if open.is_empty() {
            self.output = None;
        } else {
            self.learn(open);
        }
        self.output
    }

    /// Learn the output from the panel's open surfaces, once one of them was told it.
    fn learn(&mut self, open: &[Vec<O>]) {
        if self.output.is_none() {
            self.output = open.iter().find_map(|on| on.first().copied());
        }
    }
}

/// What the bus interface says of the panel: shown while the catcher is open (it opens with the panel and goes when
/// the fade starts, whatever dismissed it), on the output of the given connector name once its surfaces were told it.
fn presence(catcher_open: bool, output: Option<String>) -> control::Presence {
    if !catcher_open {
        return control::Presence::default();
    }
    control::Presence {
        shown: true,
        output: output.unwrap_or_default(),
    }
}

impl PanelRoles {
    /// The outputs each of the panel's open surfaces is on.
    fn open_outputs(&self, conn: &Connection) -> Vec<Vec<OutputId>> {
        [&self.card, &self.catcher]
            .into_iter()
            .flatten()
            .map(|l| conn.surface_outputs(l.surface()).unwrap_or_default())
            .collect()
    }

    fn config(&self, spec: &WindowSpec, output: Option<OutputId>) -> Option<LayerSurfaceConfig> {
        let namespace = NAMESPACE.to_string();
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
                    // the surface reaches up past the window by the shadow's margin, never above the output
                    top: (self.menu_bar + BAR_MARGIN - SHADOW_TOP as i32).max(0),
                    // and to the output's edge: the window's own margin is part of the surface
                    right: 0,
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
        let open = self.open_outputs(conn);
        let output = self.output.for_new_surface(&open);
        let config = self.config(spec, output)?;
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
            // the card opens with the exclusive keyboard
            self.keyboard.shown();
            self.card = Some(layer);
        } else {
            self.catcher = Some(layer);
            // a toggle during the fade shows the panel again in the same card, which gave its keyboard back
            if let Some(card) = &self.card {
                card.set_keyboard_interactivity(self.keyboard.shown());
            }
        }
        Some(Ok(surface))
    }

    fn event(&mut self, event: &ExtensionEvent) -> Option<RoleEvent> {
        match event.downcast_ref::<AppEvent>() {
            // the window system follows the lock, the appearance and the display options too
            Some(AppEvent::SessionLock { locked: true }) => {
                let _ = self.requests.send_blocking(Request::Dismiss);
                return None;
            }
            Some(AppEvent::Appearance { dark }) => {
                let _ = self
                    .requests
                    .send_blocking(Request::Appearance { dark: *dark });
                return None;
            }
            Some(AppEvent::DisplayOptions(options)) => {
                let _ = self
                    .requests
                    .send_blocking(Request::ReduceMotion(options.reduce_motion));
                return None;
            }
            _ => {}
        }
        match *event.downcast_ref::<ShellEvent>()? {
            ShellEvent::Configure { surface, size } => {
                let layer = self.layer(surface)?;
                // a configure without a size closes the window, which hides the panel
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
        } else if self
            .catcher
            .as_ref()
            .is_some_and(|l| l.surface() == surface)
        {
            self.catcher = None;
            if let Some(card) = &self.card {
                card.set_keyboard_interactivity(self.keyboard.hiding());
            }
        }
    }

    /// The panel's output went away: the panel goes at once. The server also closes the surfaces of an output it drops,
    /// which hides the panel the same way.
    fn outputs_changed(&mut self, conn: &Connection) {
        let open = self.open_outputs(conn);
        self.output.learn(&open);
        if output_gone(self.output.output, &described(conn)) {
            self.output.output = None;
            if self.card.is_some() || self.catcher.is_some() {
                let _ = self.requests.send_blocking(Request::OutputGone);
            }
        }
    }

    /// Tell the bus whether the panel is shown and on which output, when that changed.
    fn prepare(&mut self, conn: &mut Connection) {
        let Some(control) = &self.control else {
            return;
        };
        let open = self.open_outputs(conn);
        self.output.learn(&open);
        let output = self.output.output.and_then(|id| {
            conn.outputs()
                .into_iter()
                .find(|o| o.id == id)
                .and_then(|o| o.name)
        });
        if let Err(e) = control.publish(presence(self.catcher.is_some(), output)) {
            eprintln!("cupertino-controlcenter: cannot announce whether the panel is shown: {e}");
        }
    }

    /// Another surface took the keyboard from the shown card: the panel goes, as when it stops being the key window.
    fn keyboard_focus(&mut self, surface: SurfaceId, focused: bool) {
        if !self.card.as_ref().is_some_and(|l| l.surface() == surface) {
            return;
        }
        if let Some(request) = self.keyboard.focus(focused) {
            let _ = self.requests.send_blocking(request);
        }
    }
}

pub struct Panel {
    controller: Controller,
    requests: async_channel::Receiver<Request>,
    start: Instant,
    /// The timer that counts; one scheduled before it is ignored when it fires.
    timer: u64,
    /// The session is in the light appearance.
    light: bool,
    reduce_motion: bool,
    /// The detail view opening, open or closing.
    expansion: Option<Expansion>,
    /// The detail view is moving: its end has not been seen yet.
    moving: bool,
    /// The keyboard or assistive technology started the motion, so the focus follows the view when it ends.
    follow_focus: bool,
    /// The fade or a detail view's motion runs: the panel's window asks for frames, each one a step.
    animating: bool,
    /// The compositor runs the card's fades on its clock ([`PresentChange`]): the card draws at full opacity, and its
    /// window asks for no frame while it fades.
    compositor_fades: bool,
    /// The phase the card's presentation was last told of.
    told: Phase,
}

#[derive(Clone, Debug)]
pub enum Message {
    Request(Request),
    Input(Input),
    Timer(u64),
    /// The panel's window got a frame from the compositor while the panel moves.
    Frame,
    /// The bus thread and the window system's roles are gone.
    Unheard,
}

impl Panel {
    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    /// Carry out what the user did on the panel; the panel only acts while it is shown, not while it fades. A window
    /// the server closed hides the panel at once in any phase.
    fn input(&mut self, input: Input) -> Vec<Command<Message>> {
        let now = self.now();
        if input == Input::Closed {
            self.controller.vanish();
            self.expansion = None;
            return Vec::new();
        }
        if self.controller.phase(now) != Phase::Shown {
            return Vec::new();
        }
        match input {
            Input::Press(module) => {
                self.controller.press(module);
            }
            Input::Expand(detail, via) => {
                self.follow_focus = via == Via::Keyboard;
                self.controller.show_detail(Some(detail));
                match &mut self.expansion {
                    Some(e) if e.detail == detail => e.turn(true, now),
                    _ => self.expansion = Some(Expansion::open(detail, now, self.reduce_motion)),
                }
                self.moving = true;
            }
            // a view already closing counts as closed: Escape or a press outside then dismisses the panel
            Input::Collapse(_) if self.controller.detail().is_none() => {
                self.controller.dismiss(now)
            }
            Input::Collapse(via) => {
                self.follow_focus = via == Via::Keyboard;
                self.collapse(now);
            }
            Input::Media(command) => {
                self.controller.media(command);
            }
            Input::Join(ssid) => {
                self.controller.join(&ssid);
            }
            Input::Connect(id) => {
                self.controller.connect(&id);
            }
            Input::Output(id) => {
                self.controller.select_output(id);
            }
            Input::Settings(detail) => {
                self.controller.open_settings(now, detail.settings_pane());
            }
            Input::EditControls => {
                self.controller.edit_controls(now);
            }
            Input::Focus(key) => return vec![Command::Focus(Key::from(key))],
            Input::Slide(slider, fraction) => {
                self.controller.slide(now, slider, fraction);
            }
            Input::Release(..) => self.controller.release(now),
            Input::Dismiss => self.controller.dismiss(now),
            Input::Closed => unreachable!("handled above"),
        }
        Vec::new()
    }

    /// The open detail view closes back into its tile.
    fn collapse(&mut self, now: f64) {
        self.controller.show_detail(None);
        if let Some(e) = &mut self.expansion {
            e.turn(false, now);
            self.moving = true;
        }
    }

    /// Ends a detail view's motion that has settled: a closed view leaves the grid alone. Returns where the focus goes
    /// when the keyboard or assistive technology moved the view: into the view once it is open, back to its module's
    /// control once it has closed.
    fn settle(&mut self, now: f64) -> Option<String> {
        let e = self.expansion.filter(|e| self.moving && e.settled(now))?;
        self.moving = false;
        let key = if e.opening {
            DETAIL_FOCUS.to_owned()
        } else {
            self.expansion = None;
            focus_key(e.detail.module())
        };
        self.follow_focus.then_some(key)
    }

    /// Forget the detail view: the panel opened again or went.
    fn reset(&mut self) {
        self.expansion = None;
        self.moving = false;
    }

    /// How the panel is shown at `now`.
    fn presentation(&self, now: f64) -> Presentation {
        let phase = self.controller.phase(now);
        Presentation {
            // the compositor fades what the card draws whole
            alpha: if self.compositor_fades && phase != Phase::Hidden {
                1.0
            } else {
                self.controller.opacity(now)
            },
            light: self.light,
            input: self.controller.phase(now) == Phase::Shown,
            detail: self.expansion.map(|e| Opened {
                detail: e.detail,
                progress: e.progress(now),
                morph: !e.reduce_motion,
            }),
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

/// The card's fade when the panel turned to `phase` from `told`, for the compositor to run: from nothing when the card
/// opens, from where the fade is when it turns, on the curves the card draws itself without the compositor. None
/// under Reduce Motion, where the card appears and goes at once.
fn fade(told: Phase, phase: Phase, reduce_motion: bool) -> Option<Vec<PresentChange>> {
    let opacity = |to: f64, curve| PresentChange {
        property: PresentProperty::Opacity,
        to,
        curve,
        velocity: 0.0,
    };
    match (told, phase) {
        _ if reduce_motion || told == phase => None,
        (Phase::Hidden, Phase::Shown) => {
            Some(vec![opacity(0.0, None), opacity(1.0, Some(OPEN_CURVE))])
        }
        (Phase::Hiding, Phase::Shown) => Some(vec![opacity(1.0, Some(OPEN_CURVE))]),
        (Phase::Shown, Phase::Hiding) => Some(vec![opacity(0.0, Some(CLOSE_CURVE))]),
        _ => None,
    }
}

/// The timer for the controller's next write or refresh or the end of the fade; the frames of the fade and of a
/// detail view's motion come from the panel's window ([`Message::Frame`]), one per frame the compositor gives it. A
/// compositor that runs the fades is handed each one as it starts, and the window asks for no frame while it runs.
fn schedule(model: &mut Panel) -> Vec<Command<Message>> {
    model.timer += 1;
    let now = model.now();
    let phase = model.controller.phase(now);
    let fading = phase == Phase::Hiding || model.controller.opening(now);
    model.animating =
        (fading && !model.compositor_fades) || (model.moving && phase == Phase::Shown);
    let mut commands = Vec::new();
    if model.compositor_fades {
        if let Some(changes) = fade(model.told, phase, model.reduce_motion) {
            commands.push(Command::Window(
                WindowKey(Key::from(view::PANEL_KEY)),
                WindowRequest::Present(changes),
            ));
        }
    }
    model.told = phase;
    let Some(wait) = model
        .controller
        .next_wakeup(now)
        .map(Duration::from_secs_f64)
    else {
        return commands;
    };
    let timer = model.timer;
    commands.push(Command::Perform(Box::pin(async move {
        async_io::Timer::after(wait).await;
        Message::Timer(timer)
    })));
    commands
}

/// The controller and the motion advance to now.
fn step(model: &mut Panel) -> Vec<Command<Message>> {
    let now = model.now();
    model.controller.tick(now);
    let focus = model.settle(now);
    let mut commands = schedule(model);
    commands.extend(focus.map(|key| Command::Focus(Key::from(key))));
    commands
}

fn start(
    controller: Controller,
    requests: async_channel::Receiver<Request>,
    appearance: SystemAppearance,
    compositor_fades: bool,
) -> (Panel, Vec<Command<Message>>) {
    let first = listen(&requests);
    let mut controller = controller;
    controller.set_reduce_motion(appearance.reduce_motion);
    let model = Panel {
        controller,
        requests,
        start: Instant::now(),
        timer: 0,
        light: !appearance.dark,
        reduce_motion: appearance.reduce_motion,
        expansion: None,
        moving: false,
        follow_focus: false,
        animating: false,
        compositor_fades,
        told: Phase::Hidden,
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
            SystemAppearance::default(),
            false,
        )
    }

    fn update(model: &mut Panel, message: Message) -> Vec<Command<Message>> {
        match message {
            Message::Request(request) => {
                let now = model.now();
                match request {
                    Request::Toggle => {
                        model.controller.toggle(now);
                        if model.controller.phase(now) == Phase::Shown {
                            model.reset();
                        }
                    }
                    Request::Dismiss => model.controller.dismiss(now),
                    Request::OutputGone => {
                        model.controller.vanish();
                        model.reset();
                    }
                    Request::Appearance { dark } => model.light = !dark,
                    Request::ReduceMotion(on) => {
                        model.reduce_motion = on;
                        model.controller.set_reduce_motion(on);
                    }
                }
                let mut commands = schedule(model);
                commands.push(listen(&model.requests));
                commands
            }
            Message::Input(event) => {
                let mut commands = model.input(event);
                commands.extend(schedule(model));
                commands
            }
            Message::Timer(timer) if timer == model.timer => step(model),
            Message::Timer(_) => Vec::new(),
            Message::Frame if model.animating => step(model),
            Message::Frame => Vec::new(),
            Message::Unheard => {
                eprintln!("cupertino-controlcenter: the bus interface stopped");
                vec![Command::Exit]
            }
        }
    }

    fn view(model: &Panel) -> Vec<Window<Message>> {
        let now = model.now();
        let controls = model.controller.controls();
        let mut panel = view::panel(controls, model.presentation(now), input);
        // what moves steps once per frame the compositor gives the panel's window, never on a timer of its own
        panel.content = Element::from(FrameClock {
            content: panel.content,
            on_frame: model.animating.then_some(Message::Frame),
        });
        match model.controller.phase(now) {
            Phase::Hidden => Vec::new(),
            Phase::Shown => vec![view::catcher(input), panel],
            Phase::Hiding => vec![panel],
        }
    }
}

/// The server's outputs, in the order it announced them, and whether each is described yet.
fn described(conn: &Connection) -> Vec<(OutputId, bool)> {
    conn.outputs()
        .into_iter()
        .map(|o| (o.id, o.geometry.is_some()))
        .collect()
}

/// Whether the panel's `output` is no longer among the server's `outputs`.
fn output_gone<O: PartialEq>(output: Option<O>, outputs: &[(O, bool)]) -> bool {
    output.is_some_and(|output| !outputs.iter().any(|(id, _)| *id == output))
}

/// Connect, claim the bus name and serve until the connection or the bus goes. The panel opens on an output only when
/// it is toggled, so the process also starts and keeps running while the server has no output.
pub fn run(options: &Options) -> Result<std::convert::Infallible> {
    let menu_bar = Theme::load()?.shapes.menu_bar_height.ceil() as i32;
    let mut system = WaylandWindowSystem::connect(Profile::cupertino())
        .map_err(|e| format!("cannot connect to the Wayland display: {e:?}"))?;
    let appearance = system.appearance().unwrap_or_default();
    let conn = system.connection();
    let shell = LayerShell::bind(conn).ok_or("the server lacks zwlr_layer_shell_v1")?;
    let (sender, requests) = async_channel::unbounded();
    let bus = sender.clone();
    // the bus thread only queues; the application's loop carries the toggles out, so the effects never block the bus
    let control = control::serve(zbus::connection::Builder::session()?, move || {
        let _ = bus.send_blocking(Request::Toggle);
    })
    .map_err(|e| match e {
        zbus::Error::NameTaken => "another control panel owns org.cupertino.ControlPanel1".into(),
        e => Box::new(e) as Box<dyn Error>,
    })?;
    let system = system.with_roles(PanelRoles {
        shell,
        output: SharedOutput::default(),
        menu_bar,
        requests: sender,
        card: None,
        catcher: None,
        keyboard: CardKeyboard::default(),
        control: Some(control),
    });
    // the compositor runs the card's fades when it presents shell surfaces as a whole; else the card draws them
    let compositor_fades = system.capabilities().present_animation;
    cupertino_kit::app::run_on::<Panel>(
        system,
        start(
            Controller::new(backends(options)),
            requests,
            appearance,
            compositor_fades,
        ),
    )?;
    Err("the control panel's window system stopped".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cupertino_controlcenter::model::{Detail, Module};

    /// The bus says the panel is shown exactly while its catcher is open, with its output's name once known; a
    /// fading or hidden panel is not shown and names no output.
    #[test]
    fn the_panel_is_shown_while_its_catcher_is_open() {
        let shown = |output: &str| control::Presence {
            shown: true,
            output: output.into(),
        };
        assert_eq!(presence(true, Some("DP-1".into())), shown("DP-1"));
        assert_eq!(presence(true, None), shown(""));
        assert_eq!(
            presence(false, Some("DP-1".into())),
            control::Presence::default()
        );
        assert_eq!(presence(false, None), control::Presence::default());
    }

    fn panel() -> Panel {
        panel_with(false)
    }

    /// A panel on the in-memory backends; `compositor_fades` hands its fades to the compositor.
    fn panel_with(compositor_fades: bool) -> Panel {
        let options = Options {
            fake_backends: true,
            alsa_card: None,
        };
        start(
            Controller::new(backends(&options)),
            async_channel::unbounded().1,
            SystemAppearance::default(),
            compositor_fades,
        )
        .0
    }

    /// The presentation changes among `commands`.
    fn fades(commands: &[Command<Message>]) -> Vec<Vec<PresentChange>> {
        commands
            .iter()
            .filter_map(|c| match c {
                Command::Window(key, WindowRequest::Present(changes))
                    if *key == WindowKey(Key::from(view::PANEL_KEY)) =>
                {
                    Some(changes.clone())
                }
                _ => None,
            })
            .collect()
    }

    /// With a compositor that runs them, the card's fades go to it as they start, on the card's own curves, and the
    /// card draws at full opacity without asking for frames; a toggle during the fade-out turns it from where it is.
    #[test]
    fn the_compositor_runs_the_card_s_fades_and_the_card_asks_for_no_frame() {
        let mut model = panel_with(true);
        let opened = Panel::update(&mut model, Message::Request(Request::Toggle));
        let opacity = |to: f64, curve| PresentChange {
            property: PresentProperty::Opacity,
            to,
            curve,
            velocity: 0.0,
        };
        assert_eq!(
            fades(&opened),
            [vec![opacity(0.0, None), opacity(1.0, Some(OPEN_CURVE))]]
        );
        assert!(!model.animating, "no frame while it fades in");
        assert_eq!(model.presentation(model.now()).alpha, 1.0);
        let closed = Panel::update(&mut model, Message::Input(Input::Dismiss));
        assert_eq!(fades(&closed), [vec![opacity(0.0, Some(CLOSE_CURVE))]]);
        assert!(!model.animating, "no frame while it fades out");
        assert!(timer(&closed), "the end of the fade-out is timed");
        let again = Panel::update(&mut model, Message::Request(Request::Toggle));
        assert_eq!(fades(&again), [vec![opacity(1.0, Some(OPEN_CURVE))]]);
        // without the compositor's fades the card draws them
        let mut model = panel();
        assert!(fades(&Panel::update(
            &mut model,
            Message::Request(Request::Toggle)
        ))
        .is_empty());
        assert!(model.animating);
    }

    #[test]
    fn under_reduce_motion_the_compositor_is_handed_no_fade() {
        assert_eq!(fade(Phase::Hidden, Phase::Shown, true), None);
        assert_eq!(fade(Phase::Shown, Phase::Hiding, true), None);
        assert_eq!(fade(Phase::Shown, Phase::Shown, false), None);
        assert_eq!(fade(Phase::Hiding, Phase::Hidden, false), None);
    }

    fn app_ids(model: &Panel) -> Vec<String> {
        Panel::view(model)
            .into_iter()
            .map(|w| w.spec.app_id)
            .collect()
    }

    fn timer(commands: &[Command<Message>]) -> bool {
        commands.iter().any(|c| matches!(c, Command::Perform(_)))
    }

    #[test]
    fn a_toggle_shows_the_card_over_the_catcher() {
        let mut model = panel();
        assert!(app_ids(&model).is_empty(), "a hidden panel has no window");
        Panel::update(&mut model, Message::Request(Request::Toggle));
        assert_eq!(app_ids(&model), [CATCHER_APP_ID, APP_ID]);
    }

    #[test]
    fn a_dismissal_fades_the_card_without_the_catcher() {
        let mut model = panel();
        Panel::update(&mut model, Message::Request(Request::Toggle));
        Panel::update(&mut model, Message::Input(Input::Dismiss));
        assert_eq!(app_ids(&model), [APP_ID]);
        assert!(
            model.animating,
            "the card's window asks for the fade's frames"
        );
    }

    #[test]
    fn the_lock_dismisses_a_shown_panel() {
        let mut model = panel();
        Panel::update(&mut model, Message::Request(Request::Toggle));
        Panel::update(&mut model, Message::Request(Request::Dismiss));
        assert_eq!(app_ids(&model), [APP_ID]);
        assert_eq!(model.controller.phase(model.now()), Phase::Hiding);
    }

    #[test]
    fn the_lock_leaves_a_hidden_panel_hidden() {
        let mut model = panel();
        Panel::update(&mut model, Message::Request(Request::Dismiss));
        assert!(app_ids(&model).is_empty());
    }

    #[test]
    fn a_toggle_during_the_fade_shows_the_panel_again_and_escape_still_dismisses_it() {
        let mut model = panel();
        Panel::update(&mut model, Message::Request(Request::Toggle));
        Panel::update(&mut model, Message::Input(Input::Dismiss));
        Panel::update(&mut model, Message::Request(Request::Toggle));
        assert_eq!(app_ids(&model), [CATCHER_APP_ID, APP_ID]);
        // what Escape on the card sends
        Panel::update(&mut model, Message::Input(Input::Dismiss));
        assert_eq!(model.controller.phase(model.now()), Phase::Hiding);
    }

    /// Runs the timer as if `seconds` had passed, and returns the keys the update moved the focus to.
    fn after(model: &mut Panel, seconds: f64) -> Vec<String> {
        model.start -= Duration::from_secs_f64(seconds);
        let current = model.timer;
        Panel::update(model, Message::Timer(current))
            .into_iter()
            .filter_map(|c| match c {
                Command::Focus(Key::Str(key)) => Some(key.into_owned()),
                _ => None,
            })
            .collect()
    }

    fn detail(model: &Panel) -> Option<(Detail, f64, bool)> {
        model
            .presentation(model.now())
            .detail
            .map(|o| (o.detail, o.progress, o.morph))
    }

    #[test]
    fn a_detail_view_grows_out_of_its_tile_and_takes_the_focus_once_open() {
        let mut model = panel();
        Panel::update(&mut model, Message::Request(Request::Toggle));
        Panel::update(
            &mut model,
            Message::Input(Input::Expand(Detail::WiFi, Via::Keyboard)),
        );
        assert!(
            model.animating,
            "the card's window asks for the motion's frames"
        );
        assert_eq!(model.controller.detail(), Some(Detail::WiFi));
        let (_, progress, morph) = detail(&model).unwrap();
        assert!(progress < 0.1 && morph);
        assert!(after(&mut model, 0.1).is_empty(), "still moving");
        assert_eq!(after(&mut model, 1.0), [DETAIL_FOCUS]);
        assert_eq!(detail(&model), Some((Detail::WiFi, 1.0, true)));
        assert!(after(&mut model, 1.0).is_empty(), "the focus moves once");
    }

    #[test]
    fn collapsing_shrinks_the_view_back_and_gives_the_focus_to_its_module() {
        let mut model = panel();
        Panel::update(&mut model, Message::Request(Request::Toggle));
        Panel::update(
            &mut model,
            Message::Input(Input::Expand(Detail::Sound, Via::Keyboard)),
        );
        after(&mut model, 1.0);
        Panel::update(&mut model, Message::Input(Input::Collapse(Via::Keyboard)));
        assert_eq!(model.controller.detail(), None);
        assert!(detail(&model).is_some(), "the view is still closing");
        assert_eq!(after(&mut model, 1.0), [focus_key(Module::Sound)]);
        assert_eq!(detail(&model), None);
    }

    #[test]
    fn escape_while_the_view_closes_dismisses_the_panel() {
        let mut model = panel();
        Panel::update(&mut model, Message::Request(Request::Toggle));
        Panel::update(
            &mut model,
            Message::Input(Input::Expand(Detail::WiFi, Via::Keyboard)),
        );
        after(&mut model, 0.1);
        Panel::update(&mut model, Message::Input(Input::Collapse(Via::Keyboard)));
        assert_eq!(model.controller.phase(model.now()), Phase::Shown);
        assert!(detail(&model).is_some(), "the view is still closing");
        Panel::update(&mut model, Message::Input(Input::Collapse(Via::Keyboard)));
        assert_eq!(model.controller.phase(model.now()), Phase::Hiding);
    }

    #[test]
    fn a_click_leaves_the_focus_where_it_is() {
        let mut model = panel();
        Panel::update(&mut model, Message::Request(Request::Toggle));
        Panel::update(
            &mut model,
            Message::Input(Input::Expand(Detail::Display, Via::Pointer)),
        );
        assert!(after(&mut model, 1.0).is_empty(), "no ring after a click");
        Panel::update(&mut model, Message::Input(Input::Collapse(Via::Keyboard)));
        assert_eq!(after(&mut model, 1.0), [focus_key(Module::Display)]);
    }

    #[test]
    fn a_reversal_turns_the_motion_where_it_is() {
        let mut model = panel();
        Panel::update(&mut model, Message::Request(Request::Toggle));
        Panel::update(
            &mut model,
            Message::Input(Input::Expand(Detail::Bluetooth, Via::Keyboard)),
        );
        after(&mut model, 0.08);
        let (_, halfway, _) = detail(&model).unwrap();
        Panel::update(&mut model, Message::Input(Input::Collapse(Via::Keyboard)));
        let (_, turned, _) = detail(&model).unwrap();
        assert!((turned - halfway).abs() < 0.05, "{turned} from {halfway}");
        Panel::update(
            &mut model,
            Message::Input(Input::Expand(Detail::Bluetooth, Via::Keyboard)),
        );
        assert_eq!(after(&mut model, 1.0), [DETAIL_FOCUS]);
    }

    #[test]
    fn reduce_motion_cross_fades_and_a_light_session_tints_the_glass() {
        let mut model = panel();
        Panel::update(&mut model, Message::Request(Request::ReduceMotion(true)));
        Panel::update(
            &mut model,
            Message::Request(Request::Appearance { dark: false }),
        );
        Panel::update(&mut model, Message::Request(Request::Toggle));
        Panel::update(
            &mut model,
            Message::Input(Input::Expand(Detail::Display, Via::Pointer)),
        );
        let (_, _, morph) = detail(&model).unwrap();
        assert!(!morph, "the view fades in place");
        assert!(model.presentation(model.now()).light);
        Panel::update(
            &mut model,
            Message::Request(Request::Appearance { dark: true }),
        );
        assert!(!model.presentation(model.now()).light);
    }

    #[test]
    fn opening_the_panel_again_starts_on_the_grid() {
        let mut model = panel();
        Panel::update(&mut model, Message::Request(Request::Toggle));
        Panel::update(
            &mut model,
            Message::Input(Input::Expand(Detail::WiFi, Via::Pointer)),
        );
        Panel::update(&mut model, Message::Input(Input::Dismiss));
        after(&mut model, 1.0);
        Panel::update(&mut model, Message::Request(Request::Toggle));
        assert_eq!(detail(&model), None);
        assert_eq!(model.controller.detail(), None);
    }

    #[test]
    fn an_arrow_key_moves_the_focus_and_the_settings_rows_dismiss() {
        let mut model = panel();
        Panel::update(&mut model, Message::Request(Request::Toggle));
        let commands = Panel::update(
            &mut model,
            Message::Input(Input::Focus("module.bluetooth".to_owned())),
        );
        assert!(commands
            .iter()
            .any(|c| matches!(c, Command::Focus(Key::Str(k)) if k == "module.bluetooth")));
        Panel::update(&mut model, Message::Input(Input::Settings(Detail::WiFi)));
        assert_eq!(model.controller.phase(model.now()), Phase::Hiding);
    }

    #[test]
    fn the_card_ignores_input_while_it_fades() {
        let mut model = panel();
        Panel::update(&mut model, Message::Request(Request::Toggle));
        let off = model.controller.controls().tile(Module::Bluetooth);
        Panel::update(&mut model, Message::Input(Input::Press(Module::Bluetooth)));
        let before = model.controller.controls().tile(Module::Bluetooth);
        assert_ne!(before, off, "a press on the shown card switches Bluetooth");
        Panel::update(&mut model, Message::Input(Input::Dismiss));
        Panel::update(&mut model, Message::Input(Input::Press(Module::Bluetooth)));
        assert_eq!(model.controller.controls().tile(Module::Bluetooth), before);
    }

    #[test]
    fn the_fade_ends_with_no_window_on_the_timer() {
        let mut model = panel();
        Panel::update(&mut model, Message::Request(Request::Toggle));
        Panel::update(&mut model, Message::Input(Input::Dismiss));
        // a second later, the fade is over
        model.start -= Duration::from_secs(1);
        let current = model.timer;
        Panel::update(&mut model, Message::Timer(current));
        assert!(app_ids(&model).is_empty());
    }

    /// The fade steps once per frame the card's window gets: each frame moves it on, and once it ends the frames
    /// stop and the card's window goes.
    #[test]
    fn the_fade_steps_at_each_frame_of_the_card_s_window_until_it_ends() {
        let mut model = panel();
        Panel::update(&mut model, Message::Request(Request::Toggle));
        model.start -= Duration::from_secs(1);
        Panel::update(&mut model, Message::Frame);
        Panel::update(&mut model, Message::Input(Input::Dismiss));
        let mut frames = 0;
        let mut last = model.presentation(model.now()).alpha;
        while model.animating {
            model.start -= Duration::from_micros(16_667);
            Panel::update(&mut model, Message::Frame);
            frames += 1;
            let alpha = model.presentation(model.now()).alpha;
            assert!(
                alpha < last || !model.animating,
                "frame {frames} moves the fade"
            );
            last = alpha;
            assert!(frames < 200, "the fade ends");
        }
        assert!(frames > 5, "the fade runs over frames: {frames}");
        assert!(app_ids(&model).is_empty());
        let before = model.timer;
        assert!(Panel::update(&mut model, Message::Frame).is_empty());
        assert_eq!(model.timer, before, "a frame after the fade does nothing");
    }

    #[test]
    fn a_stale_timer_is_ignored() {
        let mut model = panel();
        Panel::update(&mut model, Message::Request(Request::Toggle));
        let stale = model.timer;
        Panel::update(&mut model, Message::Input(Input::Dismiss));
        assert_ne!(model.timer, stale, "each schedule starts a new timer");
        let current = model.timer;
        assert!(Panel::update(&mut model, Message::Timer(stale)).is_empty());
        assert_eq!(model.timer, current, "nothing is scheduled again");
    }

    #[test]
    fn a_window_the_server_closed_hides_the_panel_at_once_and_a_toggle_opens_it_again() {
        for fading in [false, true] {
            let mut model = panel();
            Panel::update(&mut model, Message::Request(Request::Toggle));
            if fading {
                Panel::update(&mut model, Message::Input(Input::Dismiss));
            }
            let commands = Panel::update(&mut model, Message::Input(Input::Closed));
            assert!(
                app_ids(&model).is_empty(),
                "fading {fading}: no window is left"
            );
            assert_eq!(model.controller.phase(model.now()), Phase::Hidden);
            assert!(
                !timer(&commands) && !model.animating,
                "fading {fading}: no fade frame is asked for"
            );
            Panel::update(&mut model, Message::Request(Request::Toggle));
            assert_eq!(app_ids(&model), [CATCHER_APP_ID, APP_ID]);
            let off = model.controller.controls().tile(Module::Bluetooth);
            Panel::update(&mut model, Message::Input(Input::Press(Module::Bluetooth)));
            assert_ne!(
                model.controller.controls().tile(Module::Bluetooth),
                off,
                "fading {fading}: the reopened card switches Bluetooth"
            );
        }
    }

    #[test]
    fn the_output_going_hides_the_panel_at_once() {
        let mut model = panel();
        Panel::update(&mut model, Message::Request(Request::Toggle));
        Panel::update(&mut model, Message::Request(Request::OutputGone));
        assert!(app_ids(&model).is_empty());
        // a second close of the other window changes nothing
        Panel::update(&mut model, Message::Input(Input::Closed));
        assert_eq!(model.controller.phase(model.now()), Phase::Hidden);
    }

    #[test]
    fn the_panel_hears_when_its_output_goes() {
        let (a, b) = (4_u32, 7_u32);
        assert!(!output_gone::<u32>(None, &[]), "no output, nothing to lose");
        assert!(!output_gone(Some(a), &[(a, true), (b, true)]));
        assert!(
            !output_gone(Some(a), &[(a, false)]),
            "an output described anew is still there"
        );
        assert!(output_gone(Some(a), &[(b, true)]));
        assert!(output_gone(Some(a), &[]), "the last output went");
    }

    /// The panel opens on output `a`, the active one; the person moves to output `b`, which becomes the active one, and
    /// dismisses the panel; while the card fades they toggle it again: the new catcher opens on the card's output.
    #[test]
    fn a_toggle_during_the_fade_opens_the_catcher_on_the_card_s_output() {
        let (a, b) = (4_u32, 7_u32);
        let mut shared = SharedOutput::default();
        // the catcher opens first, then the card, in one pass: neither names an output, so both go to the server's
        // active output, `a`
        assert_eq!(shared.for_new_surface(&[]), None, "the catcher");
        assert_eq!(shared.for_new_surface(&[vec![]]), None, "the card");
        // both are told they are on `a`; the active output becomes `b`; the fade starts and the catcher goes, so only
        // the card is open when the toggle comes
        let card = [vec![a]];
        let catcher = shared.for_new_surface(&card);
        assert_eq!(
            catcher,
            Some(a),
            "the catcher shares the card's output, not the active `b`"
        );
        assert_eq!(catcher, card[0].first().copied());
        // learnt once, the output holds while a surface is open
        assert_eq!(shared.for_new_surface(&[vec![]]), Some(a));
        // after the fade nothing is open: the next panel opens on the active output again, now `b`
        assert_eq!(shared.for_new_surface(&[]), None);
        assert_eq!(shared.for_new_surface(&[vec![b]]), Some(b));
    }

    #[test]
    fn the_card_asks_for_the_keyboard_whenever_it_is_shown_and_gives_it_back_for_the_fade() {
        let mut keyboard = CardKeyboard::default();
        assert_eq!(keyboard.shown(), KeyboardInteractivity::Exclusive);
        assert_eq!(keyboard.hiding(), KeyboardInteractivity::None);
        assert_eq!(
            keyboard.shown(),
            KeyboardInteractivity::Exclusive,
            "a toggle during the fade takes the keyboard again"
        );
    }

    #[test]
    fn losing_the_keyboard_while_shown_dismisses_the_panel() {
        let mut keyboard = CardKeyboard::default();
        keyboard.shown();
        assert_eq!(keyboard.focus(true), None);
        assert_eq!(keyboard.focus(false), Some(Request::Dismiss));
        assert_eq!(keyboard.focus(false), None, "one leave, one dismissal");
    }

    #[test]
    fn giving_the_keyboard_back_for_the_fade_is_not_a_dismissal() {
        let mut keyboard = CardKeyboard::default();
        keyboard.shown();
        keyboard.focus(true);
        keyboard.hiding();
        assert_eq!(keyboard.focus(false), None);
        // the leave can come after a toggle during the fade has asked for the keyboard again
        keyboard.focus(true);
        keyboard.hiding();
        keyboard.shown();
        assert_eq!(keyboard.focus(false), None);
        assert_eq!(keyboard.focus(true), None);
        assert_eq!(keyboard.focus(false), Some(Request::Dismiss));
    }
}
