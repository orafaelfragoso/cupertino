//! Glue between the dock model and the toolkit surfaces: draws the bar over a server-rendered
//! backdrop, feeds the model the pointer and the server's window list, and carries out what a
//! click on a tile asks for. It also draws the application switcher's panel on an overlay surface
//! that lives for one switch, and tells the server which entry the pointer picked; and it opens
//! the application grid on a keyboard-exclusive overlay surface when the server forwards the
//! app-grid action, and launches from it through the same path as a tile. When the server opens
//! the overview it lays the windows out and answers with their frames, draws the workspaces bar,
//! the highlight and the titles on a keyboard-exclusive overlay surface, and sends the picks; hot
//! corners and gestures the server reports are mapped to their action (`dock.conf`); a right
//! click on a tile or the separator opens its context menu as a popup. The decisions live in the
//! library modules (`session`, `dock`, `conf`, `switcher`, `app_grid`, `overview_ui`, `menu`,
//! `actions`); drawing is in `render`.

use cupertino_assets::Assets;
use cupertino_client::{
    key, ActivationRequest, Anchor, AppCommand, Backdrop, Button, Client, Corner, Dock as DockRole,
    DockChange, DockUpdate, Event, Gesture, KeyboardInteractivity, Layer, LayerSurfaceConfig,
    Margin, Modifiers, OutputId, OverviewMode, OverviewWindow, OverviewWorkspace, PointerEvent,
    SurfaceId,
};
use cupertino_defaults::dock::DockSettings;
use cupertino_defaults::watch::{ConfigWatcher, WatchEvent};
use cupertino_dock::actions::{ActionMap, ActionTarget, Trigger};
use cupertino_dock::app_grid::{self, AppGrid, GridLayout, Key as GridKey, Outcome};
use cupertino_dock::conf::{
    default_pins, dock_config, load_pins, session_file_manager, store_pins, FILE_MANAGER, TERMINALS,
};
use cupertino_dock::config::Orientation;
use cupertino_dock::desktop::DesktopIndex;
use cupertino_dock::force_quit::{self, Button as FqButton, Dialog, Entry as FqEntry, Extend};
use cupertino_dock::menu::{self, MenuCommand, MenuItem, MenuLayout, TileFacts};
use cupertino_dock::overview_ui::{self, Mode, OverviewUi, UiAction, WindowEntry, Workspace};
use cupertino_dock::render::{
    self, ForceQuitView, GridView, Icons, LabelText, OverviewView, Panel,
};
use cupertino_dock::session::{Activation, Session};
use cupertino_dock::switcher::{self, HoverGate, Switcher, SwitcherLayout};
use cupertino_dock::tile_drag::{Poof, TilePress};
use cupertino_dock::tiles::TileKind;
use cupertino_dock::{Dock, DragOutcome};
use cupertino_geometry::Rect as GRect;
use cupertino_overview::Key as OverviewKey;
use cupertino_theme::Typography;
use cupertino_wm::geometry::{Point, Rect};
use std::collections::HashMap;
use std::error::Error;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// Role of the surface for the server (`wlr-layer-shell` namespace).
/// Namespace of the surface that draws the tile held by the pointer.
/// The layer-shell namespace of the Force Quit dialog.
const FORCE_QUIT_NAMESPACE: &str = "cupertino-force-quit";
const DRAG_NAMESPACE: &str = "cupertino-dock-drag";
const NAMESPACE: &str = "cupertino-dock";

/// Role of the switcher's overlay surface for the server (`wlr-layer-shell` namespace).
const SWITCHER_NAMESPACE: &str = "cupertino-dock-switcher";

/// Material of the bar, drawn by the server below the surface.
const MATERIAL: &str = "Menu";

/// Corner radius of the bar as a fraction of the tile size. approximate.
const BAR_RADIUS: f64 = 0.3;

/// Environment variables that hand the activation token to a started application.
const ACTIVATION_TOKEN_ENV: &str = "XDG_ACTIVATION_TOKEN";
const STARTUP_ID_ENV: &str = "DESKTOP_STARTUP_ID";

/// What opens the trash and a directory when the session's file manager is not installed: the
/// session's default handler of `x-scheme-handler/trash` and `inode/directory`.
const OPEN_COMMAND: &str = "xdg-open";
const TRASH_URI: &str = "trash:///";

/// Material of a context menu's popup, the menu bar's.
const MENU_MATERIAL: &str = "Menu";

/// Command line of the process.
pub struct Options {
    /// Terminal emulator that runs entries with `Terminal=true`; the first installed of the
    /// common ones when absent.
    pub terminal: Option<String>,
}

impl Options {
    /// Parse the arguments; the problem is returned as the error.
    pub fn parse(args: impl Iterator<Item = String>) -> std::result::Result<Self, String> {
        let mut options = Options { terminal: None };
        let mut args = args;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--terminal" => {
                    options.terminal = Some(args.next().ok_or("--terminal needs a program")?)
                }
                _ => return Err(format!("unknown argument {arg}")),
            }
        }
        Ok(options)
    }
}

/// An application waiting for its activation token.
struct Pending {
    /// Key of the tile that bounces once the program started.
    key: String,
    argv: Vec<String>,
}

/// The switcher's panel while a switch is under way: the model and the overlay surface, created
/// when the switch opens and destroyed when it ends. The surface is as large as the panel, centred
/// by the server on the output it names (the one under the pointer when the switch opened, not
/// necessarily the dock's), and takes no keys: the server keeps those.
struct Overlay {
    output: OutputId,
    switcher: Switcher,
    layout: SwitcherLayout,
    surface: SurfaceId,
    backdrop: Backdrop,
    hover: HoverGate,
    /// Entry the last `switcher_select` asked for, until the server answers.
    requested: Option<usize>,
    /// The first draw after the show delay was asked for.
    draw_requested: bool,
}

/// The open application grid: the model and its overlay surface, anchored to every edge of the
/// output below the 30 pt top band and inside the dock bar's exclusive zone, with the keyboard
/// while it is open. The layout is known once the server sized the surface.
struct Grid {
    grid: AppGrid,
    surface: SurfaceId,
    backdrop: Backdrop,
    layout: Option<GridLayout>,
}

/// The open overview's chrome: the layout model and its overlay surface over the whole output
/// (so surface and output coordinates agree), with the keyboard while it is open.
struct OverviewSurface {
    ui: OverviewUi,
    surface: SurfaceId,
}

/// An open context menu: a popup of the bar surface with a grab.
/// The Force Quit Applications dialog on screen.
struct ForceQuit {
    dialog: Dialog,
    surface: SurfaceId,
    layout: force_quit::Layout,
    hover_row: Option<usize>,
    hover_button: Option<FqButton>,
}

struct Menu {
    items: Vec<MenuItem>,
    layout: MenuLayout,
    surface: SurfaceId,
    backdrop: Backdrop,
    hovered: Option<usize>,
}

struct App {
    client: Client,
    surface: SurfaceId,
    role: DockRole,
    backdrop: Backdrop,
    session: Session,
    icons: Icons,
    labels: LabelText,
    overlay: Option<Overlay>,
    grid: Option<Grid>,
    overview: Option<OverviewSurface>,
    menu: Option<Menu>,
    /// The applications that stopped answering (their ids), as the server last said.
    unresponsive: std::collections::HashSet<String>,
    force_quit: Option<ForceQuit>,
    /// What the hot corners and gestures do.
    actions: ActionMap,
    /// The switch whose overlay surface the server closed mid-switch; the next step of that switch
    /// builds the surface again around it, so a panel that was on screen is not delayed again.
    orphan: Option<Switcher>,
    start: Instant,
    /// Pointer position in the surface, while the pointer is over it.
    pointer: Option<Point>,
    /// The left press on an application tile that is a click or the start of a drag.
    press: Option<TilePress>,
    /// The puff where the last dragged tile was removed, until it is over.
    poof: Option<Poof>,
    /// The surface over the whole output that draws the tile held by the pointer, settling after a
    /// drop or fading after a removal: it may leave the bar's own surface. It takes no input.
    drag_surface: Option<SurfaceId>,
    /// Where the pins are stored (`None` when neither `XDG_CONFIG_HOME` nor `HOME` is set).
    pins_path: Option<PathBuf>,
    /// The session's appearance, for the indicators and separators.
    dark: bool,
    terminal: Option<String>,
    pending: HashMap<ActivationRequest, Pending>,
    output: OutputId,
    /// `dock.conf` as last read, the last good values of the next read; its path and the watch on it (`None` without one).
    conf: DockSettings,
    conf_path: Option<PathBuf>,
    conf_changes: std::sync::mpsc::Receiver<WatchEvent>,
    _conf_watch: Option<ConfigWatcher>,
    /// Height of the dock's output in points, which picks the default tile size.
    display_height: f64,
}

/// The edges the dock's surface is anchored to and its size for a dock on `orientation` that is `extent` deep.
fn placement(orientation: Orientation, extent: f64) -> (Anchor, (u32, u32)) {
    let extent = extent as u32;
    match orientation {
        Orientation::Bottom => (Anchor::Bottom | Anchor::Left | Anchor::Right, (0, extent)),
        Orientation::Left => (Anchor::Left | Anchor::Top | Anchor::Bottom, (extent, 0)),
        Orientation::Right => (Anchor::Right | Anchor::Top | Anchor::Bottom, (extent, 0)),
    }
}

