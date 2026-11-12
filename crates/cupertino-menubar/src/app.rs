//! Glue between the server, the menu controllers and the session bus on CupertinoKit: one bar per output, drawn by
//! [`view`], whose window gets its layer surface, its menu-bar object and its backdrop from [`BarRoles`]. The shell's
//! events, the session bus, `menubar.conf` and the timers arrive as messages. The decisions live in the library
//! modules (`feed`, `routing`, `controller`); this file only connects them to the toolkit.

use cupertino_apps::launch::{self, Launch, OPEN_COMMAND};
use cupertino_apps::{Locale, MimeApps, MimeDatabase, Visibility, XdgDirs};
use cupertino_assets::Assets;
use cupertino_defaults::accessibility::DisplayOptions;
use cupertino_defaults::appearance::AppearanceSettings;
use cupertino_defaults::menubar::MenuBarSettings;
use cupertino_defaults::watch::{ConfigWatcher, WatchEvent};
use cupertino_icons::{SystemIcons, ThemeIcons};
use cupertino_kit::core::{Application, Command, RuntimeOptions, Window};
use cupertino_kit::geometry::{Rect as GRect, Size};
use cupertino_kit::platform::{KeyEvent, Keysym, Mods, WindowSpec};
use cupertino_kit::text::{KeyInput, Keymap, TextSystem};
use cupertino_menubar::clock::{clock_text, local_time, next_tick, separators_off, ClockSettings};
use cupertino_menubar::controller::{MenuController, Mode, Outcome};
use cupertino_menubar::desktop::app_name;
use cupertino_menubar::extras::{self, ExtraAction, Extras, Passphrase, SessionAction};
use cupertino_menubar::extras_bus::{ExtrasBus, SessionAgent};
use cupertino_menubar::feed::{status_icon, status_icon_px, BarFeed, Shared, StatusState};
use cupertino_menubar::geom::Rect;
use cupertino_menubar::layout::{BarMetrics, SlotKey};
use cupertino_menubar::recent::RecentSources;
use cupertino_menubar::routing::{self, menu_key, Action, Context};
use cupertino_menubar::tone::BarTone;
use cupertino_menubar::view::{
    self, BarFont, BarId, BarPointer, BarScene, Input, KitMeasure, PromptEvent, PromptPhase,
    PromptScene, RowIcons,
};
use cupertino_menubar::wakeup::Wakeup;
use cupertino_menubar::worker::{self, Worker};
use cupertino_modifiers::{Modifier, Physical, Profile};
use cupertino_protocol_app::{AppEffects, Backdrop};
use cupertino_shell_client::{
    Anchor, KeyboardInteractivity, Layer, LayerShell, LayerSurface, LayerSurfaceConfig, Margin,
    MenuBar, MenuBarUpdate, Session as SessionObserver, SessionChange, SessionState, Shell,
    ShellEvent,
};
use cupertino_theme::Theme;
use cupertino_ui_wayland::{RoleEvent, SurfaceRoles, WaylandWindowSystem};
use cupertino_wayland_client::{
    Connection, Error as WaylandError, Event, ExtensionEvent, OutputGeometry, OutputId, SurfaceId,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// How long the Wi-Fi panel says it is looking for networks after a scan it asked for, if none is listed by then.
const WIFI_SCAN_TIME: Duration = Duration::from_secs(8);
/// Role of the surfaces for the server (`wlr-layer-shell` namespace).
const NAMESPACE: &str = "cupertino-menubar";

/// The name the process reports problems under.
const PROCESS: &str = "cupertino-menubar";

/// The scale status icons are loaded at: sharp on outputs up to it, scaled down on the others.
const ICON_SCALE: f64 = 2.0;

/// The keysyms of the Alt keys, which show the alternates of the open menus' items while held.
const ALT_L: Keysym = Keysym(0xffe9);
const ALT_R: Keysym = Keysym(0xffea);

/// How long a join may take before the prompt says it failed.
const JOIN_TIME: Duration = Duration::from_secs(30);

/// The join prompt of one bar: which network, what is typed and how far the attempt stands.
struct JoinPrompt {
    bar: BarId,
    ssid: String,
    show: bool,
    text: Passphrase,
    phase: PromptPhase,
    /// What the service had answered and what the device had failed with when the join was asked, so an earlier
    /// attempt's answer is not taken for this one's.
    joins_at_send: u32,
    failure_at_send: String,
    /// The service took the request: the device's state tells the rest.
    answered: bool,
    saw_busy: bool,
    deadline: Option<Instant>,
}

/// The line that says why a join failed, from the device's failure.
fn failure_line(failure: &str) -> String {
    match failure {
        "wrong-passphrase" => "The password is not correct.",
        "network-not-found" => "The network is not in range.",
        "no-address" => "The network gave no address.",
        "connection-lost" => "The connection was lost.",
        _ => "The network could not be joined.",
    }
    .to_owned()
}

/// The shell objects of one bar, made by the roles when its window opens and dropped with the bar.
struct BarSurface {
    bar: BarId,
    output: OutputId,
    layer: Option<LayerSurface>,
    menu_bar: Option<MenuBar>,
    /// The solid strip under Reduce Transparency ([`cupertino_menubar::tone::solid_bar`]); hidden otherwise.
    backdrop: Option<Backdrop>,
    /// The bar's size as last configured, which the backdrop covers while it shows.
    size: Option<(u32, u32)>,
}

impl BarSurface {
    /// Shows the backdrop over the whole bar while `display` makes the bar solid, and hides it (a shape with no area)
    /// otherwise, so the bar draws nothing of its own over the wallpaper.
    fn shape_backdrop(&self, display: DisplayOptions, background: bool) {
        let (Some(backdrop), Some((w, h))) = (&self.backdrop, self.size) else {
            return;
        };
        let rect = if cupertino_menubar::tone::solid_bar(display) || background {
            GRect::new(0.0, 0.0, f64::from(w), f64::from(h))
        } else {
            GRect::new(0.0, 0.0, 0.0, 0.0)
        };
        if let Err(e) = backdrop.set_shape(&rect, 0.0) {
            eprintln!("{PROCESS}: cannot shape the bar's backdrop: {e}");
        }
    }
}

/// The shell objects of every bar, shared by the roles that make them and the model that uses them.
#[derive(Default)]
struct Surfaces {
    bars: Vec<BarSurface>,
    /// `menubar.conf` asks for a backdrop of the bar's own over the wallpaper.
    background: bool,
    /// Every output the connection lists, as of the last batch the window system read: the model hears of a change
    /// only after the events of that batch, a bar's close among them.
    outputs: Vec<OutputId>,
    /// The connector name of each output that has one (`wl_output.name`), as of the same batch.
    names: HashMap<OutputId, String>,
}

impl Surfaces {
    fn get(&self, bar: BarId) -> Option<&BarSurface> {
        self.bars.iter().find(|s| s.bar == bar)
    }

    fn of_surface(&self, surface: SurfaceId) -> Option<&BarSurface> {
        self.bars
            .iter()
            .find(|s| s.layer.as_ref().is_some_and(|l| l.surface() == surface))
    }
}

/// What the shell told the roles about a bar, for the model.
#[derive(Clone, Debug)]
pub enum ShellMessage {
    /// The bar's menu-bar object exists, with what its version offers.
    Created {
        bar: BarId,
        edits: bool,
        force_quits: bool,
    },
    Configured {
        bar: BarId,
        width: u32,
    },
    MenuBar {
        bar: BarId,
        update: Box<MenuBarUpdate>,
    },
    Keyboard {
        bar: BarId,
        focused: bool,
    },
    Session(SessionChange),
    Outputs(Vec<(OutputId, OutputGeometry)>),
}

/// A source whose receiver has something to drain.
#[derive(Clone, Copy, Debug)]
pub enum Wake {
    Worker,
    Extras,
    Conf,
}

/// Gives each bar's window its layer surface along the top of its output, the output's menu-bar object and the bar's
/// material. Hears the shell's events for the model.
struct BarRoles {
    layer_shell: LayerShell,
    effects: AppEffects,
    shell: Shell,
    _observer: SessionObserver,
    observed: SessionState,
    surfaces: Rc<RefCell<Surfaces>>,
    /// The bar's height, in whole points.
    height: u32,
    events: async_channel::Sender<ShellMessage>,
}

impl BarRoles {
    fn send(&self, message: ShellMessage) {
        // the receiver is gone only while the process exits
        let _ = self.events.send_blocking(message);
    }

    fn create_bar(
        &mut self,
        conn: &mut Connection,
        bar: BarId,
    ) -> std::result::Result<SurfaceId, WaylandError> {
        let gone = || WaylandError::Protocol("the menu bar's output is gone");
        let output = self.surfaces.borrow().get(bar).ok_or_else(gone)?.output;
        let layer = self
            .layer_shell
            .layer_surface(
                conn,
                &LayerSurfaceConfig {
                    namespace: NAMESPACE.to_string(),
                    output: Some(output),
                    layer: Layer::Top,
                    anchor: Anchor::Top | Anchor::Left | Anchor::Right,
                    size: (0, self.height),
                    margin: Margin::default(),
                    // the server reserves the bar's space
                    exclusive_zone: 0,
                    keyboard: KeyboardInteractivity::None,
                },
            )
            .map_err(|e| match e {
                cupertino_shell_client::Error::Transport(e) => e,
                _ => gone(),
            })?;
        let surface = layer.surface();
        let menu_bar = self
            .shell
            .menu_bar(conn, surface, output)
            .map_err(|_| gone())?;
        let backdrop = self.effects.backdrop(conn, surface)?;
        // the solid strip of Reduce Transparency, the menu bar material's opaque recipe; hidden while the bar is
        // see-through ([`BarSurface::shape_backdrop`])
        backdrop.set_material("MenuBar");
        self.send(ShellMessage::Created {
            bar,
            edits: menu_bar.can_edit(),
            force_quits: menu_bar.can_force_quit_app(),
        });
        let mut surfaces = self.surfaces.borrow_mut();
        let entry = surfaces
            .bars
            .iter_mut()
            .find(|s| s.bar == bar)
            .ok_or_else(gone)?;
        entry.layer = Some(layer);
        entry.menu_bar = Some(menu_bar);
        entry.backdrop = Some(backdrop);
        Ok(surface)
    }
}

impl SurfaceRoles for BarRoles {
    fn create(
        &mut self,
        conn: &mut Connection,
        spec: &WindowSpec,
    ) -> Option<std::result::Result<SurfaceId, WaylandError>> {
        let bar = BarId::of(&spec.app_id)?;
        Some(self.create_bar(conn, bar))
    }

    fn event(&mut self, event: &ExtensionEvent) -> Option<RoleEvent> {
        if let Some(change) = self.observed.apply(event) {
            if let SessionChange::DisplayOptions(display) = change {
                let surfaces = self.surfaces.borrow();
                for bar in &surfaces.bars {
                    bar.shape_backdrop(display, surfaces.background);
                }
            }
            self.send(ShellMessage::Session(change));
            // the window system follows the appearance and the lock too
            return None;
        }
        match event.downcast_ref::<ShellEvent>()? {
            &ShellEvent::Configure { surface, size } => {
                let mut surfaces = self.surfaces.borrow_mut();
                let background = surfaces.background;
                let bar = surfaces
                    .bars
                    .iter_mut()
                    .find(|s| s.layer.as_ref().is_some_and(|l| l.surface() == surface))?;
                let layer = bar.layer.as_ref()?;
                // a configure without a size closes the window
                let Ok(size) = layer.configured(size) else {
                    return Some(RoleEvent::Closed(surface));
                };
                bar.size = Some(size);
                bar.shape_backdrop(self.observed.display, background);
                let message = ShellMessage::Configured {
                    bar: bar.bar,
                    width: size.0,
                };
                drop(surfaces);
                self.send(message);
                Some(RoleEvent::Configure { surface, size })
            }
            &ShellEvent::Closed(surface) => {
                self.surfaces.borrow().of_surface(surface)?;
                Some(RoleEvent::Closed(surface))
            }
            ShellEvent::MenuBar { surface, update } => {
                let bar = self.surfaces.borrow().of_surface(*surface)?.bar;
                self.send(ShellMessage::MenuBar {
                    bar,
                    update: update.clone(),
                });
                None
            }
            // the bar has no dock, and the session's requests are for the session agent
            _ => None,
        }
    }

    fn keyboard_focus(&mut self, surface: SurfaceId, focused: bool) {
        let bar = self.surfaces.borrow().of_surface(surface).map(|s| s.bar);
        if let Some(bar) = bar {
            self.send(ShellMessage::Keyboard { bar, focused });
        }
    }

    fn outputs_changed(&mut self, conn: &Connection) {
        let outputs = conn.outputs();
        let mut surfaces = self.surfaces.borrow_mut();
        surfaces.names = outputs
            .iter()
            .filter_map(|o| Some((o.id, o.name.clone()?)))
            .collect();
        surfaces.outputs = outputs.into_iter().map(|o| o.id).collect();
        drop(surfaces);
        self.send(ShellMessage::Outputs(described_outputs(conn)));
    }
}

/// The outputs the server has described; one it has not described yet gets its bar when it has.
fn described_outputs(conn: &Connection) -> Vec<(OutputId, OutputGeometry)> {
    conn.outputs()
        .into_iter()
        .filter_map(|o| Some((o.id, o.geometry?)))
        .collect()
}

/// The bar of one output.
struct Bar {
    id: BarId,
    output: OutputId,
    geometry: OutputGeometry,
    /// Width of the surface from its last configure.
    width: u32,
    controller: MenuController,
    feed: BarFeed,
    keyboard: KeyboardInteractivity,
    /// The bar's own surface has the keyboard.
    focused: bool,
    /// Alt is held on a surface of the bar.
    alt: bool,
    /// The lightness of the wallpaper under the bar, from the server; `None` before it is known.
    backdrop_lightness: Option<u16>,
    /// The palette of the bar's strip.
    tone: BarTone,
}

impl Bar {
    /// A surface of this bar has the keyboard: the bar, or one of its menus, which take it with their grab.
    fn has_keyboard(&self) -> bool {
        self.focused || self.controller.is_open()
    }
}

/// The session bus, the system services and the configuration behind the bars.
struct Services {
    worker: Worker,
    events: Receiver<worker::Event>,
    extras: ExtrasBus,
    extras_events: Receiver<Extras>,
    /// Reports of the watch on `menubar.conf`; the watch lives as long as the process (`None` without one).
    conf_changes: Receiver<WatchEvent>,
    _conf_watch: Option<ConfigWatcher>,
    /// Where Recent Items is read from; `None` when the session's directories are not known.
    recent: Option<RecentSources>,
}

pub struct App {
    surfaces: Rc<RefCell<Surfaces>>,
    shell: async_channel::Receiver<ShellMessage>,
    wakes: async_channel::Receiver<Wake>,
    services: Services,
    shared: Shared,
    bars: Vec<Bar>,
    text: Arc<TextSystem>,
    font: BarFont,
    metrics: BarMetrics,
    height: f64,
    display: DisplayOptions,
    icons: Rc<RowIcons>,
    data_dirs: Vec<PathBuf>,
    start: Instant,
    /// When the Wi-Fi panel's "looking for networks" state ends, while a scan it asked for is under way.
    wifi_scan_until: Option<Instant>,
    /// The join prompt, while the person is asked for a passphrase.
    join: Option<JoinPrompt>,
    clock: ClockSettings,
    /// `menubar.conf` as last read, the last good values of the next read.
    conf: MenuBarSettings,
    /// Whether the session's appearance is dark.
    dark: bool,
    /// The appearance the schedule asked for last and the session has not shown yet.
    appearance_written: Option<bool>,
    /// Unix seconds at which the clock text is due again.
    next_clock: f64,
    /// The minute of the last reading of the extras' services.
    extras_minute: Option<i64>,
    /// Numbers the next bar.
    next_bar: u64,
    /// Launches waiting for their activation token, by number.
    pending: HashMap<u64, Launch>,
    next_launch: u64,
    /// The editing chords of the Help menu's search field.
    keymap: Keymap,
    wakeup: Wakeup,
}

#[derive(Clone, Debug)]
pub enum Message {
    Shell(ShellMessage),
    Input(Input),
    Wake(Wake),
    Timer(u64),
    Token {
        launch: u64,
        token: Option<String>,
    },
    /// The roles or the bus threads are gone.
    Unheard,
}

/// `menubar.conf` from the configuration directory (`CUPERTINO_MENUBAR_CONFIG` overrides the path); every key unset
/// when there is none. Lines that are not understood are reported; with `last_good` their keys keep its values.
fn load_conf(last_good: Option<&MenuBarSettings>) -> Result<MenuBarSettings> {
    let Some(path) = MenuBarSettings::path() else {
        return Ok(MenuBarSettings::UNSET);
    };
    let (settings, problems) = MenuBarSettings::load(&path, last_good)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    for problem in problems {
        eprintln!(
            "{PROCESS}: {} line {}: {}",
            path.display(),
            problem.line,
            problem.message
        );
    }
    Ok(settings)
}

/// Watch `menubar.conf`; its changes wake the application. A watch that cannot be set up is reported: the file then
/// applies at the next start.
fn watch_conf(wakes: async_channel::Sender<Wake>) -> (Option<ConfigWatcher>, Receiver<WatchEvent>) {
    let (tx, rx) = std::sync::mpsc::channel();
    let Some(path) = MenuBarSettings::path() else {
        return (None, rx);
    };
    // the schedule of `appearance.conf` is applied by this process too
    let files: Vec<PathBuf> = std::iter::once(path.clone())
        .chain(AppearanceSettings::path())
        .collect();
    let watch = ConfigWatcher::new(files, move |event| {
        // the receivers are gone only while the process exits
        if tx.send(event).is_ok() {
            let _ = wakes.send_blocking(Wake::Conf);
        }
    });
    match watch {
        Ok(w) => (Some(w), rx),
        Err(e) => {
            eprintln!(
                "{PROCESS}: cannot watch {}: {e}; changes apply at the next start",
                path.display()
            );
            (None, rx)
        }
    }
}

/// Store the session's look in `compositor.json`, which the server follows while it runs: dark or light, keeping a
/// high-contrast variant the file already holds.
fn write_appearance(dark: bool) -> std::io::Result<()> {
    use cupertino_defaults::compositor::{AppearanceName, CompositorSettings};
    let Some(path) = CompositorSettings::path() else {
        return Err(std::io::Error::other("there is no settings folder"));
    };
    let (mut file, _) = CompositorSettings::load(&path, None)?;
    let contrast = matches!(
        file.appearance,
        Some(AppearanceName::LightHighContrast | AppearanceName::DarkHighContrast)
    );
    file.appearance = Some(match (dark, contrast) {
        (false, false) => AppearanceName::Light,
        (true, false) => AppearanceName::Dark,
        (false, true) => AppearanceName::LightHighContrast,
        (true, true) => AppearanceName::DarkHighContrast,
    });
    file.store(&path)?;
    Ok(())
}

/// Connect, claim the bus names and serve until the connection or the bus fails.
pub fn run() -> Result<std::convert::Infallible> {
    let assets = Assets::discover();
    let theme = Theme::load()?;
    let conf = load_conf(None)?;
    let metrics = BarMetrics {
        height: theme.shapes.menu_bar_height,
        selection_corner_radius: theme.shapes.menu_bar_selection_corner_radius,
        ..BarMetrics::default()
    };
    let font = BarFont::new(theme.tokens.typography());
    // the extras' symbols are drawn by the toolkit from the same packs; a missing one is reported now
    let _ = SystemIcons::required(&assets, &[], &extras::SYMBOL_NAMES);
    let keymap = Keymap::new(
        cupertino_keyboard::binding::user_profile(),
        cupertino_keyboard::binding::load_default().0,
    );

    let mut system = WaylandWindowSystem::connect(keymap.profile().clone())
        .map_err(|e| format!("cannot connect to the Wayland display: {e:?}"))?;
    let conn = system.connection();
    let layer_shell = LayerShell::bind(conn).ok_or("the server lacks zwlr_layer_shell_v1")?;
    let effects = AppEffects::bind(conn).ok_or("the server lacks cupertino_app_effects_v1")?;
    let shell = Shell::bind(conn).ok_or("the server lacks cupertino_shell_v1")?;
    let (observer, mut observed, early) = shell.observe(conn)?;
    for event in early {
        if let Event::Extension(event) = event {
            observed.apply(&event);
        }
    }
    let outputs = described_outputs(conn);

    let (wake, wakes) = async_channel::unbounded();
    let (conf_watch, conf_changes) = watch_conf(wake.clone());
    let session = zbus::block_on(zbus::Connection::session())
        .map_err(|e| format!("cannot connect to the session bus: {e}"))?;
    let waker = wake.clone();
    let (worker, events) = Worker::start(session.clone(), move || {
        let _ = waker.send_blocking(Wake::Worker);
    })?;
    let waker = wake.clone();
    let (extras, extras_events) = ExtrasBus::start(session, move || {
        let _ = waker.send_blocking(Wake::Extras);
    });
    let recent = match XdgDirs::from_env() {
        Ok(dirs) => Some(RecentSources::new(
            &dirs,
            Locale::from_env(),
            Visibility::from_env(),
        )),
        Err(e) => {
            eprintln!("{PROCESS}: no Recent Items: {e}");
            None
        }
    };

    let surfaces = Rc::new(RefCell::new(Surfaces {
        bars: Vec::new(),
        background: conf.background,
        outputs: conn.outputs().into_iter().map(|o| o.id).collect(),
        names: conn
            .outputs()
            .into_iter()
            .filter_map(|o| Some((o.id, o.name?)))
            .collect(),
    }));
    let (shell_events, shell_messages) = async_channel::unbounded();
    let system = system.with_roles(BarRoles {
        layer_shell,
        effects,
        shell,
        _observer: observer,
        observed,
        surfaces: Rc::clone(&surfaces),
        height: metrics.height.ceil() as u32,
        events: shell_events,
    });
    let start = move |options: &RuntimeOptions| {
        let mut app = App {
            surfaces,
            shell: shell_messages,
            wakes,
            services: Services {
                worker,
                events,
                extras,
                extras_events,
                conf_changes,
                _conf_watch: conf_watch,
                recent,
            },
            shared: {
                let mut shared = Shared::default();
                shared.conf = conf.clone();
                shared
            },
            dark: observed.dark,
            appearance_written: None,
            bars: Vec::new(),
            text: Arc::clone(&options.text),
            font,
            height: metrics.height,
            metrics,
            display: observed.display,
            icons: Rc::new(RowIcons::new(
                ThemeIcons::new(&assets),
                SystemIcons::new(&assets),
            )),
            data_dirs: cupertino_defaults::data_dirs(),
            start: Instant::now(),
            wifi_scan_until: None,
            join: None,
            clock: conf.clock,
            conf,
            next_clock: f64::NEG_INFINITY,
            extras_minute: None,
            next_bar: 0,
            pending: HashMap::new(),
            next_launch: 0,
            keymap,
            wakeup: Wakeup::default(),
        };
        let mut commands = vec![listen_shell(&app.shell), listen_wakes(&app.wakes)];
        commands.extend(app.outputs(outputs));
        commands.extend(app.housekeeping());
        (app, commands)
    };
    cupertino_kit::app::run_on_with::<App>(system, start)?;
    Err("the menu bar's connection or bus went away".into())
}

fn listen_shell(shell: &async_channel::Receiver<ShellMessage>) -> Command<Message> {
    let shell = shell.clone();
    Command::Perform(Box::pin(async move {
        shell.recv().await.map_or(Message::Unheard, Message::Shell)
    }))
}

fn listen_wakes(wakes: &async_channel::Receiver<Wake>) -> Command<Message> {
    let wakes = wakes.clone();
    Command::Perform(Box::pin(async move {
        wakes.recv().await.map_or(Message::Unheard, Message::Wake)
    }))
}

/// The modifiers of `key` on the keys that carry them, as the search field's chords are written.
fn key_input(key: &KeyEvent, profile: &Profile) -> KeyInput {
    let held: Vec<Physical> = [
        (Mods::SHIFT, Modifier::Shift),
        (Mods::CTRL, Modifier::Ctrl),
        (Mods::ALT, Modifier::Alt),
        (Mods::PRIMARY, Modifier::Primary),
    ]
    .into_iter()
    .filter(|(mods, _)| key.mods.contains(*mods))
    .map(|(_, role)| profile.physical_for(role))
    .collect();
    let has = |p: Physical| held.contains(&p);
    KeyInput::new(
        key.code,
        has(Physical::Shift),
        has(Physical::Ctrl),
        has(Physical::Alt),
        has(Physical::Super),
        key.text,
    )
}

/// Ends the application after a failure; the server restarts the process.
fn fail(e: impl std::fmt::Display) -> Vec<Command<Message>> {
    eprintln!("{PROCESS}: {e}");
    vec![Command::Exit]
}

impl App {
    /// Seconds on the monotonic clock the controllers are driven with.
    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    fn index(&self, bar: BarId) -> Option<usize> {
        self.bars.iter().position(|b| b.id == bar)
    }

    fn menu_bar<R>(&self, bar: BarId, f: impl FnOnce(&MenuBar) -> R) -> Option<R> {
        self.surfaces
            .borrow()
            .get(bar)
            .and_then(|s| s.menu_bar.as_ref())
            .map(f)
    }

    // ---- outputs -------------------------------------------------------------------------

    /// Create the bars of new outputs, drop those of outputs that went away and follow the geometry of the others.
    fn outputs(&mut self, outputs: Vec<(OutputId, OutputGeometry)>) -> Vec<Command<Message>> {
        let mut commands = Vec::new();
        let gone: Vec<BarId> = self
            .bars
            .iter()
            .filter(|b| !outputs.iter().any(|(o, _)| *o == b.output))
            .map(|b| b.id)
            .collect();
        for bar in gone {
            commands.extend(self.destroy_bar(bar));
        }
        for (output, geometry) in outputs {
            match self.bars.iter().position(|b| b.output == output) {
                Some(i) if self.bars[i].geometry != geometry => {
                    self.bars[i].geometry = geometry;
                    self.set_screen(i);
                }
                Some(_) => {}
                None => commands.extend(self.create_bar(output, geometry)),
            }
        }
        commands
    }

    fn create_bar(&mut self, output: OutputId, geometry: OutputGeometry) -> Vec<Command<Message>> {
        let id = BarId(self.next_bar);
        self.next_bar += 1;
        let mut controller = MenuController::new(
            Rect::new(
                0.0,
                0.0,
                f64::from(geometry.width),
                f64::from(geometry.height),
            ),
            Box::new(KitMeasure {
                text: Arc::clone(&self.text),
                font: self.font,
            }),
        );
        controller.bar_metrics = self.metrics.clone();
        controller.menu_metrics.shortcut_style = self.shared.chords.style();
        controller.cfg.flash_choice = !self.display.reduce_motion;
        self.surfaces.borrow_mut().bars.push(BarSurface {
            bar: id,
            output,
            layer: None,
            menu_bar: None,
            backdrop: None,
            size: None,
        });
        self.bars.push(Bar {
            id,
            output,
            geometry,
            width: geometry.width,
            controller,
            // the menu-bar object tells what it offers when the window opens
            feed: BarFeed::new(id.0, false, false),
            keyboard: KeyboardInteractivity::None,
            focused: false,
            alt: false,
            backdrop_lightness: None,
            tone: BarTone::Appearance,
        });
        if self.bars.len() == 1 {
            if let Err(e) = self
                .services
                .worker
                .send(worker::Request::AppMenuRegistrar(true))
            {
                return fail(e);
            }
        }
        Vec::new()
    }

    /// Drop a bar; its window closes with it, after its shell objects.
    fn destroy_bar(&mut self, bar: BarId) -> Vec<Command<Message>> {
        self.bars.retain(|b| b.id != bar);
        self.surfaces.borrow_mut().bars.retain(|s| s.bar != bar);
        if self.bars.is_empty() {
            if let Err(e) = self
                .services
                .worker
                .send(worker::Request::AppMenuRegistrar(false))
            {
                return fail(e);
            }
        }
        Vec::new()
    }

    /// The screen the controller lays menus out on: the bar's width and the output's height.
    fn set_screen(&mut self, i: usize) {
        let bar = &mut self.bars[i];
        bar.controller.set_output(Rect::new(
            0.0,
            0.0,
            f64::from(bar.width),
            f64::from(bar.geometry.height),
        ));
    }

    // ---- shell ---------------------------------------------------------------------------

    fn shell_message(&mut self, message: ShellMessage) -> Vec<Command<Message>> {
        match message {
            ShellMessage::Created {
                bar,
                edits,
                force_quits,
            } => {
                if let Some(i) = self.index(bar) {
                    self.bars[i].feed = BarFeed::new(bar.0, edits, force_quits);
                }
                Vec::new()
            }
            ShellMessage::Configured { bar, width } => {
                let Some(i) = self.index(bar) else {
                    return Vec::new();
                };
                self.bars[i].width = width;
                self.set_screen(i);
                Vec::new()
            }
            ShellMessage::MenuBar { bar, update } => match self.index(bar) {
                Some(i) => self.menu_bar_update(i, &update),
                None => Vec::new(),
            },
            ShellMessage::Keyboard { bar, focused } => {
                let Some(i) = self.index(bar) else {
                    return Vec::new();
                };
                self.bars[i].focused = focused;
                if focused {
                    let outcome = self.bars[i].controller.keyboard_arrived();
                    self.apply(i, outcome)
                } else {
                    self.bars[i].alt = false;
                    // the bar held the keyboard for the shortcut and lost it to something else
                    if self.bars[i].controller.mode() == Mode::TitleFocus {
                        let outcome = self.bars[i].controller.close();
                        self.apply(i, outcome)
                    } else {
                        self.sync_alt(i)
                    }
                }
            }
            ShellMessage::Session(change) => self.session(change),
            ShellMessage::Outputs(outputs) => self.outputs(outputs),
        }
    }

    /// The display options the bar's text tone follows: with a backdrop of its own the bar is solid as far as its text
    /// goes, whatever the wallpaper under it.
    fn tone_display(&self) -> DisplayOptions {
        DisplayOptions {
            reduce_transparency: self.display.reduce_transparency || self.conf.background,
            ..self.display
        }
    }

    /// Switch the look when the schedule of `appearance.conf` says it is time: the session follows `compositor.json`.
    fn follow_schedule(&mut self, unix: f64) {
        let schedule = AppearanceSettings::current();
        if !schedule.auto {
            self.appearance_written = None;
            return;
        }
        let now = local_time(unix.floor() as i64);
        let dark = schedule.dark_at(u32::from(now.hour) * 60 + u32::from(now.minute));
        if dark == self.dark {
            self.appearance_written = None;
            return;
        }
        if self.appearance_written == Some(dark) {
            return;
        }
        if let Err(e) = write_appearance(dark) {
            eprintln!("{PROCESS}: cannot switch the appearance: {e}");
            return;
        }
        self.appearance_written = Some(dark);
    }

    fn session(&mut self, change: SessionChange) -> Vec<Command<Message>> {
        match change {
            // the toolkit follows the appearance; the bar's tone is chosen afresh in it
            SessionChange::Appearance { dark } => {
                self.dark = dark;
                let display = self.tone_display();
                for bar in &mut self.bars {
                    bar.tone = BarTone::fresh(bar.backdrop_lightness, display);
                }
                Vec::new()
            }
            // the roles show the bar's solid strip or hide it, and the menus' glass follows the options in the server;
            // a solid bar keeps the appearance's text
            SessionChange::DisplayOptions(display) => {
                self.display = display;
                let display = self.tone_display();
                for bar in &mut self.bars {
                    bar.tone = BarTone::fresh(bar.backdrop_lightness, display);
                    bar.controller.cfg.flash_choice = !display.reduce_motion;
                }
                Vec::new()
            }
            // the server draws the lock screen: only the menus this process owns have to go
            SessionChange::Lock { locked: true } => {
                let mut commands = Vec::new();
                for i in 0..self.bars.len() {
                    let outcome = self.bars[i].controller.close();
                    commands.extend(self.apply(i, outcome));
                }
                commands
            }
            SessionChange::Lock { locked: false } => Vec::new(),
        }
    }

    fn menu_bar_update(&mut self, i: usize, update: &MenuBarUpdate) -> Vec<Command<Message>> {
        let data_dirs = &self.data_dirs;
        self.bars[i]
            .feed
            .apply(update, &mut self.shared, |id| app_name(id, data_dirs));
        let tone_display = self.tone_display();
        if let Some(lightness) = update.backdrop_lightness {
            let bar = &mut self.bars[i];
            bar.backdrop_lightness = Some(lightness);
            bar.tone = bar.tone.next(Some(lightness), tone_display);
        }
        if let Some(agent) = update.session_agent {
            let agent = match agent {
                cupertino_shell_client::SessionAgent::None => SessionAgent::None,
                cupertino_shell_client::SessionAgent::Process(pid) => SessionAgent::Process(pid),
                cupertino_shell_client::SessionAgent::Any => SessionAgent::Any,
            };
            if let Err(e) = self.services.extras.session_agent(agent) {
                return fail(e);
            }
        }
        if update.shortcuts.is_some() {
            let style = self.shared.chords.style();
            for bar in &mut self.bars {
                bar.controller.menu_metrics.shortcut_style = style;
            }
        }
        let mut commands = Vec::new();
        if update.engage {
            let outcome = self.bars[i].controller.focus_bar();
            commands.extend(self.apply(i, outcome));
        }
        if update.engage_help {
            let bar = &mut self.bars[i];
            let outcome = bar.controller.focus_help_search(bar.has_keyboard());
            commands.extend(self.apply(i, outcome));
        }
        commands.extend(self.refresh());
        commands
    }

    // ---- input ---------------------------------------------------------------------------

    fn input(&mut self, input: Input) -> Vec<Command<Message>> {
        let now = self.now();
        match input {
            Input::Pointer { bar, pointer } => {
                let Some(i) = self.index(bar) else {
                    return Vec::new();
                };
                let controller = &mut self.bars[i].controller;
                let outcomes = match pointer {
                    BarPointer::Move(p) => vec![controller.pointer_move(p, now)],
                    BarPointer::Press(p) => vec![controller.pointer_press(p, now)],
                    BarPointer::Release(p) => vec![controller.pointer_release(p, now)],
                    BarPointer::Click(p) => vec![
                        controller.pointer_move(p, now),
                        controller.pointer_press(p, now),
                        controller.pointer_release(p, now),
                    ],
                };
                outcomes
                    .into_iter()
                    .flat_map(|outcome| self.apply(i, outcome))
                    .collect()
            }
            Input::Key { bar, key } => {
                let Some(i) = self.index(bar) else {
                    return Vec::new();
                };
                self.bars[i].alt = match key.keysym {
                    ALT_L | ALT_R => key.pressed,
                    _ => key.mods.contains(Mods::ALT),
                };
                let mut commands = self.sync_alt(i);
                if !key.pressed {
                    return commands;
                }
                let input = key_input(&key, self.keymap.profile());
                let command = key.mods.contains(Mods::CTRL)
                    || key.mods.contains(Mods::ALT)
                    || key.mods.contains(Mods::PRIMARY);
                let code = menu_key(key.code).or_else(|| routing::typed_key(key.text, command));
                let controller = &mut self.bars[i].controller;
                let outcome =
                    match controller.search_key(code, &mut self.keymap, input, Instant::now()) {
                        Some(outcome) => Some(outcome),
                        None => code.map(|code| controller.key(code, now)),
                    };
                if let Some(outcome) = outcome {
                    commands.extend(self.apply(i, outcome));
                }
                commands
            }
            Input::Dismissed(bar) => match self.index(bar) {
                // the server dismissed the grab: the menus are over
                Some(i) => {
                    let outcome = self.bars[i].controller.close();
                    self.apply(i, outcome)
                }
                None => Vec::new(),
            },
            Input::Prompt { bar, event } => self.prompt_event(bar, event),
            Input::Closed(bar) => {
                let Some(i) = self.index(bar) else {
                    return Vec::new();
                };
                let output = self.bars[i].output;
                let mut commands = self.destroy_bar(bar);
                if self.surfaces.borrow().outputs.contains(&output) {
                    commands.extend(fail(
                        "the server closed the menu bar of an output that is still present",
                    ));
                }
                commands
            }
        }
    }

    /// Open the join prompt for `ssid` on the bar `i`.
    fn open_prompt(&mut self, i: usize, ssid: String) {
        let link = &self.shared.extras.wifi_link;
        self.join = Some(JoinPrompt {
            bar: self.bars[i].id,
            ssid,
            show: false,
            text: Passphrase::new(""),
            phase: PromptPhase::Editing,
            joins_at_send: link.joins,
            failure_at_send: link.failure.clone(),
            answered: false,
            saw_busy: false,
            deadline: None,
        });
    }

    /// Something happened in the join prompt.
    fn prompt_event(&mut self, bar: BarId, event: PromptEvent) -> Vec<Command<Message>> {
        let Some(join) = self.join.as_mut().filter(|j| j.bar == bar) else {
            return Vec::new();
        };
        match event {
            PromptEvent::Typed(text) => {
                join.text = text;
                if matches!(join.phase, PromptPhase::Failed(_)) {
                    join.phase = PromptPhase::Editing;
                }
            }
            PromptEvent::Show(on) => {
                join.show = on;
                // the hidden field starts empty: nothing typed there is handed back to it
                if !on {
                    join.text = Passphrase::new("");
                }
            }
            PromptEvent::Join(text) => {
                if text.as_str().is_empty() || join.phase == PromptPhase::Joining {
                    return Vec::new();
                }
                join.text = text.clone();
                let link = &self.shared.extras.wifi_link;
                join.joins_at_send = link.joins;
                join.failure_at_send = link.failure.clone();
                join.answered = false;
                join.saw_busy = false;
                join.phase = PromptPhase::Joining;
                join.deadline = Some(Instant::now() + JOIN_TIME);
                if let Err(e) = self.services.extras.join(join.ssid.clone(), text) {
                    return fail(e);
                }
            }
            PromptEvent::Cancel => {
                let joining = join.phase == PromptPhase::Joining;
                self.join = None;
                if joining {
                    if let Err(e) = self.services.extras.act(ExtraAction::CancelJoin) {
                        return fail(e);
                    }
                }
            }
        }
        Vec::new()
    }

    /// The service's state moved: see how far the join stands.
    fn follow_join(&mut self) {
        let Some(join) = self.join.as_mut() else {
            return;
        };
        if join.phase != PromptPhase::Joining {
            return;
        }
        let link = &self.shared.extras.wifi_link;
        if link.joins > join.joins_at_send {
            if let Some(refused) = &link.refused {
                join.phase = PromptPhase::Failed(refused.clone());
                join.deadline = None;
                return;
            }
            join.answered = true;
        }
        if !join.answered {
            return;
        }
        if link.joined(&join.ssid) {
            self.join = None;
        } else if link.busy() {
            join.saw_busy = true;
        } else if !link.failure.is_empty()
            && (join.saw_busy || link.failure != join.failure_at_send)
        {
            join.phase = PromptPhase::Failed(failure_line(&link.failure));
            join.deadline = None;
        } else if join.saw_busy {
            join.phase = PromptPhase::Failed(failure_line(""));
            join.deadline = None;
        }
    }

    /// Show the alternates of the open menus' items while Alt is held on a surface of their bar.
    fn sync_alt(&mut self, i: usize) -> Vec<Command<Message>> {
        let bar = &mut self.bars[i];
        let held = bar.alt && bar.has_keyboard();
        let outcome = bar.controller.set_alt(held);
        if outcome.effects.is_empty() {
            Vec::new()
        } else {
            self.apply(i, outcome)
        }
    }

    // ---- bus -----------------------------------------------------------------------------

    fn wake(&mut self, wake: Wake) -> Vec<Command<Message>> {
        match wake {
            Wake::Worker => self.drain_worker(),
            Wake::Extras => self.drain_extras(),
            Wake::Conf => {
                self.reload_conf();
                Vec::new()
            }
        }
    }

    /// Apply the changes of `menubar.conf`: the file is read again with the current values as the last good ones, and a
    /// new clock setting redraws the clock now. A file that cannot be read keeps the current values (reported).
    fn reload_conf(&mut self) {
        while let Ok(event) = self.services.conf_changes.try_recv() {
            match event {
                WatchEvent::Error(e) => {
                    eprintln!("{PROCESS}: watching menubar.conf failed: {e}")
                }
                WatchEvent::Changed(_) => {
                    match load_conf(Some(&self.conf)) {
                        Ok(conf) => {
                            if self.clock != conf.clock || self.conf.shown.clock != conf.shown.clock
                            {
                                self.clock = conf.clock;
                                self.next_clock = f64::NEG_INFINITY;
                            }
                            let background = conf.background != self.conf.background;
                            self.shared.conf = conf.clone();
                            self.conf = conf;
                            if background {
                                let mut surfaces = self.surfaces.borrow_mut();
                                surfaces.background = self.conf.background;
                                for bar in &surfaces.bars {
                                    bar.shape_backdrop(self.display, self.conf.background);
                                }
                                drop(surfaces);
                                let display = self.tone_display();
                                for bar in &mut self.bars {
                                    bar.tone = BarTone::fresh(bar.backdrop_lightness, display);
                                }
                            }
                        }
                        Err(e) => eprintln!("{PROCESS}: {e}; the clock keeps its settings"),
                    }
                    self.follow_schedule(unix_now());
                }
            }
        }
    }

    fn drain_worker(&mut self) -> Vec<Command<Message>> {
        loop {
            let event = match self.services.events.try_recv() {
                Ok(event) => event,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return fail(worker::WorkerGone),
            };
            match event {
                worker::Event::Layout { source, root } => self.shared.apply_layout(&source, root),
                worker::Event::LayoutFailed { source } => self.shared.layout_failed(&source),
                worker::Event::StatusUpdated { item, menu } => {
                    let px = status_icon_px(ICON_SCALE);
                    let icons = &self.icons;
                    let icon = status_icon(&item, px, |name, px| icons.load(name, px))
                        .map(|pixmap| view::icon_image(&pixmap));
                    self.shared.set_status(StatusState {
                        item: *item,
                        icon,
                        menu,
                    });
                }
                worker::Event::StatusRemoved { id } => self.shared.remove_status(&id),
            }
        }
        self.refresh()
    }

    fn drain_extras(&mut self) -> Vec<Command<Message>> {
        let mut commands = Vec::new();
        loop {
            match self.services.extras_events.try_recv() {
                Ok(extras) => {
                    // the session agent went away: the system menu's session items would do nothing, so it closes
                    let agent_gone = self.shared.extras.loses_session(&extras);
                    let (list_open, scanning) = (
                        self.shared.extras.wifi_list_open,
                        self.shared.extras.wifi_scanning,
                    );
                    self.shared.extras = extras;
                    // what the panel keeps itself outlives a reading; a list that arrived ends the scan
                    self.shared.extras.wifi_list_open = list_open;
                    let listed = self
                        .shared
                        .extras
                        .wifi
                        .as_ref()
                        .is_some_and(|w| !w.networks.is_empty());
                    self.shared.extras.wifi_scanning = scanning && !listed;
                    self.follow_join();
                    if agent_gone {
                        for i in 0..self.bars.len() {
                            if let Some(outcome) =
                                self.bars[i].controller.close_slot(&SlotKey::System)
                            {
                                commands.extend(self.apply(i, outcome));
                            }
                        }
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return fail(worker::WorkerGone),
            }
        }
        commands.extend(self.refresh());
        commands
    }

    // ---- content and timers --------------------------------------------------------------

    /// Give every bar the content its feed describes, load menus it needs and forget the rest.
    fn refresh(&mut self) -> Vec<Command<Message>> {
        let mut commands = Vec::new();
        for i in 0..self.bars.len() {
            let content = self.bars[i].feed.content(&self.shared);
            if self.bars[i].controller.content() != &content {
                let outcome = self.bars[i].controller.set_content(content);
                commands.extend(self.apply(i, outcome));
            }
            if let Some(request) = self.bars[i].feed.request_layout(&mut self.shared) {
                if let Err(e) = self.services.worker.send(request) {
                    return fail(e);
                }
            }
        }
        let in_use: Vec<_> = self
            .bars
            .iter()
            .map(|b| b.feed.source(&self.shared))
            .collect();
        self.shared.prune_layouts(&in_use);
        commands
    }

    /// Update the clock, read the extras' services once a minute, run due menu timers and schedule the timer for the
    /// next of them.
    fn housekeeping(&mut self) -> Vec<Command<Message>> {
        let mut commands = Vec::new();
        let unix = unix_now();
        if unix >= self.next_clock {
            self.next_clock = next_tick(unix, &self.clock);
            self.shared.clock = clock_text(
                &local_time(unix.floor() as i64),
                &self.clock,
                separators_off(unix),
            );
            commands.extend(self.refresh());
        }
        // the services announce their changes; the duration rows change with the hour and announce nothing
        let minute = (unix / 60.0).floor() as i64;
        if self.extras_minute != Some(minute) {
            self.extras_minute = Some(minute);
            self.follow_schedule(unix);
            if let Err(e) = self.services.extras.refresh() {
                return fail(e);
            }
        }
        let now = self.now();
        let mut next = Duration::from_secs_f64((self.next_clock - unix).max(0.0));
        if let Some(until) = self.wifi_scan_until {
            if Instant::now() >= until {
                self.wifi_scan_until = None;
                self.shared.extras.wifi_scanning = false;
                commands.extend(self.refresh());
            } else {
                next = next.min(until.saturating_duration_since(Instant::now()));
            }
        }
        if let Some(until) = self.join.as_ref().and_then(|j| j.deadline) {
            if Instant::now() >= until {
                if let Some(join) = self.join.as_mut() {
                    join.phase = PromptPhase::Failed(failure_line(""));
                    join.deadline = None;
                }
            } else {
                next = next.min(until.saturating_duration_since(Instant::now()));
            }
        }
        for i in 0..self.bars.len() {
            if self.bars[i]
                .controller
                .next_deadline()
                .is_some_and(|d| d <= now)
            {
                let outcome = self.bars[i].controller.tick(now);
                commands.extend(self.apply(i, outcome));
            }
            if let Some(deadline) = self.bars[i].controller.next_deadline() {
                next = next.min(Duration::from_secs_f64((deadline - now).max(0.0)));
            }
        }
        let due = Instant::now() + next;
        if let Some(timer) = self.wakeup.request(due) {
            commands.push(Command::Perform(Box::pin(async move {
                async_io::Timer::at(due).await;
                Message::Timer(timer)
            })));
        }
        commands
    }

    // ---- launches ------------------------------------------------------------------------

    /// Ask for an activation token for a launch from bar `bar`; it starts when the token arrives.
    fn begin_launch(&mut self, bar: BarId, launch: Launch) -> Command<Message> {
        let number = self.next_launch;
        self.next_launch += 1;
        self.pending.insert(number, launch);
        Command::ActivationToken {
            window: bar.window(),
            app_id: None,
            on_token: Box::new(move |token| Message::Token {
                launch: number,
                token,
            }),
        }
    }

    /// Start the launch that waited for `token` (it uses the token to take the focus).
    fn complete_launch(&mut self, number: u64, token: Option<String>) {
        let Some(launch) = self.pending.remove(&number) else {
            return;
        };
        let Some(token) = token else {
            eprintln!(
                "{PROCESS}: no activation token for {}",
                launch_name(&launch)
            );
            return;
        };
        match launch.command(&token) {
            Some(spec) if spec.terminal => eprintln!(
                "{PROCESS}: {} runs in a terminal, and the menu bar has none",
                launch_name(&launch)
            ),
            Some(spec) => {
                launch::spawn(PROCESS, spec.to_command());
            }
            None => eprintln!("{PROCESS}: {} has no command", launch_name(&launch)),
        }
    }

    /// Open `file` from bar `bar` with the default application of its type, or through [`OPEN_COMMAND`] when no
    /// installed application handles it. The associations are read at each open, so changed defaults apply at once.
    fn open_document(&mut self, bar: BarId, file: &Path) -> Option<Command<Message>> {
        let apps = XdgDirs::from_env().and_then(|dirs| {
            let db = MimeDatabase::load(&dirs.all_data_dirs()).map_err(|e| e.to_string())?;
            MimeApps::load(&dirs, db, &Locale::from_env(), &Visibility::from_env())
                .map_err(|e| e.to_string())
        });
        match apps.map(|apps| launch::open_with_default(&apps, file)) {
            Ok(Some(launch)) => return Some(self.begin_launch(bar, launch)),
            Ok(None) => {
                let mut command = std::process::Command::new(OPEN_COMMAND);
                command.arg(file);
                launch::spawn(PROCESS, command);
            }
            Err(e) => eprintln!("{PROCESS}: cannot open {}: {e}", file.display()),
        }
        None
    }

    /// Read Recent Items for the system menu that is opening; the bars show it at their next refresh.
    fn read_recent_items(&mut self) {
        if let Some(sources) = &self.services.recent {
            let mut recent = sources.read(unix_now());
            recent.apps.truncate(self.conf.recent_apps as usize);
            recent
                .documents
                .truncate(self.conf.recent_documents as usize);
            self.shared.recent = recent;
        }
    }

    /// Clear Menu: Recent Items lists only what is used from now on.
    fn clear_recent_items(&mut self) {
        if let Some(sources) = &self.services.recent {
            if let Err(e) = sources.clear(unix_now()) {
                eprintln!("{PROCESS}: cannot write {}: {e}", sources.cleared.display());
            }
            self.shared.recent = Default::default();
        }
    }

    // ---- effects -------------------------------------------------------------------------

    /// Carry out what the controller of bar `i` asked for, then bring the keyboard in line.
    fn apply(&mut self, i: usize, outcome: Outcome) -> Vec<Command<Message>> {
        let actions: Vec<Action> = {
            let bar = &self.bars[i];
            let cx = Context {
                feed: &bar.feed,
                shared: &self.shared,
                layout: bar.controller.layout(),
                origin: (bar.geometry.x, bar.geometry.y),
            };
            outcome
                .effects
                .into_iter()
                .filter_map(|e| routing::resolve(e, &cx))
                .collect()
        };
        let bar = self.bars[i].id;
        let mut commands = Vec::new();
        for action in actions {
            match action {
                // the windows show the controller as it is after each update
                Action::Redraw => {}
                Action::SetMenuOpen(open) => {
                    if !open {
                        // the menus took the keyboard with them: an Alt released over them never reaches the bar
                        self.bars[i].alt = false;
                        self.bars[i].controller.set_alt(false);
                    }
                    self.menu_bar(bar, |m| m.set_menu_open(open));
                }
                Action::Request(request) => {
                    if let Err(e) = self.services.worker.send(request) {
                        return fail(e);
                    }
                }
                Action::WindowCommand(command) => {
                    self.menu_bar(bar, |m| m.window_command(command));
                }
                Action::MoveToWorkspace(workspace) => {
                    self.menu_bar(bar, |m| m.move_to_workspace(workspace));
                }
                Action::ActivateWindow(id) => {
                    self.menu_bar(bar, |m| m.activate_window(id));
                }
                Action::Edit(action) => {
                    self.menu_bar(bar, |m| m.edit(action));
                }
                Action::ForceQuitApp(app_id) => {
                    self.menu_bar(bar, |m| m.force_quit_app(&app_id));
                }
                Action::Launch(launch) => commands.push(self.begin_launch(bar, launch)),
                Action::OpenDocument(file) => commands.extend(self.open_document(bar, &file)),
                Action::SystemMenuOpening {
                    refresh_capabilities,
                } => {
                    self.read_recent_items();
                    if refresh_capabilities {
                        self.menu_bar(bar, |m| {
                            m.session_action(client_session_action(
                                SessionAction::RefreshCapabilities,
                            ))
                        });
                    }
                }
                Action::ClearRecentItems => self.clear_recent_items(),
                Action::ShowAbout => {
                    eprintln!("{PROCESS}: about requested (there is no about panel yet)")
                }
                Action::Extra(ExtraAction::Session(action)) => {
                    self.menu_bar(bar, |m| m.session_action(client_session_action(action)));
                }
                Action::Extra(ExtraAction::PromptNetwork(ssid)) => self.open_prompt(i, ssid),
                Action::Extra(ExtraAction::ToggleWifiList) => {
                    self.shared.extras.wifi_list_open = !self.shared.extras.wifi_list_open;
                    commands.extend(self.refresh());
                }
                Action::Extra(action) => {
                    if matches!(
                        action,
                        ExtraAction::ScanWifi | ExtraAction::SetWifiEnabled(true)
                    ) {
                        self.shared.extras.wifi_scanning = true;
                        self.wifi_scan_until = Some(Instant::now() + WIFI_SCAN_TIME);
                        commands.extend(self.refresh());
                    }
                    if let Err(e) = self.services.extras.act(action) {
                        return fail(e);
                    }
                }
            }
        }
        self.sync_keyboard(i);
        commands
    }

    /// The bar takes the keyboard exclusively while a title is focused by the shortcut (the server gives a layer the
    /// keyboard only for exclusive interactivity); open menus get it through their popup grab. The change applies with
    /// the bar's next frame, which the title's highlight brings.
    fn sync_keyboard(&mut self, i: usize) {
        let bar = &mut self.bars[i];
        let wanted = if bar.controller.mode() == Mode::TitleFocus {
            KeyboardInteractivity::Exclusive
        } else {
            KeyboardInteractivity::None
        };
        if bar.keyboard != wanted {
            bar.keyboard = wanted;
            if let Some(layer) = self
                .surfaces
                .borrow()
                .get(bar.id)
                .and_then(|s| s.layer.as_ref())
            {
                layer.set_keyboard_interactivity(wanted);
            }
        }
    }
}

impl Application for App {
    type Model = App;
    type Message = Message;
    const RESIDENT: bool = true;

    fn init() -> (App, Vec<Command<Message>>) {
        unreachable!(
            "the menu bar starts only through `run`, which hands the model to `run_on_with`"
        )
    }

    fn update(app: &mut App, message: Message) -> Vec<Command<Message>> {
        let mut commands = match message {
            Message::Shell(message) => {
                let mut commands = app.shell_message(message);
                commands.push(listen_shell(&app.shell));
                commands
            }
            Message::Input(input) => app.input(input),
            Message::Wake(wake) => {
                let mut commands = app.wake(wake);
                commands.push(listen_wakes(&app.wakes));
                commands
            }
            Message::Timer(timer) => {
                if !app.wakeup.fired(timer) {
                    return Vec::new();
                }
                Vec::new()
            }
            Message::Token { launch, token } => {
                app.complete_launch(launch, token);
                Vec::new()
            }
            Message::Unheard => fail("the roles or the bus threads are gone"),
        };
        commands.extend(app.housekeeping());
        commands
    }

    fn view(app: &App) -> Vec<Window<Message>> {
        let surfaces = app.surfaces.borrow();
        app.bars
            .iter()
            .flat_map(|bar| {
                let output = surfaces.names.get(&bar.output).map(String::as_str);
                let open_panels: Vec<SlotKey> = [SlotKey::ControlPanel, SlotKey::Clock]
                    .into_iter()
                    .filter(|slot| {
                        app.shared
                            .extras
                            .panels
                            .open_on(slot, output, app.bars.len())
                    })
                    .collect();
                let prompt = app
                    .join
                    .as_ref()
                    .filter(|j| j.bar == bar.id)
                    .map(|j| PromptScene {
                        ssid: &j.ssid,
                        at: prompt_origin(bar, app.height),
                        show: j.show,
                        text: &j.text,
                        phase: j.phase.clone(),
                    });
                view::windows(
                    &BarScene {
                        bar: bar.id,
                        controller: &bar.controller,
                        shared: &app.shared,
                        font: app.font,
                        tone: bar.tone,
                        solid: cupertino_menubar::tone::solid_bar(app.tone_display()),
                        size: Size::new(f64::from(bar.width), app.height),
                        open_panels: &open_panels,
                        icons: &app.icons,
                        prompt,
                    },
                    Message::Input,
                )
            })
            .collect()
    }
}

/// Where the join prompt's top-left corner is on `bar`: where the Wi-Fi panel's is, its right edge level with the
/// Wi-Fi item's, a little below the bar.
fn prompt_origin(bar: &Bar, bar_height: f64) -> cupertino_menubar::geom::Point {
    use cupertino_menubar::layout::panel;
    let right = bar
        .controller
        .layout()
        .slots
        .iter()
        .find(|slot| matches!(&slot.key, SlotKey::Status(id) if id == "extra:wifi"))
        .map_or(f64::from(bar.width) - 8.0, |slot| slot.rect.x + slot.rect.w);
    cupertino_menubar::geom::Point::new((right - panel::WIDTH).max(8.0), bar_height + panel::GAP)
}

/// Seconds since the Unix epoch (0 for a clock set before it).
fn unix_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

/// What a launch starts, for messages.
fn launch_name(launch: &Launch) -> String {
    match launch {
        Launch::Program(argv) => argv.join(" "),
        Launch::App(app) => app.id.clone(),
        Launch::Open { app, file } => format!("{} with {}", file.display(), app.id),
    }
}

fn client_session_action(action: SessionAction) -> cupertino_shell_client::SessionAction {
    match action {
        SessionAction::Sleep => cupertino_shell_client::SessionAction::Sleep,
        SessionAction::Restart => cupertino_shell_client::SessionAction::Restart,
        SessionAction::ShutDown => cupertino_shell_client::SessionAction::ShutDown,
        SessionAction::Lock => cupertino_shell_client::SessionAction::Lock,
        SessionAction::LogOut => cupertino_shell_client::SessionAction::LogOut,
        SessionAction::SwitchUser => cupertino_shell_client::SessionAction::SwitchUser,
        SessionAction::RefreshCapabilities => {
            cupertino_shell_client::SessionAction::RefreshCapabilities
        }
    }
}
