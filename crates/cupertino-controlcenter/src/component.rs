//! The control panel as a component of the shell host (`cupertino-shell`): the card's window and the catcher under it
//! on layer surfaces, `org.cupertino.ControlPanel1` on the component's own session bus connection, the readings and
//! changes carried out off the UI thread through the services that own them ([`effects`]), do not disturb and the
//! on-screen display through the host bus, and the fades on the compositor's clock.
//!
//! The panel opens on the output a `TogglePanel` publication names, else on the server's active output (the one the
//! person works on), and stays there until it has gone: the server closes its surfaces with their output, which hides
//! it at once. The card takes the exclusive keyboard while the panel is shown and gives it back as the fade starts; a
//! shown card that loses the keyboard to another surface, such as the launcher, dismisses the panel.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cupertino_kit::bus::{self as kit_bus, Bus};
use cupertino_kit::controls::FrameClock;
use cupertino_kit::core::{
    Application, Command as Do, Element, Key, RendererClass, Window, WindowKey,
};
use cupertino_kit::geometry::Size;
use cupertino_kit::platform::{PresentChange, PresentProperty, SystemAppearance, WindowRequest};
use cupertino_kit::task;
use cupertino_kit_shell::bus::{self, Message as Published, OsdKind, Panel, Topic};
use cupertino_kit_shell::outputs::{self, OutputId};
use cupertino_kit_shell::{
    design, host, session, Anchor, Component, ComponentName, Event, Keyboard, Layer, LayerSpec,
    Margin, Program, Role, Surface,
};
use cupertino_radios::watch::Radio;

use crate::control::{self, Call, Presence, Served};
use crate::controller::{Change, Controller, Effect, OverlayKind, Read};
use crate::effects::{self, Buses, Deadline, Through, PLAYER_DEADLINE};
use crate::layout::{BAR_MARGIN, SHADOW_TOP, SURFACE};
use crate::lifecycle::{self, Phase};
use crate::model::{Reading, VolumeState};
use crate::motion::Expansion;
use crate::view::{
    self, edit_key, focus_key, BandPictures, Bands, BandsWanted, Editing, Held, Input, Opened,
    Presentation, Via, CATCHER_KEY, DETAIL_FOCUS, EDIT_FOCUS, PANEL_KEY, PANEL_ROOT,
};

/// The layer surfaces' namespace, for the server.
const NAMESPACE: &str = "cupertino-controlcenter";
/// The settings application, which a detail view's last row opens on its page.
const SETTINGS: &str = "cupertino-settings";
/// How long naming a caller should take: a few small files of `/proc`.
const CALLER_DEADLINE: Duration = Duration::from_secs(1);
/// How long reading the menu bar's height from the theme should take: one small file, once per process.
const THEME_DEADLINE: Duration = Duration::from_secs(2);
/// What the focus capsule says while the notifications component, which owns do not disturb, is stopped.
const NO_FOCUS: &str = "the notifications are not running";

/// The control panel.
pub struct ControlCenter;

/// The radios whose readings are out of date, which their services' signals mark from the connection's tasks, and
/// where those tasks wake the model.
struct RadioSignals {
    changed: AtomicU8,
    wake: async_channel::Sender<()>,
}

impl RadioSignals {
    fn bit(radio: Radio) -> u8 {
        match radio {
            Radio::Network => 1,
            Radio::Bluetooth => 2,
        }
    }

    /// Note that `radio` changed, and wake the model unless a wake waits already.
    fn mark(&self, radio: Radio) {
        self.changed.fetch_or(Self::bit(radio), Ordering::AcqRel);
        let _ = self.wake.try_send(());
    }

    /// The radios marked since the last call: Wi-Fi, Bluetooth.
    fn take(&self) -> (bool, bool) {
        let changed = self.changed.swap(0, Ordering::AcqRel);
        (
            changed & Self::bit(Radio::Network) != 0,
            changed & Self::bit(Radio::Bluetooth) != 0,
        )
    }
}