/// Watch `dock.conf` at `path`; its changes wake the loop. A watch that cannot be set up is reported: the file then
/// applies at the next start.
fn watch_conf(
    client: &Client,
    path: Option<&PathBuf>,
) -> (Option<ConfigWatcher>, std::sync::mpsc::Receiver<WatchEvent>) {
    let (tx, rx) = std::sync::mpsc::channel();
    let Some(path) = path else { return (None, rx) };
    let waker = client.waker();
    let watch = ConfigWatcher::new(vec![path.clone()], move |event| {
        // the receiver is gone only while the process exits
        if tx.send(event).is_ok() {
            waker.wake();
        }
    });
    match watch {
        Ok(w) => (Some(w), rx),
        Err(e) => {
            eprintln!(
                "cupertino-dock: cannot watch {}: {e}; changes apply at the next start",
                path.display()
            );
            (None, rx)
        }
    }
}

/// `dock.conf` at `path` (nothing set when it does not exist). Lines that are not understood are
/// reported; with `last_good` their keys keep its values.
fn load_dock_settings(
    path: &std::path::Path,
    last_good: Option<&DockSettings>,
) -> Result<DockSettings> {
    let (settings, problems) = DockSettings::load(path, last_good)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    for problem in problems {
        eprintln!(
            "cupertino-dock: {} line {}: {}",
            path.display(),
            problem.line,
            problem.message
        );
    }
    Ok(settings)
}

/// Connect, take the dock role of the first output and serve until the connection fails.
pub fn run(options: &Options) -> Result<std::convert::Infallible> {
    let assets = Assets::discover();
    let mut client = Client::connect_shell()?;
    let (output, geometry) = client
        .outputs()
        .into_iter()
        .find_map(|o| o.geometry.map(|g| (o.id, g)))
        .ok_or("the server has no output")?;

    let desktop = DesktopIndex::load_from(&cupertino_defaults::data_dirs());
    let conf_path = DockSettings::path();
    let file = match &conf_path {
        Some(path) => Some(load_dock_settings(path, None)?),
        None => None,
    };
    let cfg = dock_config(file.as_ref(), f64::from(geometry.height));
    let actions = file
        .as_ref()
        .map_or_else(ActionMap::default, |f| f.actions.clone());
    // the pins the dock stored (a drag, Keep in Dock) win over those of `dock.conf`, which the dock never rewrites
    let pins_path = cupertino_defaults::config_file("CUPERTINO_DOCK_PINS", "dock-pins");
    let stored = match &pins_path {
        Some(path) => {
            load_pins(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?
        }
        None => None,
    };
    let pins: Vec<String> = match stored.or_else(|| {
        file.as_ref()
            .map(|f| f.pinned.clone())
            .filter(|p| !p.is_empty())
    }) {
        Some(pins) => pins,
        None => {
            let terminals: Vec<&str> = options
                .terminal
                .iter()
                .map(String::as_str)
                .chain(TERMINALS.iter().copied())
                .collect();
            // four after the built-in file manager
            default_pins(&desktop, &terminals, 4)
        }
    };
    let mut dock = Dock::new(cfg);
    if let Some(file_manager) = session_file_manager(&desktop) {
        dock.set_built_in(&file_manager);
    }
    dock.set_pinned(&pins.iter().map(String::as_str).collect::<Vec<_>>());
    let session = Session::new(dock, desktop);

    let (anchor, size) = placement(session.dock.cfg.orientation, session.dock.surface_extent());
    let surface = client.layer_surface(&LayerSurfaceConfig {
        namespace: NAMESPACE.to_string(),
        output: Some(output),
        layer: Layer::Top,
        anchor,
        size,
        margin: Margin::default(),
        // windows keep clear of the bar, not of the room magnified tiles and bounces need
        exclusive_zone: session.dock.exclusive_zone().ceil() as i32,
        keyboard: KeyboardInteractivity::None,
    })?;
    let role = client.dock(surface, output)?;
    let backdrop = client.backdrop(surface)?;
    backdrop.set_material(MATERIAL);
    let dark = client.is_dark();
    let labels = LabelText::new(&Typography::default(), &assets)?;
    let (conf_watch, conf_changes) = watch_conf(&client, conf_path.as_ref());
    let conf = file.unwrap_or_default();
    let display_height = f64::from(geometry.height);
    let mut app = App {
        client,
        surface,
        role,
        backdrop,
        session,
        icons: Icons::new(&assets),
        labels,
        overlay: None,
        grid: None,
        unresponsive: fixture_unresponsive(),
        force_quit: None,
        overview: None,
        menu: None,
        actions,
        orphan: None,
        start: Instant::now(),
        pointer: None,
        press: None,
        poof: None,
        drag_surface: None,
        pins_path,
        dark,
        terminal: options.terminal.clone(),
        pending: HashMap::new(),
        output,
        conf,
        conf_path,
        conf_changes,
        _conf_watch: conf_watch,
        display_height,
    };
    loop {
        let timeout = app.until_due();
        for event in app.client.wait(timeout)? {
            app.handle(event)?;
        }
        app.reload_conf()?;
        app.show_panel_when_due()?;
        app.settle_grid_wheel()?;
        app.open_menu_when_held()?;
    }
}

/// The grid key a toolkit key stands for; a character without a command modifier
/// types into the search field.
/// The dialog's key for an evdev `code`.
fn force_quit_key(code: u32) -> Option<force_quit::Key> {
    Some(match code {
        key::ESCAPE => force_quit::Key::Escape,
        key::ENTER | key::KEYPAD_ENTER => force_quit::Key::Return,
        key::UP => force_quit::Key::Up,
        key::DOWN => force_quit::Key::Down,
        key::HOME => force_quit::Key::Home,
        key::END => force_quit::Key::End,
        _ => return None,
    })
}

fn grid_key(code: u32, mods: Modifiers, character: Option<char>) -> Option<GridKey> {
    let named = match code {
        key::ESCAPE => Some(GridKey::Escape),
        key::ENTER | key::KEYPAD_ENTER => Some(GridKey::Return),
        key::BACKSPACE => Some(GridKey::Backspace),
        key::LEFT => Some(GridKey::Left),
        key::RIGHT => Some(GridKey::Right),
        key::UP => Some(GridKey::Up),
        key::DOWN => Some(GridKey::Down),
        key::HOME => Some(GridKey::Home),
        key::END => Some(GridKey::End),
        key::PAGE_UP => Some(GridKey::PageUp),
        key::PAGE_DOWN => Some(GridKey::PageDown),
        _ => None,
    };
    if named.is_some() {
        return named;
    }
    if mods.ctrl || mods.alt || mods.logo {
        return None;
    }
    character.filter(|c| !c.is_control()).map(GridKey::Char)
}

fn geometry_rect(r: &Rect) -> GRect {
    GRect::new(r.x, r.y, r.w, r.h)
}

/// The overview key a toolkit key stands for.
fn overview_key(code: u32, mods: Modifiers) -> Option<OverviewKey> {
    Some(match code {
        key::LEFT => OverviewKey::Left,
        key::RIGHT => OverviewKey::Right,
        key::UP => OverviewKey::Up,
        key::DOWN => OverviewKey::Down,
        key::TAB if mods.shift => OverviewKey::Previous,
        key::TAB => OverviewKey::Next,
        key::ENTER | key::KEYPAD_ENTER | key::SPACE => OverviewKey::Activate,
        key::ESCAPE => OverviewKey::Cancel,
        _ => return None,
    })
}

fn trigger_of_corner(corner: Corner) -> Trigger {
    match corner {
        Corner::TopLeft => Trigger::CornerTopLeft,
        Corner::TopRight => Trigger::CornerTopRight,
        Corner::BottomLeft => Trigger::CornerBottomLeft,
        Corner::BottomRight => Trigger::CornerBottomRight,
        // the enum is non-exhaustive for the client; the toolkit drops values it does not know
        _ => Trigger::CornerTopLeft,
    }
}

fn trigger_of_gesture(gesture: Gesture) -> Trigger {
    match gesture {
        Gesture::SwipeUp => Trigger::SwipeUp,
        Gesture::SwipeDown => Trigger::SwipeDown,
        Gesture::Pinch => Trigger::Pinch,
        Gesture::Spread => Trigger::Spread,
        _ => Trigger::SwipeUp,
    }
}

/// Whether `program` is an executable file on `PATH` (or at the path it names).
fn installed(program: &str) -> bool {
    if program.contains('/') {
        return std::path::Path::new(program).is_file();
    }
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
}

/// Run a command detached from the loop; its exit is reaped on a thread. False when it did not start.
fn spawn(mut command: Command) -> bool {
    let program = command.get_program().to_string_lossy().into_owned();
    match command.spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || {
                if let Err(e) = child.wait() {
                    eprintln!("cupertino-dock: {program}: {e}");
                }
            });
            true
        }
        Err(e) => {
            eprintln!("cupertino-dock: cannot run {program}: {e}");
            false
        }
    }
}

impl App {
    /// Seconds on the monotonic clock the model is driven with.
    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    /// The surface as the model's screen.
    fn screen(&mut self) -> Result<Rect> {
        let (width, height) = self.client.size(self.surface)?;
        Ok(Rect::new(0.0, 0.0, f64::from(width), f64::from(height)))
    }

