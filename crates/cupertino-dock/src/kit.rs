//! The dock as a CupertinoKit application: the model around [`Session`] and the surfaces that come and go (the context
//! menu, the held tile, the Force Quit dialog, and on every output the switcher's panel ([`SwitcherSurface`])), what
//! each message does to it, and its windows ([`view`]). Every window is drawn by CupertinoKit. What needs the session
//! (the dock role's requests, starting programs, the files the dock reads and writes, the icons) goes through
//! [`Host`], so the model runs headless in tests; where each window goes is [`Model::placed`], which the shell's
//! surface roles follow ([`crate::component`]).
//!
//! What moves by itself (a bounce, the magnification, auto-hide, the removal puff, the switcher's fade) advances with
//! the frames the compositor gives the window it is on: that window's root sends [`Input::Frame`] at each one while it
//! moves. A wake every [`FRAME_FALLBACK`] seconds keeps an animation going on a window the compositor gives no frames
//! (covered, or not shown yet); what is due at a set time (the switcher's delay, a held press, the edge dwell) is a
//! timer.
//!
//! The app-grid action (a hot corner or gesture mapped to it) opens the launcher's application browser: the dock
//! sends it to the server as `run_action("app-grid")`, which asks the launcher.
//!
//! The dock is on one output at a time: the first output when it starts (the main display), and the first remaining
//! one when its output goes away. Every other output has an edge strip along the dock's edge; the pointer resting
//! against that edge for [`EDGE_DWELL`] seconds moves the dock there. Each strip also holds the dock role of its output,
//! so the hot corners, gestures and the switcher of every output reach the dock; the window feed is taken from the
//! bar's role only. The server allows one dock role per output, and Kit makes a window before it closes the one it
//! replaces, so the bar moves in two views: the first without the bar and the strips of the two outputs, the next with
//! them where they go ([`Message::HandedOff`]).

mod bar;
mod feed;
mod menus;
mod tick;
mod windows;

use std::collections::{BTreeMap, HashMap, HashSet};

use cupertino_defaults::dock::DockSettings;
use cupertino_kit::core::{Command, WindowKey};
use cupertino_kit::platform::Keysym;
use cupertino_wm::geometry::{Point, Rect};

use crate::actions::{ActionMap, Trigger};
use crate::config::Orientation;
use crate::desktop::DesktopIndex;
use crate::fan;
use crate::force_quit::{self, Button, Dialog, Extend};
use crate::menu::{MenuCommand, MenuItem};
use crate::session::Session;
use crate::stack::{self, StackEntry};
use crate::switcher_kit::SwitcherSurface;
use crate::tile_drag::{Poof, TilePress};
use crate::tiles::{TileId, TileKind, TileLifecycle};
use crate::view::{self, IconSet, Input};

pub use feed::Placed;
pub use windows::{dropped_app, view};

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
    /// Whether every dock role is told this, so the dock takes it from the bar's role alone.
    fn is_shared(&self) -> bool {
        !matches!(
            self,
            DockEvent::Switcher { .. } | DockEvent::Trigger(_) | DockEvent::Engage
        )
    }
}

/// What the dock asks the server, through the dock role of one output.
#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    ActivateApp(String),
    /// Show one window of the application `app_id`: restored when minimized, raised and given the keyboard.
    ActivateWindow {
        id: u32,
        app_id: String,
    },
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

/// A look at the mounted volumes' trash under way ([`Host::look_at_trash`]): ends in whether their answer changed.
pub type TrashLook = std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send>>;

/// The trash being emptied ([`Host::empty_trash`]): ends in what the alert says about the items that could not be
/// deleted, or in `None` when everything went.
pub type Emptying = std::pin::Pin<Box<dyn std::future::Future<Output = Option<String>> + Send>>;

/// A program the dock started whose exit the host follows ([`Host::spawn`]): ends when its process exits, in whether
/// it failed (an unsuccessful exit status or a signal).
pub type Exited = std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send>>;

/// Icons loading away from the model ([`Host::preload`]): ends in each one's key, side in whole points and renditions.
pub type IconLoad =
    std::pin::Pin<Box<dyn std::future::Future<Output = Vec<(IconKey, u32, IconSet)>> + Send>>;

