//! The menu bar as a component of the shell host (`cupertino-shell`): one bar per output on the `menu_bar` role, a
//! persistent surface the server holds with its last picture while the component restarts (D19), drawn by
//! [`crate::view`]; its menus are Kit popups of the bar. The server's menu-bar feed, the outputs, the appearance and
//! the lock arrive as shell events; the session bus work (the status notifier watcher, the menu exporters, the
//! registrar) runs on the component's own session connection ([`crate::worker`], [`crate::extras_bus`]), the radios
//! and the battery on its system connection; do not disturb, the panels that are open and the session agent's
//! entries (`SessionMenu`) come over the host bus, and the bar's toggles and do not disturb go out on it.
//!
//! Nothing here waits on the UI thread: the bus work is the connections' tasks, which Kit runs between frames; the
//! settings files, the desktop entries, Recent Items, the icons, the application associations and the X server of the
//! X11 bridge are read on the I/O pool, and their answers come back as messages. The decisions live in the library
//! modules (`feed`, `routing`, `controller`); this module connects them to the shell.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use cupertino_apps::Launch;
use cupertino_defaults::accessibility::DisplayOptions;
use cupertino_defaults::menubar::MenuBarSettings;
use cupertino_kit::core::{Application, Command as Do, RendererClass, Window, WindowKey};
use cupertino_kit::geometry::Size;
use cupertino_kit::platform::SystemAppearance;
use cupertino_kit::text::{Keymap, TextSystem};
use cupertino_kit_shell::bus::{self, Message as Published, Panel, Sender, Topic};
use cupertino_kit_shell::outputs::OutputId;
use cupertino_kit_shell::{
    design, menu_bar, session, Anchor, Component, ComponentName, Event, Layer, LayerSpec, Margin,
    Program, Role, Surface,
};

use crate::clock::ClockSettings;
use crate::desktop::AppName;
use crate::extras::{Extras, FocusLifetime, FocusState, OpenPanels, SessionCaps, SessionState};
use crate::extras_bus::ExtrasBus;
use crate::feed::{Shared, X11Query};
use crate::icons::{Loader, Pixels};
use crate::layout::{BarMetrics, SlotKey};
use crate::recent::{RecentItems, RecentSources};
use crate::registered::{Matched, SeenX11};
use crate::view::{self, BarFont, BarScene, Input, PromptScene, RowIcons};
use crate::wakeup::Wakeup;
use crate::worker::{self, Request, Worker};
use crate::x11::XServer;

mod bars;
mod tasks;

use bars::{Bar, JoinPrompt};

/// The bars' layer surfaces' namespace, for the server.
const NAMESPACE: &str = "cupertino-menubar";
/// The key the server holds every bar's picture and space under while the component restarts: one bar per output.
const HOLD: &str = "menu-bar";
/// The name the component reports problems under.
const PROCESS: &str = "cupertino-menubar";

/// The menu bar.
pub struct MenuBar;

/// A value a task hands the model once, through a message that has to be cloneable.
pub struct Handed<T>(Arc<Mutex<Option<T>>>);

impl<T> Clone for Handed<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T> Handed<T> {
    fn new(value: T) -> Self {
        Self(Arc::new(Mutex::new(Some(value))))
    }

    /// The value, the first time it is asked for.
    fn take(&self) -> Option<T> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
}

/// What the I/O pool answers about the active window of a bar on the X11 bridge: the connection to the X server and
/// what it has seen go back to the model with the answer.
pub struct X11Answer {
    server: XServer,
    seen: SeenX11,
    query: X11Query,
    matched: Matched,
}

/// A change of `menubar.conf` or `appearance.conf` the watch reported.
#[derive(Clone, Debug)]
pub enum ConfChange {
    Changed,
    Failed(String),
}