/// What the component holds.
pub struct Model {
    controller: Controller,
    /// The start of the model's clock, which the fades, motions and writes are measured on.
    start: Instant,
    buses: Buses,
    /// The system bus connection is being made.
    connecting: bool,
    /// Effects that wait for their bus connection.
    waiting: Vec<(Through, Effect)>,
    /// Where `org.cupertino.ControlPanel1` hands its calls, and where they come out.
    calls: (async_channel::Sender<Call>, async_channel::Receiver<Call>),
    /// Where the presence goes to be announced on the bus, once the interface is served.
    presences: Option<async_channel::Sender<Presence>>,
    /// What the bus and the host bus were last told.
    told: Option<Presence>,
    appearance: SystemAppearance,
    /// The detail view opening, open or closing.
    expansion: Option<Expansion>,
    /// The tile the pointer holds while the controls are edited.
    held: Option<Held>,
    /// The detail view is moving: its end has not been seen yet.
    moving: bool,
    /// The keyboard or assistive technology started the motion, so the focus follows the view when it ends.
    follow_focus: bool,
    /// A detail view's motion runs: the panel's window asks for frames, each one a step.
    animating: bool,
    /// The phase the card's fade was last handed to the compositor for.
    faded: Phase,
    /// The output the panel opened on; `None` is the server's active output.
    output: Option<OutputId>,
    /// The menu bar's height, in points, once read from the theme.
    menu_bar: Option<f64>,
    /// A toggle that came before the menu bar's height, with its output.
    toggle_waiting: Option<Option<OutputId>>,
    bands: BandPictures,
    /// The pictures of the shadow's bands being drawn.
    drawing: Vec<BandsWanted>,
    /// The timer that counts; one scheduled before it is ignored when it fires.
    timer: u64,
    radios: Arc<RadioSignals>,
    radio_wakes: async_channel::Receiver<()>,
    /// The media calls' deadlines that have not come, by number: dropping one ends its calls.
    deadlines: HashMap<u64, async_channel::Sender<()>>,
    next_deadline: u64,
}

#[derive(Clone)]
pub enum Message {
    /// The component's session bus connection, or none.
    Session(Option<zbus::Connection>),
    SessionClosed,
    /// Whether `org.cupertino.ControlPanel1` is served.
    Served(Result<Served, String>),
    /// The system bus connection, or none.
    System(Option<zbus::Connection>),
    SystemClosed,
    /// A call of the bus interface.
    Call(Call),
    /// A caller was named for the call that waited.
    Judged,
    /// Open the panel or close it: on this output, or the server's active one.
    Toggle(Option<OutputId>),
    Input(Input),
    Read(Read),
    Changed(Change, Result<Option<VolumeState>, String>),
    /// The menu bar's height, in points.
    MenuBar(f64),
    /// A radio's service said its reading is out of date.
    Radios,
    /// The radios' signals are followed, or why not.
    RadiosFollowed(Result<(), String>),
    /// Do not disturb as the notifications published it.
    FocusMode(Reading<bool>),
    Locked(bool),
    Appearance(SystemAppearance),
    /// The server's outputs changed.
    Outputs,
    /// The server closed one of the panel's windows.
    Closed,
    /// The timer numbered so fired.
    Timer(u64),
    /// The panel's window got a frame from the compositor while a detail view moves.
    Frame,
    /// The media call of this deadline has had its time.
    Deadline(u64),
    /// The picture of the shadow's bands asked for, or none.
    Bands(BandsWanted, Option<Bands>),
    /// Move the focus to the control of this key, its ring showing (`true`) or as it was.
    Focus(String, bool),
    /// The bus announcements ended with the connection.
    Announced,
}

/// A message the runtime hands back on its next turn, after the view that follows has been built.
fn later(message: Message) -> Do<Message> {
    Do::Perform(Box::pin(async move { message }))
}

/// The next call of the bus interface.
fn listen(calls: &async_channel::Receiver<Call>) -> Do<Message> {
    let calls = calls.clone();
    Do::Perform(Box::pin(async move {
        match calls.recv().await {
            Ok(call) => Message::Call(call),
            // the model holds the sender, so the channel never closes while it lives
            Err(_) => std::future::pending().await,
        }
    }))
}

/// The next wake of the radios' signals.
fn hear_radios(wakes: &async_channel::Receiver<()>) -> Do<Message> {
    let wakes = wakes.clone();
    Do::Perform(Box::pin(async move {
        match wakes.recv().await {
            Ok(()) => Message::Radios,
            Err(_) => std::future::pending().await,
        }
    }))
}

fn input(input: Input) -> Message {
    Message::Input(input)
}

fn read(read: Read) -> Message {
    Message::Read(read)
}

fn changed(change: Change, result: Result<Option<VolumeState>, String>) -> Message {
    Message::Changed(change, result)
}

