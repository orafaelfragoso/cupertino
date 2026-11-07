//! The dock as a CupertinoKit application: the model around [`Session`] and the surfaces that come and go (the context
//! menu, the held tile, the application grid, the Force Quit dialog, the switcher's panel and the overview's chrome),
//! what each message does to it, and its windows ([`view`]). What needs the session (the dock role's requests, the
//! layer surfaces' placement and keyboard, starting programs, the pins file, icons and the two surfaces the process
//! still paints itself) goes through [`Host`], so the model runs headless in tests.
//!
//! The dock is on one output at a time: the first output when it starts (the main display), and the first remaining
//! one when its output goes away. Every other output has an edge strip along the dock's edge; the pointer resting
//! against that edge for [`EDGE_DWELL`] seconds moves the dock there. Each strip also holds the dock role of its output,
//! so the hot corners, gestures, the overview and the switcher of every output reach the process; the window feed is
//! taken from the bar's role only.

use std::collections::{HashMap, HashSet};

use cupertino_defaults::dock::DockSettings;
use cupertino_kit::controls::ImageSource;
use cupertino_kit::core::{Command, Window, WindowKey};
use cupertino_kit::geometry::Size;
use cupertino_kit::platform::{KeyEvent, Keysym, Mods};
use cupertino_wm::geometry::{Point, Rect};

use crate::actions::{ActionMap, ActionTarget, Trigger};
use crate::app_grid::{self, AppGrid, GridLayout, Outcome, PER_PAGE};
use crate::conf::{dock_config, FILE_MANAGER};
use crate::config::Orientation;
use crate::desktop::DesktopIndex;
use crate::force_quit::{self, Button, Dialog, Entry, Extend};
use crate::menu::{self, MenuCommand, MenuItem, TileFacts};
use crate::overview_ui::{Mode, OverviewUi, UiAction, WindowEntry, Workspace};
use crate::session::{Activation, Session};
use crate::switcher::{HoverGate, Switcher, SwitcherLayout};
use crate::tile_drag::{Poof, TilePress};
use crate::tiles::{TileId, TileKind, TileLifecycle};
use crate::view::{self, bridge, IconSet, Input, Pointer};
use crate::DragOutcome;

/// How long the pointer rests against another output's edge before the dock moves there, in seconds.
pub const EDGE_DWELL: f64 = 0.4;

/// The interval of the frames that drive an animation, in seconds.
pub const FRAME: f64 = 1.0 / 60.0;

/// How long the trash's state is trusted before the folder is looked at again, in seconds.
pub const TRASH_CHECK_EVERY: f64 = 1.0;

/// Device pixels per point the icons are loaded for, so they stay sharp at every output scale.
pub const ICON_SCALES: [f64; 3] = [1.0, 2.0, 3.0];

/// Side of an icon in the Force Quit dialog's rows and in its question, in points.
const FORCE_QUIT_ROW_ICON: f64 = 16.0;

/// What assistive technology hears about a tile.
pub const TRASH_NAME: &str = "Trash";
pub const RUNNING: &str = "Running";
pub const OPENING: &str = "Opening";
pub const NOT_RESPONDING: &str = "Not responding";
pub const NEEDS_ATTENTION: &str = "Needs attention";
pub const MINIMIZED: &str = "Minimized window";
pub const TRASH_FULL: &str = "Contains items";

/// An output of the compositor, by its key (the `wl_output` registry name), in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OutputInfo {
    pub key: u32,
    pub width: f64,
    pub height: f64,
    /// Device pixels per point.
    pub scale: f64,
}

/// The icon a window set for itself, as the server sent it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowIcon {
    pub id: u32,
    /// A key into the icon theme.
    pub name: Option<String>,
    /// The side of the square image and its premultiplied ARGB8888 pixels, little-endian.
    pub image: Option<(u32, Vec<u8>)>,
}

/// What the server told one dock role.
#[derive(Clone, Debug, PartialEq)]
pub enum DockEvent {
    Window {
        id: u32,
        app_id: String,
        title: String,
        minimized: bool,
    },
    WindowIcon(WindowIcon),
    WindowClosed(u32),
    Launching(String),
    ToggleAutoHide,
    ToggleAppGrid,
    /// The switcher's list and selection, for its panel on the output with key `output`.
    Switcher {
        output: u32,
        apps: Vec<String>,
        selected: usize,
    },
    SwitcherClosed {
        committed: bool,
    },
    /// The overview of the role's output opened or changed: its windows' desktop frames by id, in output points.
    Overview {
        mode: Mode,
        area: Rect,
        windows: Vec<(u32, Rect)>,
        workspaces: Vec<Workspace>,
    },
    OverviewClosed,
    /// A hot corner or a gesture of the role's output.
    Trigger(Trigger),
    Attention(String),
    Unresponsive {
        app_id: String,
        unresponsive: bool,
    },
    ForceQuitDialog,
    /// The Focus Dock shortcut: the bar takes the keyboard.
    Engage,
}

impl DockEvent {
    /// Whether every dock role is told this, so the process takes it from the bar's role alone.
    fn is_shared(&self) -> bool {
        !matches!(
            self,
            DockEvent::Switcher { .. }
                | DockEvent::Overview { .. }
                | DockEvent::OverviewClosed
                | DockEvent::Trigger(_)
                | DockEvent::Engage
        )
    }
}

/// What the process asks the server, through the dock role of one output.
#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    ActivateApp(String),
    /// Restore the minimized window out of its tile, at `tile` in the bar's window.
    RestoreWindow {
        id: u32,
        tile: Rect,
    },
    /// The slot the next minimized window flies to, in the bar's window.
    MinimizeTarget(Rect),
    App(String, AppAction),
    ForceQuitApp(String),
    RunAction(String),
    SwitcherSelect(usize),
    SwitcherActivate(usize),
    SwitcherCancel,
    OverviewFrame(u32, Rect),
    OverviewSelect(u32),
    OverviewClose,
    SwitchWorkspace(u32),
    AddWorkspace,
    RemoveWorkspace(u32),
    MoveWorkspace {
        id: u32,
        index: usize,
    },
}

/// What a tile menu asks of a running application.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppAction {
    ShowWindows,
    Hide,
    Quit,
}

/// Where the bar's layer surface is: its output, its edge, its depth and the area it keeps windows out of.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub output: u32,
    pub orientation: Orientation,
    /// Depth of the surface in points: room for the magnified tiles and the bounces.
    pub extent: f64,
    pub exclusive_zone: i32,
}

/// The layer surfaces whose keyboard interactivity the model changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// The bar on the output with this key, while it is engaged.
    Bar(u32),
    Grid,
    Overview,
}

/// The surfaces whose size the compositor decides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sized {
    Bar(u32),
    Grid,
}

/// An icon the model shows.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum IconKey {
    /// The application of the tile or window with this key: its desktop entry's icon, the one its window set, else
    /// the generic application icon.
    App(String),
    /// A theme icon by name (the application grid's), else the generic application icon.
    Named(String),
    /// The trash, empty or full.
    Trash(bool),
}

/// What the switcher's panel shows, for the host to paint.
pub struct SwitcherPaint<'a> {
    pub switcher: &'a Switcher,
    pub layout: &'a SwitcherLayout,
    pub desktop: &'a DesktopIndex,
    pub dark: bool,
    pub alpha: f64,
}