#[derive(Clone)]
pub enum Message {
    Input(Input),
    MenuBar(WindowKey, Box<menu_bar::Update>),
    Outputs,
    Appearance(SystemAppearance),
    Locked(bool),
    Bus(Sender, Published),
    /// The session connection, or none.
    Session(Option<zbus::Connection>),
    SessionClosed,
    /// The system connection, or why there is none.
    System(Result<zbus::Connection, String>),
    SystemClosed,
    /// The session bus worker serves the watcher, or could not.
    WorkerStarted(Handed<Result<(Worker, async_channel::Receiver<worker::Event>), String>>),
    Worker(worker::Event),
    Extras(Extras),
    /// `menubar.conf` as read, or why it could not be; the first read comes with the watch on the settings files.
    Conf(
        Result<MenuBarSettings, String>,
        Handed<Option<cupertino_defaults::watch::ConfigWatcher>>,
    ),
    ConfChanged(ConfChange),
    /// The appearance the schedule of `appearance.conf` asked for and the component wrote, if it wrote one.
    Scheduled(Option<bool>),
    Keymap(Handed<Keymap>),
    Recent(RecentItems),
    RecentCleared(Result<(), String>),
    AppName(String, AppName),
    X11(Handed<X11Answer>),
    IconsWanted,
    RowIcon((u64, u32), Option<Pixels>),
    StatusIcon(Box<crate::dbus::sni::StatusItem>, u64, Option<Pixels>),
    /// The default application for a document of a bar, or why there is none.
    Opened(
        WindowKey,
        std::path::PathBuf,
        Result<Option<Launch>, String>,
    ),
    Timer(u64),
    Token {
        launch: u64,
        token: Option<String>,
    },
    /// A task that answers nothing ended.
    Done,
}

/// The connections behind the bars and what waits for them.
#[derive(Default)]
struct Services {
    session: Option<zbus::Connection>,
    system: Option<Result<zbus::Connection, String>>,
    worker: Option<Worker>,
    worker_events: Option<async_channel::Receiver<worker::Event>>,
    extras_events: Option<async_channel::Receiver<Extras>>,
    /// Requests made before the worker serves, sent once it does.
    waiting: Vec<Request>,
    extras: Option<ExtrasBus>,
    /// Where Recent Items is read from; `None` when the session's directories are not known.
    recent: Option<Arc<RecentSources>>,
}

/// The X server of the X11 bridge: here while no question is asked of it, on the I/O pool while one is.
#[derive(Default)]
struct X11 {
    idle: Option<(XServer, SeenX11)>,
    /// The latest question of each bar waiting for the X server.
    waiting: Vec<X11Query>,
}

pub struct Model {
    services: Services,
    shared: Shared,
    bars: Vec<Bar>,
    text: Arc<TextSystem>,
    font: BarFont,
    metrics: BarMetrics,
    height: f64,
    display: DisplayOptions,
    dark: bool,
    icons: Rc<RowIcons>,
    icons_wanted: async_channel::Receiver<()>,
    loader: Loader,
    data_dirs: Vec<std::path::PathBuf>,
    start: Instant,
    /// When the Wi-Fi panel's "looking for networks" state ends, while a scan it asked for is under way.
    wifi_scan_until: Option<Instant>,
    /// The join prompt, while the person is asked for a passphrase.
    join: Option<JoinPrompt>,
    clock: ClockSettings,
    /// `menubar.conf` as last read, the last good values of the next read; `None` before the first read, while no bar
    /// is made.
    conf: Option<MenuBarSettings>,
    conf_changes: (
        async_channel::Sender<ConfChange>,
        async_channel::Receiver<ConfChange>,
    ),
    /// The appearance the schedule asked for last and the session has not shown yet.
    appearance_written: Option<bool>,
    /// A step of the schedule is under way on the I/O pool.
    scheduling: bool,
    /// Unix seconds at which the clock text is due again.
    next_clock: f64,
    /// The minute of the last reading of the extras' services.
    extras_minute: Option<i64>,
    /// Numbers the next bar.
    next_bar: u64,
    /// Launches waiting for their activation token, by number.
    pending: HashMap<u64, Launch>,
    next_launch: u64,
    /// The editing chords of the Help menu's search field, once read.
    keymap: Option<Keymap>,
    wakeup: Wakeup,
    /// The names of applications by id, as their desktop entries give them, and the ids being read.
    names: HashMap<String, AppName>,
    reading_names: HashSet<String>,
    x11: X11,
    /// The latest reading of each status item's icon, by item id: an older one that ends later is not shown.
    status_reads: HashMap<String, u64>,
    next_status_read: u64,
    /// Do not disturb, the open panels and the session agent's entries, as the host bus last told them.
    focus: Option<FocusState>,
    panels: OpenPanels,
    session: Option<SessionState>,
}