/// The card's fade when the panel turned to `phase` from `told`, for the compositor to run: from nothing when the card
/// opens, from where the fade is when it turns. None under Reduce Motion, where the card appears and goes at once.
fn fade(told: Phase, phase: Phase, reduce_motion: bool) -> Option<Vec<PresentChange>> {
    let opacity = |to: f64, curve| PresentChange {
        property: PresentProperty::Opacity,
        to,
        curve,
        velocity: 0.0,
    };
    match (told, phase) {
        _ if reduce_motion || told == phase => None,
        (Phase::Hidden, Phase::Shown) => Some(vec![
            opacity(0.0, None),
            opacity(1.0, Some(lifecycle::open())),
        ]),
        (Phase::Hiding, Phase::Shown) => Some(vec![opacity(1.0, Some(lifecycle::open()))]),
        (Phase::Shown, Phase::Hiding) => Some(vec![opacity(0.0, Some(lifecycle::close()))]),
        _ => None,
    }
}

/// Starts the settings application on `pane`, as a program of its own the host reaps.
fn open_settings(pane: &str) -> Result<(), String> {
    let mut command = std::process::Command::new(SETTINGS);
    command
        .args(["--pane", pane])
        .stdin(Stdio::null())
        .stdout(Stdio::null());
    if host::spawn(command) {
        Ok(())
    } else {
        Err(format!("cannot start {SETTINGS}"))
    }
}

impl Model {
    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    /// Carry out `effects`: the bus calls once their connection is there, the rest at once.
    fn run(&mut self, effects: Vec<Effect>) -> Vec<Do<Message>> {
        let mut commands = Vec::new();
        for effect in effects {
            commands.extend(self.carry(effect));
        }
        commands
    }

    fn carry(&mut self, effect: Effect) -> Vec<Do<Message>> {
        let through = match &effect {
            Effect::Read(source) => effects::read_through(*source),
            Effect::Change(change) => effects::change_through(change),
            Effect::OpenSettings(pane) => {
                let now = self.now();
                self.controller.settings_opened(now, open_settings(pane));
                return Vec::new();
            }
            Effect::Overlay(kind, level) => {
                let kind = match kind {
                    OverlayKind::Volume => OsdKind::Volume,
                    OverlayKind::Mute => OsdKind::Mute,
                    OverlayKind::Brightness => OsdKind::Brightness,
                };
                bus::publish(Published::ShowOsd {
                    kind,
                    level: *level,
                });
                return Vec::new();
            }
            Effect::FocusMode(on) => {
                bus::publish(Published::SetDoNotDisturb {
                    on: *on,
                    lifetime: None,
                });
                return Vec::new();
            }
        };
        match through {
            Some(Through::Session) if self.buses.session.is_none() => {
                // the session bus connection is made as the component starts
                self.waiting.push((Through::Session, effect));
                Vec::new()
            }
            Some(Through::System) if self.buses.system.is_none() => {
                self.waiting.push((Through::System, effect));
                self.connect_system()
            }
            _ => self.dispatch(effect),
        }
    }

    /// Start `effect` on the connections there are; one whose connection is missing fails at once.
    fn dispatch(&mut self, effect: Effect) -> Vec<Do<Message>> {
        let media = matches!(
            effect,
            Effect::Read(crate::controller::Source::Media) | Effect::Change(Change::Media { .. })
        );
        let (deadline, mut commands) = if media {
            let (deadline, timer) = self.deadline();
            (Some(deadline), vec![timer])
        } else {
            (None, Vec::new())
        };
        match effect {
            Effect::Read(source) => {
                commands.push(effects::read(source, &self.buses, deadline, read));
            }
            Effect::Change(change) => {
                commands.push(effects::change(change, &self.buses, deadline, changed));
            }
            // carried out by [`Model::carry`]
            Effect::OpenSettings(_) | Effect::Overlay(..) | Effect::FocusMode(_) => {}
        }
        commands
    }

    /// A deadline for a media call, and the timer that brings it.
    fn deadline(&mut self) -> (Deadline, Do<Message>) {
        let (sender, receiver) = async_channel::bounded(1);
        self.next_deadline += 1;
        self.deadlines.insert(self.next_deadline, sender);
        let timer = Do::After {
            delay: PLAYER_DEADLINE,
            message: Message::Deadline(self.next_deadline),
        };
        (receiver, timer)
    }