    fn handle(&mut self, event: Event) -> Result<()> {
        match event {
            // the server sized the grid's surface to the usable area: that is the grid's layer
            Event::Configured { surface, size } if self.is_grid(surface) => {
                let grid = self.grid.as_mut().expect("is_grid saw it");
                grid.layout = Some(GridLayout::new(f64::from(size.0), f64::from(size.1)));
            }
            // a draw follows every configure and scale change
            Event::Configured { .. } | Event::ScaleChanged { .. } => {}
            Event::Draw(surface) if self.is_drag_surface(surface) => self.draw_dragged()?,
            Event::Draw(surface) if self.is_overlay(surface) => self.draw_overlay()?,
            Event::Draw(surface) if self.is_grid(surface) => self.draw_grid()?,
            Event::Draw(surface) if self.is_force_quit(surface) => self.draw_force_quit()?,
            Event::Draw(surface) if self.is_overview(surface) => self.draw_overview()?,
            Event::Draw(surface) if self.is_menu(surface) => self.draw_menu()?,
            Event::Draw(_) => self.draw()?,
            // the output went away: the switch goes on in the server, the panel has no place to be
            Event::Closed(surface) if self.is_drag_surface(surface) => self.drag_surface = None,
            Event::Closed(surface) if self.is_overlay(surface) => self.lose_overlay()?,
            Event::Closed(surface) if self.is_grid(surface) => self.drop_grid()?,
            Event::Closed(surface) if self.is_force_quit(surface) => self.close_force_quit()?,
            Event::Closed(surface) if self.is_overview(surface) => self.drop_overview()?,
            Event::Closed(surface) if self.is_menu(surface) => self.drop_menu()?,
            Event::Closed(_) => return Err("the server closed the dock surface".into()),
            // the drag surface takes no input; the bar's surface keeps the pointer until the release
            Event::Pointer { surface, .. } if self.is_drag_surface(surface) => {}
            Event::Pointer { surface, event } if self.is_overlay(surface) => {
                self.overlay_pointer(event)?
            }
            Event::Pointer { surface, event } if self.is_grid(surface) => {
                self.grid_pointer(event)?
            }
            Event::Pointer { surface, event } if self.is_force_quit(surface) => {
                self.force_quit_pointer(event)?
            }
            Event::Pointer { surface, event } if self.is_overview(surface) => {
                self.overview_pointer(event)?
            }
            Event::Pointer { surface, event } if self.is_menu(surface) => {
                self.menu_pointer(event)?
            }
            Event::Pointer { event, .. } => self.pointer(event)?,
            Event::Dock { update, .. } => self.feed(update)?,
            Event::Appearance { dark } => {
                self.dark = dark;
                self.client.request_redraw(self.surface)?;
                if let Some(overlay) = &self.overlay {
                    self.client.request_redraw(overlay.surface)?;
                }
                if let Some(grid) = &self.grid {
                    self.client.request_redraw(grid.surface)?;
                }
            }
            // the materials follow the options in the server; the grid's fade follows Reduce Motion
            Event::DisplayOptions(options) => {
                if let Some(grid) = &mut self.grid {
                    grid.grid.set_reduce_motion(options.reduce_motion);
                    self.client.request_redraw(grid.surface)?;
                }
            }
            Event::ActivationToken { request, token } => self.complete(request, &token)?,
            Event::OutputsChanged => {
                if !self.client.outputs().iter().any(|o| o.id == self.output) {
                    return Err("the output of the dock went away".into());
                }
            }
            Event::Key {
                surface,
                key,
                pressed: true,
                mods,
                character,
            } if self.is_grid(surface) => {
                if let Some(key) = grid_key(key, mods, character) {
                    let now = self.now();
                    let outcome = self
                        .grid
                        .as_mut()
                        .expect("is_grid saw it")
                        .grid
                        .key(key, now);
                    self.grid_outcome(outcome)?;
                }
            }
            Event::Key {
                surface,
                key,
                pressed: true,
                mods,
                ..
            } if self.is_overview(surface) => {
                if let Some(key) = overview_key(key, mods) {
                    let now = self.now();
                    let action = self
                        .overview
                        .as_mut()
                        .expect("is_overview saw it")
                        .ui
                        .key(key, now);
                    self.overview_action(action)?;
                }
            }
            Event::Key {
                surface,
                key,
                pressed: true,
                mods,
                ..
            } if self.is_force_quit(surface) => {
                if let Some(key) = force_quit_key(key) {
                    let outcome = self
                        .force_quit
                        .as_mut()
                        .expect("is_force_quit saw it")
                        .dialog
                        .key(key, mods.shift);
                    self.force_quit_outcome(outcome)?;
                }
            }
            // the keyboard moved elsewhere: the grid loses its reason to stay open
            Event::KeyboardLeave(surface) if self.is_grid(surface) => self.close_grid()?,
            // the server dismissed the menu's grab: the menu is over
            Event::PopupDone(surface) if self.is_menu(surface) => self.drop_menu()?,
            // the lock screen covers everything: the grid and a menu go at once, without animation;
            // the overview's chrome waits for the server's `overview_closed`
            Event::SessionLock { locked: true } => {
                self.close_force_quit()?;
                self.drop_grid()?;
                self.drop_menu()?;
            }
            // the bar has no transient interface of its own; it takes no keys and has no menu-bar role
            Event::SessionLock { .. }
            | Event::LockRequested { .. }
            | Event::Key { .. }
            | Event::KeyboardLeave(_)
            | Event::PopupDone(_)
            | Event::MenuBar { .. } => {}
        }
        Ok(())
    }

    /// Apply a group of the server's window changes at once.
    fn feed(&mut self, update: DockUpdate) -> Result<()> {
        let now = self.now();
        for change in update {
            match change {
                DockChange::Window(w) => {
                    self.session
                        .window(w.id, &w.app_id, &w.title, w.minimized, now)
                }
                DockChange::WindowClosed(id) => self.session.window_closed(id, now),
                DockChange::Launching(app_id) => self.session.launching(&app_id, now),
                DockChange::ToggleAutoHide => {
                    self.session.dock.toggle_autohide(now);
                    self.client.set_exclusive_zone(
                        self.surface,
                        self.session.dock.exclusive_zone().ceil() as i32,
                    )?;
                }
                DockChange::SwitcherChanged {
                    output,
                    apps,
                    selected,
                } => self.switcher_changed(output, apps, selected)?,
                DockChange::SwitcherClosed { committed } => self.switcher_closed(committed)?,
                DockChange::ToggleAppGrid => self.toggle_app_grid()?,
                DockChange::OverviewChanged {
                    mode,
                    area,
                    windows,
                    workspaces,
                } => self.overview_changed(mode, area, windows, workspaces)?,
                DockChange::OverviewClosed => self.overview_closed()?,
                DockChange::HotCorner(corner) => self.run_trigger(trigger_of_corner(corner))?,
                DockChange::Gesture(gesture) => self.run_trigger(trigger_of_gesture(gesture))?,
                // a refused activation: the tile hops once (informational attention)
                DockChange::Attention(app_id) => self.session.attention(&app_id, now),
                DockChange::Unresponsive {
                    app_id,
                    unresponsive,
                } => {
                    if unresponsive {
                        self.unresponsive.insert(app_id);
                    } else {
                        self.unresponsive.remove(&app_id);
                    }
                    self.refresh_force_quit()?;
                }
                DockChange::ForceQuitDialog => self.toggle_force_quit()?,
            }
        }
        self.session.list_taken();
        self.client.request_redraw(self.surface)?;
        Ok(())
    }

    /// The model sees the pointer on the next frame; a left press activates the tile under it, an
    /// application tile on release (a press that moves on is a drag-reorder instead).
    fn pointer(&mut self, event: PointerEvent) -> Result<()> {
        match event {
            PointerEvent::Enter(p) | PointerEvent::Motion(p) => {
                self.pointer = Some(Point::new(p.x, p.y));
                self.drag_pointer(Point::new(p.x, p.y))?;
                self.client.request_redraw(self.surface)?;
            }
            PointerEvent::Leave => {
                self.pointer = None;
                self.client.request_redraw(self.surface)?;
            }
            PointerEvent::Button {
                button: Button::Left,
                pressed: true,
                position,
            } => self.press(Point::new(position.x, position.y))?,
            PointerEvent::Button {
                button: Button::Left,
                pressed: false,
                position,
            } => self.release(Point::new(position.x, position.y))?,
            PointerEvent::Button {
                button: Button::Right,
                pressed: true,
                position,
            } => self.open_menu(Point::new(position.x, position.y))?,
            PointerEvent::Button { .. } | PointerEvent::Scroll { .. } => {}
        }
        Ok(())
    }

    /// A left press: an application tile waits for the release or the first move, any other tile is
    /// activated at once.
    fn press(&mut self, position: Point) -> Result<()> {
        let now = self.now();
        let screen = self.screen()?;
        // a release was lost (the server ended the grab): the drag of that press is over
        self.session.dock.cancel_drag();
        match self.session.dock.hit_test(position, screen, now) {
            Some(hit) if matches!(hit.kind, TileKind::App | TileKind::Recent) => {
                self.press = Some(TilePress::new(hit.id, position, now))
            }
            _ => {
                self.press = None;
                self.click(position)?;
            }
        }
        Ok(())
    }