/// What the model needs the session for.
pub trait Host {
    /// Seconds on the monotonic clock the model is driven with.
    fn now(&self) -> f64;
    /// [`Message::Wake`] with `generation` after `after` seconds.
    fn wake(&mut self, after: f64, generation: u64) -> Vec<Command<Message>>;
    /// Send `request` through the dock role of the output with key `output`.
    fn request(&mut self, output: u32, request: Request);
    /// Put the bar's surface there; a bar on another output takes it when its surface is created.
    fn place_bar(&mut self, placement: Placement);
    /// Give `part` the keyboard (exclusive keyboard interactivity), or let it go.
    fn keyboard(&mut self, part: Part, on: bool);
    /// The Force Quit dialog is now this large.
    fn resize_force_quit(&mut self, width: f64, height: f64);
    /// Start `argv` with the activation token; whether it started.
    fn spawn(&mut self, argv: Vec<String>, token: Option<String>) -> bool;
    /// Open `target` with the session's default handler.
    fn open(&mut self, target: &str);
    /// Run `command` through the shell.
    fn shell(&mut self, command: &str);
    /// Store the pinned applications in their order.
    fn store_pins(&mut self, pins: &[String]);
    /// `dock.conf` read again, `last_good` keeping the keys of lines that are not understood; `None` when it cannot
    /// be read.
    fn read_conf(&mut self, last_good: &DockSettings) -> Option<DockSettings>;
    /// The terminal emulator that runs entries with `Terminal=true`, if one is installed.
    fn terminal(&mut self) -> Option<String>;
    /// The renditions of `icon` at `side` points for each of [`ICON_SCALES`].
    fn icon(&mut self, desktop: &DesktopIndex, icon: &IconKey, side: f64) -> IconSet;
    /// A window set its own icon, or removed it; `apps` are its application id and the dock's key for it.
    fn window_icon(&mut self, apps: [String; 2], icon: WindowIcon);
    fn window_closed(&mut self, id: u32);
    /// Whether the trash holds anything.
    fn trash_full(&mut self) -> bool;
    /// The switcher's panel painted at `scale` device pixels per point.
    fn paint_switcher(&mut self, scale: f64, panel: &SwitcherPaint<'_>) -> Option<ImageSource>;
    /// The overview's chrome over an output `size` points large, painted at `scale` device pixels per point.
    fn paint_overview(
        &mut self,
        scale: f64,
        size: (f64, f64),
        ui: &OverviewUi,
        now: f64,
        dark: bool,
    ) -> Option<ImageSource>;
}

#[derive(Clone, Debug)]
pub enum Message {
    Input(Input),
    /// A group of events of the dock role of the output with key `output`.
    Dock {
        output: u32,
        events: Vec<DockEvent>,
    },
    /// The session's appearance.
    Dark(bool),
    /// The session's Reduce Motion.
    ReduceMotion(bool),
    /// The session was locked or unlocked.
    Locked(bool),
    Outputs(Vec<OutputInfo>),
    /// The compositor sized a surface, in points.
    Sized(Sized, f64, f64),
    /// The bar on the output with this key lost the keyboard.
    KeyboardLeft(u32),
    /// The activation token of the launch waiting as `launch` arrived (`None` when there is none).
    Token(u64, Option<String>),
    Wake(u64),
    /// `dock.conf` changed on disk.
    ConfChanged,
    /// Nothing will tell the model about the session any more: the process ends.
    Unheard,
}

impl Message {
    /// Whether the message came from the session rather than from the windows or a timer.
    pub fn is_heard(&self) -> bool {
        matches!(
            self,
            Message::Dock { .. }
                | Message::Dark(_)
                | Message::ReduceMotion(_)
                | Message::Locked(_)
                | Message::Outputs(_)
                | Message::Sized(..)
                | Message::KeyboardLeft(_)
                | Message::ConfChanged
        )
    }
}

impl From<Input> for Message {
    fn from(input: Input) -> Self {
        Message::Input(input)
    }
}

/// An application waiting for its activation token.
struct Pending {
    /// Key of the tile that bounces once the program started.
    key: String,
    argv: Vec<String>,
}

/// An open context menu.
struct OpenMenu {
    tile: TileId,
    items: Vec<MenuItem>,
    label: String,
}

struct ForceQuit {
    output: u32,
    dialog: Dialog,
    layout: force_quit::Layout,
    hover_row: Option<usize>,
    hover_button: Option<Button>,
}

struct Grid {
    output: u32,
    grid: AppGrid,
    /// Known once the compositor sized the surface.
    layout: Option<GridLayout>,
}

struct SwitcherState {
    output: u32,
    switcher: Switcher,
    layout: SwitcherLayout,
    hover: HoverGate,
    /// Entry the last `switcher_select` asked for, until the server answers.
    requested: Option<usize>,
    image: Option<ImageSource>,
}

struct Overview {
    output: u32,
    ui: OverviewUi,
    image: Option<ImageSource>,
}

pub struct Model {
    pub session: Session,
    host: Box<dyn Host>,
    pub outputs: Vec<OutputInfo>,
    /// The output the bar is on.
    pub output: Option<u32>,
    /// The bar's surface size as the compositor gave it.
    bar_size: Option<(f64, f64)>,
    /// Takes the next value when the compositor closed the bar's surface, so a new one is opened.
    bar_generation: u64,
    /// The bar's role told nothing yet: its first group is the whole window list.
    awaiting_list: bool,
    pub dark: bool,
    pub reduce_motion: bool,
    pub locked: bool,
    pub actions: ActionMap,
    /// `dock.conf` as last read.
    conf: DockSettings,
    /// The pointer over the bar.
    pointer: Option<Point>,
    press: Option<TilePress>,
    poof: Option<Poof>,
    /// When the switcher's and the overview's images were last painted.
    bridges_painted: Option<f64>,
    /// A paint of those images was left to the next frame.
    bridges_owed: bool,
    menu: Option<OpenMenu>,
    /// The bar has the keyboard.
    pub engaged: bool,
    /// The applications that stopped answering (their ids), as the server last said.
    unresponsive: HashSet<String>,
    force_quit: Option<ForceQuit>,
    grid: Option<Grid>,
    switcher: Option<SwitcherState>,
    /// The switch whose panel the compositor closed mid-switch; its next step builds the panel again around it.
    orphan: Option<Switcher>,
    overview: Option<Overview>,
    pending: HashMap<u64, Pending>,
    next_launch: u64,
    /// The edge strip the pointer is on, and since when.
    edge: Option<(u32, f64)>,
    icons: HashMap<(IconKey, u32), IconSet>,
    trash_checked: Option<f64>,
    trash_full: bool,
    wake_generation: u64,
    wake_at: Option<f64>,
    /// Changes with every update, for what the bar draws under its tiles.
    revision: u64,
    // what the host was last told
    placed: Option<Placement>,
    keyboard: Option<Part>,
    minimize_target: Option<(u32, Rect)>,
    force_quit_size: Option<(f64, f64)>,
}

impl Model {
    pub fn new(
        session: Session,
        actions: ActionMap,
        conf: DockSettings,
        host: Box<dyn Host>,
    ) -> Self {
        Self {
            session,
            host,
            outputs: Vec::new(),
            output: None,
            bar_size: None,
            bar_generation: 0,
            awaiting_list: true,
            dark: false,
            reduce_motion: false,
            locked: false,
            actions,
            conf,
            pointer: None,
            press: None,
            poof: None,
            bridges_painted: None,
            bridges_owed: false,
            menu: None,
            engaged: false,
            unresponsive: HashSet::new(),
            force_quit: None,
            grid: None,
            switcher: None,
            orphan: None,
            overview: None,
            pending: HashMap::new(),
            next_launch: 0,
            edge: None,
            icons: HashMap::new(),
            trash_checked: None,
            trash_full: false,
            wake_generation: 0,
            wake_at: None,
            revision: 0,
            placed: None,
            keyboard: None,
            minimize_target: None,
            force_quit_size: None,
        }
    }

    /// The applications the server reported as not responding (the screenshot fixtures set some at start).
    pub fn set_unresponsive(&mut self, apps: HashSet<String>) {
        self.unresponsive = apps;
    }

    fn now(&self) -> f64 {
        self.host.now()
    }

    fn output_info(&self, key: u32) -> Option<OutputInfo> {
        self.outputs.iter().copied().find(|o| o.key == key)
    }

    /// The bar's window key.
    pub fn bar_key(&self) -> Option<WindowKey> {
        self.output.map(|o| view::bar_key(o, self.bar_generation))
    }

    /// The bar's window as the model's screen: the size the compositor gave it, else the one it was asked for.
    pub fn screen(&self) -> Option<Rect> {
        let output = self.output_info(self.output?)?;
        let (w, h) = self.bar_size.unwrap_or_else(|| {
            let extent = self.session.dock.surface_extent().ceil();
            if self.session.dock.cfg.orientation.is_horizontal() {
                (output.width, extent)
            } else {
                (extent, output.height)
            }
        });
        Some(Rect::new(0.0, 0.0, w, h))
    }

    /// The output in the bar window's points: the window sits on the dock's edge of it.
    fn output_in_bar(&self) -> Option<Rect> {
        let screen = self.screen()?;
        let out = self.output_info(self.output?)?;
        let (x, y) = match self.session.dock.cfg.orientation {
            Orientation::Bottom => (0.0, screen.h - out.height),
            Orientation::Left => (0.0, 0.0),
            Orientation::Right => (screen.w - out.width, 0.0),
        };
        Some(Rect::new(x, y, out.width, out.height))
    }

    fn placement(&self) -> Option<Placement> {
        Some(Placement {
            output: self.output?,
            orientation: self.session.dock.cfg.orientation,
            extent: self.session.dock.surface_extent().ceil(),
            // windows keep clear of the bar, not of the room magnified tiles and bounces need
            exclusive_zone: self.session.dock.exclusive_zone().ceil() as i32,
        })
    }

    fn bar_request(&mut self, request: Request) {
        if let Some(output) = self.output {
            self.host.request(output, request);
        }
    }

    // ---- the window feed ------------------------------------------------------------------------------------