    /// Start the effects that waited for the `through` connection, which is there now or could not be made.
    fn release(&mut self, through: Through) -> Vec<Do<Message>> {
        let (ready, waiting) = std::mem::take(&mut self.waiting)
            .into_iter()
            .partition(|(t, _)| *t == through);
        self.waiting = waiting;
        let ready: Vec<(Through, Effect)> = ready;
        let mut commands = Vec::new();
        for (_, effect) in ready {
            commands.extend(self.dispatch(effect));
        }
        commands
    }

    fn connect_system(&mut self) -> Vec<Do<Message>> {
        if self.connecting {
            return Vec::new();
        }
        self.connecting = true;
        kit_bus::connect(
            Bus::System,
            |connection| Message::System(connection.ok()),
            Message::SystemClosed,
        )
    }

    /// The system bus connection came, or could not be made: the radios' signals are followed on it, and the effects
    /// that waited for it start (and fail without it).
    fn system(&mut self, connection: Option<zbus::Connection>) -> Vec<Do<Message>> {
        self.connecting = false;
        self.buses.system = connection.clone();
        let mut commands = self.release(Through::System);
        if let Some(system) = connection {
            let radios = Arc::clone(&self.radios);
            commands.push(Do::Perform(Box::pin(async move {
                let on = Arc::new(move |radio| radios.mark(radio));
                Message::RadiosFollowed(
                    cupertino_radios::watch::follow(&system, on)
                        .await
                        .map_err(|e| e.to_string()),
                )
            })));
        }
        commands
    }

    /// The session bus connection came: the interface is served on it, and the effects that waited for it start.
    fn session(&mut self, connection: zbus::Connection) -> Vec<Do<Message>> {
        self.buses.session = Some(connection.clone());
        let calls = self.calls.0.clone();
        let mut commands = vec![Do::Perform(Box::pin(async move {
            Message::Served(
                control::serve(&connection, calls)
                    .await
                    .map_err(|e| e.to_string()),
            )
        }))];
        commands.extend(self.release(Through::Session));
        commands
    }

    /// Open the panel, or dismiss it when it is shown. A panel that opens from hidden goes on `output` when the server
    /// has it, else on the server's active output; one that opens again during its fade stays where it was.
    fn toggle(&mut self, output: Option<OutputId>) -> Vec<Do<Message>> {
        if self.menu_bar.is_none() {
            // the card is placed below the menu bar: it opens once the bar's height is known
            self.toggle_waiting = Some(output);
            return Vec::new();
        }
        let now = self.now();
        if self.controller.phase(now) == Phase::Hidden {
            self.output = output.filter(|id| outputs::get(*id).is_some());
        }
        let effects = self.controller.toggle(now);
        let mut commands = self.run(effects);
        if self.controller.phase(now) == Phase::Shown {
            self.reset();
            // the card's root holds the focus, so the panel hears the keyboard leave it
            commands.push(later(Message::Focus(PANEL_ROOT.to_owned(), false)));
        }
        commands
    }

    /// Carry out a call of the bus interface.
    fn call(&mut self, call: Call) -> Vec<Do<Message>> {
        let mut commands = vec![listen(&self.calls.1)];
        match call {
            Call::Judge { credentials, reply } => commands.push(task::io(
                CALLER_DEADLINE,
                move || {
                    let caller = control::judge(credentials.as_deref());
                    // the call waits on the receiver; one that went away needs no answer
                    let _ = reply.try_send(Arc::new(caller));
                },
                |()| Message::Judged,
            )),
            Call::Toggle => commands.extend(self.toggle(None)),
        }
        commands
    }

    /// The panel's windows are gone: it is hidden at once.
    fn vanish(&mut self) {
        self.controller.vanish();
        self.reset();
    }

    /// Forget the detail view and a held tile: the panel opened again or went, or its controls are to be edited.
    fn reset(&mut self) {
        self.expansion = None;
        self.moving = false;
        self.held = None;
    }

    /// The scales the card's output draws at: its own, or while the server places the card every output's.
    fn scales(&self) -> Vec<f64> {
        let mut scales: Vec<f64> = match self.output.and_then(outputs::get) {
            Some(output) => vec![output.scale],
            None => outputs::all().into_iter().map(|o| o.scale).collect(),
        };
        scales.sort_by(f64::total_cmp);
        scales.dedup();
        scales
    }