    /// Pointer motion with a tile pressed: begin the drag once the press moved far enough, and
    /// keep the gap under the pointer while it goes on. The server keeps the pointer on the dock
    /// surface until the release, so the drag follows the pointer outside the bar.
    fn drag_pointer(&mut self, position: Point) -> Result<()> {
        let Some(press) = self.press.as_mut() else {
            return Ok(());
        };
        let (began, id, origin) = (press.moved(position), press.id, press.origin);
        let now = self.now();
        let screen = self.screen()?;
        if began {
            self.session
                .dock
                .begin_drag(id, origin, position, screen, now);
        } else if self.session.dock.is_dragging() {
            self.session.dock.drag_move(position, screen, now);
        }
        Ok(())
    }

    /// A press held in place on a tile opens the tile's menu after [`HOLD_MENU_DELAY`] seconds;
    /// the press is then neither a click nor a drag.
    fn open_menu_when_held(&mut self) -> Result<()> {
        let now = self.now();
        let Some(origin) = self
            .press
            .as_mut()
            .and_then(|p| p.held(now).then_some(p.origin))
        else {
            return Ok(());
        };
        self.press = None;
        self.open_menu(origin)
    }

    /// A left release: a drag drops the tile, a press that stayed in place is a click.
    fn release(&mut self, position: Point) -> Result<()> {
        let Some(press) = self.press.take() else {
            return Ok(());
        };
        if !press.is_dragging() {
            return self.click(press.origin);
        }
        let now = self.now();
        let screen = self.screen()?;
        match self.session.dock.end_drag(screen, now) {
            DragOutcome::Cancelled => {}
            DragOutcome::Reordered { .. } => self.store_pins(),
            DragOutcome::Removed { kind, .. } => {
                self.poof = Some(Poof {
                    centre: position,
                    started: now,
                });
                if kind == TileKind::App {
                    self.store_pins();
                }
            }
        }
        self.client.request_redraw(self.surface)?;
        Ok(())
    }

    /// Write the pinned applications in their order (nothing is stored without a config directory).
    /// Move or resize the dock's surface when its edge or depth differs from `placed`, what they were before a change.
    fn sync_placement(&mut self, placed: (Orientation, f64)) -> Result<()> {
        let now_placed = (
            self.session.dock.cfg.orientation,
            self.session.dock.surface_extent(),
        );
        if now_placed != placed {
            let (anchor, size) = placement(now_placed.0, now_placed.1);
            self.client
                .set_layer_placement(self.surface, anchor, size)?;
        }
        Ok(())
    }

    /// Apply the changes of `dock.conf`: the file is read again with the current values as the last good ones (its bad
    /// lines are reported), and what changed applies now. A file that cannot be read keeps the current values.
    fn reload_conf(&mut self) -> Result<()> {
        while let Ok(event) = self.conf_changes.try_recv() {
            let path = match (event, &self.conf_path) {
                (WatchEvent::Error(e), _) => {
                    eprintln!("cupertino-dock: watching dock.conf failed: {e}");
                    continue;
                }
                (WatchEvent::Changed(_), Some(path)) => path.clone(),
                (WatchEvent::Changed(_), None) => unreachable!("only a configured file is watched"),
            };
            let after = match load_dock_settings(&path, Some(&self.conf)) {
                Ok(after) => after,
                Err(e) => {
                    eprintln!("cupertino-dock: {e}; the dock keeps its settings");
                    continue;
                }
            };
            let before = std::mem::replace(&mut self.conf, after.clone());
            self.apply_conf_change(&before, &after)?;
        }
        Ok(())
    }

    /// Apply the keys whose value differs between `before` and `after`, the values `dock.conf` held; a key removed
    /// from the file goes back to its default. The pins of `dock.conf` apply at the next start (the dock keeps the pins
    /// the user changes in `dock-pins`).
    fn apply_conf_change(&mut self, before: &DockSettings, after: &DockSettings) -> Result<()> {
        let target = dock_config(Some(after), self.display_height);
        let now = self.now();
        let placed = (
            self.session.dock.cfg.orientation,
            self.session.dock.surface_extent(),
        );
        let cfg = &mut self.session.dock.cfg;
        if before.tile_size != after.tile_size {
            cfg.tile_size = target.tile_size;
        }
        if before.large_size != after.large_size {
            cfg.large_size = target.large_size;
        }
        if before.magnification != after.magnification {
            cfg.magnification = target.magnification;
        }
        if before.position != after.position {
            cfg.orientation = target.orientation;
        }
        if before.recents != after.recents {
            cfg.show_recents = target.show_recents;
        }
        if before.autohide != after.autohide && cfg.autohide != target.autohide {
            self.session.dock.toggle_autohide(now);
        }
        if before.actions != after.actions {
            self.actions = after.actions.clone();
        }
        self.sync_placement(placed)?;
        self.client.set_exclusive_zone(
            self.surface,
            self.session.dock.exclusive_zone().ceil() as i32,
        )?;
        self.client.request_redraw(self.surface)?;
        Ok(())
    }

    /// A failure goes to stderr: the dock keeps the order it shows, and the next change writes the
    /// file again.
    fn store_pins(&self) {
        let Some(path) = &self.pins_path else { return };
        if let Err(e) = store_pins(path, &self.session.dock.pinned_keys()) {
            eprintln!("cupertino-dock: cannot write {}: {e}", path.display());
        }
    }

    /// Carry out what the tile at `position` stands for.
    fn click(&mut self, position: Point) -> Result<()> {
        let now = self.now();
        let screen = self.screen()?;
        let Some(hit) = self.session.dock.hit_test(position, screen, now) else {
            return Ok(());
        };
        match self.session.activation(hit.id) {
            Some(Activation::ActivateApp(app_id)) => self.role.activate_app(&app_id),
            Some(Activation::RestoreWindow(id)) => {
                let tile = self
                    .session
                    .dock
                    .layout(screen, now)
                    .tiles
                    .into_iter()
                    .find(|t| t.id == hit.id)
                    .expect("a hit tile is in the layout it was hit in");
                self.role.restore_window(id, &geometry_rect(&tile.rect))?;
            }
            Some(Activation::Launch(key)) => self.begin_launch(key)?,
            Some(Activation::OpenTrash) => self.open_in_file_manager(TRASH_URI)?,
            None => {}
        }
        Ok(())
    }

    /// Open `target` (a folder or the trash) in the session's file manager like a launch of its tile (an
    /// activation token, so its window takes the focus, and a bounce); without it through [`OPEN_COMMAND`].
    fn open_in_file_manager(&mut self, target: &str) -> Result<()> {
        match self.session.file_manager_command(target) {
            Some(argv) => {
                let request = self.client.request_activation_token(self.surface)?;
                self.pending.insert(
                    request,
                    Pending {
                        key: FILE_MANAGER.to_string(),
                        argv,
                    },
                );
            }
            None => {
                let mut command = Command::new(OPEN_COMMAND);
                command.arg(target);
                spawn(command);
            }
        }
        Ok(())
    }

    /// Fetch an activation token for the launch; the program starts when it arrives.
    fn begin_launch(&mut self, key: String) -> Result<()> {
        let launch = self.session.launch(&key);
        let mut argv = launch.argv;
        if launch.terminal {
            let Some(terminal) = self.terminal.clone().or_else(|| {
                TERMINALS
                    .iter()
                    .find(|t| installed(t))
                    .map(|t| t.to_string())
            }) else {
                eprintln!("cupertino-dock: {key} runs in a terminal and none is installed (start with --terminal)");
                return Ok(());
            };
            argv.splice(0..0, [terminal, "-e".to_string()]);
        }
        if argv.is_empty() {
            eprintln!("cupertino-dock: {key} has no command");
            return Ok(());
        }
        let request = self.client.request_activation_token(self.surface)?;
        self.pending.insert(request, Pending { key, argv });
        Ok(())
    }

    /// Start the program that waited for `token` (the application uses it to take the focus) and
    /// bounce its tile.
    fn complete(&mut self, request: ActivationRequest, token: &str) -> Result<()> {
        let Pending { key, argv } = self
            .pending
            .remove(&request)
            .expect("token for a request this process made");
        let (program, args) = argv.split_first().expect("a pending launch has a program");
        let mut command = Command::new(program);
        command
            .args(args)
            .env(ACTIVATION_TOKEN_ENV, token)
            .env(STARTUP_ID_ENV, token);
        if spawn(command) {
            let now = self.now();
            self.session.dock.app_launching(&key, now);
            self.client.request_redraw(self.surface)?;
        }
        Ok(())
    }

    fn is_overlay(&self, surface: SurfaceId) -> bool {
        self.overlay.as_ref().is_some_and(|o| o.surface == surface)
    }