/// A folder being listed away from the model ([`Host::folder_entries`]): ends in the entries its stack shows.
pub type FolderListing =
    std::pin::Pin<Box<dyn std::future::Future<Output = Vec<StackEntry>> + Send>>;

/// Whether a program the dock asked the host to start ([`Host::spawn`]) started, and how its end is known.
pub enum Started {
    /// It did not start.
    No,
    /// It started, and nothing says when it exits: the guard against a second launch lasts [`OPEN_GUARD`].
    Untracked,
    /// It started; this ends as it exits.
    Watched(Exited),
}

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

/// What the model needs the session for. What reads or writes a file, or waits for another program, is work the host
/// does away from the model: it hands back the command that does it, or the future that ends with it.
pub trait Host {
    /// Seconds on the monotonic clock the model is driven with.
    fn now(&self) -> f64;
    /// [`Message::Wake`] with `generation` after `after` seconds.
    fn wake(&mut self, after: f64, generation: u64) -> Vec<Command<Message>>;
    /// Send `request` through the dock role of the window `role`: the bar's, or the edge strip of another output.
    fn request(&mut self, role: &WindowKey, request: Request);
    /// Start `argv` with the activation token.
    fn spawn(&mut self, argv: Vec<String>, token: Option<String>) -> Started;
    /// Open `target` with the session's default handler.
    fn open(&mut self, target: &str);
    /// Run `command` through the shell.
    fn shell(&mut self, command: &str);
    /// Store the pinned applications in their order.
    fn store_pins(&mut self, pins: &[String]) -> Vec<Command<Message>>;
    /// Read `dock.conf` again, `last_good` keeping the keys of lines that are not understood: ends in
    /// [`Message::ConfRead`].
    fn read_conf(&mut self, last_good: &DockSettings) -> Vec<Command<Message>>;
    /// The terminal emulator that runs entries with `Terminal=true`, if one is installed.
    fn terminal(&mut self) -> Option<String>;
    /// The folders the bar shows as stacks, with the theme names of their icons, in order (the user's downloads
    /// folder); none by default.
    fn folders(&mut self) -> Vec<(String, String)> {
        Vec::new()
    }
    /// The folder a new file manager window opens on: the person's home folder; none by default.
    fn home(&mut self) -> Option<String> {
        None
    }
    /// List `folder` away from the model: what ends in the entries its stack shows, the newest first, at most
    /// [`stack::MAX_ENTRIES`] ([`stack::newest_first`]). A large or slow folder holds up nothing else.
    fn folder_entries(&mut self, folder: &str) -> FolderListing {
        let _ = folder;
        Box::pin(std::future::ready(Vec::new()))
    }
    /// Read the desktop file IDs of the recently used applications, the most recent first, for the bar's recents
    /// section: ends in [`Message::RecentApps`]; nothing by default.
    fn recent_apps(&mut self) -> Vec<Command<Message>> {
        Vec::new()
    }
    /// Ask the application on the bus name `bus_name` for a window (`org.freedesktop.Application.Activate`), handing
    /// it the activation token: the call under way, which ends in whether it succeeded, or `None` when it cannot be
    /// made. Where it cannot be made or fails, the model starts the entry again.
    fn dbus_activate(&mut self, bus_name: &str, token: Option<String>) -> Option<BusCall> {
        let _ = (bus_name, token);
        None
    }
    /// The renditions of `icon` at `side` points for each of [`ICON_SCALES`], when the host has them at hand without
    /// reading a file; `None` leaves them to [`Host::preload`].
    fn icon(&mut self, desktop: &DesktopIndex, icon: &IconKey, side: f64) -> Option<IconSet> {
        let _ = (desktop, icon, side);
        None
    }
    /// Load `icons`, each at its side in points, away from the model: what ends in their renditions, or `None` when
    /// the host loads none.
    fn preload(&mut self, desktop: &DesktopIndex, icons: Vec<(IconKey, f64)>) -> Option<IconLoad> {
        let _ = (desktop, icons);
        None
    }
    /// A window set its own icon, or removed it; `apps` are its application id and the dock's key for it.
    fn window_icon(&mut self, apps: [String; 2], icon: WindowIcon);
    fn window_closed(&mut self, id: u32);
    /// Look whether the trash holds anything: ends in [`Message::TrashFull`]; nothing by default, an empty trash.
    fn trash_full(&mut self) -> Vec<Command<Message>> {
        Vec::new()
    }
    /// The next look at the mounted volumes' trash, away from the model, unless one is still out or none was asked
    /// for; the model looks at the trash again when it ends in a change.
    fn look_at_trash(&mut self) -> Option<TrashLook> {
        None
    }
    /// Delete everything in the trash, the home trash and the mounted volumes', away from the model.
    fn empty_trash(&mut self) -> Emptying {
        Box::pin(async { None })
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
    /// The view without the bar and the strips of the outputs it moves between was shown: the next one shows them
    /// where they go.
    HandedOff,
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
    /// The trash was emptied ([`Host::empty_trash`]); with what to say when something could not be deleted.
    TrashEmptied(Option<String>),
    /// Something was put in the trash or taken out of it.
    TrashChanged,
    /// A look at the mounted volumes' trash ([`Host::look_at_trash`]) ended; whether their answer changed.
    TrashLooked(bool),
    /// Whether the trash holds anything ([`Host::trash_full`]).
    TrashFull(bool),
    Wake(u64),
    /// `dock.conf` changed on disk.
    ConfChanged,
    /// `dock.conf` as read again ([`Host::read_conf`]); `None` when it could not be read.
    ConfRead(Option<Box<DockSettings>>),
    /// The launcher's history of applications ([`Host::recent_apps`]), the most recent first.
    RecentApps(Vec<String>),
    /// An application asked for its badge's count to change ([`crate::badge`]).
    Badge(crate::badge::BadgeUpdate),
    /// Something was dropped on the bar: an application's desktop file is kept in the dock where it landed.
    Dropped(cupertino_kit::core::Dropped),
    /// What the shell told the component, which the component takes before the model ([`crate::component`]).
    Shell(crate::component::Heard),
    /// Work away from the model ended with nothing to tell.
    Done,
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
    /// The tile's icon darkened, as pressed: the tile shows it while its menu is up and while the menu fades out.
    dark: IconSet,
}

/// How long a tile keeps its pressed darkening after its menu closed: the time the menu takes to fade out.
const MENU_FADE_OUT: f64 = 0.21;

struct ForceQuit {
    output: u32,
    dialog: Dialog,
    layout: force_quit::Layout,
    hover_row: Option<usize>,
    hover_button: Option<Button>,
}

pub struct Model {
    pub session: Session,
    host: Box<dyn Host>,
    pub outputs: Vec<OutputInfo>,
    /// The output the bar is on.
    pub output: Option<u32>,
    /// Takes the next value when the compositor closed the bar's surface, so a new one is opened.
    bar_generation: u64,
    /// The bar moves to another output, or its surface opens anew: the view leaves the bar out, and the strip of the
    /// output it left (`left`), until [`Message::HandedOff`].
    handoff: bool,
    left: Option<u32>,
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
    /// The tile whose menu closed last, its darkened icon and when the menu closed: it keeps the darkening while the
    /// menu fades out ([`MENU_FADE_OUT`]).
    menu_fade: Option<(TileId, IconSet, f64)>,
    /// The trash's alert that is up: the question before it is emptied, or what could not be deleted.
    confirm: Option<view::confirm::Alert>,
    stack: Option<OpenStack>,
    /// The tile the left button is held on, and its icon darkened: the pressed tile shows it until the button comes up
    /// or the pointer leaves the tile.
    held: Option<(TileId, IconSet)>,
    /// The name that showed as the pointer left a bar that hides: it stays over its tile, riding the bar down and
    /// fading with it, until the bar is hidden or the pointer comes back.
    leaving_label: Option<(TileId, String, Option<&'static str>)>,
    /// The counts the applications show on their tiles.
    badges: crate::badge::Badges,
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
    /// The switcher's panel of every output, by output key.
    switchers: BTreeMap<u32, SwitcherSurface>,
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
    /// The icons of applications loaded before a window's own icon changed: drawn until their new ones come.
    stale_icons: HashMap<IconKey, BTreeMap<u32, IconSet>>,
    /// The icons asked for in the background and not arrived yet, by key and side in whole points.
    requested: HashSet<(IconKey, u32)>,
    /// The icons the view wants that are neither loaded nor asked for, gathered for one load in the background.
    wanted: Vec<(IconKey, u32)>,
    /// What the bar's recents were last worked out from: the application tiles and the applications that quit. The
    /// launcher's history is read again when it changes.
    recents_basis: Option<Vec<String>>,
    /// The name of the desktop entry of each application key looked up so far (`None` without one): the entries do
    /// not change while the dock runs.
    names: HashMap<String, Option<String>>,
    trash_checked: Option<f64>,
    trash_full: bool,
    /// The trash's state came back from a look ([`Model::ready_to_show`]).
    trash_looked: bool,
    /// Reads of the launcher's history asked for and not arrived yet.
    recents_reading: usize,
    wake_generation: u64,
    wake_at: Option<f64>,
    /// The work the host handed back while a message was taken, run after it.
    later: Vec<Command<Message>>,
    // what the shell was last told
    placed: Option<Placement>,
    keyboard: bool,
    /// Bumped whenever the bar's placement or keyboard interactivity changes, which apply with the surface's next
    /// commit: the bar presents again, so they reach the server even when nothing it draws changed.
    surface_changes: u64,
    /// The bar's window the slot of the next minimized window was last sent through, and the slot.
    minimize_target: Option<(WindowKey, Rect)>,
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
            bar_generation: 0,
            handoff: false,
            left: None,
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
            menu_fade: None,
            confirm: None,
            stack: None,
            held: None,
            leaving_label: None,
            badges: crate::badge::Badges::default(),
            folders_asked: false,
            folder_icons: HashMap::new(),
            engaged: false,
            focus_first: false,
            menu_takes_keyboard: false,
            focused_tile: None,
            tile_order: Vec::new(),
            unresponsive: HashSet::new(),
            force_quit: None,
            switchers: BTreeMap::new(),
            pending: HashMap::new(),
            next_launch: 0,
            next_listing: 0,
            opening: HashMap::new(),
            edge: None,
            icons: HashMap::new(),
            stale_icons: HashMap::new(),
            requested: HashSet::new(),
            wanted: Vec::new(),
            recents_basis: None,
            names: HashMap::new(),
            trash_checked: None,
            trash_full: false,
            trash_looked: false,
            recents_reading: 0,
            wake_generation: 0,
            wake_at: None,
            later: Vec::new(),
            placed: None,
            keyboard: false,
            surface_changes: 0,
            minimize_target: None,
        }
    }

