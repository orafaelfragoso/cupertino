//! The dock as a CupertinoKit application: the model around [`Session`] and the surfaces that come and go (the context
//! menu, the held tile, the Force Quit dialog, and on every output the switcher's panel
//! ([`SwitcherSurface`]) and the overview's chrome ([`OverviewSurface`])), what each message does to it, and its
//! windows ([`view`]). Every window is drawn by CupertinoKit. What needs the session (the dock role's requests, the
//! layer surfaces' placement and keyboard, starting programs, the pins file and the icons) goes through [`Host`], so
//! the model runs headless in tests.
//!
//! What moves by itself (a bounce, the magnification, auto-hide, the removal puff, the switcher's fade, the
//! overview's reflow and highlight) advances with the frames the compositor gives the window it is on: that window's
//! root sends [`Input::Frame`] at each one while it moves. A wake every [`FRAME_FALLBACK`] seconds keeps an animation
//! going on a window the compositor gives no frames (covered, or not shown yet); what is due at a set time (the
//! switcher's delay, a held press, the edge dwell) is a timer.
//!
//! The app-grid action (a hot corner or gesture mapped to it) opens the launcher's application browser: the dock
//! sends it to the server as `run_action("app-grid")`, which asks the launcher.
//!
//! The dock is on one output at a time: the first output when it starts (the main display), and the first remaining
//! one when its output goes away. Every other output has an edge strip along the dock's edge; the pointer resting
//! against that edge for [`EDGE_DWELL`] seconds moves the dock there. Each strip also holds the dock role of its output,
//! so the hot corners, gestures, the overview and the switcher of every output reach the process; the window feed is
//! taken from the bar's role only.

use std::collections::{BTreeMap, HashMap, HashSet};

use cupertino_defaults::dock::DockSettings;
use cupertino_kit::core::{Command, Key, Window, WindowKey};
use cupertino_kit::geometry::Size;
use cupertino_kit::platform::{KeyEvent, Keysym, Mods};
use cupertino_wm::geometry::{Point, Rect};

use crate::actions::{ActionMap, ActionTarget, Trigger};
use crate::conf::{dock_config, FILE_MANAGER};
use crate::config::Orientation;
use crate::desktop::DesktopIndex;
use crate::fan::{self, FanLayout};
use crate::force_quit::{self, Button, Dialog, Entry, Extend};
use crate::menu::{self, MenuCommand, MenuItem, TileFacts};
use crate::overview_kit::{OverviewInput, OverviewRequest, OverviewSurface};
use crate::overview_ui::{Mode, WindowEntry, Workspace};
use crate::session::{Activation, Session};
use crate::stack::{self, StackEntry, StackHit, StackLayout};
use crate::switcher_kit::{SwitcherEntry, SwitcherInput, SwitcherRequest, SwitcherSurface};
use crate::tile_drag::{Poof, TilePress};
use crate::tiles::{TileId, TileKind, TileLifecycle};
use crate::view::{self, IconSet, Input, Pointer, Wm};
use crate::DragOutcome;

/// How long the pointer rests against another output's edge before the dock moves there, in seconds.
pub const EDGE_DWELL: f64 = 0.4;

/// While something moves, the interval of the wake that advances it when its window gets no frames from the
/// compositor, in seconds.
pub const FRAME_FALLBACK: f64 = 0.1;

/// How long the trash's state is trusted before the folder is looked at again, in seconds.
pub const TRASH_CHECK_EVERY: f64 = 1.0;

/// Device pixels per point the icons are loaded for, so they stay sharp at every output scale.
pub const ICON_SCALES: [f64; 3] = [1.0, 2.0, 3.0];

/// How long after asking an application to open (a launch, or a new window of its resident process) a second ask is
/// dropped while no window of it has appeared, in seconds: a double click opens it once.
pub const OPEN_GUARD: f64 = 5.0;

/// How long a launch waits for its activation token, in seconds: past it the program starts without one, as when the
/// compositor offers none, and a token that comes later is dropped.
pub const TOKEN_TIMEOUT: f64 = 2.0;

/// How much a bouncing tile dims at the top of a hop under Reduce Motion, where it stays in place.
pub const REDUCED_BOUNCE_DIM: f64 = 0.5;

/// The server action a hot corner or gesture mapped to the application browser asks for: the launcher answers it.
pub const APP_GRID_ACTION: &str = "app-grid";

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
/// What the question before the trash is emptied says: its title, what happens, and its two buttons.
pub const EMPTY_TRASH_TITLE: &str = cupertino_trash::EMPTY_TITLE;
pub const EMPTY_TRASH_MESSAGE: &str = cupertino_trash::EMPTY_MESSAGE;
pub const EMPTY_TRASH: &str = cupertino_trash::EMPTY_BUTTON;
pub const CANCEL: &str = cupertino_trash::CANCEL_BUTTON;
/// What the name over the tile of an application that runs without windows says under the name.
pub const NO_WINDOWS: &str = "No open windows";

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
    /// Whether a process of the application that showed a window is still connected, with or without windows now.
    Resident {
        app_id: String,
        resident: bool,
    },
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

/// An `org.freedesktop.Application.Activate` call under way ([`Host::dbus_activate`]): whether it succeeded.
pub type BusCall = std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send>>;

/// A program the dock started ([`Host::spawn`]): ends when its process exits, in whether it failed (an unsuccessful
/// exit status or a signal).
pub type Exited = std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send>>;

/// Icons loading away from the model ([`Host::preload`]): ends in each one's key, side in whole points and renditions.
pub type IconLoad =
    std::pin::Pin<Box<dyn std::future::Future<Output = Vec<(IconKey, u32, IconSet)>> + Send>>;

/// A folder being listed away from the model ([`Host::folder_entries`]): ends in the entries its stack shows.
pub type FolderListing =
    std::pin::Pin<Box<dyn std::future::Future<Output = Vec<StackEntry>> + Send>>;

/// Where the bar's layer surface is: its output, its edge, its depth and the area it keeps windows out of.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub output: u32,
    pub orientation: Orientation,
    /// Depth of the surface in points: room for the magnified tiles and the bounces.
    pub extent: f64,
    pub exclusive_zone: i32,
    /// Depth of the bar itself in points, whether or not it reserves room.
    pub bar_depth: f64,
}

/// The layer surfaces whose keyboard interactivity the model changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// The bar on the output with this key, while it is engaged.
    Bar(u32),
    /// The overview's chrome on the output with this key, while the overview is open.
    Overview(u32),
}

/// The surfaces whose size the compositor decides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sized {
    Bar(u32),
}

/// An icon the model shows.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum IconKey {
    /// The application of the tile or window with this key: its desktop entry's icon, the one its window set, else
    /// the generic application icon.
    App(String),
    /// A theme icon by name (a folder's), else the generic application icon.
    Named(String),
    /// The trash, empty or full.
    Trash(bool),
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
    /// Start `argv` with the activation token: when it started, what ends as its process exits.
    fn spawn(&mut self, argv: Vec<String>, token: Option<String>) -> Option<Exited>;
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
    /// The folders the bar shows as stacks, with the theme names of their icons, in order (the user's downloads
    /// folder); none by default.
    fn folders(&mut self) -> Vec<(String, String)> {
        Vec::new()
    }
    /// List `folder` away from the model: what ends in the entries its stack shows, the newest first, at most
    /// [`stack::MAX_ENTRIES`] ([`stack::newest_first`]). A large or slow folder holds up nothing else.
    fn folder_entries(&mut self, folder: &str) -> FolderListing {
        let _ = folder;
        Box::pin(std::future::ready(Vec::new()))
    }
    /// The desktop file IDs of the recently used applications, the most recent first: the bar's recents section.
    fn recent_apps(&mut self) -> Vec<String> {
        Vec::new()
    }
    /// Ask the application on the bus name `bus_name` for a window (`org.freedesktop.Application.Activate`), handing
    /// it the activation token: the call under way, which ends in whether it succeeded, or `None` when it cannot be
    /// made. Where it cannot be made or fails, the model starts the entry again.
    fn dbus_activate(&mut self, bus_name: &str, token: Option<String>) -> Option<BusCall> {
        let _ = (bus_name, token);
        None
    }
    /// The renditions of `icon` at `side` points for each of [`ICON_SCALES`].
    fn icon(&mut self, desktop: &DesktopIndex, icon: &IconKey, side: f64) -> IconSet;
    /// Load `icons`, each at its side in points, away from the model, as [`Host::icon`] would: what ends in those it
    /// could load from an icon theme or the assets (an application's own window image stays for [`Host::icon`]), or
    /// `None` when the host loads only on the spot.
    fn preload(&mut self, desktop: &DesktopIndex, icons: Vec<(IconKey, f64)>) -> Option<IconLoad> {
        let _ = (desktop, icons);
        None
    }
    /// A window set its own icon, or removed it; `apps` are its application id and the dock's key for it.
    fn window_icon(&mut self, apps: [String; 2], icon: WindowIcon);
    fn window_closed(&mut self, id: u32);
    /// Whether the trash holds anything.
    fn trash_full(&mut self) -> bool;
    /// Delete everything in the trash away from the model: what ends in whether it all went.
    fn empty_trash(&mut self) -> BusCall {
        Box::pin(async { false })
    }
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
    /// The program started for tile `key` at `asked` (when the open was asked for) exited; `failed` is whether it
    /// failed.
    Exited {
        key: String,
        asked: f64,
        failed: bool,
    },
    /// The folder of the stack waiting for listing `listing` was listed.
    FolderListed {
        listing: u64,
        entries: Vec<StackEntry>,
    },
    /// Icons loaded in the background ([`Host::preload`]): those asked for, by key and side in whole points, and
    /// those loaded.
    IconsLoaded {
        asked: Vec<(IconKey, u32)>,
        loaded: Vec<(IconKey, u32, IconSet)>,
    },
    /// The bus call that asked the resident application of tile `key` for a window ended; `ok` is whether it
    /// succeeded.
    BusActivated {
        key: String,
        ok: bool,
    },
    /// The trash was emptied ([`Host::empty_trash`]).
    TrashEmptied,
    /// Something was put in the trash or taken out of it.
    TrashChanged,
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
                | Message::TrashChanged
        )
    }
}