    /// The server sent the switcher's list and selection: a step of the open switch moves the
    /// selection, anything else opens a new switch with a new overlay surface on `output`.
    fn switcher_changed(
        &mut self,
        output: OutputId,
        apps: Vec<String>,
        selected: usize,
    ) -> Result<()> {
        if let Some(overlay) = self
            .overlay
            .as_mut()
            .filter(|o| o.output == output && o.switcher.continues_with(&apps))
        {
            overlay.switcher.select(selected)?;
            overlay.requested = None;
            self.client.request_redraw(overlay.surface)?;
            return Ok(());
        }
        let resumed = self.orphan.take().filter(|s| s.continues_with(&apps));
        self.close_overlay()?;
        let geometry = self
            .client
            .outputs()
            .into_iter()
            .find(|o| o.id == output)
            .and_then(|o| o.geometry)
            .ok_or("the output of the switcher went away")?;
        let layout = SwitcherLayout::new(
            apps.len(),
            f64::from(geometry.width),
            f64::from(geometry.height),
        );
        let switcher = match resumed {
            Some(mut switcher) => {
                switcher.select(selected)?;
                switcher
            }
            None => Switcher::new(apps, selected, self.now())?,
        };
        let surface = self.client.layer_surface(&LayerSurfaceConfig {
            namespace: SWITCHER_NAMESPACE.to_string(),
            output: Some(output),
            layer: Layer::Overlay,
            anchor: Anchor::empty(),
            size: (layout.panel.w as u32, layout.panel.h as u32),
            margin: Margin::default(),
            // -1: the surface ignores the zones of the dock bar and the menu bar and is centred on the output
            exclusive_zone: -1,
            // the server keeps the keys of the switch
            keyboard: KeyboardInteractivity::None,
        })?;
        let backdrop = self.client.backdrop(surface)?;
        backdrop.set_material(switcher::MATERIAL);
        self.overlay = Some(Overlay {
            output,
            switcher,
            layout,
            surface,
            backdrop,
            hover: HoverGate::default(),
            requested: None,
            draw_requested: false,
        });
        Ok(())
    }

    /// The switch ended. A panel that was on screen fades out after a commit; a cancel, or a
    /// commit before the panel appeared, removes it at once.
    fn switcher_closed(&mut self, committed: bool) -> Result<()> {
        let now = self.now();
        self.orphan = None;
        let Some(overlay) = self.overlay.as_mut() else {
            return Ok(());
        };
        if committed && overlay.switcher.is_shown(now) {
            overlay.switcher.close(now);
            self.client.request_redraw(overlay.surface)?;
            Ok(())
        } else {
            self.close_overlay()
        }
    }

    /// The server closed the overlay surface: the switch it showed waits in `orphan` for its next step.
    fn lose_overlay(&mut self) -> Result<()> {
        if let Some(Overlay {
            switcher,
            surface,
            backdrop,
            ..
        }) = self.overlay.take()
        {
            drop(backdrop);
            self.client.destroy_surface(surface)?;
            self.orphan = Some(switcher);
        }
        Ok(())
    }

    /// Remove the overlay surface with its backdrop, if any.
    fn close_overlay(&mut self) -> Result<()> {
        if let Some(Overlay {
            surface, backdrop, ..
        }) = self.overlay.take()
        {
            drop(backdrop);
            self.client.destroy_surface(surface)?;
        }
        Ok(())
    }

    /// How long the loop may sleep: until the switcher's panel is due or the grid's wheel burst
    /// settles, whichever comes first; without either, until something happens.
    fn until_due(&self) -> Option<Duration> {
        let now = self.now();
        let panel = self
            .overlay
            .as_ref()
            .filter(|o| !o.draw_requested)
            .map(|o| o.switcher.until_shown(now));
        let wheel = self.grid.as_ref().and_then(|g| g.grid.until_settled(now));
        let hold = self.press.as_ref().and_then(|p| p.hold_due(now));
        panel
            .into_iter()
            .chain(wheel)
            .chain(hold)
            .reduce(f64::min)
            .map(Duration::from_secs_f64)
    }

    fn is_force_quit(&self, surface: SurfaceId) -> bool {
        self.force_quit
            .as_ref()
            .is_some_and(|f| f.surface == surface)
    }

    /// The running applications for the dialog: the dock's running application tiles, with the unresponsive ones marked.
    fn force_quit_entries(&self) -> Vec<FqEntry> {
        self.session
            .dock
            .tiles()
            .into_iter()
            .filter(|t| t.kind == TileKind::App && t.running)
            .map(|t| FqEntry {
                app_id: t.key.clone(),
                name: self
                    .session
                    .desktop()
                    .find(&t.key)
                    .map(|e| e.name.clone())
                    .unwrap_or_else(|| {
                        if t.title.is_empty() {
                            t.key.clone()
                        } else {
                            t.title.clone()
                        }
                    }),
                unresponsive: self.unresponsive.contains(&t.key),
                built_in: t.built_in,
            })
            .collect()
    }

    /// The Force Quit shortcut: open the dialog, or close it when it is open.
    fn toggle_force_quit(&mut self) -> Result<()> {
        if self.force_quit.is_some() {
            return self.close_force_quit();
        }
        self.open_force_quit(None)
    }

    /// Open the dialog, on the second step for `ask_about` (the dock menu's Force Quit) when that is given.
    fn open_force_quit(&mut self, ask_about: Option<&str>) -> Result<()> {
        if self.force_quit.is_some() {
            self.close_force_quit()?;
        }
        let entries = self.force_quit_entries();
        let dialog = match ask_about {
            Some(app_id) => Dialog::asking_about(entries, app_id),
            None => Dialog::new(entries),
        };
        let layout = force_quit::Layout::for_dialog(&dialog);
        let (width, height) = layout.size();
        let surface = self.client.layer_surface(&LayerSurfaceConfig {
            namespace: FORCE_QUIT_NAMESPACE.to_string(),
            output: Some(self.output),
            layer: Layer::Overlay,
            // no anchor: the server centres the surface on the output
            anchor: Anchor::empty(),
            size: (width as u32, height as u32),
            margin: Margin::default(),
            exclusive_zone: -1,
            keyboard: KeyboardInteractivity::Exclusive,
        })?;
        self.force_quit = Some(ForceQuit {
            dialog,
            surface,
            layout,
            hover_row: None,
            hover_button: None,
        });
        Ok(())
    }

    fn close_force_quit(&mut self) -> Result<()> {
        if let Some(open) = self.force_quit.take() {
            self.client.destroy_surface(open.surface)?;
        }
        Ok(())
    }

    /// The applications or their answers changed: the open dialog follows.
    fn refresh_force_quit(&mut self) -> Result<()> {
        let entries = self.force_quit_entries();
        let Some(open) = self.force_quit.as_mut() else {
            return Ok(());
        };
        open.dialog.set_entries(entries);
        self.relayout_force_quit()
    }

    /// Lay the open dialog out again for its step and rows, and resize its surface when the size changed.
    fn relayout_force_quit(&mut self) -> Result<()> {
        let Some(open) = self.force_quit.as_mut() else {
            return Ok(());
        };
        let layout = force_quit::Layout::for_dialog(&open.dialog);
        let resized = layout.size() != open.layout.size();
        open.layout = layout;
        let surface = open.surface;
        if resized {
            let (width, height) = open.layout.size();
            self.client.set_layer_placement(
                surface,
                Anchor::empty(),
                (width as u32, height as u32),
            )?;
        }
        self.client.request_redraw(surface)?;
        Ok(())
    }

    fn draw_force_quit(&mut self) -> Result<()> {
        let Some(open) = self.force_quit.as_ref() else {
            return Ok(());
        };
        let view = ForceQuitView {
            dialog: &open.dialog,
            layout: &open.layout,
            desktop: self.session.desktop(),
            dark: self.dark,
            hover_row: open.hover_row,
            hover_button: open.hover_button,
        };
        let (icons, labels) = (&mut self.icons, &mut self.labels);
        self.client.draw(open.surface, |pixmap, scale| {
            render::paint_force_quit(pixmap, scale.factor(), &view, icons, labels)
        })?;
        Ok(())
    }

    /// What a key or a click did to the dialog.
    fn force_quit_outcome(&mut self, outcome: force_quit::Outcome) -> Result<()> {
        match outcome {
            force_quit::Outcome::Nothing => Ok(()),
            // the second step has another size and other parts
            force_quit::Outcome::Redraw => self.relayout_force_quit(),
            force_quit::Outcome::Close => self.close_force_quit(),
            force_quit::Outcome::Force(app_ids) => {
                for app_id in &app_ids {
                    self.role.force_quit_app(app_id);
                }
                self.close_force_quit()
            }
        }
    }

    fn force_quit_pointer(&mut self, event: PointerEvent) -> Result<()> {
        let mods = self.client.modifiers();
        let Some(open) = self.force_quit.as_mut() else {
            return Ok(());
        };
        let at = |position: cupertino_geometry::Point| Point::new(position.x, position.y);
        match event {
            PointerEvent::Motion(position) | PointerEvent::Enter(position) => {
                let p = at(position);
                let hover = (open.layout.row_at(p), open.layout.button_at(p));
                if hover != (open.hover_row, open.hover_button) {
                    (open.hover_row, open.hover_button) = hover;
                    let surface = open.surface;
                    self.client.request_redraw(surface)?;
                }
                Ok(())
            }
            PointerEvent::Leave => {
                if open.hover_row.take().is_some() | open.hover_button.take().is_some() {
                    let surface = open.surface;
                    self.client.request_redraw(surface)?;
                }
                Ok(())
            }
            PointerEvent::Button {
                button: Button::Left,
                pressed: true,
                position,
            } => {
                let p = at(position);
                let confirming = open.dialog.confirming().is_some();
                let outcome = match (open.layout.button_at(p), confirming) {
                    (Some(FqButton::Cancel), true) => open.dialog.cancel_pressed(),
                    (Some(FqButton::ForceQuit), true) => open.dialog.confirm_pressed(),
                    (Some(FqButton::ForceQuit), false) => open.dialog.force_quit_pressed(),
                    _ => match open.layout.row_at(p) {
                        Some(i) => open.dialog.select(
                            i,
                            if mods.shift {
                                Extend::Range
                            } else if mods.ctrl || mods.logo {
                                Extend::Toggle
                            } else {
                                Extend::Replace
                            },
                        ),
                        None => force_quit::Outcome::Nothing,
                    },
                };
                self.force_quit_outcome(outcome)
            }
            PointerEvent::Scroll { dy, .. } => {
                let rows = (dy / force_quit::ROW_HEIGHT).round() as isize;
                let rows = if rows == 0 {
                    dy.signum() as isize
                } else {
                    rows
                };
                let outcome = open.dialog.scroll(rows);
                self.force_quit_outcome(outcome)
            }
            PointerEvent::Button { .. } => Ok(()),
        }
    }