    /// The connector name of the card's output: the one it opened on, else the server's only output; empty when the
    /// server places it on one of several.
    fn output_name(&self) -> String {
        let named = match self.output {
            Some(id) => outputs::get(id),
            None => {
                let all = outputs::all();
                if all.len() == 1 {
                    all.into_iter().next()
                } else {
                    None
                }
            }
        };
        named.and_then(|o| o.name).unwrap_or_default()
    }

    /// How the panel is shown at `now`. The compositor fades what the card draws whole, so it draws at full opacity.
    fn presentation(&self, now: f64) -> Presentation {
        Presentation {
            alpha: 1.0,
            light: !self.appearance.dark,
            input: self.controller.phase(now) == Phase::Shown,
            // a view whose module a changed layout no longer shows has no tile to grow from
            detail: self
                .expansion
                .filter(|e| self.controller.arrangement().contains(e.detail.module()))
                .map(|e| Opened {
                    detail: e.detail,
                    progress: e.progress(now),
                    morph: !e.reduce_motion,
                }),
            edit: self
                .controller
                .editing()
                .then_some(Editing { held: self.held }),
        }
    }

    /// The pictures of the shadow's bands the card at rest lays and does not have yet, at each scale it may draw at.
    /// The bands show only without Reduce Transparency and Increase Contrast.
    fn draw_bands(&mut self, now: f64) -> Vec<Do<Message>> {
        if self.controller.phase(now) == Phase::Hidden
            || self.appearance.reduce_transparency
            || self.appearance.increase_contrast
        {
            return Vec::new();
        }
        let presentation = self.presentation(now);
        let mut commands = Vec::new();
        for scale in self.scales() {
            let Some(wanted) = view::bands_wanted(
                self.controller.controls(),
                self.controller.arrangement(),
                presentation,
                scale,
            ) else {
                continue;
            };
            if !self.bands.wants(&wanted) || self.drawing.contains(&wanted) {
                continue;
            }
            self.drawing.push(wanted);
            let env = cupertino_kit_shell::environment();
            commands.push(wanted.draw(&env, move |bands| Message::Bands(wanted, bands)));
        }
        commands
    }

    /// Tell the bus and the host bus whether the panel is shown and where, when that changed.
    fn announce(&mut self, now: f64) {
        let shown = self.controller.phase(now) == Phase::Shown;
        let presence = Presence {
            shown,
            output: if shown {
                self.output_name()
            } else {
                String::new()
            },
        };
        if self.told.as_ref() == Some(&presence) {
            return;
        }
        if let Some(presences) = &self.presences {
            // the announcing task holds the receiver for as long as the connection lives
            let _ = presences.try_send(presence.clone());
        }
        bus::publish(Published::PanelShown {
            panel: Panel::ControlPanel,
            shown,
            output: self.output.filter(|_| shown),
        });
        self.told = Some(presence);
    }

    /// After every message: hand the card's fade to the compositor, time the next write, refresh or end of the fade,
    /// say on the buses whether the panel is shown and ask for the pictures of the bands it lacks.
    fn schedule(&mut self) -> Vec<Do<Message>> {
        self.timer += 1;
        let now = self.now();
        let phase = self.controller.phase(now);
        self.animating = self.moving && phase == Phase::Shown;
        let mut commands = Vec::new();
        if let Some(changes) = fade(self.faded, phase, self.appearance.reduce_motion) {
            commands.push(Do::Window(
                WindowKey(Key::from(PANEL_KEY)),
                WindowRequest::Present(changes),
            ));
        }
        self.faded = phase;
        self.announce(now);
        commands.extend(self.draw_bands(now));
        if let Some(wait) = self.controller.next_wakeup(now) {
            commands.push(Do::After {
                delay: Duration::from_secs_f64(wait),
                message: Message::Timer(self.timer),
            });
        }
        commands
    }

    /// The controller and the motion advance to now.
    fn step(&mut self) -> Vec<Do<Message>> {
        let now = self.now();
        let effects = self.controller.tick(now);
        let mut commands = self.run(effects);
        commands.extend(self.settle(now).map(|key| Do::Focus(Key::from(key))));
        commands
    }
}