/// The next message on `channel`, as `message` makes it.
fn listen<T: Send + 'static>(
    channel: &async_channel::Receiver<T>,
    message: fn(T) -> Message,
) -> Do<Message> {
    let channel = channel.clone();
    Do::Perform(Box::pin(async move {
        match channel.recv().await {
            Ok(value) => message(value),
            // the model holds a sender, or the channel's task ends only with the component
            Err(_) => std::future::pending().await,
        }
    }))
}

/// Ends the component after a failure; the host starts it again.
fn fail(e: impl std::fmt::Display) -> Vec<Do<Message>> {
    eprintln!("{PROCESS}: {e}");
    vec![Do::Exit]
}

impl Model {
    /// Seconds on the monotonic clock the controllers are driven with.
    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    fn conf(&self) -> &MenuBarSettings {
        static UNSET: MenuBarSettings = MenuBarSettings::UNSET;
        self.conf.as_ref().unwrap_or(&UNSET)
    }

    /// The display options the bar's text tone follows: with a backdrop of its own the bar is solid as far as its text
    /// goes, whatever the wallpaper under it.
    fn tone_display(&self) -> DisplayOptions {
        DisplayOptions {
            reduce_transparency: self.display.reduce_transparency || self.conf().background,
            ..self.display
        }
    }

    /// Ask the session bus worker, or keep the request until it serves.
    fn request(&mut self, request: Request) -> Vec<Do<Message>> {
        match &self.services.worker {
            Some(worker) => match worker.send(request) {
                Ok(()) => Vec::new(),
                Err(e) => fail(e),
            },
            None => {
                self.services.waiting.push(request);
                Vec::new()
            }
        }
    }

    /// The appearance and the display options of the session: the toolkit follows them in the windows, the bar's tone
    /// is chosen afresh, and the menus' choice flashes only with motion.
    fn appearance(&mut self, appearance: SystemAppearance) {
        self.dark = appearance.dark;
        self.display = DisplayOptions {
            reduce_motion: appearance.reduce_motion,
            reduce_transparency: appearance.reduce_transparency,
            increase_contrast: appearance.increase_contrast,
            ..self.display
        };
        let display = self.tone_display();
        for bar in &mut self.bars {
            bar.retone(display);
            bar.controller.cfg.flash_choice = !display.reduce_motion;
        }
        // a look the schedule wrote is shown now
        self.appearance_written = None;
    }

    /// The server draws the lock screen: only the menus this component owns have to go.
    fn lock(&mut self, locked: bool) -> Vec<Do<Message>> {
        if !locked {
            return Vec::new();
        }
        let mut commands = Vec::new();
        for i in 0..self.bars.len() {
            let outcome = self.bars[i].controller.close();
            commands.extend(self.apply(i, outcome));
        }
        commands
    }