impl From<Input> for Message {
    fn from(input: Input) -> Self {
        Message::Input(input)
    }
}

/// What is waiting for an activation token.
enum Opening {
    /// A program to start; its tile bounces once it started when `bounce` is set.
    Spawn { argv: Vec<String>, bounce: bool },
    /// A D-Bus activatable application to ask for a window, by its bus name.
    Activate(String),
}

/// An application waiting for its activation token.
struct Pending {
    /// Key of the tile of the application.
    key: String,
    opening: Opening,
    /// When the token was asked for: past [`TOKEN_TIMEOUT`] the launch goes ahead without it.
    asked: f64,
}

/// The question the Empty Trash item asks before it deletes, open over the trash's tile.
struct OpenConfirm {
    tile: TileId,
}

/// A fan closing: when it began and how far out it was then.
#[derive(Clone, Copy)]
struct Closing {
    start: f64,
    from: f64,
}

/// A folder's stack open over its tile.
struct OpenStack {
    tile: TileId,
    folder: String,
    name: String,
    /// The listing it waits for ([`Host::folder_entries`]), by number: a listing for a stack closed meanwhile is
    /// dropped.
    listing: u64,
    /// Its entries, once listed; it shows from then.
    entries: Option<Vec<StackEntry>>,
    /// When it showed: it comes in over [`stack::ENTRY_S`] from then, or a fan over [`fan::OPEN_S`].
    opened: f64,
    /// Fan or grid, decided when it was listed.
    shape: fan::Shape,
    /// The fan has begun to close; it is gone once [`fan::CLOSE_S`] has passed.
    closing: Option<Closing>,
    /// The fan's row the pointer is on, and the one the arrow keys moved to.
    hover: Option<usize>,
    selected: Option<usize>,
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

/// The window-manager surfaces of one output.
#[derive(Default)]
struct WmSurfaces {
    switcher: SwitcherSurface,
    overview: OverviewSurface,
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
    /// The same for the list of resident applications.
    awaiting_residents: bool,
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
    menu: Option<OpenMenu>,
    confirm: Option<OpenConfirm>,
    stack: Option<OpenStack>,
    /// The tile the left button is held on, and its icon darkened: the pressed tile shows it until the button comes up
    /// or the pointer leaves the tile.
    held: Option<(TileId, IconSet)>,
    /// The folders the host shows as stacks were asked for, and their icons' theme names by path.
    folders_asked: bool,
    folder_icons: HashMap<String, String>,
    /// The bar has the keyboard.
    pub engaged: bool,
    /// Focus Dock came: the first tile takes the focus.
    focus_first: bool,
    /// A menu opened while the bar had the keyboard, and the leave its taking the keyboard brings has not come yet.
    menu_takes_keyboard: bool,
    /// The tile that has the bar's keyboard focus, as its window last said.
    focused_tile: Option<TileId>,
    /// The tiles that take the focus, in the bar's order, as the view last showed them.
    tile_order: Vec<TileId>,
    /// The applications that stopped answering (their ids), as the server last said.
    unresponsive: HashSet<String>,
    force_quit: Option<ForceQuit>,
    /// The switcher's panel and the overview's chrome of every output, by output key.
    wm: BTreeMap<u32, WmSurfaces>,
    pending: HashMap<u64, Pending>,
    next_launch: u64,
    /// The number the next folder listing goes by.
    next_listing: u64,
    /// The applications asked to open and when, by tile key, until a window of theirs appears ([`OPEN_GUARD`]).
    opening: HashMap<String, f64>,
    /// The edge strip the pointer is on, and since when.
    edge: Option<(u32, f64)>,
    /// The icons loaded, by key and side in whole points.
    icons: HashMap<IconKey, BTreeMap<u32, IconSet>>,
    /// Whether the host loads icons in the background ([`Host::preload`]); `None` until it was first asked.
    background_icons: Option<bool>,
    /// The icons asked for in the background and not arrived yet, by key and side in whole points.
    requested: HashSet<(IconKey, u32)>,
    /// What the bar's recents were last worked out from: the application tiles and the applications that quit. The
    /// launcher's history is read again when it changes.
    recents_basis: Option<Vec<String>>,
    /// The name of the desktop entry of each application key looked up so far (`None` without one): the entries do
    /// not change while the process runs.
    names: HashMap<String, Option<String>>,
    trash_checked: Option<f64>,
    trash_full: bool,
    wake_generation: u64,
    wake_at: Option<f64>,
    // what the host was last told
    placed: Option<Placement>,
    keyboard: Option<Part>,
    /// Bumped whenever the host changes the bar's placement or a surface's keyboard interactivity, which apply with the
    /// surface's next commit: the bar presents again, so they reach the server even when nothing it draws changed.
    surface_changes: u64,
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
            awaiting_residents: true,
            dark: false,
            reduce_motion: false,
            locked: false,
            actions,
            conf,
            pointer: None,
            press: None,
            poof: None,
            menu: None,
            confirm: None,
            stack: None,
            held: None,
            folders_asked: false,
            folder_icons: HashMap::new(),
            engaged: false,
            focus_first: false,
            menu_takes_keyboard: false,
            focused_tile: None,
            tile_order: Vec::new(),
            unresponsive: HashSet::new(),
            force_quit: None,
            wm: BTreeMap::new(),
            pending: HashMap::new(),
            next_launch: 0,
            next_listing: 0,
            opening: HashMap::new(),
            edge: None,
            icons: HashMap::new(),
            background_icons: None,
            requested: HashSet::new(),
            recents_basis: None,
            names: HashMap::new(),
            trash_checked: None,
            trash_full: false,
            wake_generation: 0,
            wake_at: None,
            placed: None,
            keyboard: None,
            surface_changes: 0,
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
            bar_depth: self.session.dock.bar_depth().ceil(),
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
        let first_group = from_bar && self.awaiting_residents;
        let mut residents = std::collections::BTreeSet::new();
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
                } => {
                    self.opening.remove(&self.session.key_of(&app_id));
                    self.session.window(id, &app_id, &title, minimized, now)
                }
                DockEvent::Resident { app_id, resident } => {
                    if resident {
                        residents.insert(app_id.clone());
                    } else {
                        self.opening.remove(&self.session.key_of(&app_id));
                    }
                    self.session.set_resident(&app_id, resident, now);
                }
                DockEvent::WindowIcon(icon) => {
                    if let Some(w) = self.session.window_info(icon.id) {
                        let apps = [w.app_id.clone(), w.key.clone()];
                        self.icons.retain(|k, _| !matches!(k, IconKey::App(_)));
                        self.host.window_icon(apps, icon);
                    }
                }
                DockEvent::WindowClosed(id) => {
                    if self.session.window_info(id).is_some() {
                        self.icons.retain(|k, _| !matches!(k, IconKey::App(_)));
                    }
                    self.host.window_closed(id);
                    self.session.window_closed(id, now);
                }
                DockEvent::Launching(app_id) => self.session.launching(&app_id, now),
                DockEvent::ToggleAutoHide => self.session.dock.toggle_autohide(now),
                // an older server's forward of the app-grid action, which the launcher now answers
                DockEvent::ToggleAppGrid => {}
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
                DockEvent::OverviewClosed => self.overview_closed(output),
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
                DockEvent::Engage if !self.locked => {
                    self.engaged = true;
                    self.focus_first = true;
                }
                DockEvent::Engage => {}
            }
        }
        if from_bar {
            if first_group {
                // the first group of a new role lists every resident application
                self.awaiting_residents = false;
                self.session.residents_listed(&residents, now);
            }
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
                        self.held = None;
                        self.session.dock.begin_drag(id, origin, p, screen, now);
                    } else if self.session.dock.is_dragging() {
                        self.session.dock.drag_move(p, screen, now);
                    }
                }
                // the darkening goes when the pointer leaves the tile it is held on
                if let Some((id, _)) = &self.held {
                    let over = self.session.dock.hit_test(p, screen, now).map(|h| h.id);
                    if over != Some(*id) {
                        self.held = None;
                    }
                }
                Vec::new()
            }
            Pointer::Leave => {
                self.pointer = None;
                self.held = None;
                Vec::new()
            }
            Pointer::Press(p, _) => {
                let p = at(p);
                // a release was lost (the server ended the grab): the drag of that press is over
                self.session.dock.cancel_drag();
                let hit = self.session.dock.hit_test(p, screen, now);
                self.held = hit
                    .as_ref()
                    .filter(|h| h.kind != TileKind::Separator)
                    .map(|h| (h.id, self.darkened_icon(h.id)));
                match hit {
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
        self.held = None;
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

    /// Look the desktop entry's name of every key in `keys` up once.
    fn learn_names<'k>(&mut self, keys: impl IntoIterator<Item = &'k str>) {
        for key in keys {
            if !self.names.contains_key(key) {
                let name = self.session.desktop().find(key).map(|e| e.name.clone());
                self.names.insert(key.to_owned(), name);
            }
        }
    }

    /// The name of the desktop entry of the application `key`, as [`Self::learn_names`] looked it up.
    fn entry_name(&self, key: &str) -> Option<&str> {
        self.names.get(key).and_then(|name| name.as_deref())
    }

    /// Whether the application of tile `id` runs without windows.
    fn windowless(&self, id: TileId) -> bool {
        self.session.dock.tile(id).is_some_and(|t| {
            t.kind == TileKind::App && t.running && !self.session.has_windows(&t.key)
        })
    }

    /// How much of its colour a pressed tile's icon keeps.
    const PRESSED_ICON: f32 = 0.405;

    /// The icon of tile `id` darkened as a pressed tile shows it.
    fn darkened_icon(&self, id: TileId) -> IconSet {
        let icon = self
            .tile_icon(id)
            .map(|key| self.icon(&key, &self.drawn_sides()))
            .unwrap_or_default();
        view::paint::darkened(&icon, Self::PRESSED_ICON)
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
            Some(Activation::Reopen { key, .. }) => self.reopen(key),
            Some(Activation::OpenTrash) => self.open_in_file_manager(TRASH_URI),
            Some(Activation::ToggleStack(id)) => self.toggle_stack(id),
            None => Vec::new(),
        }
    }

    /// Open the stack of folder tile `id` over it, or close it when it is the one open (or still being listed).
    /// Opening it closes a menu; it shows once its folder is listed ([`Model::folder_listed`]).
    fn toggle_stack(&mut self, id: TileId) -> Vec<Command<Message>> {
        if self.stack.as_ref().is_some_and(|s| s.tile == id) {
            self.close_stack();
            return Vec::new();
        }
        self.stack = None;
        let Some(tile) = self.session.dock.tile(id) else {
            return Vec::new();
        };
        let folder = tile.key.clone();
        self.menu = None;
        self.press = None;
        let listing = self.next_listing;
        self.next_listing += 1;
        let listed = self.host.folder_entries(&folder);
        self.stack = Some(OpenStack {
            tile: id,
            name: stack::folder_name(&folder),
            folder,
            listing,
            entries: None,
            opened: self.now(),
            shape: fan::Shape::Grid,
            closing: None,
            hover: None,
            selected: None,
        });
        vec![Command::Perform(Box::pin(async move {
            Message::FolderListed {
                listing,
                entries: listed.await,
            }
        }))]
    }

    /// The folder of the stack waiting for `listing` was listed: the stack shows, coming in from now. A listing for a
    /// stack that closed, or was opened again, meanwhile is dropped.
    fn folder_listed(&mut self, listing: u64, mut entries: Vec<StackEntry>) {
        let now = self.now();
        let Some(tile) = self
            .stack
            .as_ref()
            .filter(|s| s.listing == listing && s.entries.is_none())
            .map(|s| s.tile)
        else {
            return;
        };
        entries.truncate(stack::MAX_ENTRIES);
        let shape = self.stack_shape(tile, entries.len());
        if let Some(open) = self.stack.as_mut() {
            open.entries = Some(entries);
            open.opened = now;
            open.shape = shape;
        }
    }

    /// Fan or grid for the stack of `tile` with `count` entries: the setting's choice (`dock.conf`'s `stack`), a grid
    /// for a folder with more entries than the fan holds and where the fan has no room.
    fn stack_shape(&self, tile: TileId, count: usize) -> fan::Shape {
        let bottom = self.session.dock.cfg.orientation == Orientation::Bottom;
        let room = self.screen().map_or(0, |screen| {
            let layout = self.session.dock.layout(screen, self.now());
            fan_basis(&layout, screen, tile).map_or(0, |b| fan::room(screen.h, b.above_screen))
        });
        fan::shape(self.conf.stack.unwrap_or_default(), count, room, bottom)
    }

    /// The layout of the fan of the open stack, as the bar lays out now.
    fn fan_layout(&self) -> Option<FanLayout> {
        let open = self.stack.as_ref()?;
        let entries = open.entries.as_ref()?;
        let screen = self.screen()?;
        let layout = self.session.dock.layout(screen, self.now());
        let basis = fan_basis(&layout, screen, open.tile)?;
        Some(FanLayout::new(
            &fan::names(entries),
            stack::OPEN_FOLDER,
            basis.lowest,
        ))
    }

    /// Close the open stack: a fan folds back into its tile over [`fan::CLOSE_S`], a grid goes at once.
    fn close_stack(&mut self) {
        let now = self.now();
        let motion = !self.reduce_motion;
        let Some(open) = self.stack.as_mut() else {
            return;
        };
        if open.closing.is_some() {
            return;
        }
        if open.shape == fan::Shape::Fan && open.entries.is_some() && motion {
            let from = fan::progress(now, open.opened, None, false);
            open.closing = Some(Closing { start: now, from });
        } else {
            self.stack = None;
        }
    }

    /// Whether a stack, still moving in or out, wants the next frame.
    fn stack_moving(&self, now: f64) -> bool {
        self.stack.as_ref().is_some_and(|s| {
            s.entries.is_some()
                && s.shape == fan::Shape::Fan
                && fan::moving(now, s.opened, s.closing.map(|c| c.start))
        })
    }

    /// The tile whose name shows over it, and the name: the tile under the pointer, while no menu, stack, press or drag
    /// is on the bar. A separator has none.
    fn label(
        &self,
        layout: &crate::dock::DockLayout,
    ) -> Option<(TileId, String, Option<&'static str>)> {
        // an ordinary click keeps the name; a drag, a menu or an open stack takes it down
        if self.menu.is_some()
            || self.confirm.is_some()
            || self.stack.is_some()
            || self.press.as_ref().is_some_and(|p| p.is_dragging())
            || layout.dragging.is_some()
            || self.locked
        {
            return None;
        }
        let id = self.session.dock.hovered()?;
        let tile = layout.tiles.iter().find(|t| t.id == id)?;
        if tile.kind == TileKind::Separator {
            return None;
        }
        let name = self.tile_name(id);
        let status = self.windowless(id).then_some(NO_WINDOWS);
        (!name.is_empty()).then_some((id, name, status))
    }

    /// The pointer on the open stack. A grid takes a click on its release: an entry opens and the stack closes, the
    /// button opens the folder. A fan highlights the row under the pointer and takes the same click; a click on none
    /// of its rows closes it.
    fn stack_pointer(&mut self, pointer: Pointer) -> Vec<Command<Message>> {
        let Some(open) = self.stack.as_ref().filter(|s| s.closing.is_none()) else {
            return Vec::new();
        };
        let Some(entries) = open.entries.as_ref() else {
            return Vec::new();
        };
        if open.shape == fan::Shape::Fan {
            return self.fan_pointer(pointer);
        }
        let Pointer::Release(p) = pointer else {
            return Vec::new();
        };
        match StackLayout::new(entries.len()).hit(Point::new(p.x, p.y), entries.len()) {
            Some(StackHit::Entry(i)) => self.open_stack_entry(i),
            Some(StackHit::OpenFolder) => self.open_stack_folder(),
            None => Vec::new(),
        }
    }

    /// The pointer on the open fan.
    fn fan_pointer(&mut self, pointer: Pointer) -> Vec<Command<Message>> {
        let Some(layout) = self.fan_layout() else {
            return Vec::new();
        };
        let row_at = |model: &Self, p: cupertino_kit::geometry::Point| {
            let entries = model.stack.as_ref()?.entries.as_ref()?;
            layout.hit(
                Point::new(p.x, p.y),
                &fan::names(entries),
                stack::OPEN_FOLDER,
            )
        };
        match pointer {
            Pointer::Move(p) => {
                let row = row_at(self, p);
                if let Some(open) = self.stack.as_mut() {
                    open.hover = row;
                    open.selected = None;
                }
                Vec::new()
            }
            Pointer::Leave => {
                if let Some(open) = self.stack.as_mut() {
                    open.hover = None;
                }
                Vec::new()
            }
            Pointer::Release(p) => match row_at(self, p) {
                Some(row) if row < layout.entries => self.open_stack_entry(row),
                Some(_) => self.open_stack_folder(),
                None => {
                    self.close_stack();
                    Vec::new()
                }
            },
            _ => Vec::new(),
        }
    }

    /// A key on the open stack that none of its parts took: Escape closes it, and on a fan the arrows, Home and End
    /// move the selection (the lowest row first), Return and Space choose the selected row.
    fn stack_key(&mut self, key: KeyEvent) -> Vec<Command<Message>> {
        if !key.pressed {
            return Vec::new();
        }
        let Some(open) = self.stack.as_mut().filter(|s| s.closing.is_none()) else {
            return Vec::new();
        };
        if key.keysym == ESCAPE {
            self.close_stack();
            return Vec::new();
        }
        let Some(entries) = open.entries.as_ref().map(Vec::len) else {
            return Vec::new();
        };
        if open.shape != fan::Shape::Fan {
            return Vec::new();
        }
        // the button is the row after the last entry
        let top = entries;
        let current = open.selected.or(open.hover);
        let moved = match key.keysym {
            UP => Some(current.map_or(0, |i| (i + 1).min(top))),
            DOWN => Some(current.map_or(0, |i| i.saturating_sub(1))),
            HOME => Some(0),
            END => Some(top),
            RETURN | KP_ENTER | SPACE => {
                return match current {
                    Some(row) if row < top => self.open_stack_entry(row),
                    Some(_) => self.open_stack_folder(),
                    None => Vec::new(),
                };
            }
            _ => None,
        };
        if let Some(row) = moved {
            open.selected = Some(row);
            open.hover = None;
        }
        Vec::new()
    }

    fn open_stack_entry(&mut self, index: usize) -> Vec<Command<Message>> {
        let Some(open) = self.stack.as_ref().filter(|s| s.closing.is_none()) else {
            return Vec::new();
        };
        let path = open
            .entries
            .as_ref()
            .and_then(|e| e.get(index))
            .map(|entry| entry.path.to_string_lossy().into_owned());
        if let Some(path) = path {
            self.host.open(&path);
        }
        self.close_stack();
        Vec::new()
    }

    fn open_stack_folder(&mut self) -> Vec<Command<Message>> {
        let Some(folder) = self
            .stack
            .as_ref()
            .filter(|s| s.closing.is_none())
            .map(|s| s.folder.clone())
        else {
            return Vec::new();
        };
        self.close_stack();
        self.open_in_file_manager(&folder)
    }

    /// The folders the host shows as stacks, asked for once.
    fn ask_folders(&mut self) {
        if std::mem::replace(&mut self.folders_asked, true) {
            return;
        }
        let folders = self.host.folders();
        let paths: Vec<&str> = folders.iter().map(|(p, _)| p.as_str()).collect();
        self.session.dock.set_folders(&paths);
        self.folder_icons = folders.into_iter().collect();
    }

    /// Open the application of tile `key` as a click on its tile does, also when it has no tile: raise its windows,
    /// ask its resident process for a window, or start it.
    fn open_app(&mut self, key: String) -> Vec<Command<Message>> {
        match self.session.opening(&key) {
            Activation::ActivateApp(app_id) => {
                self.bar_request(Request::ActivateApp(app_id));
                Vec::new()
            }
            Activation::Reopen { key, .. } => self.reopen(key),
            Activation::Launch(key) => self.begin_launch(key),
            Activation::RestoreWindow(_) | Activation::OpenTrash | Activation::ToggleStack(_) => {
                Vec::new()
            }
        }
    }

    /// Whether the application of tile `key` was asked to open a moment ago and shows no window yet: a second ask
    /// would open it twice.
    fn opening_now(&self, key: &str) -> bool {
        let now = self.now();
        self.pending
            .values()
            .any(|p| p.key == key && now - p.asked < TOKEN_TIMEOUT)
            || self
                .opening
                .get(key)
                .is_some_and(|&at| now - at < OPEN_GUARD)
    }

    /// Ask the resident process of tile `key` for a new window: through `org.freedesktop.Application.Activate` when its
    /// desktop entry is D-Bus activatable, else by starting the entry again (a running instance opens a window). The
    /// tile keeps its running dot and does not bounce.
    fn reopen(&mut self, key: String) -> Vec<Command<Message>> {
        if self.opening_now(&key) {
            return Vec::new();
        }
        match self.session.bus_name(&key) {
            Some(name) => {
                self.opening.insert(key.clone(), self.now());
                self.ask_token(key, Opening::Activate(name))
            }
            None => self.start(key, false),
        }
    }

    /// Open `target` (a folder or the trash) in the session's file manager like a launch of its tile (an activation
    /// token, so its window takes the focus, and a bounce); without it through the default handler.
    fn open_in_file_manager(&mut self, target: &str) -> Vec<Command<Message>> {
        match self.session.file_manager_command(target) {
            Some(argv) => self.ask_token(
                FILE_MANAGER.to_owned(),
                Opening::Spawn { argv, bounce: true },
            ),
            None => {
                self.host.open(target);
                Vec::new()
            }
        }
    }

    /// Fetch an activation token for the launch of the tile with `key`; the program starts when it arrives. A launch
    /// asked for again before the first one showed a window is dropped.
    fn begin_launch(&mut self, key: String) -> Vec<Command<Message>> {
        if self.opening_now(&key) {
            return Vec::new();
        }
        self.start(key, true)
    }

    /// Start the entry of tile `key` with an activation token, bouncing its tile when `bounce` is set.
    fn start(&mut self, key: String, bounce: bool) -> Vec<Command<Message>> {
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
        self.opening.insert(key.clone(), self.now());
        self.ask_token(key, Opening::Spawn { argv, bounce })
    }

    fn ask_token(&mut self, key: String, opening: Opening) -> Vec<Command<Message>> {
        let launch = self.next_launch;
        self.next_launch += 1;
        let asked = self.now();
        self.pending.insert(
            launch,
            Pending {
                key,
                opening,
                asked,
            },
        );
        let window = self
            .bar_key()
            .unwrap_or_else(|| view::bar_key(0, self.bar_generation));
        vec![Command::ActivationToken {
            window,
            app_id: None,
            on_token: Box::new(move |token| Message::Token(launch, token)),
        }]
    }

    /// Start the program that waited for `token` and bounce its tile, or ask the application for a window; an
    /// application that cannot be asked is started again with the token.
    fn complete(&mut self, launch: u64, token: Option<String>) -> Vec<Command<Message>> {
        let Some(Pending { key, opening, .. }) = self.pending.remove(&launch) else {
            return Vec::new();
        };
        match opening {
            Opening::Spawn { argv, bounce } => match self.host.spawn(argv, token) {
                Some(exited) => {
                    if bounce {
                        let now = self.now();
                        self.session.dock.app_launching(&key, now);
                    }
                    // the guard against a second launch ends with a process that fails: a program that crashes at once
                    // can be opened again straight away
                    let Some(&asked) = self.opening.get(&key) else {
                        return Vec::new();
                    };
                    vec![Command::Perform(Box::pin(async move {
                        let failed = exited.await;
                        Message::Exited { key, asked, failed }
                    }))]
                }
                None => {
                    self.opening.remove(&key);
                    Vec::new()
                }
            },
            Opening::Activate(name) => match self.host.dbus_activate(&name, token) {
                Some(call) => vec![Command::Perform(Box::pin(async move {
                    Message::BusActivated {
                        key,
                        ok: call.await,
                    }
                }))],
                None => {
                    self.opening.remove(&key);
                    self.start(key, false)
                }
            },
        }
    }

    /// Launches whose activation token did not come within [`TOKEN_TIMEOUT`] go ahead without one.
    fn expire_tokens(&mut self, now: f64) -> Vec<Command<Message>> {
        let mut late: Vec<u64> = self
            .pending
            .iter()
            .filter(|(_, p)| now - p.asked >= TOKEN_TIMEOUT)
            .map(|(launch, _)| *launch)
            .collect();
        late.sort_unstable();
        late.into_iter()
            .flat_map(|launch| self.complete(launch, None))
            .collect()
    }

    /// The program started for tile `key` when it was asked to open at `asked` exited: with no window of it shown,
    /// the guard against a second launch goes, so asking again starts it again. A later ask has a guard of its own.
    ///
    /// Only a failed process ends it: a launcher that hands the application on and exits successfully (a wrapper that
    /// starts it in the background, or one that passes the open to an instance already running) is followed by a
    /// window that has not come yet, and a second ask meanwhile would open the application twice.
    fn exited(&mut self, key: &str, asked: f64, failed: bool) {
        if failed && self.opening.get(key) == Some(&asked) && !self.session.has_windows(key) {
            self.opening.remove(key);
        }
    }

    /// The bus call for a window of tile `key` ended: when it failed and no window came meanwhile, start the entry
    /// again instead (a running instance opens a window).
    fn bus_activated(&mut self, key: String, ok: bool) -> Vec<Command<Message>> {
        if ok || self.session.has_windows(&key) {
            return Vec::new();
        }
        self.opening.remove(&key);
        self.start(key, false)
    }

    // ---- context menus --------------------------------------------------------------------------------------

    /// The menu of the tile or the separator.
    fn open_menu(&mut self, id: TileId) {
        self.menu = None;
        self.confirm = None;
        self.stack = None;
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
                trash_full: self.trash_full,
            })
        };
        if items.is_empty() {
            return;
        }
        let label = self.tile_name(id);
        self.menu = Some(OpenMenu {
            tile: id,
            items,
            label,
        });
        self.menu_takes_keyboard = self.engaged;
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
            MenuCommand::Open(key) => return self.open_app(key),
            MenuCommand::OpenTrash => return self.open_in_file_manager(TRASH_URI),
            // asked for before it gets here ([`Input::MenuItem`])
            MenuCommand::EmptyTrash => {}
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
        self.awaiting_residents = true;
        self.menu = None;
        self.press = None;
        self.poof = None;
        self.pointer = None;
        self.engaged = false;
        self.edge = None;
        self.session.dock.cancel_drag();
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
            .force_quit
            .as_ref()
            .is_some_and(|f| !there(f.output, &self.outputs))
        {
            self.force_quit = None;
        }
        // the surfaces of an output that went away go with it; a new output gets its own
        self.wm.retain(|&o, _| there(o, &self.outputs));
        for out in &self.outputs {
            self.wm.entry(out.key).or_insert_with(|| {
                let mut surfaces = WmSurfaces::default();
                surfaces.overview.set_reduce_motion(self.reduce_motion);
                surfaces
            });
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
            self.awaiting_residents = true;
            self.menu = None;
            self.press = None;
            self.engaged = false;
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
                    .entry_name(&t.key)
                    .map(str::to_owned)
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
        if self.force_quit.is_none() {
            return;
        }
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

    // ---- the application switcher --------------------------------------------------------------------------

    /// The server's list and selection for the panel on `output`: a step of the switch on screen moves the selection,
    /// anything else is a new switch, which ends one still waiting on another output.
    fn switcher_changed(&mut self, output: u32, apps: Vec<String>, selected: usize) {
        let now = self.now();
        let Some(out) = self.output_info(output) else {
            eprintln!("cupertino-dock: the output of the switcher went away");
            return;
        };
        self.learn_names(apps.iter().map(String::as_str));
        let entries = apps
            .into_iter()
            .map(|app_id| SwitcherEntry {
                name: self
                    .entry_name(&app_id)
                    .map_or_else(|| app_id.clone(), str::to_owned),
                app_id,
            })
            .collect();
        for (&o, surfaces) in self.wm.iter_mut().filter(|(&o, _)| o != output) {
            if surfaces.switcher.output() == Some(o) && !surfaces.switcher.is_done(now) {
                surfaces.switcher.closed(false, now);
            }
        }
        let Some(surfaces) = self.wm.get_mut(&output) else {
            return;
        };
        if let Err(e) =
            surfaces
                .switcher
                .changed(output, (out.width, out.height), entries, selected, now)
        {
            eprintln!("cupertino-dock: {e}");
        }
    }

    /// The switch ended. A panel that was on screen fades out after a commit; a cancel, or a commit before the panel
    /// appeared, removes it at once.
    fn switcher_closed(&mut self, committed: bool) {
        let now = self.now();
        for surfaces in self.wm.values_mut() {
            surfaces.switcher.closed(committed, now);
        }
    }

    /// What the panel on `output` reported; its requests go through the dock role of that output.
    fn switcher_input(&mut self, output: u32, input: SwitcherInput) {
        let now = self.now();
        let Some(surfaces) = self.wm.get_mut(&output) else {
            return;
        };
        for request in surfaces.switcher.input(input, now) {
            self.host.request(
                output,
                match request {
                    SwitcherRequest::Select(i) => Request::SwitcherSelect(i),
                    SwitcherRequest::Activate(i) => Request::SwitcherActivate(i),
                    SwitcherRequest::Cancel => Request::SwitcherCancel,
                },
            );
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
        let Some(surfaces) = self.wm.get_mut(&output) else {
            eprintln!("cupertino-dock: the output of the overview went away");
            return;
        };
        for (id, frame) in surfaces
            .overview
            .changed(mode, area, &entries, workspaces, now)
        {
            self.host.request(output, Request::OverviewFrame(id, frame));
        }
    }

    /// The server is leaving the overview of `output`: the chrome fades and gives the keyboard back.
    fn overview_closed(&mut self, output: u32) {
        let now = self.now();
        if let Some(surfaces) = self.wm.get_mut(&output) {
            surfaces.overview.closed(now);
        }
    }

    /// What the chrome on `output` reported; every action is the server's to apply.
    fn overview_input(&mut self, output: u32, input: OverviewInput) {
        let now = self.now();
        let Some(surfaces) = self.wm.get_mut(&output) else {
            return;
        };
        for request in surfaces.overview.input(input, now) {
            self.host.request(
                output,
                match request {
                    OverviewRequest::Select(id) => Request::OverviewSelect(id),
                    OverviewRequest::Close => Request::OverviewClose,
                    OverviewRequest::SwitchWorkspace(id) => Request::SwitchWorkspace(id),
                    OverviewRequest::AddWorkspace => Request::AddWorkspace,
                    OverviewRequest::RemoveWorkspace(id) => Request::RemoveWorkspace(id),
                    OverviewRequest::MoveWorkspace { id, index } => {
                        Request::MoveWorkspace { id, index }
                    }
                },
            );
        }
    }

    /// The output whose overview is open and holds the keyboard.
    fn overview_keyboard(&self) -> Option<u32> {
        self.wm
            .iter()
            .find(|(_, s)| s.overview.wants_keyboard())
            .map(|(&o, _)| o)
    }

    /// The compositor closed a window-manager surface of `output`.
    fn wm_closed(&mut self, output: u32, which: Wm) {
        if let Some(surfaces) = self.wm.get_mut(&output) {
            match which {
                Wm::Switcher => surfaces.switcher.lost(),
                Wm::Overview => surfaces.overview.lost(),
            }
        }
    }

    // ---- hot corners and gestures ---------------------------------------------------------------------------

    fn run_trigger(&mut self, output: u32, trigger: Trigger) -> Vec<Command<Message>> {
        match self.actions.get(trigger).clone() {
            ActionTarget::None => {}
            // the launcher's application browser, which the server asks for
            ActionTarget::AppGrid => self
                .host
                .request(output, Request::RunAction(APP_GRID_ACTION.to_owned())),
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
            self.recents_basis = None;
        }
        if before.minimize_to_app != after.minimize_to_app {
            cfg.minimize_to_app = target.minimize_to_app;
        }
        if before.bounce != after.bounce {
            cfg.launch_animation = target.launch_animation;
        }
        if before.indicators != after.indicators {
            cfg.show_running_indicators = target.show_running_indicators;
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
    fn icon_set(&mut self, key: &IconKey, sides: &[f64]) {
        for &side in sides {
            let px = side.ceil().max(1.0) as u32;
            let loaded = self.icons.get(key).is_some_and(|k| k.contains_key(&px));
            if !loaded {
                let renditions = self.host.icon(self.session.desktop(), key, f64::from(px));
                self.icons
                    .entry(key.clone())
                    .or_default()
                    .insert(px, renditions);
            }
        }
    }

    /// The loaded renditions of `key` at `sides`.
    fn icon(&self, key: &IconKey, sides: &[f64]) -> IconSet {
        let Some(loaded) = self.icons.get(key) else {
            return IconSet::new();
        };
        sides
            .iter()
            .filter_map(|&s| loaded.get(&(s.ceil().max(1.0) as u32)))
            .flat_map(|set| set.iter().cloned())
            .collect()
    }

    /// The sides the tiles are drawn at: resting, and fully magnified. While the magnified side loads in the background
    /// ([`Model::preload`]), a magnified tile draws the resting side's largest rendition.
    fn drawn_sides(&self) -> Vec<f64> {
        let cfg = &self.session.dock.cfg;
        vec![cfg.tile_size, cfg.large_size.max(cfg.tile_size)]
    }

    /// The sides loaded on the spot, before the bar shows: the resting one, and the magnified one too when the host
    /// loads nothing in the background.
    fn tile_sides(&self) -> Vec<f64> {
        if self.background_icons == Some(true) {
            vec![self.session.dock.cfg.tile_size]
        } else {
            self.drawn_sides()
        }
    }

    /// Ask the host to load in the background what is drawn later: the bar's icons at their magnified side, as tiles
    /// come, so the first magnification does not wait for them. Each icon at each side is asked for once.
    fn preload(&mut self) -> Vec<Command<Message>> {
        if self.background_icons == Some(false) {
            return Vec::new();
        }
        let cfg = &self.session.dock.cfg;
        let large = cfg.large_size.max(cfg.tile_size);
        let mut wanted: Vec<(IconKey, f64)> = self
            .session
            .dock
            .tiles()
            .iter()
            .filter(|t| t.kind != TileKind::Separator)
            .filter_map(|t| self.tile_icon(t.id))
            .map(|key| (key, large))
            .collect();
        let px = |side: f64| side.ceil().max(1.0) as u32;
        wanted.retain(|(key, side)| {
            !self
                .icons
                .get(key)
                .is_some_and(|k| k.contains_key(&px(*side)))
                && self.requested.insert((key.clone(), px(*side)))
        });
        if wanted.is_empty() {
            return Vec::new();
        }
        let asked: Vec<(IconKey, u32)> = wanted.iter().map(|(k, s)| (k.clone(), px(*s))).collect();
        match self.host.preload(self.session.desktop(), wanted) {
            Some(load) => {
                self.background_icons = Some(true);
                vec![Command::Perform(Box::pin(async move {
                    Message::IconsLoaded {
                        asked,
                        loaded: load.await,
                    }
                }))]
            }
            None => {
                self.background_icons = Some(false);
                self.requested.clear();
                Vec::new()
            }
        }
    }

    /// Icons loaded in the background arrived; those the host left to the spot (an application whose only icon is
    /// its window's) load now.
    fn icons_loaded(&mut self, asked: Vec<(IconKey, u32)>, loaded: Vec<(IconKey, u32, IconSet)>) {
        for (key, px, set) in loaded {
            self.icons.entry(key).or_default().entry(px).or_insert(set);
        }
        for (key, px) in asked {
            self.requested.remove(&(key.clone(), px));
            self.icon_set(&key, &[f64::from(px)]);
        }
    }

    /// The icon key of the tile with `id`.
    fn tile_icon(&self, id: TileId) -> Option<IconKey> {
        let tile = self.session.dock.tile(id)?;
        Some(match tile.kind {
            TileKind::Trash => IconKey::Trash(self.trash_full),
            TileKind::Folder => IconKey::Named(
                self.folder_icons
                    .get(&tile.key)
                    .cloned()
                    .unwrap_or_else(|| stack::FOLDER_ICON.to_owned()),
            ),
            _ => IconKey::App(tile.key.clone()),
        })
    }

    /// Load what the next view draws, and the names it shows, so the view only reads.
    fn prefill(&mut self) {
        let tiles: Vec<(TileId, TileKind, String)> = self
            .session
            .dock
            .tiles()
            .iter()
            .map(|t| (t.id, t.kind, t.key.clone()))
            .collect();
        self.learn_names(tiles.iter().map(|t| t.2.as_str()));
        let sides = self.tile_sides();
        let mut keys: Vec<IconKey> = tiles
            .iter()
            .filter(|t| t.1 != TileKind::Separator)
            .filter_map(|t| self.tile_icon(t.0))
            .collect();
        keys.dedup();
        for key in keys {
            self.icon_set(&key, &sides);
        }
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
        let switched: Vec<(String, f64)> = self
            .wm
            .values()
            .filter_map(|s| Some((s.switcher.apps()?, s.switcher.icon_side()?)))
            .flat_map(|(apps, side)| apps.iter().map(move |a| (a.clone(), side)))
            .collect();
        for (app, side) in switched {
            self.icon_set(&IconKey::App(app), &[side]);
        }
        if let Some(entries) = self.stack.as_ref().and_then(|s| s.entries.as_ref()) {
            let keys: Vec<IconKey> = entries
                .iter()
                .map(|e| IconKey::Named(e.icon.clone()))
                .collect();
            for key in keys {
                self.icon_set(&key, &[StackLayout::ICON]);
            }
        }
    }

    /// The bar's keyboard focus follows the model: Focus Dock focuses the first tile, with its ring; a focused tile
    /// that went away hands the focus to its neighbour (the next one, else the one before), never to the bar.
    fn focus_commands(&mut self) -> Vec<Command<Message>> {
        let order: Vec<TileId> = self
            .session
            .dock
            .tiles()
            .iter()
            .filter(|t| t.kind != TileKind::Separator)
            .map(|t| t.id)
            .collect();
        let before = std::mem::replace(&mut self.tile_order, order);
        let target = if self.engaged && std::mem::take(&mut self.focus_first) {
            self.tile_order.first().copied()
        } else {
            match self.focused_tile {
                Some(gone) if !self.tile_order.contains(&gone) => {
                    let at = before.iter().position(|&t| t == gone).unwrap_or(0);
                    before[at..]
                        .iter()
                        .chain(before[..at].iter().rev())
                        .copied()
                        .find(|t| self.tile_order.contains(t))
                        .or_else(|| self.tile_order.first().copied())
                }
                _ => None,
            }
        };
        self.focus_first &= self.engaged;
        let Some(tile) = target else {
            return Vec::new();
        };
        self.focused_tile = Some(tile);
        vec![Command::Focus(view::bar::tile_key(tile.0))]
    }

    /// Advance the animations, carry out what is due and tell the host what changed; then schedule the next wake.
    fn changed(&mut self) -> Vec<Command<Message>> {
        let now = self.now();
        // the locked session, the overview and the Force Quit dialog hold the keyboard while they are up: a Focus Dock
        // meanwhile is dropped, not kept for when they go
        if self.locked || self.force_quit.is_some() || self.overview_keyboard().is_some() {
            self.engaged = false;
        }
        // an auto-hidden bar shows while it has the keyboard or one of its menus or stacks is open
        self.session.dock.keep_shown =
            self.engaged || self.stack.is_some() || self.menu.is_some() || self.confirm.is_some();
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
        for surfaces in self.wm.values_mut() {
            surfaces.switcher.tick(now);
            surfaces.overview.tick(now);
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
        let mut commands = self.expire_tokens(now);
        self.ask_folders();
        if self
            .stack
            .as_ref()
            .is_some_and(|s| self.session.dock.tile(s.tile).is_none())
            || self
                .stack
                .as_ref()
                .and_then(|s| s.closing)
                .is_some_and(|c| now >= c.start + fan::CLOSE_S)
        {
            self.stack = None;
        }
        self.refresh_recents(now);
        self.tell_host();
        commands.extend(self.preload());
        self.prefill();
        commands.extend(self.focus_commands());
        commands.extend(self.schedule(now));
        commands
    }

    /// The bar's recents section: the applications that quit since the dock started, then the launcher's history, each
    /// without a tile among the applications, at most [`crate::config::DockConfig::recents_max`]. Worked out again, the
    /// history read again, only when an application gets or loses its tile or one quits.
    fn refresh_recents(&mut self, now: f64) {
        if !self.session.dock.cfg.show_recents {
            return;
        }
        let mut basis: Vec<String> = self
            .session
            .dock
            .tiles()
            .iter()
            .filter(|t| t.kind == TileKind::App && !t.is_leaving())
            .map(|t| t.key.clone())
            .collect();
        basis.push(String::new());
        basis.extend(self.session.recently_quit().iter().cloned());
        if self.recents_basis.as_ref() == Some(&basis) {
            return;
        }
        self.recents_basis = Some(basis);
        let history = self.host.recent_apps();
        let recents = self.session.recent_apps(&history);
        let keys: Vec<&str> = recents.iter().map(String::as_str).collect();
        self.session.dock.set_recents(&keys, now);
    }

    /// Tell the host what it has to know: where the bar is, the minimize target, who has the keyboard and the dialog's
    /// size.
    fn tell_host(&mut self) {
        let placement = self.placement();
        if let Some(p) = placement.filter(|_| placement != self.placed) {
            self.placed = placement;
            self.host.place_bar(p);
            self.surface_changes += 1;
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
        let keyboard = if let Some(output) = self.overview_keyboard() {
            Some(Part::Overview(output))
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
            self.surface_changes += 1;
        }
        let size = self.force_quit.as_ref().map(|f| f.layout.size());
        if let Some((w, h)) = size.filter(|_| size != self.force_quit_size) {
            self.host.resize_force_quit(w, h);
        }
        self.force_quit_size = size;
    }

    /// Whether the bar moves by itself: a bounce, the magnification, auto-hide, a drag settling or the removal puff.
    fn bar_moving(&self, now: f64) -> bool {
        self.session.dock.wants_tick(now) || self.poof.is_some()
    }

    /// Whether the window-manager surfaces of an output move by themselves now (they want the next frame).
    fn wm_moving(surfaces: &WmSurfaces, now: f64) -> bool {
        let soon = |at: Option<f64>| at.is_some_and(|at| at - now <= FRAME_FALLBACK);
        // the switcher's wake is its delay until it shows, or the next frame of its fade
        (soon(surfaces.switcher.next_wake(now)) && surfaces.switcher.is_shown(now))
            || surfaces.overview.next_wake(now).is_some()
    }

    /// Whether anything moves on its own now.
    fn animating(&self, now: f64) -> bool {
        self.bar_moving(now) || self.wm.values().any(|s| Self::wm_moving(s, now))
    }

    /// While something moves its window's frames drive it, with a fallback wake; otherwise one wake at the next thing
    /// that is due.
    fn schedule(&mut self, now: f64) -> Vec<Command<Message>> {
        let panel = self
            .wm
            .values()
            .filter(|s| !s.switcher.is_shown(now))
            .filter_map(|s| s.switcher.next_wake(now))
            .map(|at| (at - now).max(0.0));
        let hold = self.press.as_ref().and_then(|p| p.hold_due(now));
        let edge = self
            .edge
            .filter(|(o, _)| Some(*o) != self.output)
            .map(|(_, since)| (since + EDGE_DWELL - now).max(0.0));
        let moving = (self.animating(now) || self.stack_moving(now)).then_some(FRAME_FALLBACK);
        let token = self
            .pending
            .values()
            .map(|p| (p.asked + TOKEN_TIMEOUT - now).max(0.0))
            .reduce(f64::min);
        let due = panel
            .chain(hold)
            .chain(edge)
            .chain(moving)
            .chain(token)
            .reduce(f64::min);
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

const UP: Keysym = Keysym(0xff52);
const DOWN: Keysym = Keysym(0xff54);
const HOME: Keysym = Keysym(0xff50);
const END: Keysym = Keysym(0xff57);
const ESCAPE: Keysym = Keysym(0xff1b);
const SPACE: Keysym = Keysym(0x20);
const RETURN: Keysym = Keysym(0xff0d);
const KP_ENTER: Keysym = Keysym(0xff8d);

impl Model {
    /// What assistive technology calls the tile with `id`.
    pub fn tile_name(&self, id: TileId) -> String {
        let Some(tile) = self.session.dock.tile(id) else {
            return String::new();
        };
        match tile.kind {
            TileKind::Trash => TRASH_NAME.to_owned(),
            TileKind::Folder => stack::folder_name(&tile.key),
            TileKind::Separator => String::new(),
            TileKind::MinimizedWindow if !tile.title.is_empty() => tile.title.clone(),
            _ => match self.entry_name(&tile.key) {
                Some(name) => name.to_owned(),
                None if tile.title.is_empty() => tile.key.clone(),
                None => tile.title.clone(),
            },
        }
    }
}

/// Where the fan of a folder tile stands over the bar, in the bar's window.
struct FanBasis {
    /// The middle of the tile, which is where the fan's window ends below.
    tile_cx: f64,
    tile_cy: f64,
    /// How far above the tile's middle the lowest row's middle is, and how far above the output's bottom edge.
    lowest: f64,
    above_screen: f64,
}

/// Where the fan of `tile` stands in `layout`.
fn fan_basis(layout: &crate::dock::DockLayout, screen: Rect, tile: TileId) -> Option<FanBasis> {
    let t = layout.tiles.iter().find(|t| t.id == tile)?;
    let (cx, cy) = (t.rect.x + t.rect.w / 2.0, t.rect.y + t.rect.h / 2.0);
    let lowest = (cy - layout.bar.y) + fan::LOWEST_ABOVE_BAR;
    Some(FanBasis {
        tile_cx: cx,
        tile_cy: cy,
        lowest,
        above_screen: screen.bottom() - cy + lowest,
    })
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
            model.session.dock.set_reduce_motion(on);
            for surfaces in model.wm.values_mut() {
                surfaces.overview.set_reduce_motion(on);
            }
            Vec::new()
        }
        // the lock screen covers everything: the dialog and a menu go at once, without animation; the
        // overview's chrome waits for the server's `overview_closed`
        Message::Locked(locked) => {
            model.locked = locked;
            if locked {
                model.force_quit = None;
                model.menu = None;
                model.stack = None;
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
        Message::KeyboardLeft(output) => {
            // the menu taking the keyboard is the one leave that keeps Focus Dock; any other (the server giving the
            // keyboard to a window, a press outside closing the menu as well) ends it
            if Some(output) == model.output
                && !(model.menu.is_some() && std::mem::take(&mut model.menu_takes_keyboard))
            {
                model.engaged = false;
            }
            Vec::new()
        }
        Message::Token(launch, token) => model.complete(launch, token),
        Message::Exited { key, asked, failed } => {
            model.exited(&key, asked, failed);
            Vec::new()
        }
        Message::FolderListed { listing, entries } => {
            model.folder_listed(listing, entries);
            Vec::new()
        }
        Message::IconsLoaded { asked, loaded } => {
            model.icons_loaded(asked, loaded);
            Vec::new()
        }
        Message::BusActivated { key, ok } => model.bus_activated(key, ok),
        Message::TrashEmptied | Message::TrashChanged => {
            // the trash's icon follows what is left
            model.trash_checked = None;
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
            let tile = model.menu.as_ref().map(|m| m.tile);
            let command = model
                .menu
                .as_ref()
                .and_then(|m| m.items.get(i))
                .and_then(|item| item.command.clone());
            model.menu = None;
            match (command, tile) {
                // deleting for good asks first
                (Some(MenuCommand::EmptyTrash), Some(tile)) => {
                    model.engaged = false;
                    model.confirm = Some(OpenConfirm { tile });
                    Vec::new()
                }
                (Some(command), _) => model.run_menu_command(command),
                (None, _) => Vec::new(),
            }
        }
        Input::ConfirmClosed => {
            model.confirm = None;
            Vec::new()
        }
        Input::ConfirmDone => {
            model.confirm = None;
            let emptying = model.host.empty_trash();
            vec![Command::Perform(Box::pin(async move {
                let _ = emptying.await;
                Message::TrashEmptied
            }))]
        }
        Input::MenuClosed => {
            model.menu = None;
            Vec::new()
        }
        Input::Stack(pointer) => model.stack_pointer(pointer),
        Input::StackKey(key) => model.stack_key(key),
        Input::StackEntry(i) => model.open_stack_entry(i),
        Input::StackOpenFolder => model.open_stack_folder(),
        Input::StackClosed => {
            model.close_stack();
            Vec::new()
        }
        Input::LabelGone => Vec::new(),
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
        Input::Switcher(output, input) => {
            model.switcher_input(output, input);
            Vec::new()
        }
        Input::Overview(output, input) => {
            model.overview_input(output, input);
            Vec::new()
        }
        Input::WmClosed(output, which) => {
            model.wm_closed(output, which);
            Vec::new()
        }
        Input::TileFocused(id) => {
            model.focused_tile = Some(TileId(id));
            Vec::new()
        }
        // the update that follows every message advances what moves
        Input::Frame => Vec::new(),
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

/// The sides the switcher's panel on an output draws its icons at.
fn icon_sides(surfaces: &WmSurfaces) -> Vec<f64> {
    surfaces.switcher.icon_side().into_iter().collect()
}

/// A window-manager surface's window on the output with key `output`: a key and an `app_id` (the layer surface's
/// namespace and output, `<namespace>@<output>`) of its own, and the compositor closing it reported as lost.
fn on_output(mut window: Window<Message>, output: u32, which: Wm) -> Window<Message> {
    if let Key::Str(name) = &window.key.0 {
        window.key = WindowKey(Key::from(format!("{name}@{output}")));
    }
    window.spec.app_id = format!("{}@{output}", window.spec.app_id);
    window.on_close = Some(message(Input::WmClosed(output, which)));
    window
}

/// What the held tile's window draws, as a number that changes whenever any of it does: its window repaints then, and
/// only then.
fn drag_revision(held: Option<&view::drag::Held>, puff: Option<&view::drag::Puff>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let mut rect = |r: cupertino_kit::geometry::Rect| {
        for v in [r.x(), r.y(), r.width(), r.height()] {
            v.to_bits().hash(&mut h);
        }
    };
    if let Some(held) = held {
        rect(held.rect);
        for v in [held.opacity, held.label, held.indicator] {
            v.to_bits().hash(&mut h);
        }
        for source in &held.icon {
            source.id.0.hash(&mut h);
        }
    }
    if let Some(puff) = puff {
        for v in [puff.centre.x, puff.centre.y, puff.size, puff.progress] {
            v.to_bits().hash(&mut h);
        }
    }
    h.finish()
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
    let set = |key: IconKey, sides: &[f64]| model.icon(&key, sides);
    let sides = model.drawn_sides();
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
                // under Reduce Motion a bounce dims the tile in place instead of lifting it
                opacity: t.presence * (1.0 - REDUCED_BOUNCE_DIM * t.pulse),
                indicator: if t.indicator { t.indicator_alpha } else { 0.0 },
                windowless: t.indicator && model.windowless(t.id),
                chevron: model
                    .stack
                    .as_ref()
                    .is_some_and(|s| s.tile == t.id && s.entries.is_some()),
                icon: match &model.held {
                    Some((id, dark)) if *id == t.id && !dark.is_empty() => dark.clone(),
                    _ => model
                        .tile_icon(t.id)
                        .map(|k| model.icon(&k, &sides))
                        .unwrap_or_default(),
                },
                name: model.tile_name(t.id),
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
        // the rectangles the popups over the bar stand on: from the bar's top (or a tile's middle) down to the bottom
        // of the window, so that a popup, which has no room below, stands above them
        let to_bottom = |x: f64, y: f64, width: f64| {
            let height = screen.bottom() - y;
            (height > 0.0).then(|| cupertino_kit::geometry::Rect::new(x, y, width, height))
        };
        let label = model.label(&layout);
        // the name rides its tile's bounce
        let label_anchor = label.as_ref().and_then(|(id, ..)| {
            let t = layout.tiles.iter().find(|t| t.id == *id)?;
            to_bottom(t.rect.x, layout.bar.y - t.lift, t.rect.w)
        });
        let fan_open = model
            .stack
            .as_ref()
            .filter(|s| s.shape == fan::Shape::Fan && s.entries.is_some());
        let fan_basis = fan_open.and_then(|s| fan_basis(&layout, screen, s.tile));
        let fan_anchor = fan_basis
            .as_ref()
            .and_then(|b| to_bottom(b.tile_cx - 0.5, b.tile_cy, 1.0));
        // the tile's menu and the question it asks stand over the same point
        let menu_tile =
            (model.menu.as_ref().map(|m| m.tile)).or(model.confirm.as_ref().map(|c| c.tile));
        let menu_anchor = menu_tile.and_then(|tile| {
            let t = layout.tiles.iter().find(|t| t.id == tile)?;
            to_bottom(t.rect.x + t.rect.w / 2.0 - 0.5, layout.bar.y, 1.0)
        });
        windows.push(view::bar::window(
            view::bar::BarView {
                key: key.clone(),
                output,
                size: Size::new(screen.w, screen.h),
                orientation,
                tile_size: model.session.dock.cfg.tile_size,
                bar,
                tiles,
                label_anchor,
                fan_anchor,
                menu_anchor,
                input,
                initial: model.tile_order.first().map(|t| t.0),
                ticking: model.bar_moving(now),
                surface_changes: model.surface_changes,
            },
            message,
        ));
        if let Some((id, name, status)) = label {
            windows.push(view::label::window(
                view::label::LabelView {
                    parent: key.clone(),
                    tile: id.0,
                    name: &name,
                    status,
                    orientation,
                },
                message,
            ));
        }
        if let Some((open, entries)) = model
            .stack
            .as_ref()
            .and_then(|s| Some((s, s.entries.as_ref()?)))
        {
            if let (fan::Shape::Fan, Some(basis)) = (open.shape, &fan_basis) {
                let lookup = |name: &str| -> IconSet {
                    model.icon(&IconKey::Named(name.to_owned()), &[fan::ICON])
                };
                let closing = open.closing.map(|c| (c.start, c.from));
                windows.push(view::fan::window(
                    view::fan::FanView {
                        parent: key.clone(),
                        name: &open.name,
                        entries,
                        icons: &lookup,
                        layout: FanLayout::new(
                            &fan::names(entries),
                            stack::OPEN_FOLDER,
                            basis.lowest,
                        ),
                        progress: fan::progress(now, open.opened, closing, model.reduce_motion),
                        highlighted: open.hover.or(open.selected),
                        moving: fan::moving(now, open.opened, open.closing.map(|c| c.start)),
                    },
                    message,
                ));
            } else if open.shape == fan::Shape::Grid
                && layout.tiles.iter().any(|t| t.id == open.tile)
            {
                let lookup = |name: &str| -> IconSet {
                    model.icon(&IconKey::Named(name.to_owned()), &[StackLayout::ICON])
                };
                windows.push(view::stack::window(
                    view::stack::StackView {
                        parent: key.clone(),
                        tile: open.tile.0,
                        orientation,
                        name: &open.name,
                        entries,
                        icons: &lookup,
                        since: now - open.opened,
                        reduce_motion: model.reduce_motion,
                    },
                    message,
                ));
            }
        }
        if let Some(confirm) = &model.confirm {
            if layout.tiles.iter().any(|t| t.id == confirm.tile) {
                windows.push(view::confirm::window(
                    view::confirm::ConfirmView {
                        parent: key.clone(),
                        tile: confirm.tile.0,
                        orientation,
                        title: EMPTY_TRASH_TITLE,
                        message: EMPTY_TRASH_MESSAGE,
                        cancel: CANCEL,
                        confirm: EMPTY_TRASH,
                    },
                    message,
                ));
            }
        }
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
                        revision: drag_revision(held.as_ref(), puff.as_ref()),
                        held,
                        puff,
                        ticking: false,
                    },
                    message,
                ));
            }
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
                entries: open.dialog.entries().len(),
                empty: open.dialog.entries().is_empty(),
                can_force_quit: open.dialog.can_force_quit(),
                question,
                question_icon,
                hover_button: open.hover_button,
            },
            message,
        ));
    }
    for (&output, surfaces) in &model.wm {
        let Some(out) = model.output_info(output) else {
            continue;
        };
        let icon = |app: &str| model.icon(&IconKey::App(app.to_owned()), &icon_sides(surfaces));
        let switcher = surfaces.switcher.window_with_frames(
            now,
            &icon,
            move |input| message(Input::Switcher(output, input)),
            message(Input::Frame),
        );
        windows.extend(switcher.map(|w| on_output(w, output, Wm::Switcher)));
        let overview = surfaces.overview.window_with_frames(
            now,
            Size::new(out.width, out.height),
            move |input| message(Input::Overview(output, input)),
            message(Input::Frame),
        );
        windows.extend(overview.map(|w| on_output(w, output, Wm::Overview)));
    }
    windows
}