    fn dock_events(&mut self, output: u32, events: Vec<DockEvent>) -> Vec<Command<Message>> {
        let now = self.now();
        let from_bar = Some(output) == self.output;
        if from_bar && std::mem::take(&mut self.awaiting_list) {
            // a new bar role: its first group lists every window, so one that closed meanwhile is gone
            let listed: HashSet<u32> = events
                .iter()
                .filter_map(|e| match e {
                    DockEvent::Window { id, .. } => Some(*id),
                    _ => None,
                })
                .collect();
            let known: Vec<u32> = self.session.window_ids();
            for id in known.into_iter().filter(|id| !listed.contains(id)) {
                self.host.window_closed(id);
                self.session.window_closed(id, now);
            }
        }
        let mut commands = Vec::new();
        for event in events {
            if event.is_shared() && !from_bar {
                continue;
            }
            match event {
                DockEvent::Window {
                    id,
                    app_id,
                    title,
                    minimized,
                } => self.session.window(id, &app_id, &title, minimized, now),
                DockEvent::WindowIcon(icon) => {
                    if let Some(w) = self.session.window_info(icon.id) {
                        let apps = [w.app_id.clone(), w.key.clone()];
                        self.icons.retain(|(k, _), _| !matches!(k, IconKey::App(_)));
                        self.host.window_icon(apps, icon);
                    }
                }
                DockEvent::WindowClosed(id) => {
                    if self.session.window_info(id).is_some() {
                        self.icons.retain(|(k, _), _| !matches!(k, IconKey::App(_)));
                    }
                    self.host.window_closed(id);
                    self.session.window_closed(id, now);
                }
                DockEvent::Launching(app_id) => self.session.launching(&app_id, now),
                DockEvent::ToggleAutoHide => self.session.dock.toggle_autohide(now),
                DockEvent::ToggleAppGrid => self.toggle_grid(),
                DockEvent::Switcher {
                    output,
                    apps,
                    selected,
                } => self.switcher_changed(output, apps, selected),
                DockEvent::SwitcherClosed { committed } => self.switcher_closed(committed),
                DockEvent::Overview {
                    mode,
                    area,
                    windows,
                    workspaces,
                } => self.overview_changed(output, mode, area, &windows, workspaces),
                DockEvent::OverviewClosed => self.overview_closed(),
                DockEvent::Trigger(trigger) => commands.extend(self.run_trigger(output, trigger)),
                // a refused activation: the tile hops once (informational attention)
                DockEvent::Attention(app_id) => self.session.attention(&app_id, now),
                DockEvent::Unresponsive {
                    app_id,
                    unresponsive,
                } => {
                    if unresponsive {
                        self.unresponsive.insert(app_id);
                    } else {
                        self.unresponsive.remove(&app_id);
                    }
                    self.refresh_force_quit();
                }
                DockEvent::ForceQuitDialog => self.toggle_force_quit(),
                DockEvent::Engage if !self.locked => self.engaged = true,
                DockEvent::Engage => {}
            }
        }
        if from_bar {
            self.session.list_taken();
            self.refresh_force_quit();
        }
        commands
    }

    // ---- the bar ------------------------------------------------------------------------------------------

    /// The pointer on the bar. A left press on an application tile waits for the release or the first move (a press
    /// that moves on drags the tile); any other tile is activated at once.
    fn bar_pointer(&mut self, pointer: Pointer) -> Vec<Command<Message>> {
        let now = self.now();
        let Some(screen) = self.screen() else {
            return Vec::new();
        };
        let at = |p: cupertino_kit::geometry::Point| Point::new(p.x, p.y);
        match pointer {
            Pointer::Move(p) => {
                let p = at(p);
                self.pointer = Some(p);
                if let Some(press) = self.press.as_mut() {
                    let (began, id, origin) = (press.moved(p), press.id, press.origin);
                    if began {
                        self.session.dock.begin_drag(id, origin, p, screen, now);
                    } else if self.session.dock.is_dragging() {
                        self.session.dock.drag_move(p, screen, now);
                    }
                }
                Vec::new()
            }
            Pointer::Leave => {
                self.pointer = None;
                Vec::new()
            }
            Pointer::Press(p, _) => {
                let p = at(p);
                // a release was lost (the server ended the grab): the drag of that press is over
                self.session.dock.cancel_drag();
                match self.session.dock.hit_test(p, screen, now) {
                    Some(hit) if matches!(hit.kind, TileKind::App | TileKind::Recent) => {
                        self.press = Some(TilePress::new(hit.id, p, now));
                        Vec::new()
                    }
                    Some(hit) => {
                        self.press = None;
                        self.activate(hit.id)
                    }
                    None => {
                        self.press = None;
                        Vec::new()
                    }
                }
            }
            Pointer::Release(p) => self.release(at(p)),
            Pointer::Secondary(p) => {
                if let Some(hit) = self.session.dock.hit_test(at(p), screen, now) {
                    self.open_menu(hit.id);
                }
                Vec::new()
            }
            Pointer::Scroll(..) => Vec::new(),
        }
    }

    /// A left release: a drag drops the tile, a press that stayed in place is a click.
    fn release(&mut self, position: Point) -> Vec<Command<Message>> {
        let Some(press) = self.press.take() else {
            return Vec::new();
        };
        if !press.is_dragging() {
            return self.activate(press.id);
        }
        let now = self.now();
        let Some(screen) = self.screen() else {
            return Vec::new();
        };
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
        Vec::new()
    }

    fn store_pins(&mut self) {
        let pins = self.session.dock.pinned_keys();
        self.host.store_pins(&pins);
    }

    /// The tile's rectangle in the bar's window, as it is laid out now.
    fn tile_rect(&self, id: TileId) -> Option<Rect> {
        let screen = self.screen()?;
        self.session
            .dock
            .layout(screen, self.now())
            .tiles
            .into_iter()
            .find(|t| t.id == id)
            .map(|t| t.rect)
    }

    /// Carry out what the tile stands for; the bar gives the keyboard back.
    fn activate(&mut self, id: TileId) -> Vec<Command<Message>> {
        self.engaged = false;
        match self.session.activation(id) {
            Some(Activation::ActivateApp(app_id)) => {
                self.bar_request(Request::ActivateApp(app_id));
                Vec::new()
            }
            Some(Activation::RestoreWindow(window)) => {
                if let Some(tile) = self.tile_rect(id) {
                    self.bar_request(Request::RestoreWindow { id: window, tile });
                }
                Vec::new()
            }
            Some(Activation::Launch(key)) => self.begin_launch(key),
            Some(Activation::OpenTrash) => self.open_in_file_manager(TRASH_URI),
            None => Vec::new(),
        }
    }

    /// Open `target` (a folder or the trash) in the session's file manager like a launch of its tile (an activation
    /// token, so its window takes the focus, and a bounce); without it through the default handler.
    fn open_in_file_manager(&mut self, target: &str) -> Vec<Command<Message>> {
        match self.session.file_manager_command(target) {
            Some(argv) => self.ask_token(FILE_MANAGER.to_owned(), argv),
            None => {
                self.host.open(target);
                Vec::new()
            }
        }
    }

    /// Fetch an activation token for the launch of the tile with `key`; the program starts when it arrives.
    fn begin_launch(&mut self, key: String) -> Vec<Command<Message>> {
        let launch = self.session.launch(&key);
        let mut argv = launch.argv;
        if launch.terminal {
            let Some(terminal) = self.host.terminal() else {
                eprintln!(
                    "cupertino-dock: {key} runs in a terminal and none is installed (start with --terminal)"
                );
                return Vec::new();
            };
            argv.splice(0..0, [terminal, "-e".to_owned()]);
        }
        if argv.is_empty() {
            eprintln!("cupertino-dock: {key} has no command");
            return Vec::new();
        }
        self.ask_token(key, argv)
    }

    fn ask_token(&mut self, key: String, argv: Vec<String>) -> Vec<Command<Message>> {
        let launch = self.next_launch;
        self.next_launch += 1;
        self.pending.insert(launch, Pending { key, argv });
        let window = self
            .bar_key()
            .unwrap_or_else(|| view::bar_key(0, self.bar_generation));
        vec![Command::ActivationToken {
            window,
            app_id: None,
            on_token: Box::new(move |token| Message::Token(launch, token)),
        }]
    }

    /// Start the program that waited for `token` and bounce its tile.
    fn complete(&mut self, launch: u64, token: Option<String>) {
        let Some(Pending { key, argv }) = self.pending.remove(&launch) else {
            return;
        };
        if self.host.spawn(argv, token) {
            let now = self.now();
            self.session.dock.app_launching(&key, now);
        }
    }

    // ---- context menus --------------------------------------------------------------------------------------

    /// The menu of the tile or the separator.
    fn open_menu(&mut self, id: TileId) {
        self.menu = None;
        self.press = None;
        let Some(tile) = self.session.dock.tile(id) else {
            return;
        };
        let items = if tile.kind == TileKind::Separator {
            menu::separator_menu(&self.session.dock.cfg)
        } else {
            let entry_path = self
                .session
                .desktop()
                .find(&tile.key)
                .map(|e| e.path.as_path());
            let running_app_id = self.session.running_app_id(id);
            menu::tile_menu(&TileFacts {
                kind: tile.kind,
                key: &tile.key,
                pinned: tile.pinned,
                built_in: tile.built_in,
                running_app_id,
                unresponsive: running_app_id.is_some_and(|app| self.unresponsive.contains(app)),
                entry_path,
            })
        };
        if items.is_empty() {
            return;
        }
        let label = tile_name(&self.session, id);
        self.menu = Some(OpenMenu {
            tile: id,
            items,
            label,
        });
    }