    /// What another component published on the host bus.
    fn published(&mut self, from: Sender, message: Published) -> Vec<Do<Message>> {
        match message {
            Published::DoNotDisturbChanged { on, lifetimes }
                if from == Sender::Component(ComponentName::NOTIFICATIONS) =>
            {
                self.focus = Some(FocusState {
                    active: on,
                    lifetimes: lifetimes
                        .into_iter()
                        .map(|(id, title)| FocusLifetime { id, title })
                        .collect(),
                });
            }
            // only the component that owns a panel says where it is open
            Published::PanelShown {
                panel,
                shown,
                output,
            } => {
                let place = shown.then_some(output);
                match panel {
                    Panel::ControlPanel
                        if from == Sender::Component(ComponentName::CONTROLCENTER) =>
                    {
                        self.panels.control_panel = place;
                    }
                    Panel::NotificationCenter
                        if from == Sender::Component(ComponentName::NOTIFICATIONS) =>
                    {
                        self.panels.centre = place;
                    }
                    // the bar has no item for the launcher's panels
                    _ => return Vec::new(),
                }
            }
            // a component the server gave up shows nothing: its item hides and its panel is not open
            Published::ComponentStopped { component } if from == Sender::Host => {
                if component == ComponentName::NOTIFICATIONS {
                    self.focus = None;
                    self.panels.centre = None;
                } else if component == ComponentName::CONTROLCENTER {
                    self.panels.control_panel = None;
                } else if component == ComponentName::AGENT {
                    return self.session_menu(None);
                } else {
                    return Vec::new();
                }
            }
            // the session agent's entries for the system menu, from the agent alone
            Published::SessionMenu {
                user_name,
                lock,
                log_out,
                switch_user,
                sleep,
                restart,
                shut_down,
            } if from == Sender::Component(ComponentName::AGENT) => {
                let caps = SessionCaps {
                    sleep,
                    restart,
                    shut_down,
                    log_out,
                    switch_user,
                    lock,
                };
                return self.session_menu(Some(SessionState { user_name, caps }));
            }
            _ => return Vec::new(),
        }
        self.keep_host_state();
        self.refresh()
    }

    /// The session agent's entries as it published them, `None` once the host gave it up: an open system menu then
    /// closes, as its session items would do nothing.
    fn session_menu(&mut self, session: Option<SessionState>) -> Vec<Do<Message>> {
        let gone = self.session.is_some() && session.is_none();
        self.session = session;
        self.keep_host_state();
        let mut commands = Vec::new();
        if gone {
            for i in 0..self.bars.len() {
                if let Some(outcome) = self.bars[i].controller.close_slot(&SlotKey::System) {
                    commands.extend(self.apply(i, outcome));
                }
            }
        }
        commands.extend(self.refresh());
        commands
    }

    /// Put what the host bus told into the extras the bars show.
    fn keep_host_state(&mut self) {
        self.shared.extras.focus.clone_from(&self.focus);
        self.shared.extras.panels = self.panels.clone();
        self.shared.extras.session.clone_from(&self.session);
        // the server locks through the lock screen whether or not the agent runs
        self.shared.extras.lock_without_agent = true;
    }

    /// The bars and their windows for every output the server has described, once `menubar.conf` is read.
    fn follow_outputs(&mut self) -> Vec<Do<Message>> {
        if self.conf.is_none() {
            return Vec::new();
        }
        let outputs: Vec<(OutputId, cupertino_kit::geometry::Rect)> =
            cupertino_kit_shell::outputs::all()
                .into_iter()
                .filter_map(|o| Some((o.id, o.geometry?)))
                .collect();
        let mut commands = Vec::new();
        let gone: Vec<usize> = (0..self.bars.len())
            .filter(|&i| !outputs.iter().any(|(o, _)| *o == self.bars[i].output))
            .rev()
            .collect();
        for i in gone {
            commands.extend(self.destroy_bar(i));
        }
        for (output, geometry) in outputs {
            match self.bars.iter().position(|b| b.output == output) {
                Some(i) => self.bars[i].set_geometry(geometry),
                None => commands.extend(self.create_bar(output, geometry)),
            }
        }
        commands
    }
}

impl Application for MenuBar {
    type Model = Model;
    type Message = Message;
    const RESIDENT: bool = true;
    /// A thin bar repainted when its menus or status items change and once a minute for the clock: a GPU context would
    /// only cost memory.
    const RENDERER: RendererClass = RendererClass::Cpu;