    fn is_grid(&self, surface: SurfaceId) -> bool {
        self.grid.as_ref().is_some_and(|g| g.surface == surface)
    }

    /// The app-grid action: open the grid, or start closing it when it is open. An action during
    /// the exit animation does nothing.
    fn toggle_app_grid(&mut self) -> Result<()> {
        match &self.grid {
            Some(grid) if grid.grid.is_closing() => Ok(()),
            Some(_) => self.close_grid(),
            None => self.open_grid(),
        }
    }

    /// Create the grid's surface over the usable area of the dock's output, with the keyboard.
    fn open_grid(&mut self) -> Result<()> {
        let mut grid = AppGrid::from_desktop(self.session.desktop(), self.now());
        grid.set_reduce_motion(self.client.display_options().reduce_motion);
        let surface = self.client.layer_surface(&LayerSurfaceConfig {
            namespace: app_grid::NAMESPACE.to_string(),
            output: Some(self.output),
            layer: Layer::Overlay,
            anchor: Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right,
            // the server fills both axes within the exclusive zones, so the dock bar stays visible; the top band keeps the
            // menu bar visible, whose reservation is not a layer zone
            size: (0, 0),
            margin: Margin {
                top: app_grid::TOP_BAND as i32,
                right: 0,
                bottom: 0,
                left: 0,
            },
            exclusive_zone: 0,
            keyboard: KeyboardInteractivity::Exclusive,
        })?;
        let backdrop = self.client.backdrop(surface)?;
        backdrop.set_material(app_grid::MATERIAL);
        self.grid = Some(Grid {
            grid,
            surface,
            backdrop,
            layout: None,
        });
        Ok(())
    }

    /// Start the grid's exit animation and give the keyboard back at once.
    fn close_grid(&mut self) -> Result<()> {
        let now = self.now();
        let Some(grid) = self.grid.as_mut().filter(|g| !g.grid.is_closing()) else {
            return Ok(());
        };
        grid.grid.close(now);
        self.client
            .set_keyboard_interactivity(grid.surface, KeyboardInteractivity::None)?;
        self.client.request_redraw(grid.surface)?;
        Ok(())
    }

    /// Remove the grid's surface with its backdrop, if any.
    fn drop_grid(&mut self) -> Result<()> {
        if let Some(Grid {
            surface, backdrop, ..
        }) = self.grid.take()
        {
            drop(backdrop);
            self.client.destroy_surface(surface)?;
        }
        Ok(())
    }

    /// Carry out what a key or click on the grid asked for: a launch goes through the tiles' path
    /// and closes the grid.
    fn grid_outcome(&mut self, outcome: Option<Outcome>) -> Result<()> {
        match outcome {
            Some(Outcome::Launch(key)) => {
                self.begin_launch(key)?;
                self.close_grid()?;
            }
            Some(Outcome::Close) => self.close_grid()?,
            None => {}
        }
        if let Some(grid) = &self.grid {
            self.client.request_redraw(grid.surface)?;
        }
        Ok(())
    }

    /// A press on the grid launches, flips or closes; the wheel accumulates towards a flip.
    fn grid_pointer(&mut self, event: PointerEvent) -> Result<()> {
        let now = self.now();
        let Some(grid) = self.grid.as_mut() else {
            return Ok(());
        };
        match event {
            PointerEvent::Button {
                button: Button::Left,
                pressed: true,
                position,
            } => {
                let Some(layout) = grid.layout.as_ref() else {
                    return Ok(());
                };
                let outcome = grid
                    .grid
                    .click(Point::new(position.x, position.y), layout, now);
                self.grid_outcome(outcome)
            }
            PointerEvent::Scroll { dx, dy, .. } => {
                grid.grid.scroll(dx, dy, now);
                Ok(())
            }
            PointerEvent::Enter(_)
            | PointerEvent::Motion(_)
            | PointerEvent::Leave
            | PointerEvent::Button { .. } => Ok(()),
        }
    }

    /// A wheel burst that is 75 ms old flips a page.
    fn settle_grid_wheel(&mut self) -> Result<()> {
        let now = self.now();
        if let Some(grid) = self.grid.as_mut() {
            if grid.grid.settle(now) {
                self.client.request_redraw(grid.surface)?;
            }
        }
        Ok(())
    }

    /// One frame of the grid; a finished exit removes the surface. Nothing before the server sized it.
    fn draw_grid(&mut self) -> Result<()> {
        let now = self.now();
        let Some(grid) = self.grid.as_mut() else {
            return Ok(());
        };
        let Some(layout) = grid.layout else {
            return Ok(());
        };
        grid.grid.tick(now);
        if grid.grid.is_gone(now) {
            return self.drop_grid();
        }
        let alpha = grid.grid.visual(now).alpha;
        grid.backdrop
            .set_shape(&GRect::new(0.0, 0.0, layout.width, layout.height), 0.0)?;
        grid.backdrop.set_alpha(alpha)?;
        let view = GridView {
            grid: &grid.grid,
            layout: &layout,
            now,
        };
        let (icons, labels) = (&mut self.icons, &mut self.labels);
        self.client.draw(grid.surface, |pixmap, scale| {
            render::paint_app_grid(pixmap, scale.factor(), &view, icons, labels)
        })?;
        if grid.grid.animating(now) {
            self.client.request_redraw(grid.surface)?;
        }
        Ok(())
    }

    /// Ask for the first frame of the panel once its delay has passed.
    fn show_panel_when_due(&mut self) -> Result<()> {
        let now = self.now();
        if let Some(overlay) = self
            .overlay
            .as_mut()
            .filter(|o| !o.draw_requested && o.switcher.is_shown(now))
        {
            overlay.draw_requested = true;
            self.client.request_redraw(overlay.surface)?;
        }
        Ok(())
    }

    /// Hovering over a tile (once the pointer left its first position) or pressing it selects
    /// that entry; the server answers with the new selection. Releasing the left button on a tile
    /// switches to it, anywhere else on the panel ends the switch.
    fn overlay_pointer(&mut self, event: PointerEvent) -> Result<()> {
        let Some(overlay) = self.overlay.as_mut().filter(|o| !o.switcher.is_fading()) else {
            return Ok(());
        };
        let (position, pressed) = match event {
            PointerEvent::Enter(p) | PointerEvent::Motion(p) => (p, false),
            PointerEvent::Button {
                button: Button::Left,
                pressed: true,
                position,
            } => (position, true),
            PointerEvent::Button {
                button: Button::Left,
                pressed: false,
                position,
            } => {
                let at = Point::new(
                    position.x + overlay.layout.panel.x,
                    position.y + overlay.layout.panel.y,
                );
                match overlay.layout.hit(at, overlay.switcher.apps().len()) {
                    Some(entry) => self.role.switcher_activate(entry)?,
                    None => self.role.switcher_cancel(),
                }
                return Ok(());
            }
            PointerEvent::Leave | PointerEvent::Button { .. } | PointerEvent::Scroll { .. } => {
                return Ok(())
            }
        };
        // the surface covers only the panel: its origin is the panel's in output points
        let at = Point::new(
            position.x + overlay.layout.panel.x,
            position.y + overlay.layout.panel.y,
        );
        let armed = overlay.hover.moved(at);
        let Some(entry) = overlay.layout.hit(at, overlay.switcher.apps().len()) else {
            return Ok(());
        };
        if (pressed || armed)
            && entry != overlay.switcher.selected()
            && overlay.requested != Some(entry)
        {
            overlay.requested = Some(entry);
            self.role.switcher_select(entry)?;
        }
        Ok(())
    }

    /// One frame of the panel: nothing before its delay, then the content at the fade's opacity;
    /// a finished fade removes the surface.
    fn draw_overlay(&mut self) -> Result<()> {
        let now = self.now();
        let Some(overlay) = self.overlay.as_mut() else {
            return Ok(());
        };
        if overlay.switcher.is_gone(now) {
            return self.close_overlay();
        }
        if !overlay.switcher.is_shown(now) {
            return Ok(());
        }
        let alpha = overlay.switcher.alpha(now);
        let size = GRect::new(0.0, 0.0, overlay.layout.panel.w, overlay.layout.panel.h);
        overlay.backdrop.set_shape(&size, SwitcherLayout::RADIUS)?;
        overlay.backdrop.set_alpha(alpha)?;
        let panel = Panel {
            switcher: &overlay.switcher,
            layout: &overlay.layout,
            desktop: self.session.desktop(),
            dark: self.dark,
            alpha,
        };
        let (icons, labels) = (&mut self.icons, &mut self.labels);
        self.client.draw(overlay.surface, |pixmap, scale| {
            render::paint_switcher(pixmap, scale.factor(), &panel, icons, labels)
        })?;
        if overlay.switcher.is_fading() {
            self.client.request_redraw(overlay.surface)?;
        }
        Ok(())
    }