impl Model {
    /// Carry out what the person did on the panel; the panel only acts while it is shown, not while it fades. A window
    /// the server closed hides the panel at once in any phase.
    fn input(&mut self, input: Input) -> Vec<Do<Message>> {
        let now = self.now();
        if input == Input::Closed {
            self.vanish();
            return Vec::new();
        }
        if self.controller.phase(now) != Phase::Shown {
            return Vec::new();
        }
        let effects = match input {
            Input::Press(module) => self.controller.press(module),
            Input::Expand(detail, via) => {
                self.follow_focus = via == Via::Keyboard;
                match &mut self.expansion {
                    Some(e) if e.detail == detail => e.turn(true, now),
                    _ => {
                        self.expansion =
                            Some(Expansion::open(detail, now, self.appearance.reduce_motion));
                    }
                }
                self.moving = true;
                self.controller.show_detail(Some(detail))
            }
            // a view already closing counts as closed: Escape or a press outside then dismisses the panel
            Input::Collapse(_) if self.controller.detail().is_none() => {
                self.controller.dismiss(now);
                Vec::new()
            }
            Input::Collapse(via) => {
                self.follow_focus = via == Via::Keyboard;
                if let Some(e) = &mut self.expansion {
                    e.turn(false, now);
                    self.moving = true;
                }
                self.controller.show_detail(None)
            }
            Input::Media(command) => self.controller.media(command),
            Input::Join(ssid) => self.controller.join(&ssid),
            Input::Connect(id) => self.controller.connect(&id),
            Input::Output(id) => self.controller.select_output(id),
            Input::Settings(detail) => self.controller.open_settings(detail.settings_pane()),
            input @ (Input::EditControls
            | Input::Remove(..)
            | Input::Add(..)
            | Input::Step(..)
            | Input::DragTo(..)
            | Input::Drop
            | Input::Done
            | Input::Cancel) => return self.edit(input),
            Input::Focus(key) => return vec![Do::Focus(Key::from(key))],
            Input::Slide(slider, fraction) => self.controller.slide(now, slider, fraction),
            Input::Release(..) => self.controller.release(now),
            Input::Dismiss => {
                self.controller.dismiss(now);
                Vec::new()
            }
            Input::Closed => Vec::new(),
        };
        self.run(effects)
    }