    fn init() -> (Model, Vec<Do<Message>>) {
        let environment = cupertino_kit_shell::environment();
        let design = design::menu_bar();
        let metrics = BarMetrics {
            height: design.height,
            selection_corner_radius: design.selection_radius,
            selection_inset_y: BarMetrics::item_inset_y(design.height),
            ..BarMetrics::default()
        };
        let (notify, icons_wanted) = async_channel::bounded(1);
        let preferences = environment.theme.preferences;
        let mut model = Model {
            services: Services::default(),
            shared: Shared::default(),
            bars: Vec::new(),
            text: environment.text,
            font: BarFont::of(&design),
            metrics,
            height: design.height,
            display: DisplayOptions {
                reduce_motion: preferences.reduce_motion,
                reduce_transparency: preferences.reduce_transparency,
                increase_contrast: preferences.appearance.is_high_contrast(),
                ..DisplayOptions::default()
            },
            dark: preferences.appearance.is_dark(),
            icons: Rc::new(RowIcons::new(notify)),
            icons_wanted,
            loader: Loader::default(),
            data_dirs: cupertino_defaults::data_dirs(),
            start: Instant::now(),
            wifi_scan_until: None,
            join: None,
            clock: MenuBarSettings::UNSET.clock,
            conf: None,
            conf_changes: async_channel::unbounded(),
            appearance_written: None,
            scheduling: false,
            next_clock: f64::NEG_INFINITY,
            extras_minute: None,
            next_bar: 0,
            pending: HashMap::new(),
            next_launch: 0,
            keymap: None,
            wakeup: Wakeup::default(),
            names: HashMap::new(),
            reading_names: HashSet::new(),
            x11: X11 {
                idle: Some((XServer::default(), SeenX11::new())),
                waiting: Vec::new(),
            },
            status_reads: HashMap::new(),
            next_status_read: 0,
            focus: None,
            panels: OpenPanels::default(),
            session: None,
        };
        model.services.recent = tasks::recent_sources().map(Arc::new);
        bus::follow(&[
            Topic::Host,
            Topic::Panels,
            Topic::DoNotDisturb,
            Topic::Session,
        ]);
        let mut commands = bus::session(
            |connection| Message::Session(connection.ok()),
            Message::SessionClosed,
        );
        commands.extend(cupertino_kit::bus::connect(
            cupertino_kit::bus::Bus::System,
            |connection| Message::System(connection.map_err(|e| e.to_string())),
            Message::SystemClosed,
        ));
        commands.push(tasks::first_conf());
        commands.push(tasks::keymap());
        commands.push(tasks::prepare_icons(&model.loader));
        commands.push(listen(&model.icons_wanted, |()| Message::IconsWanted));
        commands.extend(model.lock(session::locked()));
        commands.extend(model.housekeeping());
        (model, commands)
    }