    /// One frame: advance the model, publish what the server needs (input region, minimize
    /// target, backdrop) and draw the tiles.
    fn draw(&mut self) -> Result<()> {
        let now = self.now();
        let screen = self.screen()?;
        let dock = &mut self.session.dock;
        dock.pointer_moved(self.pointer, screen, now);
        dock.tick(now);
        let layout = dock.layout(screen, now);
        let input: Vec<GRect> = dock
            .input_rects(screen, now)
            .iter()
            .map(geometry_rect)
            .collect();
        self.client.set_input_region(self.surface, &input)?;
        self.role
            .set_minimize_target(&geometry_rect(&dock.next_minimized_slot(screen)))?;
        let bar = layout
            .bar
            .intersection(&screen)
            .map_or(GRect::new(0.0, 0.0, 0.0, 0.0), |r| geometry_rect(&r));
        self.backdrop
            .set_shape(&bar, dock.cfg.tile_size * BAR_RADIUS)?;
        let poof = self
            .poof
            .and_then(|p| p.progress(now).map(|progress| (p.centre, progress)));
        if poof.is_none() {
            self.poof = None;
        }
        let wants_frame = dock.wants_tick(now) || poof.is_some();
        let (session, icons, dark) = (&self.session, &mut self.icons, self.dark);
        self.client.draw(self.surface, |pixmap, scale| {
            render::paint(pixmap, scale.factor(), session, &layout, icons, dark);
        })?;
        let held = layout.dragging.is_some() || poof.is_some();
        self.sync_drag_surface(held)?;
        // frame callbacks pace the animations
        if wants_frame {
            self.client.request_redraw(self.surface)?;
        }
        Ok(())
    }

    /// The surface of the held, settling or fading tile exists exactly while there is one.
    fn sync_drag_surface(&mut self, needed: bool) -> Result<()> {
        match (needed, self.drag_surface) {
            (true, None) => {
                let surface = self.client.layer_surface(&LayerSurfaceConfig {
                    namespace: DRAG_NAMESPACE.to_string(),
                    output: Some(self.output),
                    layer: Layer::Overlay,
                    anchor: Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right,
                    size: (0, 0),
                    margin: Margin::default(),
                    // -1: the whole output, so the surface's coordinates are the output's
                    exclusive_zone: -1,
                    keyboard: KeyboardInteractivity::None,
                })?;
                self.client.set_input_region(surface, &[])?;
                self.client.request_redraw(surface)?;
                self.drag_surface = Some(surface);
            }
            (true, Some(surface)) => self.client.request_redraw(surface)?,
            (false, Some(surface)) => {
                self.client.destroy_surface(surface)?;
                self.drag_surface = None;
            }
            (false, None) => {}
        }
        Ok(())
    }

    fn is_drag_surface(&self, surface: SurfaceId) -> bool {
        self.drag_surface == Some(surface)
    }

    /// One frame of the drag surface: the held tile with its label, and the puff of a removal.
    fn draw_dragged(&mut self) -> Result<()> {
        let Some(surface) = self.drag_surface else {
            return Ok(());
        };
        let now = self.now();
        let screen = self.screen()?;
        let in_surface = self.screen_in_surface()?;
        // the bar's surface sits at (-x, -y) of the output
        let origin = Point::new(-in_surface.x, -in_surface.y);
        let layout = self.session.dock.layout(screen, now);
        let poof = self
            .poof
            .and_then(|p| p.progress(now).map(|progress| (p.centre, progress)));
        let tile_size = self.session.dock.cfg.tile_size;
        let (session, icons, labels, dark) =
            (&self.session, &mut self.icons, &mut self.labels, self.dark);
        self.client.draw(surface, |pixmap, scale| {
            if let Some(dragged) = &layout.dragging {
                render::paint_dragged(
                    pixmap,
                    scale.factor(),
                    session,
                    icons,
                    labels,
                    dragged,
                    origin,
                    dark,
                );
            }
            if let Some((centre, progress)) = poof {
                render::paint_poof(
                    pixmap,
                    scale.factor(),
                    Point::new(centre.x + origin.x, centre.y + origin.y),
                    tile_size,
                    progress,
                    dark,
                );
            }
        })?;
        Ok(())
    }

    // ---- overview ----------------------------------------------------------------------------

    fn is_overview(&self, surface: SurfaceId) -> bool {
        self.overview.as_ref().is_some_and(|o| o.surface == surface)
    }

    /// The window feed's title and minimized state for the overview's windows.
    fn window_entries(&self, windows: &[OverviewWindow]) -> Vec<WindowEntry> {
        windows
            .iter()
            .map(|w| {
                let info = self.session.window_info(w.id);
                WindowEntry {
                    id: w.id,
                    frame: Rect::new(w.frame.x(), w.frame.y(), w.frame.width(), w.frame.height()),
                    title: info.map(|i| i.title.clone()).unwrap_or_default(),
                    minimized: info.is_some_and(|i| i.minimized),
                }
            })
            .collect()
    }

    /// The server opened the overview or changed its contents: lay the windows out, tell the
    /// server their frames, and show the chrome (creating its surface when the overview opens).
    fn overview_changed(
        &mut self,
        mode: OverviewMode,
        area: GRect,
        windows: Vec<OverviewWindow>,
        workspaces: Vec<OverviewWorkspace>,
    ) -> Result<()> {
        let now = self.now();
        let entries = self.window_entries(&windows);
        let workspaces: Vec<Workspace> = workspaces
            .iter()
            .map(|w| Workspace {
                id: w.id,
                current: w.current,
            })
            .collect();
        let mode = match mode {
            OverviewMode::All => Mode::All,
            OverviewMode::App(_) => Mode::App,
        };
        match self
            .overview
            .as_mut()
            .filter(|o| !o.ui.is_closing() && o.ui.mode() == mode)
        {
            Some(open) => {
                open.ui.set_windows(&entries, now);
                open.ui.set_workspaces(workspaces);
            }
            None => {
                self.drop_overview()?;
                let area = Rect::new(area.x(), area.y(), area.width(), area.height());
                let ui = OverviewUi::new(mode, area, &entries, workspaces, now);
                let surface = self.client.layer_surface(&LayerSurfaceConfig {
                    namespace: overview_ui::NAMESPACE.to_string(),
                    output: Some(self.output),
                    layer: Layer::Overlay,
                    anchor: Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right,
                    size: (0, 0),
                    margin: Margin::default(),
                    // -1: the whole output, over the dock bar and the menu bar, so the surface's
                    // coordinates are the output's (the area the server sent is in them)
                    exclusive_zone: -1,
                    keyboard: KeyboardInteractivity::Exclusive,
                })?;
                self.overview = Some(OverviewSurface { ui, surface });
            }
        }
        let open = self.overview.as_ref().expect("set above");
        for (id, frame) in open.ui.frames() {
            self.role.overview_frame(id, &geometry_rect(&frame))?;
        }
        self.client.request_redraw(open.surface)?;
        Ok(())
    }

    /// The server is leaving the overview: the chrome fades and gives the keyboard back.
    fn overview_closed(&mut self) -> Result<()> {
        let now = self.now();
        let Some(open) = self.overview.as_mut().filter(|o| !o.ui.is_closing()) else {
            return Ok(());
        };
        open.ui.close(now);
        self.client
            .set_keyboard_interactivity(open.surface, KeyboardInteractivity::None)?;
        self.client.request_redraw(open.surface)?;
        Ok(())
    }

    /// Remove the overview's surface, if any.
    fn drop_overview(&mut self) -> Result<()> {
        if let Some(OverviewSurface { surface, .. }) = self.overview.take() {
            self.client.destroy_surface(surface)?;
        }
        Ok(())
    }

    /// Carry out what the pointer or a key asked for: every action is the server's to apply.
    fn overview_action(&mut self, action: UiAction) -> Result<()> {
        match action {
            UiAction::None => {}
            UiAction::Select(id) => self.role.overview_select(id),
            UiAction::Close => self.role.overview_close(),
            UiAction::SwitchWorkspace(id) => self.role.switch_workspace(id),
            UiAction::AddWorkspace => self.role.add_workspace(),
            UiAction::RemoveWorkspace(id) => self.role.remove_workspace(id),
            UiAction::MoveWorkspace { id, index } => self.role.move_workspace(id, index)?,
        }
        if let Some(open) = &self.overview {
            self.client.request_redraw(open.surface)?;
        }
        Ok(())
    }

    fn overview_pointer(&mut self, event: PointerEvent) -> Result<()> {
        let now = self.now();
        let Some(open) = self.overview.as_mut() else {
            return Ok(());
        };
        let action = match event {
            PointerEvent::Enter(p) | PointerEvent::Motion(p) => {
                open.ui.pointer_moved(Some(Point::new(p.x, p.y)), now);
                UiAction::None
            }
            PointerEvent::Leave => {
                open.ui.pointer_moved(None, now);
                UiAction::None
            }
            PointerEvent::Button {
                button: Button::Left,
                pressed: true,
                position,
            } => open.ui.press(Point::new(position.x, position.y), now),
            PointerEvent::Button {
                button: Button::Left,
                pressed: false,
                position,
            } => open.ui.release(Point::new(position.x, position.y), now),
            PointerEvent::Button { .. } | PointerEvent::Scroll { .. } => UiAction::None,
        };
        self.overview_action(action)
    }