    /// The applications the server reported as not responding (the screenshot fixtures set some at start).
    pub fn set_unresponsive(&mut self, apps: HashSet<String>) {
        self.unresponsive = apps;
    }

    /// Whether the bar has all it draws at hand: the trash was looked at, the launcher's history the recents come
    /// from was read, and the icon of every tile is loaded at every size it is drawn at. What the server tells the
    /// bar's role (the windows, the residents) comes once the bar shows.
    pub fn ready_to_show(&self) -> bool {
        self.trash_looked
            && self.recents_reading == 0
            && self.wanted.is_empty()
            && self.requested.is_empty()
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

    /// The bar's window as the model's screen: as long as the output's edge, as deep as the surface's extent.
    pub fn screen(&self) -> Option<Rect> {
        let output = self.output_info(self.output?)?;
        let extent = self.session.dock.surface_extent().ceil();
        let (w, h) = if self.session.dock.cfg.orientation.is_horizontal() {
            (output.width, extent)
        } else {
            (extent, output.height)
        };
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

    /// The window whose dock role a request about the output with key `output` goes through: the bar's, else that
    /// output's edge strip.
    fn role(&self, output: u32) -> WindowKey {
        match self.bar_key() {
            Some(bar) if self.output == Some(output) => bar,
            _ => view::edge::key(output),
        }
    }

    fn bar_request(&mut self, request: Request) {
        let Some(bar) = self.bar_key() else { return };
        // the tile a window is restored out of is the slot the server takes for the next minimized window too: the
        // slot is sent again after it
        if matches!(request, Request::RestoreWindow { .. }) {
            self.minimize_target = None;
        }
        self.host.request(&bar, request);
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
    /// The count tile `id` shows on its badge: an application's that runs and asked for one.
    pub fn badge(&self, id: TileId) -> Option<u64> {
        let tile = self.session.dock.tile(id)?;
        if tile.kind != TileKind::App || !tile.running {
            return None;
        }
        let app = self
            .session
            .desktop()
            .find(&tile.key)
            .map_or(tile.key.as_str(), |e| e.id.as_str());
        self.badges.shown(app)
    }

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
    // over a tile that stands above the bar's top edge (a magnified one) the rows stand that much higher
    let rise = (layout.bar.y - t.rect.y).max(0.0);
    let lowest = (cy - layout.bar.y) + fan::LOWEST_ABOVE_BAR + rise;
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
        Message::Dropped(dropped) => model.dropped(dropped),
        Message::Dock { output, events } => model.dock_events(output, events),
        Message::Dark(dark) => {
            model.dark = dark;
            Vec::new()
        }
        Message::ReduceMotion(on) => {
            model.reduce_motion = on;
            model.session.dock.set_reduce_motion(on);
            Vec::new()
        }
        Message::Locked(locked) => {
            model.lock(locked);
            Vec::new()
        }
        Message::Outputs(outputs) => {
            model.outputs_changed(outputs);
            Vec::new()
        }
        Message::HandedOff => {
            model.handoff = false;
            model.left = None;
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
        Message::TrashEmptied(failed) => {
            // the trash's icon follows what is left, and what could not be deleted is said
            model.trash_checked = None;
            if let Some(why) = failed {
                model.engaged = false;
                model.confirm = Some(view::confirm::Alert::Failed(why));
            }
            Vec::new()
        }
        Message::TrashChanged | Message::TrashLooked(true) => {
            model.trash_checked = None;
            Vec::new()
        }
        Message::TrashFull(full) => {
            model.trash_looked = true;
            model.trash_full = full;
            model.session.dock.set_trash_full(full);
            Vec::new()
        }
        Message::Wake(generation) => {
            if generation == model.wake_generation {
                model.wake_at = None;
            }
            Vec::new()
        }
        Message::Badge(update) => {
            model.badges.apply(&update);
            Vec::new()
        }
        Message::ConfChanged => model.host.read_conf(&model.conf),
        Message::ConfRead(settings) => {
            if let Some(settings) = settings {
                model.conf_changed(*settings);
            }
            Vec::new()
        }
        Message::RecentApps(history) => {
            model.recents_reading = model.recents_reading.saturating_sub(1);
            let now = model.now();
            let recents = model.session.recent_apps(&history);
            let keys: Vec<&str> = recents.iter().map(String::as_str).collect();
            model.session.dock.set_recents(&keys, now);
            Vec::new()
        }
        Message::TrashLooked(false) | Message::Shell(_) | Message::Done => Vec::new(),
    };
    commands.extend(model.changed());
    commands.append(&mut model.later);
    commands
}

impl Model {
    /// The session was locked or unlocked. The lock screen covers everything: the dialog and a menu go at once,
    /// without animation.
    fn lock(&mut self, locked: bool) {
        self.locked = locked;
        if !locked {
            return;
        }
        // the question is not left for whoever unlocks; what could not be deleted stays to be read
        if self.confirm == Some(view::confirm::Alert::Question) {
            self.confirm = None;
        }
        self.force_quit = None;
        self.menu = None;
        self.stack = None;
        self.press = None;
        self.engaged = false;
        self.edge = None;
        self.session.dock.cancel_drag();
    }

    /// The keyboard left the bar's window. The menu taking the keyboard is the one leave that keeps Focus Dock; any
    /// other (the server giving the keyboard to a window, a press outside closing the menu as well) ends it.
    fn blurred(&mut self) {
        if !(self.menu.is_some() && std::mem::take(&mut self.menu_takes_keyboard)) {
            self.engaged = false;
        }
    }

    /// The menu's item at `i` was chosen: emptying the trash asks first.
    fn menu_item(&mut self, i: usize) -> Vec<Command<Message>> {
        let tile = self.menu.as_ref().map(|m| m.tile);
        let command = self
            .menu
            .as_ref()
            .and_then(|m| m.items.get(i))
            .and_then(|item| item.command.clone());
        self.menu = None;
        match (command, tile) {
            // deleting for good asks first
            (Some(MenuCommand::EmptyTrash), Some(_)) => {
                self.engaged = false;
                self.confirm = Some(view::confirm::Alert::Question);
                Vec::new()
            }
            (Some(command), _) => self.run_menu_command(command),
            (None, _) => Vec::new(),
        }
    }

    /// The question before the trash is emptied was answered yes; only the question empties: a press that comes
    /// after it closed deletes nothing.
    fn confirm_done(&mut self) -> Vec<Command<Message>> {
        if self.confirm.take() != Some(view::confirm::Alert::Question) {
            return Vec::new();
        }
        let emptying = self.host.empty_trash();
        vec![Command::Perform(Box::pin(async move {
            Message::TrashEmptied(emptying.await)
        }))]
    }

    /// The pointer came onto (`on`) or left the edge strip of `output`.
    fn edge(&mut self, output: u32, on: bool) {
        if on {
            let now = self.now();
            if self.edge.is_none_or(|(o, _)| o != output) {
                self.edge = Some((output, now));
            }
        } else if self.edge.is_some_and(|(o, _)| o == output) {
            self.edge = None;
        }
    }
}

fn input_message(model: &mut Model, input: Input) -> Vec<Command<Message>> {
    match input {
        Input::Bar(pointer) => return model.bar_pointer(pointer),
        Input::Activate(id) => return model.activate(TileId(id)),
        Input::ShowMenu(id) => model.open_menu(TileId(id)),
        Input::LeaveKeyboard => model.engaged = false,
        Input::Blurred => model.blurred(),
        Input::MenuItem(i) => return model.menu_item(i),
        Input::ConfirmClosed => model.confirm = None,
        Input::ConfirmDone => return model.confirm_done(),
        Input::MenuClosed => {
            let now = model.now();
            model.menu_fade = model.menu.take().map(|m| (m.tile, m.dark, now));
        }
        Input::Stack(pointer) => return model.stack_pointer(pointer),
        Input::StackKey(key) => return model.stack_key(key),
        Input::StackEntry(i) => return model.open_stack_entry(i),
        Input::StackOpenFolder => return model.open_stack_folder(),
        Input::StackClosed => model.close_stack(),
        Input::LabelGone => {}
        Input::ForceQuit(pointer) => model.force_quit_pointer(pointer),
        Input::ForceQuitKey(key) => model.force_quit_key(key),
        Input::ForceQuitRow(i) => {
            if let Some(open) = model.force_quit.as_mut() {
                let outcome = open.dialog.select(i, Extend::Replace);
                model.force_quit_outcome(outcome);
            }
        }
        Input::ForceQuitButton(button) => model.force_quit_button(button),
        Input::Edge(output, on) => model.edge(output, on),
        Input::Switcher(output, input) => model.switcher_input(output, input),
        Input::SwitcherLost(output) => {
            if let Some(switcher) = model.switchers.get_mut(&output) {
                switcher.lost();
            }
        }
        Input::TileFocused(id) => model.focused_tile = Some(TileId(id)),
        // the update that follows every message advances what moves
        Input::Frame => {}
        Input::Closed(key) => model.closed(key),
    }
    Vec::new()
}

fn message(input: Input) -> Message {
    Message::Input(input)
}

fn kit_point(p: Point) -> cupertino_kit::geometry::Point {
    cupertino_kit::geometry::Point::new(p.x, p.y)
}