    fn update(model: &mut Model, message: Message) -> Vec<Do<Message>> {
        let mut commands = match message {
            Message::Input(input) => model.input(input),
            Message::MenuBar(key, update) => model.menu_bar_update(&key, *update),
            Message::Outputs => model.follow_outputs(),
            Message::Appearance(appearance) => {
                model.appearance(appearance);
                Vec::new()
            }
            Message::Locked(locked) => model.lock(locked),
            Message::Bus(from, message) => model.published(from, message),
            Message::Session(Some(connection)) => model.session(connection),
            Message::Session(None) | Message::SessionClosed => {
                // without its bus connection the bar shows no menus of other programs: the host starts it again
                return fail("no session bus connection");
            }
            Message::System(system) => model.system(system),
            Message::SystemClosed => {
                eprintln!("{PROCESS}: the system bus connection closed: no Wi-Fi, battery or Bluetooth items");
                Vec::new()
            }
            Message::WorkerStarted(started) => match started.take() {
                Some(started) => model.worker_started(started),
                None => Vec::new(),
            },
            Message::Worker(event) => model.worker_event(event),
            Message::Extras(extras) => model.extras(extras),
            Message::Conf(conf, watch) => model.conf_read(conf, watch.take().flatten()),
            Message::ConfChanged(change) => model.conf_changed(change),
            Message::Scheduled(written) => {
                model.scheduling = false;
                if written.is_some() {
                    model.appearance_written = written;
                }
                Vec::new()
            }
            Message::Keymap(keymap) => {
                model.keymap = keymap.take().or(model.keymap.take());
                Vec::new()
            }
            Message::Recent(recent) => {
                model.shared.recent = recent;
                model.refresh()
            }
            Message::RecentCleared(result) => {
                if let Err(e) = result {
                    eprintln!("{PROCESS}: {e}");
                }
                Vec::new()
            }
            Message::AppName(app_id, name) => model.app_name(app_id, name),
            Message::X11(answer) => match answer.take() {
                Some(answer) => model.x11_answer(answer),
                None => Vec::new(),
            },
            Message::IconsWanted => {
                let mut commands = model.load_row_icons();
                commands.push(listen(&model.icons_wanted, |()| Message::IconsWanted));
                commands
            }
            Message::RowIcon(key, pixels) => {
                model.icons.loaded(key, pixels.as_ref());
                Vec::new()
            }
            Message::StatusIcon(item, read, pixels) => model.status_icon(*item, read, pixels),
            Message::Opened(window, file, launch) => model.opened(&window, &file, launch),
            Message::Timer(timer) => {
                model.wakeup.fired(timer);
                Vec::new()
            }
            Message::Token { launch, token } => {
                model.complete_launch(launch, token);
                Vec::new()
            }
            Message::Done => Vec::new(),
        };
        commands.extend(model.housekeeping());
        commands
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        model
            .bars
            .iter()
            .flat_map(|bar| {
                let open_panels: Vec<SlotKey> = [SlotKey::ControlPanel, SlotKey::Clock]
                    .into_iter()
                    .filter(|slot| {
                        model
                            .shared
                            .extras
                            .panels
                            .open_on(slot, bar.output, model.bars.len())
                    })
                    .collect();
                let prompt = model
                    .join
                    .as_ref()
                    .filter(|j| j.bar == bar.id)
                    .map(|j| PromptScene {
                        ssid: &j.ssid,
                        at: bar.prompt_origin(model.height),
                        show: j.show,
                        text: &j.text,
                        phase: j.phase.clone(),
                    });
                view::windows(
                    &BarScene {
                        bar: bar.id,
                        controller: &bar.controller,
                        shared: &model.shared,
                        font: model.font,
                        tone: bar.tone,
                        slot_tones: &bar.slot_tones,
                        solid: crate::tone::solid_bar(model.tone_display()),
                        size: Size::new(bar.width(), model.height),
                        open_panels: &open_panels,
                        icons: &model.icons,
                        prompt,
                    },
                    Message::Input,
                )
            })
            .collect()
    }
}

impl Program for MenuBar {
    /// A bar along the top of its output, on the top layer, whose space the server reserves: with the keyboard while
    /// a title is focused by the shortcut, its menus getting it with their popup grab. The menus and the join prompt
    /// are Kit's popups of the bar.
    fn surface(model: &Model, key: &WindowKey) -> Surface {
        let Some(bar) = model.bars.iter().find(|bar| bar.id.window() == *key) else {
            return Surface {
                role: Role::Window,
                persistent: None,
                holes: Vec::new(),
            };
        };
        Surface {
            role: Role::MenuBar(LayerSpec {
                namespace: NAMESPACE,
                output: Some(bar.output),
                layer: Layer::Top,
                anchor: Anchor {
                    top: true,
                    left: true,
                    right: true,
                    bottom: false,
                },
                size: Size::new(0.0, model.height.ceil()),
                margin: Margin::default(),
                // the bar's space, which the server keeps while it holds the bar (D19)
                exclusive_zone: model.height.ceil() as i32,
                keyboard: bar.keyboard,
            }),
            persistent: Some(HOLD),
            holes: Vec::new(),
        }
    }

    fn event(event: Event) -> Option<Message> {
        match event {
            Event::MenuBar(window, update) => Some(Message::MenuBar(window, update)),
            Event::Outputs => Some(Message::Outputs),
            Event::Appearance(appearance) => Some(Message::Appearance(appearance)),
            Event::Session(session::Change::Locked(locked)) => Some(Message::Locked(locked)),
            Event::Bus { from, message } => Some(Message::Bus(from, message)),
            // a closed bar's `on_close` makes it anew
            _ => None,
        }
    }
}

impl Component for MenuBar {
    const NAME: ComponentName = ComponentName::MENUBAR;
}