    fn run_menu_command(&mut self, command: MenuCommand) -> Vec<Command<Message>> {
        let now = self.now();
        self.engaged = false;
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
                return self.open_in_file_manager(&dir.to_string_lossy());
            }
            MenuCommand::ShowAllWindows(app) => {
                self.bar_request(Request::App(app, AppAction::ShowWindows))
            }
            MenuCommand::Hide(app) => self.bar_request(Request::App(app, AppAction::Hide)),
            MenuCommand::Quit(app) => self.bar_request(Request::App(app, AppAction::Quit)),
            MenuCommand::ForceQuit(app) => self.open_force_quit(Some(&app)),
            MenuCommand::Open(key) => return self.begin_launch(key),
            MenuCommand::OpenTrash => return self.open_in_file_manager(TRASH_URI),
            MenuCommand::ToggleAutoHide => self.session.dock.toggle_autohide(now),
            MenuCommand::ToggleMagnification => {
                self.session.dock.cfg.magnification = !self.session.dock.cfg.magnification
            }
            // the items are disabled: the position is the file's
            MenuCommand::Position(_) => {}
        }
        Vec::new()
    }

    // ---- moving between outputs ----------------------------------------------------------------------------

    /// Put the dock on the output with key `output`: what was under way on the bar ends.
    fn relocate(&mut self, output: u32) {
        self.output = Some(output);
        self.bar_size = None;
        self.awaiting_list = true;
        self.menu = None;
        self.press = None;
        self.poof = None;
        self.pointer = None;
        self.engaged = false;
        self.edge = None;
        self.session.dock.cancel_drag();
        if self.grid.as_ref().is_some_and(|g| g.output != output) {
            self.grid = None;
        }
    }

    fn outputs_changed(&mut self, outputs: Vec<OutputInfo>) {
        self.outputs = outputs;
        let first = self.outputs.first().map(|o| o.key);
        match self.output {
            Some(o) if self.output_info(o).is_some() => {}
            _ => match first {
                Some(o) => self.relocate(o),
                None => self.output = None,
            },
        }
        let there = |o: u32, outputs: &[OutputInfo]| outputs.iter().any(|i| i.key == o);
        if self
            .grid
            .as_ref()
            .is_some_and(|g| !there(g.output, &self.outputs))
        {
            self.grid = None;
        }
        if self
            .force_quit
            .as_ref()
            .is_some_and(|f| !there(f.output, &self.outputs))
        {
            self.force_quit = None;
        }
        if self
            .overview
            .as_ref()
            .is_some_and(|v| !there(v.output, &self.outputs))
        {
            self.overview = None;
        }
        if let Some(s) = self.switcher.take_if(|s| !there(s.output, &self.outputs)) {
            self.orphan = Some(s.switcher);
        }
        if self.edge.is_some_and(|(o, _)| !there(o, &self.outputs)) {
            self.edge = None;
        }
    }

    /// The compositor closed a window the model still shows.
    fn closed(&mut self, key: WindowKey) {
        if Some(&key) == self.bar_key().as_ref() {
            self.bar_generation += 1;
            self.bar_size = None;
            self.awaiting_list = true;
            self.menu = None;
            self.press = None;
            self.engaged = false;
        } else if key == bridge::switcher_key() {
            if let Some(s) = self.switcher.take() {
                self.orphan = Some(s.switcher);
            }
        } else if key == bridge::overview_key() {
            self.overview = None;
        } else if key == view::grid::key() {
            self.grid = None;
        } else if key == view::force_quit::key() {
            self.force_quit = None;
        }
    }

    // ---- the Force Quit dialog -------------------------------------------------------------------------------

    /// The running applications for the dialog: the dock's running application tiles, the unresponsive ones marked.
    fn force_quit_entries(&self) -> Vec<Entry> {
        self.session
            .dock
            .tiles()
            .into_iter()
            .filter(|t| t.kind == TileKind::App && t.running)
            .map(|t| Entry {
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

    fn toggle_force_quit(&mut self) {
        if self.force_quit.take().is_none() {
            self.open_force_quit(None);
        }
    }

    /// Open the dialog on the bar's output, on the second step for `ask_about` when that is given.
    fn open_force_quit(&mut self, ask_about: Option<&str>) {
        let Some(output) = self.output.or(self.outputs.first().map(|o| o.key)) else {
            return;
        };
        let entries = self.force_quit_entries();
        let dialog = match ask_about {
            Some(app_id) => Dialog::asking_about(entries, app_id),
            None => Dialog::new(entries),
        };
        let layout = force_quit::Layout::for_dialog(&dialog);
        self.force_quit = Some(ForceQuit {
            output,
            dialog,
            layout,
            hover_row: None,
            hover_button: None,
        });
        self.force_quit_size = None;
    }

    /// The applications or their answers changed: the open dialog follows.
    fn refresh_force_quit(&mut self) {
        let entries = self.force_quit_entries();
        if let Some(open) = self.force_quit.as_mut() {
            if open.dialog.entries() != entries.as_slice() {
                open.dialog.set_entries(entries);
                open.layout = force_quit::Layout::for_dialog(&open.dialog);
            }
        }
    }

    fn force_quit_outcome(&mut self, outcome: force_quit::Outcome) {
        match outcome {
            force_quit::Outcome::Nothing => {}
            // the second step has another size and other parts
            force_quit::Outcome::Redraw => {
                if let Some(open) = self.force_quit.as_mut() {
                    open.layout = force_quit::Layout::for_dialog(&open.dialog);
                }
            }
            force_quit::Outcome::Close => self.force_quit = None,
            force_quit::Outcome::Force(app_ids) => {
                for app_id in app_ids {
                    self.bar_request(Request::ForceQuitApp(app_id));
                }
                self.force_quit = None;
            }
        }
    }

    fn force_quit_button(&mut self, button: Button) {
        let Some(open) = self.force_quit.as_mut() else {
            return;
        };
        let confirming = open.dialog.confirming().is_some();
        let outcome = match (button, confirming) {
            (Button::Cancel, true) => open.dialog.cancel_pressed(),
            (Button::ForceQuit, true) => open.dialog.confirm_pressed(),
            (Button::ForceQuit, false) => open.dialog.force_quit_pressed(),
            (Button::Cancel, false) => force_quit::Outcome::Nothing,
        };
        self.force_quit_outcome(outcome);
    }

    fn force_quit_pointer(&mut self, pointer: Pointer) {
        let Some(open) = self.force_quit.as_mut() else {
            return;
        };
        match pointer {
            Pointer::Move(p) => {
                (open.hover_row, open.hover_button) = view::force_quit::hit(&open.layout, p);
            }
            Pointer::Leave => (open.hover_row, open.hover_button) = (None, None),
            Pointer::Press(p, mods) => {
                let (row, button) = view::force_quit::hit(&open.layout, p);
                if let Some(button) = button {
                    return self.force_quit_button(button);
                }
                if let Some(i) = row {
                    let how = if mods.contains(Mods::SHIFT) {
                        Extend::Range
                    } else if mods.contains(Mods::CTRL) || mods.contains(Mods::PRIMARY) {
                        Extend::Toggle
                    } else {
                        Extend::Replace
                    };
                    let outcome = open.dialog.select(i, how);
                    self.force_quit_outcome(outcome);
                }
            }
            Pointer::Scroll(_, delta) => {
                let dy = delta.y;
                let rows = (dy / force_quit::ROW_HEIGHT).round() as isize;
                let rows = if rows == 0 {
                    dy.signum() as isize
                } else {
                    rows
                };
                let outcome = open.dialog.scroll(rows);
                self.force_quit_outcome(outcome);
            }
            Pointer::Release(_) | Pointer::Secondary(_) => {}
        }
    }

    fn force_quit_key(&mut self, key: KeyEvent) {
        if !key.pressed {
            return;
        }
        let Some(open) = self.force_quit.as_mut() else {
            return;
        };
        let mapped = match key.keysym {
            ESCAPE => force_quit::Key::Escape,
            RETURN | KP_ENTER => force_quit::Key::Return,
            UP => force_quit::Key::Up,
            DOWN => force_quit::Key::Down,
            HOME => force_quit::Key::Home,
            END => force_quit::Key::End,
            _ => return,
        };
        let outcome = open.dialog.key(mapped, key.mods.contains(Mods::SHIFT));
        self.force_quit_outcome(outcome);
    }

    // ---- the application grid ------------------------------------------------------------------------------

    /// The app-grid action: open the grid, or start closing it when it is open; nothing during its exit.
    fn toggle_grid(&mut self) {
        match &self.grid {
            Some(grid) if grid.grid.is_closing() => {}
            Some(_) => self.close_grid(),
            None => self.open_grid(),
        }
    }

    fn open_grid(&mut self) {
        if self.locked {
            return;
        }
        let Some(output) = self.output else {
            return;
        };
        let mut grid = AppGrid::from_desktop(self.session.desktop(), self.now());
        grid.set_reduce_motion(self.reduce_motion);
        self.grid = Some(Grid {
            output,
            grid,
            layout: None,
        });
    }

    /// Start the grid's exit animation; the keyboard goes back at once.
    fn close_grid(&mut self) {
        let now = self.now();
        if let Some(grid) = self.grid.as_mut().filter(|g| !g.grid.is_closing()) {
            grid.grid.close(now);
        }
    }

    /// What a key or click on the grid asked for: a launch goes through the tiles' path and closes the grid.
    fn grid_outcome(&mut self, outcome: Option<Outcome>) -> Vec<Command<Message>> {
        match outcome {
            Some(Outcome::Launch(key)) => {
                self.close_grid();
                self.begin_launch(key)
            }
            Some(Outcome::Close) => {
                self.close_grid();
                Vec::new()
            }
            None => Vec::new(),
        }
    }

    fn grid_pointer(&mut self, pointer: Pointer) -> Vec<Command<Message>> {
        let now = self.now();
        let Some(grid) = self.grid.as_mut().filter(|g| !g.grid.is_closing()) else {
            return Vec::new();
        };
        match pointer {
            Pointer::Press(p, _) => {
                let Some(layout) = grid.layout.as_ref() else {
                    return Vec::new();
                };
                let outcome = grid.grid.click(Point::new(p.x, p.y), layout, now);
                self.grid_outcome(outcome)
            }
            Pointer::Scroll(_, delta) => {
                grid.grid.scroll(delta.x, delta.y, now);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn grid_key(&mut self, key: KeyEvent) -> Vec<Command<Message>> {
        let now = self.now();
        if !key.pressed {
            return Vec::new();
        }
        let Some(grid) = self.grid.as_mut().filter(|g| !g.grid.is_closing()) else {
            return Vec::new();
        };
        let named = match key.keysym {
            ESCAPE => Some(app_grid::Key::Escape),
            RETURN | KP_ENTER => Some(app_grid::Key::Return),
            BACKSPACE => Some(app_grid::Key::Backspace),
            LEFT => Some(app_grid::Key::Left),
            RIGHT => Some(app_grid::Key::Right),
            UP => Some(app_grid::Key::Up),
            DOWN => Some(app_grid::Key::Down),
            HOME => Some(app_grid::Key::Home),
            END => Some(app_grid::Key::End),
            PAGE_UP => Some(app_grid::Key::PageUp),
            PAGE_DOWN => Some(app_grid::Key::PageDown),
            _ => None,
        };
        let command = key.mods.contains(Mods::CTRL)
            || key.mods.contains(Mods::ALT)
            || key.mods.contains(Mods::PRIMARY);
        let mapped = named.or_else(|| {
            (!command)
                .then_some(key.text)
                .flatten()
                .filter(|c| !c.is_control())
                .map(app_grid::Key::Char)
        });
        match mapped {
            Some(k) => {
                let outcome = grid.grid.key(k, now);
                self.grid_outcome(outcome)
            }
            None => Vec::new(),
        }
    }

    fn grid_item(&mut self, index: usize) -> Vec<Command<Message>> {
        let key = self
            .grid
            .as_ref()
            .filter(|g| !g.grid.is_closing())
            .and_then(|g| g.grid.result(index))
            .map(|app| app.key.clone());
        match key {
            Some(key) => self.grid_outcome(Some(Outcome::Launch(key))),
            None => Vec::new(),
        }
    }

    // ---- the application switcher --------------------------------------------------------------------------

    /// A step of the open switch moves the selection; anything else opens a new switch on `output`.
    fn switcher_changed(&mut self, output: u32, apps: Vec<String>, selected: usize) {
        if let Some(open) = self
            .switcher
            .as_mut()
            .filter(|s| s.output == output && s.switcher.continues_with(&apps))
        {
            if open.switcher.select(selected).is_err() {
                eprintln!(
                    "cupertino-dock: the server sent a selection outside the switcher's list"
                );
            }
            open.requested = None;
            return;
        }
        let resumed = self.orphan.take().filter(|s| s.continues_with(&apps));
        self.switcher = None;
        let Some(out) = self.output_info(output) else {
            eprintln!("cupertino-dock: the output of the switcher went away");
            return;
        };
        let layout = SwitcherLayout::new(apps.len(), out.width, out.height);
        let switcher = match resumed {
            Some(mut switcher) => switcher.select(selected).map(|_| switcher),
            None => Switcher::new(apps, selected, self.now()),
        };
        match switcher {
            Ok(switcher) => {
                self.switcher = Some(SwitcherState {
                    output,
                    switcher,
                    layout,
                    hover: HoverGate::default(),
                    requested: None,
                    image: None,
                })
            }
            Err(e) => eprintln!("cupertino-dock: {e}"),
        }
    }

    /// The switch ended. A panel that was on screen fades out after a commit; a cancel, or a commit before the panel
    /// appeared, removes it at once.
    fn switcher_closed(&mut self, committed: bool) {
        let now = self.now();
        self.orphan = None;
        match self.switcher.as_mut() {
            Some(open) if committed && open.switcher.is_shown(now) => open.switcher.close(now),
            _ => self.switcher = None,
        }
    }

    /// Hovering over a tile (once the pointer left its first position) or pressing it selects that entry; the server
    /// answers with the new selection. Releasing the left button on a tile switches to it, anywhere else on the panel
    /// ends the switch.
    fn switcher_pointer(&mut self, pointer: Pointer) {
        let Some(open) = self.switcher.as_mut().filter(|s| !s.switcher.is_fading()) else {
            return;
        };
        // the window covers only the panel: its origin is the panel's in output points
        let at = |p: cupertino_kit::geometry::Point| {
            Point::new(p.x + open.layout.panel.x, p.y + open.layout.panel.y)
        };
        let n = open.switcher.apps().len();
        let output = open.output;
        let (position, pressed) = match pointer {
            Pointer::Move(p) => (at(p), false),
            Pointer::Press(p, _) => (at(p), true),
            Pointer::Release(p) => {
                let request = match open.layout.hit(at(p), n) {
                    Some(entry) => Request::SwitcherActivate(entry),
                    None => Request::SwitcherCancel,
                };
                self.host.request(output, request);
                return;
            }
            _ => return,
        };
        let armed = open.hover.moved(position);
        let Some(entry) = open.layout.hit(position, n) else {
            return;
        };
        if (pressed || armed) && entry != open.switcher.selected() && open.requested != Some(entry)
        {
            open.requested = Some(entry);
            self.host.request(output, Request::SwitcherSelect(entry));
        }
    }

    // ---- the overview --------------------------------------------------------------------------------------

    /// The server opened the overview of `output` or changed it: lay the windows out, tell the server their frames,
    /// and show the chrome.
    fn overview_changed(
        &mut self,
        output: u32,
        mode: Mode,
        area: Rect,
        windows: &[(u32, Rect)],
        workspaces: Vec<Workspace>,
    ) {
        let now = self.now();
        let entries: Vec<WindowEntry> = windows
            .iter()
            .map(|&(id, frame)| {
                let info = self.session.window_info(id);
                WindowEntry {
                    id,
                    frame,
                    title: info.map(|i| i.title.clone()).unwrap_or_default(),
                    minimized: info.is_some_and(|i| i.minimized),
                }
            })
            .collect();
        match self
            .overview
            .as_mut()
            .filter(|o| o.output == output && !o.ui.is_closing() && o.ui.mode() == mode)
        {
            Some(open) => {
                open.ui.set_windows(&entries, now);
                open.ui.set_workspaces(workspaces);
            }
            None => {
                self.overview = Some(Overview {
                    output,
                    ui: OverviewUi::new(mode, area, &entries, workspaces, now),
                    image: None,
                })
            }
        }
        let open = self.overview.as_ref().expect("set above");
        for (id, frame) in open.ui.frames() {
            self.host.request(output, Request::OverviewFrame(id, frame));
        }
    }

    /// The server is leaving the overview: the chrome fades and gives the keyboard back.
    fn overview_closed(&mut self) {
        let now = self.now();
        if let Some(open) = self.overview.as_mut().filter(|o| !o.ui.is_closing()) {
            open.ui.close(now);
        }
    }

    /// Every action is the server's to apply.
    fn overview_action(&mut self, action: UiAction) {
        let Some(output) = self.overview.as_ref().map(|o| o.output) else {
            return;
        };
        let request = match action {
            UiAction::None => return,
            UiAction::Select(id) => Request::OverviewSelect(id),
            UiAction::Close => Request::OverviewClose,
            UiAction::SwitchWorkspace(id) => Request::SwitchWorkspace(id),
            UiAction::AddWorkspace => Request::AddWorkspace,
            UiAction::RemoveWorkspace(id) => Request::RemoveWorkspace(id),
            UiAction::MoveWorkspace { id, index } => Request::MoveWorkspace { id, index },
        };
        self.host.request(output, request);
    }

    fn overview_pointer(&mut self, pointer: Pointer) {
        let now = self.now();
        let Some(open) = self.overview.as_mut() else {
            return;
        };
        let at = |p: cupertino_kit::geometry::Point| Point::new(p.x, p.y);
        let action = match pointer {
            Pointer::Move(p) => {
                open.ui.pointer_moved(Some(at(p)), now);
                UiAction::None
            }
            Pointer::Leave => {
                open.ui.pointer_moved(None, now);
                UiAction::None
            }
            Pointer::Press(p, _) => open.ui.press(at(p), now),
            Pointer::Release(p) => open.ui.release(at(p), now),
            Pointer::Secondary(_) | Pointer::Scroll(..) => UiAction::None,
        };
        self.overview_action(action);
    }

    fn overview_key(&mut self, key: KeyEvent) {
        use cupertino_overview::Key as K;
        if !key.pressed {
            return;
        }
        let now = self.now();
        let Some(open) = self.overview.as_mut() else {
            return;
        };
        let mapped = match key.keysym {
            LEFT => K::Left,
            RIGHT => K::Right,
            UP => K::Up,
            DOWN => K::Down,
            LEFT_TAB => K::Previous,
            TAB if key.mods.contains(Mods::SHIFT) => K::Previous,
            TAB => K::Next,
            RETURN | KP_ENTER | SPACE => K::Activate,
            ESCAPE => K::Cancel,
            _ => return,
        };
        let action = open.ui.key(mapped, now);
        self.overview_action(action);
    }

    // ---- hot corners and gestures ---------------------------------------------------------------------------

    fn run_trigger(&mut self, output: u32, trigger: Trigger) -> Vec<Command<Message>> {
        match self.actions.get(trigger).clone() {
            ActionTarget::None => {}
            ActionTarget::AppGrid => self.toggle_grid(),
            ActionTarget::Exec(command) => self.host.shell(&command),
            ActionTarget::Server(name) => self.host.request(output, Request::RunAction(name)),
        }
        Vec::new()
    }

    // ---- dock.conf -------------------------------------------------------------------------------------------

    /// Apply the keys whose value differs between the file as it was and as it is; a key removed from the file goes
    /// back to its default. The pins of `dock.conf` apply at the next start (the dock keeps the pins the user changes).
    fn conf_changed(&mut self, after: DockSettings) {
        let before = std::mem::replace(&mut self.conf, after.clone());
        let height = self
            .output
            .and_then(|o| self.output_info(o))
            .map_or(0.0, |o| o.height);
        let target = dock_config(Some(&after), height);
        let now = self.now();
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
            self.bar_size = None;
        }
        if before.recents != after.recents {
            cfg.show_recents = target.show_recents;
        }
        if before.autohide != after.autohide && cfg.autohide != target.autohide {
            self.session.dock.toggle_autohide(now);
        }
        if before.actions != after.actions {
            self.actions = after.actions;
        }
    }

    // ---- after every message --------------------------------------------------------------------------------

    /// The icon of `key` at every size it is drawn at, loaded once.
    fn icon_set(&mut self, key: &IconKey, sides: &[f64]) -> IconSet {
        let mut set = IconSet::new();
        for &side in sides {
            let px = side.ceil().max(1.0) as u32;
            let host = &mut self.host;
            let desktop = self.session.desktop();
            let renditions = self
                .icons
                .entry((key.clone(), px))
                .or_insert_with(|| host.icon(desktop, key, f64::from(px)));
            set.extend(renditions.iter().cloned());
        }
        set
    }

    /// The sides the tiles are drawn at: resting and fully magnified.
    fn tile_sides(&self) -> Vec<f64> {
        let cfg = &self.session.dock.cfg;
        vec![cfg.tile_size, cfg.large_size.max(cfg.tile_size)]
    }

    /// The icon key of the tile with `id`.
    fn tile_icon(&self, id: TileId) -> Option<IconKey> {
        let tile = self.session.dock.tile(id)?;
        Some(match tile.kind {
            TileKind::Trash => IconKey::Trash(self.trash_full),
            _ => IconKey::App(tile.key.clone()),
        })
    }

    /// Load what the next view draws, so the view only reads.
    fn prefill_icons(&mut self) {
        let sides = self.tile_sides();
        let mut keys: Vec<IconKey> = self
            .session
            .dock
            .tiles()
            .iter()
            .filter(|t| t.kind != TileKind::Separator)
            .filter_map(|t| self.tile_icon(t.id))
            .collect();
        if let Some(open) = &self.force_quit {
            let fq: Vec<IconKey> = open
                .dialog
                .entries()
                .iter()
                .map(|e| IconKey::App(e.app_id.clone()))
                .collect();
            for key in &fq {
                self.icon_set(key, &[FORCE_QUIT_ROW_ICON, force_quit::ICON_SIZE]);
            }
        }
        keys.dedup();
        for key in keys {
            self.icon_set(&key, &sides);
        }
        let grid_icons: Vec<(String, f64)> = match &self.grid {
            Some(Grid {
                grid,
                layout: Some(layout),
                ..
            }) => {
                let side = layout
                    .image_rect(&layout.cell_rect(&layout.page_frame(0, 0.0), 0))
                    .w;
                let page = grid.page();
                let from = page.saturating_sub(1) * PER_PAGE;
                let to = (page + 2) * PER_PAGE;
                (from..to)
                    .filter_map(|i| grid.result(i))
                    .map(|app| (app.icon.clone(), side))
                    .collect()
            }
            _ => Vec::new(),
        };
        for (name, side) in grid_icons {
            self.icon_set(&IconKey::Named(name), &[side]);
        }
    }

    /// Advance the animations, carry out what is due and tell the host what changed; then schedule the next wake.
    fn changed(&mut self) -> Vec<Command<Message>> {
        let now = self.now();
        self.revision += 1;
        // the locked session, the grid, the overview and the Force Quit dialog hold the keyboard while they are up: a
        // Focus Dock meanwhile is dropped, not kept for when they go
        if self.locked
            || self.force_quit.is_some()
            || self.grid.as_ref().is_some_and(|g| !g.grid.is_closing())
            || self.overview.as_ref().is_some_and(|o| !o.ui.is_closing())
        {
            self.engaged = false;
        }
        // an auto-hidden bar shows while it has the keyboard
        self.session.dock.keep_shown = self.engaged;
        if let Some(screen) = self.screen() {
            self.session.dock.pointer_moved(self.pointer, screen, now);
        }
        self.session.dock.tick(now);
        if self
            .trash_checked
            .is_none_or(|at| now - at >= TRASH_CHECK_EVERY)
        {
            self.trash_checked = Some(now);
            self.trash_full = self.host.trash_full();
            self.session.dock.set_trash_full(self.trash_full);
        }
        // a press held in place on a tile opens its menu; the press is then neither a click nor a drag
        if let Some(id) = self
            .press
            .as_mut()
            .and_then(|p| p.held(now).then_some(p.id))
        {
            self.press = None;
            self.open_menu(id);
        }
        if let Some(grid) = self.grid.as_mut() {
            grid.grid.settle(now);
            grid.grid.tick(now);
            if grid.grid.is_gone(now) {
                self.grid = None;
            }
        }
        if self
            .switcher
            .as_ref()
            .is_some_and(|s| s.switcher.is_gone(now))
        {
            self.switcher = None;
        }
        if self.overview.as_ref().is_some_and(|o| o.ui.is_gone(now)) {
            self.overview = None;
        }
        if self.poof.is_some_and(|p| p.progress(now).is_none()) {
            self.poof = None;
        }
        if let Some((output, since)) = self.edge {
            if now - since >= EDGE_DWELL
                && Some(output) != self.output
                && !self.session.dock.is_dragging()
                && !self.locked
            {
                self.relocate(output);
            }
        }
        self.tell_host();
        self.prefill_icons();
        self.paint_bridges(now);
        self.schedule(now)
    }

    /// Tell the host what it has to know: where the bar is, the minimize target, who has the keyboard and the dialog's
    /// size.
    fn tell_host(&mut self) {
        let placement = self.placement();
        if let Some(p) = placement.filter(|_| placement != self.placed) {
            self.placed = placement;
            self.host.place_bar(p);
        }
        if self.bar_size.is_none() {
            // no bar yet, so no role to send it on: it goes once the bar is sized
            self.minimize_target = None;
        } else if let (Some(output), Some(screen)) = (self.output, self.screen()) {
            let target = (output, self.session.dock.next_minimized_slot(screen));
            if self.minimize_target != Some(target) {
                self.minimize_target = Some(target);
                self.host.request(output, Request::MinimizeTarget(target.1));
            }
        }
        let keyboard = if self.overview.as_ref().is_some_and(|o| !o.ui.is_closing()) {
            Some(Part::Overview)
        } else if self.grid.as_ref().is_some_and(|g| !g.grid.is_closing()) {
            Some(Part::Grid)
        } else if self.engaged {
            self.output.map(Part::Bar)
        } else {
            None
        };
        if keyboard != self.keyboard {
            if let Some(part) = self.keyboard.take() {
                self.host.keyboard(part, false);
            }
            if let Some(part) = keyboard {
                self.host.keyboard(part, true);
            }
            self.keyboard = keyboard;
        }
        let size = self.force_quit.as_ref().map(|f| f.layout.size());
        if let Some((w, h)) = size.filter(|_| size != self.force_quit_size) {
            self.host.resize_force_quit(w, h);
        }
        self.force_quit_size = size;
    }

    /// The switcher's panel and the overview's chrome, painted again by the host.
    fn paint_bridges(&mut self, now: f64) {
        if self.switcher.is_none() && self.overview.is_none() {
            self.bridges_owed = false;
            return;
        }
        // each paint is the whole panel or the whole output, and a message comes with every pointer motion: at most one
        // paint a frame, a message sooner leaves it to the next frame's wake
        if self
            .bridges_painted
            .is_some_and(|at| now >= at && now - at < FRAME - 1e-6)
        {
            self.bridges_owed = true;
            return;
        }
        self.bridges_painted = Some(now);
        self.bridges_owed = false;
        let outputs = self.outputs.clone();
        let info = |key: u32| outputs.iter().copied().find(|o| o.key == key);
        if let Some(open) = self.switcher.as_mut() {
            if let (true, Some(out)) = (open.switcher.is_shown(now), info(open.output)) {
                let panel = SwitcherPaint {
                    switcher: &open.switcher,
                    layout: &open.layout,
                    desktop: self.session.desktop(),
                    dark: self.dark,
                    alpha: open.switcher.alpha(now),
                };
                open.image = self.host.paint_switcher(out.scale, &panel);
            }
        }
        if let Some(open) = self.overview.as_mut() {
            if let Some(out) = info(open.output) {
                let size = (out.width, out.height);
                open.image = self
                    .host
                    .paint_overview(out.scale, size, &open.ui, now, self.dark);
            }
        }
    }

    /// Whether anything moves on its own now.
    fn animating(&self, now: f64) -> bool {
        self.session.dock.wants_tick(now)
            || self.bridges_owed
            || self.poof.is_some()
            || self.grid.as_ref().is_some_and(|g| g.grid.animating(now))
            || self
                .switcher
                .as_ref()
                .is_some_and(|s| s.switcher.is_fading())
            || self
                .overview
                .as_ref()
                .is_some_and(|o| o.ui.is_animating(now))
    }

    /// One wake at the next frame while something moves, else at the next thing that is due.
    fn schedule(&mut self, now: f64) -> Vec<Command<Message>> {
        let due = if self.animating(now) {
            Some(FRAME)
        } else {
            let panel = self
                .switcher
                .as_ref()
                .filter(|s| !s.switcher.is_shown(now))
                .map(|s| s.switcher.until_shown(now));
            let wheel = self.grid.as_ref().and_then(|g| g.grid.until_settled(now));
            let hold = self.press.as_ref().and_then(|p| p.hold_due(now));
            let edge = self
                .edge
                .filter(|(o, _)| Some(*o) != self.output)
                .map(|(_, since)| (since + EDGE_DWELL - now).max(0.0));
            panel
                .into_iter()
                .chain(wheel)
                .chain(hold)
                .chain(edge)
                .reduce(f64::min)
        };
        let Some(due) = due else {
            return Vec::new();
        };
        let at = now + due;
        if self.wake_at.is_some_and(|w| w <= at + 1e-3) {
            return Vec::new();
        }
        self.wake_generation += 1;
        self.wake_at = Some(at);
        self.host.wake(due, self.wake_generation)
    }
}

/// The trash, which opens in the session's file manager, or else with the session's default handler.
const TRASH_URI: &str = "trash:///";

const LEFT: Keysym = Keysym(0xff51);
const UP: Keysym = Keysym(0xff52);
const RIGHT: Keysym = Keysym(0xff53);
const DOWN: Keysym = Keysym(0xff54);
const HOME: Keysym = Keysym(0xff50);
const END: Keysym = Keysym(0xff57);
const PAGE_UP: Keysym = Keysym(0xff55);
const PAGE_DOWN: Keysym = Keysym(0xff56);
const ESCAPE: Keysym = Keysym(0xff1b);
const RETURN: Keysym = Keysym(0xff0d);
const KP_ENTER: Keysym = Keysym(0xff8d);
const SPACE: Keysym = Keysym(0x20);
const BACKSPACE: Keysym = Keysym(0xff08);
const TAB: Keysym = Keysym(0xff09);
const LEFT_TAB: Keysym = Keysym(0xfe20);

/// What assistive technology calls the tile with `id`.
pub fn tile_name(session: &Session, id: TileId) -> String {
    let Some(tile) = session.dock.tile(id) else {
        return String::new();
    };
    match tile.kind {
        TileKind::Trash => TRASH_NAME.to_owned(),
        TileKind::Separator => String::new(),
        TileKind::MinimizedWindow if !tile.title.is_empty() => tile.title.clone(),
        TileKind::Recent => std::path::Path::new(&tile.key)
            .file_name()
            .map_or_else(|| tile.key.clone(), |n| n.to_string_lossy().into_owned()),
        _ => session
            .desktop()
            .find(&tile.key)
            .map(|e| e.name.clone())
            .unwrap_or_else(|| {
                if tile.title.is_empty() {
                    tile.key.clone()
                } else {
                    tile.title.clone()
                }
            }),
    }
}

/// What assistive technology says about the state of the tile laid out as `t`.
fn tile_description(model: &Model, id: TileId, lifecycle: TileLifecycle) -> String {
    let Some(tile) = model.session.dock.tile(id) else {
        return String::new();
    };
    match tile.kind {
        TileKind::Trash if tile.full => TRASH_FULL.to_owned(),
        TileKind::MinimizedWindow => MINIMIZED.to_owned(),
        TileKind::App => {
            let unresponsive = model
                .session
                .running_app_id(id)
                .is_some_and(|app| model.unresponsive.contains(app));
            match lifecycle {
                _ if unresponsive => NOT_RESPONDING.to_owned(),
                TileLifecycle::Launching => OPENING.to_owned(),
                TileLifecycle::Attention => NEEDS_ATTENTION.to_owned(),
                TileLifecycle::Running => RUNNING.to_owned(),
                TileLifecycle::Idle => String::new(),
            }
        }
        _ => String::new(),
    }
}

pub fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
    let mut commands = match message {
        Message::Input(input) => input_message(model, input),
        Message::Dock { output, events } => model.dock_events(output, events),
        Message::Dark(dark) => {
            model.dark = dark;
            Vec::new()
        }
        Message::ReduceMotion(on) => {
            model.reduce_motion = on;
            if let Some(grid) = model.grid.as_mut() {
                grid.grid.set_reduce_motion(on);
            }
            Vec::new()
        }
        // the lock screen covers everything: the grid, the dialog and a menu go at once, without animation; the
        // overview's chrome waits for the server's `overview_closed`
        Message::Locked(locked) => {
            model.locked = locked;
            if locked {
                model.force_quit = None;
                model.grid = None;
                model.menu = None;
                model.press = None;
                model.engaged = false;
                model.edge = None;
                model.session.dock.cancel_drag();
            }
            Vec::new()
        }
        Message::Outputs(outputs) => {
            model.outputs_changed(outputs);
            Vec::new()
        }
        Message::Sized(Sized::Bar(output), w, h) => {
            if Some(output) == model.output {
                model.bar_size = Some((w, h));
            }
            Vec::new()
        }
        Message::Sized(Sized::Grid, w, h) => {
            let now = model.now();
            if let Some(grid) = model.grid.as_mut() {
                if grid.layout.is_none() {
                    // the entry animation starts with the first frame
                    grid.grid.restart_entry(now);
                }
                grid.layout = Some(GridLayout::new(w, h));
            }
            Vec::new()
        }
        Message::KeyboardLeft(output) => {
            if Some(output) == model.output && model.menu.is_none() {
                model.engaged = false;
            }
            Vec::new()
        }
        Message::Token(launch, token) => {
            model.complete(launch, token);
            Vec::new()
        }
        Message::Wake(generation) => {
            if generation == model.wake_generation {
                model.wake_at = None;
            }
            Vec::new()
        }
        Message::ConfChanged => {
            if let Some(settings) = model.host.read_conf(&model.conf) {
                model.conf_changed(settings);
            }
            Vec::new()
        }
        Message::Unheard => {
            eprintln!("cupertino-dock: the connection to the server is gone");
            return vec![Command::Exit];
        }
    };
    commands.extend(model.changed());
    commands
}

fn input_message(model: &mut Model, input: Input) -> Vec<Command<Message>> {
    match input {
        Input::Bar(pointer) => model.bar_pointer(pointer),
        Input::Activate(id) => model.activate(TileId(id)),
        Input::ShowMenu(id) => {
            model.open_menu(TileId(id));
            Vec::new()
        }
        Input::LeaveKeyboard => {
            model.engaged = false;
            Vec::new()
        }
        Input::MenuItem(i) => {
            let command = model
                .menu
                .as_ref()
                .and_then(|m| m.items.get(i))
                .and_then(|item| item.command.clone());
            model.menu = None;
            match command {
                Some(command) => model.run_menu_command(command),
                None => Vec::new(),
            }
        }
        Input::MenuClosed => {
            model.menu = None;
            Vec::new()
        }
        Input::Grid(pointer) => model.grid_pointer(pointer),
        Input::GridKey(key) => model.grid_key(key),
        Input::GridLeft => {
            model.close_grid();
            Vec::new()
        }
        Input::GridItem(i) => model.grid_item(i),
        Input::GridPage(i) => {
            let now = model.now();
            if let Some(grid) = model.grid.as_mut() {
                grid.grid.go_to_page(i, now);
            }
            Vec::new()
        }
        Input::ForceQuit(pointer) => {
            model.force_quit_pointer(pointer);
            Vec::new()
        }
        Input::ForceQuitKey(key) => {
            model.force_quit_key(key);
            Vec::new()
        }
        Input::ForceQuitRow(i) => {
            if let Some(open) = model.force_quit.as_mut() {
                let outcome = open.dialog.select(i, Extend::Replace);
                model.force_quit_outcome(outcome);
            }
            Vec::new()
        }
        Input::ForceQuitButton(button) => {
            model.force_quit_button(button);
            Vec::new()
        }
        Input::Edge(output, true) => {
            let now = model.now();
            if model.edge.is_none_or(|(o, _)| o != output) {
                model.edge = Some((output, now));
            }
            Vec::new()
        }
        Input::Edge(output, false) => {
            if model.edge.is_some_and(|(o, _)| o == output) {
                model.edge = None;
            }
            Vec::new()
        }
        Input::Switcher(pointer) => {
            model.switcher_pointer(pointer);
            Vec::new()
        }
        Input::Overview(pointer) => {
            model.overview_pointer(pointer);
            Vec::new()
        }
        Input::OverviewKey(key) => {
            model.overview_key(key);
            Vec::new()
        }
        Input::Closed(key) => {
            model.closed(key);
            Vec::new()
        }
    }
}

fn message(input: Input) -> Message {
    Message::Input(input)
}

fn kit_point(p: Point) -> cupertino_kit::geometry::Point {
    cupertino_kit::geometry::Point::new(p.x, p.y)
}

/// Every window the model shows.
pub fn view(model: &Model) -> Vec<Window<Message>> {
    let now = model.now();
    let mut windows = Vec::new();
    let orientation = model.session.dock.cfg.orientation;
    for out in &model.outputs {
        if Some(out.key) != model.output && !model.locked {
            windows.push(view::edge::window(
                out.key,
                Size::new(out.width, out.height),
                orientation,
                message,
            ));
        }
    }
    let icons = |key: &(IconKey, u32)| model.icons.get(key).cloned().unwrap_or_default();
    let set = |key: IconKey, sides: &[f64]| -> IconSet {
        sides
            .iter()
            .flat_map(|&s| icons(&(key.clone(), s.ceil().max(1.0) as u32)))
            .collect()
    };
    let sides = model.tile_sides();
    if let (Some(output), Some(screen), Some(key)) = (model.output, model.screen(), model.bar_key())
    {
        let layout = model.session.dock.layout(screen, now);
        let tiles: Vec<view::bar::TileView> = layout
            .tiles
            .iter()
            .filter(|t| t.presence > 0.01)
            .map(|t| view::bar::TileView {
                id: t.id.0,
                kind: t.kind,
                rect: view::kit_rect(&t.visible_rect(orientation)),
                rest: view::kit_rect(&t.rect),
                opacity: t.presence,
                indicator: t.indicator,
                icon: model
                    .tile_icon(t.id)
                    .map(|k| set(k, &sides))
                    .unwrap_or_default(),
                name: tile_name(&model.session, t.id),
                description: tile_description(model, t.id, t.lifecycle),
            })
            .collect();
        let bar = layout.bar.intersection(&screen).map_or_else(
            || cupertino_kit::geometry::Rect::new(0.0, 0.0, 0.0, 0.0),
            |r| view::kit_rect(&r),
        );
        let input = model
            .session
            .dock
            .input_rects(screen, now)
            .iter()
            .map(view::kit_rect)
            .collect();
        windows.push(view::bar::window(
            view::bar::BarView {
                key: key.clone(),
                output,
                size: Size::new(screen.w, screen.h),
                orientation,
                tile_size: model.session.dock.cfg.tile_size,
                bar,
                tiles,
                input,
                revision: model.revision,
            },
            message,
        ));
        if let Some(menu) = &model.menu {
            if let Some(anchor) = layout.tiles.iter().find(|t| t.id == menu.tile) {
                windows.extend(view::menu::windows(
                    view::menu::MenuView {
                        parent: key.clone(),
                        items: &menu.items,
                        label: menu.label.clone(),
                        tile: menu.tile.0,
                        anchor: view::kit_rect(&anchor.visible_rect(orientation)),
                        orientation,
                    },
                    message,
                ));
            }
        }
        let poof = model
            .poof
            .and_then(|p| p.progress(now).map(|progress| (p.centre, progress)));
        if layout.dragging.is_some() || poof.is_some() {
            if let (Some(out), Some(in_bar)) = (model.output_info(output), model.output_in_bar()) {
                // the bar's window sits at (-x, -y) of the output
                let (ox, oy) = (-in_bar.x, -in_bar.y);
                let held = layout.dragging.as_ref().map(|d| view::drag::Held {
                    rect: view::kit_rect(&d.tile.rect.translate(ox, oy)),
                    icon: match d.tile.kind {
                        TileKind::Trash => set(IconKey::Trash(model.trash_full), &sides),
                        _ => set(IconKey::App(d.key.clone()), &sides),
                    },
                    opacity: d.opacity,
                    label: d.label,
                    indicator: d.indicator,
                });
                let puff = poof.map(|(centre, progress)| view::drag::Puff {
                    centre: kit_point(Point::new(centre.x + ox, centre.y + oy)),
                    size: model.session.dock.cfg.tile_size,
                    progress,
                });
                windows.push(view::drag::window(
                    view::drag::DragView {
                        output,
                        size: Size::new(out.width, out.height),
                        orientation,
                        held,
                        puff,
                        revision: model.revision,
                    },
                    message,
                ));
            }
        }
    }
    if let Some(grid) = &model.grid {
        if let Some(out) = model.output_info(grid.output) {
            let lookup = |name: &str| -> IconSet {
                model
                    .icons
                    .iter()
                    .filter(|((k, _), _)| matches!(k, IconKey::Named(n) if n == name))
                    .flat_map(|(_, set)| set.iter().cloned())
                    .collect()
            };
            windows.push(view::grid::window(
                view::grid::GridView {
                    output: grid.output,
                    grid: &grid.grid,
                    layout: grid.layout,
                    now,
                    icons: &lookup,
                    size: Size::new(out.width, out.height),
                },
                message,
            ));
        }
    }
    if let Some(open) = &model.force_quit {
        let rows = match &open.layout {
            force_quit::Layout::List(list) => list
                .rows
                .iter()
                .filter_map(|&(i, rect)| {
                    let e = open.dialog.entries().get(i)?;
                    Some(view::force_quit::RowView {
                        index: i,
                        rect: view::kit_rect(&rect),
                        name: e.name.clone(),
                        unresponsive: e.unresponsive,
                        selected: open.dialog.is_selected(i),
                        hovered: open.hover_row == Some(i),
                        icon: set(IconKey::App(e.app_id.clone()), &[FORCE_QUIT_ROW_ICON]),
                    })
                })
                .collect(),
            force_quit::Layout::Question(_) => Vec::new(),
        };
        let confirming = open.dialog.confirming();
        let question = confirming
            .as_ref()
            .map_or_else(String::new, |entries| force_quit::question(entries));
        let question_icon = confirming
            .as_ref()
            .and_then(|entries| entries.first())
            .map(|e| set(IconKey::App(e.app_id.clone()), &[force_quit::ICON_SIZE]))
            .unwrap_or_default();
        windows.push(view::force_quit::window(
            view::force_quit::ForceQuitView {
                output: open.output,
                layout: open.layout.clone(),
                rows,
                empty: open.dialog.entries().is_empty(),
                can_force_quit: open.dialog.can_force_quit(),
                question,
                question_icon,
                hover_button: open.hover_button,
            },
            message,
        ));
    }
    if let Some(open) = &model.switcher {
        if open.switcher.is_shown(now) {
            let name = open
                .switcher
                .apps()
                .get(open.switcher.selected())
                .map(|app| {
                    model
                        .session
                        .desktop()
                        .find(app)
                        .map_or_else(|| app.clone(), |e| e.name.clone())
                })
                .unwrap_or_default();
            windows.push(bridge::switcher(
                bridge::Painted {
                    output: open.output,
                    size: Size::new(open.layout.panel.w, open.layout.panel.h),
                    image: open.image.clone(),
                    alpha: open.switcher.alpha(now),
                    name,
                },
                SwitcherLayout::RADIUS,
                message,
            ));
        }
    }
    if let Some(open) = &model.overview {
        if let Some(out) = model.output_info(open.output) {
            windows.push(bridge::overview(
                bridge::Painted {
                    output: open.output,
                    size: Size::new(out.width, out.height),
                    image: open.image.clone(),
                    alpha: 1.0,
                    name: bridge::OVERVIEW_TITLE.to_owned(),
                },
                message,
            ));
        }
    }
    windows
}