    /// What the person did while editing the controls.
    fn edit(&mut self, input: Input) -> Vec<Do<Message>> {
        match input {
            // the controls are edited on the grid: an open view goes at once
            Input::EditControls => {
                if self.controller.edit_controls() {
                    self.reset();
                }
            }
            Input::Remove(module, via) => {
                let place = self.controller.arrangement().position(module);
                if self.controller.edit_remove(module) && via == Via::Keyboard {
                    // the focus goes to the tile that took the place, else the one before, else Done
                    let shown = self.controller.arrangement().shown();
                    let next = place
                        .and_then(|place| shown.get(place).or(shown.last()))
                        .map_or_else(|| EDIT_FOCUS.to_owned(), |module| edit_key(*module));
                    return vec![Do::Focus(Key::from(next))];
                }
            }
            Input::Add(module, via) => {
                if self.controller.edit_add(module) && via == Via::Keyboard {
                    // the tile is there to take the focus once the view that follows has built it
                    return vec![later(Message::Focus(edit_key(module), true))];
                }
            }
            Input::Step(module, forward) => {
                self.controller.edit_step(module, forward);
            }
            Input::DragTo(module, origin) => {
                if self.controller.editing() && self.controller.arrangement().contains(module) {
                    self.held = Some(Held { module, origin });
                    self.controller.edit_move_near(module, origin);
                }
            }
            Input::Drop => self.held = None,
            Input::Done => {
                self.held = None;
                let effects = self.controller.finish_edit();
                return self.run(effects);
            }
            Input::Cancel => {
                self.held = None;
                self.controller.cancel_edit();
            }
            _ => {}
        }
        Vec::new()
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

    /// A publication of the host bus.
    fn published(message: Published) -> Option<Message> {
        match message {
            Published::TogglePanel {
                panel: Panel::ControlPanel,
                output,
            } => Some(Message::Toggle(output)),
            Published::DoNotDisturbChanged { on, .. } => {
                Some(Message::FocusMode(Reading::Ready(on)))
            }
            Published::ComponentStopped { component }
                if component == ComponentName::NOTIFICATIONS =>
            {
                Some(Message::FocusMode(Reading::Unavailable(
                    NO_FOCUS.to_owned(),
                )))
            }
            _ => None,
        }
    }

    /// A message from outside the panel's windows: the buses, the server and the session.
    fn outside(&mut self, message: Message) -> Vec<Do<Message>> {
        match message {
            Message::Session(Some(connection)) => self.session(connection),
            Message::Session(None) | Message::SessionClosed => {
                // without its bus connection the component serves nobody: the host starts it again
                eprintln!("cupertino-controlcenter: no session bus connection");
                vec![Do::Exit]
            }
            Message::Served(Ok(served)) => {
                let (presences, announced) = async_channel::unbounded();
                self.presences = Some(presences);
                // announced anew on the bus that serves the interface
                self.told = None;
                vec![Do::Perform(Box::pin(async move {
                    served.announce(announced).await;
                    Message::Announced
                }))]
            }
            Message::Served(Err(e)) => {
                eprintln!(
                    "cupertino-controlcenter: {} is not served: {e}",
                    control::BUS_NAME
                );
                vec![Do::Exit]
            }
            Message::System(connection) => self.system(connection),
            Message::SystemClosed => {
                self.buses.system = None;
                Vec::new()
            }
            Message::RadiosFollowed(Err(e)) => {
                eprintln!(
                    "cupertino-controlcenter: cannot follow the radios' change signals, an open panel reads them every second: {e}"
                );
                Vec::new()
            }
            Message::Call(call) => self.call(call),
            Message::Toggle(output) => self.toggle(output),
            Message::MenuBar(height) => {
                self.menu_bar = Some(height);
                match self.toggle_waiting.take() {
                    Some(output) => self.toggle(output),
                    None => Vec::new(),
                }
            }
            Message::Radios => {
                let now = self.now();
                let (wifi, bluetooth) = self.radios.take();
                let effects = self.controller.radios_changed(now, wifi, bluetooth);
                let mut commands = self.run(effects);
                commands.push(hear_radios(&self.radio_wakes));
                commands
            }
            Message::FocusMode(reading) => {
                self.controller.set_focus_mode(reading);
                Vec::new()
            }
            Message::Locked(true) => {
                // the server draws the lock screen: a shown panel goes
                let now = self.now();
                self.controller.dismiss(now);
                Vec::new()
            }
            Message::Appearance(appearance) => {
                self.appearance = appearance;
                self.controller.set_reduce_motion(appearance.reduce_motion);
                Vec::new()
            }
            Message::Outputs => {
                // the server closes the surfaces of an output it drops; a panel on one that went goes at once
                if self.output.is_some_and(|id| outputs::get(id).is_none()) {
                    self.vanish();
                    self.output = None;
                }
                Vec::new()
            }
            Message::Closed => {
                self.vanish();
                Vec::new()
            }
            _ => Vec::new(),
        }
    }
}

impl Application for ControlCenter {
    type Model = Model;
    type Message = Message;
    const RESIDENT: bool = true;
    /// The panel draws a frame as it opens and as a detail view moves; its fades are the compositor's. The GPU would
    /// draw those frames little faster than the CPU and keep a context for them.
    const RENDERER: RendererClass = RendererClass::Cpu;

    fn init() -> (Model, Vec<Do<Message>>) {
        bus::follow(&[Topic::Host, Topic::Panels, Topic::DoNotDisturb]);
        let calls = async_channel::unbounded();
        let (wake, radio_wakes) = async_channel::bounded(1);
        let mut commands = bus::session(
            |connection| Message::Session(connection.ok()),
            Message::SessionClosed,
        );
        commands.push(listen(&calls.1));
        commands.push(hear_radios(&radio_wakes));
        commands.push(task::io(
            THEME_DEADLINE,
            || design::menu_bar().height,
            Message::MenuBar,
        ));
        let mut model = Model {
            controller: Controller::new(),
            start: Instant::now(),
            buses: Buses::default(),
            connecting: false,
            waiting: Vec::new(),
            calls,
            presences: None,
            told: None,
            appearance: SystemAppearance::default(),
            expansion: None,
            held: None,
            moving: false,
            follow_focus: false,
            animating: false,
            faded: Phase::Hidden,
            output: None,
            menu_bar: None,
            toggle_waiting: None,
            bands: BandPictures::default(),
            drawing: Vec::new(),
            timer: 0,
            radios: Arc::new(RadioSignals {
                changed: AtomicU8::new(0),
                wake,
            }),
            radio_wakes,
            deadlines: HashMap::new(),
            next_deadline: 0,
        };
        // the first opening shows the readings at once: they are taken as the component starts
        let now = model.now();
        let effects = model.controller.refresh(now);
        commands.extend(model.run(effects));
        commands.extend(model.schedule());
        (model, commands)
    }