    /// One frame of the chrome; a finished fade removes the surface.
    fn draw_overview(&mut self) -> Result<()> {
        let now = self.now();
        let Some(open) = self.overview.as_ref() else {
            return Ok(());
        };
        if open.ui.is_gone(now) {
            return self.drop_overview();
        }
        let view = OverviewView {
            ui: &open.ui,
            now,
            dark: self.dark,
        };
        let labels = &mut self.labels;
        self.client.draw(open.surface, |pixmap, scale| {
            render::paint_overview(pixmap, scale.factor(), &view, labels)
        })?;
        if open.ui.is_animating(now) {
            self.client.request_redraw(open.surface)?;
        }
        Ok(())
    }

    // ---- hot corners and gestures ---------------------------------------------------------------

    /// A corner or gesture the server reported: run what `dock.conf` maps it to.
    fn run_trigger(&mut self, trigger: Trigger) -> Result<()> {
        match self.actions.get(trigger).clone() {
            ActionTarget::None => Ok(()),
            ActionTarget::AppGrid => self.toggle_app_grid(),
            ActionTarget::Exec(command_line) => {
                let mut command = Command::new("sh");
                command.args(["-c", &command_line]);
                spawn(command);
                Ok(())
            }
            ActionTarget::Server(name) => {
                self.role.run_action(&name);
                Ok(())
            }
        }
    }

    // ---- context menus ----------------------------------------------------------------------------

    fn is_menu(&self, surface: SurfaceId) -> bool {
        self.menu.as_ref().is_some_and(|m| m.surface == surface)
    }

    /// The output in the bar surface's coordinates: the popup stays inside it.
    fn screen_in_surface(&mut self) -> Result<Rect> {
        let (sw, sh) = self.client.size(self.surface)?;
        let out = self
            .client
            .outputs()
            .into_iter()
            .find(|o| o.id == self.output)
            .and_then(|o| o.geometry)
            .ok_or("the output of the dock went away")?;
        let (w, h) = (f64::from(out.width), f64::from(out.height));
        let (x, y) = match self.session.dock.cfg.orientation {
            Orientation::Bottom => (0.0, f64::from(sh) - h),
            Orientation::Left => (0.0, 0.0),
            Orientation::Right => (f64::from(sw) - w, 0.0),
        };
        Ok(Rect::new(x, y, w, h))
    }

    /// A right click on the bar: the menu of the tile or the separator under it, as a popup.
    fn open_menu(&mut self, position: Point) -> Result<()> {
        self.drop_menu()?;
        let now = self.now();
        let screen = self.screen()?;
        let Some(hit) = self.session.dock.hit_test(position, screen, now) else {
            return Ok(());
        };
        let items = match self.session.dock.tile(hit.id) {
            Some(tile) if tile.kind == TileKind::Separator => {
                menu::separator_menu(&self.session.dock.cfg)
            }
            Some(tile) => {
                let entry_path = self
                    .session
                    .desktop()
                    .find(&tile.key)
                    .map(|e| e.path.as_path());
                let running_app_id = self.session.running_app_id(hit.id);
                menu::tile_menu(&TileFacts {
                    kind: tile.kind,
                    key: &tile.key,
                    pinned: tile.pinned,
                    built_in: tile.built_in,
                    running_app_id,
                    unresponsive: running_app_id.is_some_and(|app| self.unresponsive.contains(app)),
                    entry_path,
                })
            }
            None => return Ok(()),
        };
        if items.is_empty() {
            return Ok(());
        }
        let anchor = self
            .session
            .dock
            .layout(screen, now)
            .tiles
            .into_iter()
            .find(|t| t.id == hit.id)
            .expect("a hit tile is in the layout it was hit in")
            .rect;
        let widths: HashMap<String, f64> = items
            .iter()
            .filter(|i| !i.separator)
            .map(|i| (i.label.clone(), self.labels.width(&i.label, menu::FONT_PT)))
            .collect();
        let output = self.screen_in_surface()?;
        let layout = menu::layout(
            &items,
            &anchor,
            &output,
            self.session.dock.cfg.orientation,
            &|label| widths[label],
        );
        let surface = self
            .client
            .popup(self.surface, &geometry_rect(&layout.rect))?;
        let backdrop = self.client.backdrop(surface)?;
        backdrop.set_material(MENU_MATERIAL);
        backdrop.set_shape(
            &GRect::new(0.0, 0.0, layout.rect.w, layout.rect.h),
            menu::RADIUS,
        )?;
        self.menu = Some(Menu {
            items,
            layout,
            surface,
            backdrop,
            hovered: None,
        });
        Ok(())
    }

    /// Remove the menu's popup with its backdrop, if any.
    fn drop_menu(&mut self) -> Result<()> {
        if let Some(Menu {
            surface, backdrop, ..
        }) = self.menu.take()
        {
            drop(backdrop);
            self.client.destroy_surface(surface)?;
        }
        Ok(())
    }

    /// Hovering highlights a row; a left press on a row runs its command and closes the menu.
    fn menu_pointer(&mut self, event: PointerEvent) -> Result<()> {
        let Some(menu) = self.menu.as_mut() else {
            return Ok(());
        };
        let at = |p: cupertino_geometry::Point| {
            Point::new(p.x + menu.layout.rect.x, p.y + menu.layout.rect.y)
        };
        match event {
            PointerEvent::Enter(p) | PointerEvent::Motion(p) => {
                menu.hovered = menu.layout.hit(&menu.items, at(p));
                self.client.request_redraw(menu.surface)?;
            }
            PointerEvent::Leave => {
                menu.hovered = None;
                self.client.request_redraw(menu.surface)?;
            }
            PointerEvent::Button {
                button: Button::Left,
                pressed: true,
                position,
            } => {
                let picked = menu
                    .layout
                    .hit(&menu.items, at(position))
                    .and_then(|i| menu.items[i].command.clone());
                self.drop_menu()?;
                if let Some(command) = picked {
                    self.run_menu_command(command)?;
                }
            }
            PointerEvent::Button { .. } | PointerEvent::Scroll { .. } => {}
        }
        Ok(())
    }

    fn run_menu_command(&mut self, command: MenuCommand) -> Result<()> {
        let now = self.now();
        match command {
            MenuCommand::KeepInDock(key) => {
                self.session.dock.pin(&key, None, now);
                self.store_pins();
            }
            MenuCommand::RemoveFromDock(key) => {
                self.session.dock.unpin(&key, now);
                self.store_pins();
            }
            MenuCommand::ShowInFileManager(dir) => {
                self.open_in_file_manager(&dir.to_string_lossy())?
            }
            MenuCommand::ShowAllWindows(app) => {
                self.role.app_command(&app, AppCommand::ShowWindows)
            }
            MenuCommand::Hide(app) => self.role.app_command(&app, AppCommand::Hide),
            MenuCommand::Quit(app) => self.role.app_command(&app, AppCommand::Quit),
            MenuCommand::ForceQuit(app) => self.open_force_quit(Some(&app))?,
            MenuCommand::Open(key) => self.begin_launch(key)?,
            MenuCommand::OpenTrash => self.open_in_file_manager(TRASH_URI)?,
            MenuCommand::ToggleAutoHide => {
                self.session.dock.toggle_autohide(now);
                self.client.set_exclusive_zone(
                    self.surface,
                    self.session.dock.exclusive_zone().ceil() as i32,
                )?;
            }
            MenuCommand::ToggleMagnification => {
                let placed = (
                    self.session.dock.cfg.orientation,
                    self.session.dock.surface_extent(),
                );
                self.session.dock.cfg.magnification = !self.session.dock.cfg.magnification;
                // the magnified tiles need the deeper surface
                self.sync_placement(placed)?;
            }
            // the items are disabled: the position is the file's
            MenuCommand::Position(_) => {}
        }
        self.client.request_redraw(self.surface)?;
        Ok(())
    }

    fn draw_menu(&mut self) -> Result<()> {
        let Some(menu) = self.menu.as_ref() else {
            return Ok(());
        };
        let (items, layout, hovered, dark, labels) = (
            &menu.items,
            &menu.layout,
            menu.hovered,
            self.dark,
            &mut self.labels,
        );
        self.client.draw(menu.surface, |pixmap, scale| {
            render::paint_menu(pixmap, scale.factor(), items, layout, hovered, dark, labels)
        })?;
        Ok(())
    }
}

/// The application ids that the screenshot fixtures declare unresponsive.
#[cfg(cupertino_capture_fixtures)]
fn fixture_unresponsive() -> std::collections::HashSet<String> {
    std::env::var("CUPERTINO_DOCK_FIXTURE_UNRESPONSIVE")
        .map(|ids| ids.split(',').map(str::to_owned).collect())
        .unwrap_or_default()
}

#[cfg(not(cupertino_capture_fixtures))]
fn fixture_unresponsive() -> std::collections::HashSet<String> {
    std::collections::HashSet::new()
}
