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
    /// The output the panel's surfaces are on went away: the panel is hidden at once.
    OutputGone,
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
/// The output is chosen when the panel opens with no surface left (the first output the server describes, see
/// [`opening_output`]) and kept while the card or the catcher exists, so the two stay together and a toggle during
/// the fade reopens on the card's output. When that output goes, the panel is dismissed at once (it does not move to
/// another output); the next toggle opens it on an output that exists.
struct PanelRoles {
    shell: LayerShell,
    /// The output of the panel's surfaces; `None` while it has none, or with no output to choose (the server then
    /// closes what opens).
    output: Option<OutputId>,
    /// The menu bar's height, in whole points.
    menu_bar: i32,
    requests: async_channel::Sender<Request>,
    card: Option<LayerSurface>,
    catcher: Option<LayerSurface>,
    keyboard: CardKeyboard,
}

impl PanelRoles {
    fn config(&self, spec: &WindowSpec) -> Option<LayerSurfaceConfig> {
        let namespace = NAMESPACE.to_string();
        let output = self.output;
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
        if self.card.is_none() && self.catcher.is_none() {
            self.output = opening_output(&described(conn));
        }
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
        if let Some(AppEvent::SessionLock { locked: true }) = event.downcast_ref::<AppEvent>() {
            let _ = self.requests.send_blocking(Request::Dismiss);
            // the window system follows the lock too
            return None;
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
        if output_gone(self.output, &described(conn)) {
            self.output = None;
            if self.card.is_some() || self.catcher.is_some() {
                let _ = self.requests.send_blocking(Request::OutputGone);
            }
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

    /// Carry out what the user did on the card; the card only acts while it is shown, not while it fades. A window the
    /// server closed hides the panel at once in any phase.
    fn input(&mut self, input: Input) {
        let now = self.now();
        if input == Input::Closed {
            self.controller.vanish();
            return;
        }
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
            Input::Closed => unreachable!("handled above"),
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
                    Request::OutputGone => model.controller.vanish(),
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
            Phase::Hiding => vec![view::panel(controls, model.controller.opacity(now), input)],
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

/// The output a panel that has no surface opens on: the first one the server has described. `None` without one; the
/// server then puts the surfaces on its active output, or closes them when it has no output at all.
fn opening_output<O: Copy>(outputs: &[(O, bool)]) -> Option<O> {
    outputs
        .iter()
        .find(|(_, described)| *described)
        .map(|(id, _)| *id)
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
    let conn = system.connection();
    let shell = LayerShell::bind(conn).ok_or("the server lacks zwlr_layer_shell_v1")?;
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
        output: None,
        menu_bar,
        requests: sender,
        card: None,
        catcher: None,
        keyboard: CardKeyboard::default(),
    });
    cupertino_kit::app::run_on::<Panel>(
        system,
        start(Controller::new(backends(options)), requests),
    )?;
    Err("the control panel's window system stopped".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cupertino_controlcenter::model::Module;

    fn panel() -> Panel {
        let options = Options {
            fake_backends: true,
            alsa_card: None,
        };
        start(
            Controller::new(backends(&options)),
            async_channel::unbounded().1,
        )
        .0
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
        let commands = Panel::update(&mut model, Message::Input(Input::Dismiss));
        assert_eq!(app_ids(&model), [APP_ID]);
        assert!(timer(&commands), "the fade's next frame is scheduled");
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
                !timer(&commands),
                "fading {fading}: no fade frame is scheduled"
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
    fn the_panel_opens_on_the_first_described_output_and_hears_when_its_output_goes() {
        let (a, b) = (4_u32, 7_u32);
        assert_eq!(opening_output::<u32>(&[]), None);
        assert_eq!(opening_output(&[(a, false), (b, true)]), Some(b));
        assert_eq!(opening_output(&[(a, true), (b, true)]), Some(a));
        assert!(!output_gone::<u32>(None, &[]), "no output, nothing to lose");
        assert!(!output_gone(Some(a), &[(a, true), (b, true)]));
        assert!(
            !output_gone(Some(a), &[(a, false)]),
            "an output described anew is still there"
        );
        assert!(output_gone(Some(a), &[(b, true)]));
        assert!(output_gone(Some(a), &[]), "the last output went");
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