    fn update(model: &mut Model, message: Message) -> Vec<Do<Message>> {
        let mut commands = match message {
            Message::Input(input) => model.input(input),
            Message::Read(read) => {
                let now = model.now();
                let effects = model.controller.read(now, read);
                model.run(effects)
            }
            Message::Changed(change, result) => {
                let now = model.now();
                let effects = model.controller.changed(now, change, result);
                model.run(effects)
            }
            Message::Timer(timer) if timer != model.timer => return Vec::new(),
            Message::Timer(_) => model.step(),
            Message::Frame if model.animating => model.step(),
            Message::Frame | Message::Judged | Message::Announced => return Vec::new(),
            Message::RadiosFollowed(Ok(())) => return Vec::new(),
            Message::Deadline(id) => {
                // the calls waiting on it end now
                model.deadlines.remove(&id);
                return Vec::new();
            }
            Message::Bands(wanted, bands) => {
                model.drawing.retain(|w| *w != wanted);
                if let Some(bands) = bands {
                    model.bands.keep(bands);
                }
                Vec::new()
            }
            Message::Focus(key, true) => return vec![Do::Focus(Key::from(key))],
            Message::Focus(key, false) => return vec![Do::MoveFocus(Key::from(key))],
            message => model.outside(message),
        };
        commands.extend(model.schedule());
        commands
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        let now = model.now();
        let phase = model.controller.phase(now);
        if phase == Phase::Hidden {
            return Vec::new();
        }
        let mut panel = view::arranged(
            model.controller.controls(),
            model.controller.arrangement(),
            model.presentation(now),
            &model.bands,
            input,
        );
        // what moves steps once per frame the compositor gives the panel's window, never on a timer of its own
        panel.content = Element::from(FrameClock {
            content: panel.content,
            on_frame: model.animating.then_some(Message::Frame),
            on_layers_settled: None,
        });
        match phase {
            Phase::Shown => vec![view::catcher(input), panel],
            Phase::Hiding | Phase::Hidden => vec![panel],
        }
    }
}

impl Program for ControlCenter {
    /// The card: on the overlay layer, top right below the menu bar of its output, with the exclusive keyboard while
    /// the panel is shown, so it is the key window as soon as it opens. The catcher: on the top layer under it, over
    /// the whole output, with no keyboard.
    fn surface(model: &Model, window: &WindowKey) -> Surface {
        let spec = if *window == WindowKey(Key::from(CATCHER_KEY)) {
            LayerSpec {
                namespace: NAMESPACE,
                output: model.output,
                layer: Layer::Top,
                anchor: Anchor {
                    top: true,
                    right: true,
                    bottom: true,
                    left: true,
                },
                size: Size::new(0.0, 0.0),
                margin: Margin::default(),
                exclusive_zone: -1,
                keyboard: Keyboard::None,
            }
        } else {
            let shown = model.controller.phase(model.now()) == Phase::Shown;
            let menu_bar = model.menu_bar.unwrap_or_default().ceil() as i32;
            LayerSpec {
                namespace: NAMESPACE,
                output: model.output,
                layer: Layer::Overlay,
                anchor: Anchor {
                    top: true,
                    right: true,
                    ..Anchor::default()
                },
                size: SURFACE,
                margin: Margin {
                    // the bar reserves no layer zone, so the margin carries its height; the surface reaches up past
                    // the window by the shadow's margin, never above the output
                    top: (menu_bar + BAR_MARGIN - SHADOW_TOP as i32).max(0),
                    ..Margin::default()
                },
                exclusive_zone: -1,
                keyboard: if shown {
                    Keyboard::Exclusive
                } else {
                    Keyboard::None
                },
            }
        };
        Surface {
            role: Role::Layer(spec),
            persistent: None,
            holes: Vec::new(),
        }
    }

    fn event(event: Event) -> Option<Message> {
        match event {
            Event::Session(session::Change::Locked(locked)) => Some(Message::Locked(locked)),
            Event::Appearance(appearance) => Some(Message::Appearance(appearance)),
            Event::Outputs => Some(Message::Outputs),
            Event::Closed(_) => Some(Message::Closed),
            Event::Bus { message, .. } => Model::published(message),
            _ => None,
        }
    }
}

impl Component for ControlCenter {
    const NAME: ComponentName = ComponentName::CONTROLCENTER;
}
