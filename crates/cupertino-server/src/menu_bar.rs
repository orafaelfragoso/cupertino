//! Server side of the menu bar (`cupertino_shell_v1.get_menu_bar`, `cupertino_menu_bar_v1`): the
//! bar is a layer surface of the menu bar process; the server owns its frame, layering, auto-hide
//! and reveal (`cupertino_wm::menu_bar`), reserves its height in the usable area, applies the
//! presentation when the surface is drawn, and feeds every bar the active application, the menu
//! address its active window exports and the window state. The bar process owns everything else.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use crate::dbus::{LoginTrust, OwnerRefusal, Request as DbusRequest};
use crate::input::{translate_action, Action, Chord};
use crate::owner_check::{OwnerCheck, OwnerKey};
use crate::render::wallpaper::BarBands;
use cupertino_input::{KeyCombo, Modifiers};
use cupertino_keyboard::modifiers::{Physical, Profile, RawMods};
use cupertino_keyboard::rewrite::ClientClass;
use cupertino_protocols::server::app_effects::cupertino_app_menu_v1::{
    self, CupertinoAppMenuV1, Error as AppMenuError,
};
use cupertino_protocols::server::cupertino_menu_bar_v1::{
    self, AgentTrust, AppWindowFlags, BoundAction, ClientKind, Command as BarCommand,
    CupertinoMenuBarV1, EditAction, Error as BarError, KeyClass, ModifierKey, SessionAction,
    WindowControl,
};
use cupertino_protocols::server::cupertino_shell_v1::{CupertinoShellV1, Error as ShellError};
use cupertino_wm::menu_bar::{BarSetConfig, MenuBarSet};
use cupertino_wm::{AppId, Command, LifecycleState, OutputId, Window, WindowId};
use smithay::desktop::{layer_map_for_output, WindowSurfaceType};
use smithay::input::keyboard::keysyms;
use smithay::output::Output;
use smithay::reexports::calloop::channel::{self, Event as ChannelEvent};
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::RegistrationToken;
use smithay::reexports::wayland_server::backend::{ClientId, ObjectId};
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, New, Resource, WEnum,
};

use crate::authorization::{authorized, capabilities, refuse, require, role, Permit, ShellRole};
use crate::state::{Cupertino, PointerMode};

/// Opacity lost by the bar of an inactive output. approximate.
const INACTIVE_DIM: f64 = 0.4;

/// Most windows the window list of a group names.
const MAX_APP_WINDOWS: usize = 1000;

/// Most connections whose process the bars' groups remember; past it the one active longest ago is read again.
const MAX_REMEMBERED_CLIENTS: usize = 16;

/// Longest window title or application id, in bytes, an event to a shell client carries: a client sets both to any
/// length, and one message must stay under the 4096 bytes the Wayland wire allows.
pub const MAX_TEXT_BYTES: usize = 1024;

/// Longest time, in seconds from its arrival, an `edit` request waits for the keyboard to come back to its window.
const EDIT_WAIT: f64 = 0.5;

/// The actions a bar shows the chord of, each with the binding action that runs it.
const BOUND_ACTIONS: [(BoundAction, Action); 14] = [
    (BoundAction::CloseWindow, Action::CloseWindow),
    (BoundAction::Minimize, Action::Minimize),
    (BoundAction::Zoom, Action::ToggleZoom),
    (BoundAction::ToggleFullscreen, Action::ToggleFullscreen),
    (BoundAction::HideApp, Action::HideApp),
    (BoundAction::HideOthers, Action::HideOthers),
    (BoundAction::QuitApp, Action::QuitApp),
    (BoundAction::TileLeft, Action::TileLeft),
    (BoundAction::TileRight, Action::TileRight),
    (BoundAction::TileFill, Action::TileFill),
    (BoundAction::ForceQuit, Action::ForceQuit),
    (BoundAction::Lock, Action::Lock),
    (BoundAction::LogOut, Action::LogOut),
    (BoundAction::HelpSearch, Action::HelpSearch),
];

/// Whether the binding `bound` shows as the chord of the bar's item for `item`: the item that minimizes also takes the
/// chord that restores a tiled window before it minimizes.
fn runs_as(item: Action, bound: Action) -> bool {
    item == Action::Minimize && bound == Action::MinimizeOrRestore
}

/// Properties a GTK client announced for one window (`org.gtk.Menus` / `org.gtk.Actions` objects).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GtkMenuProps {
    /// Unique bus name that owns the object paths.
    pub bus_name: String,
    pub application_id: Option<String>,
    pub application_object_path: Option<String>,
    pub window_object_path: Option<String>,
    pub menubar_object_path: Option<String>,
    pub app_menu_object_path: Option<String>,
}

impl GtkMenuProps {
    /// The window exports menus: a menu bar, an application menu or both.
    pub fn has_menus(&self) -> bool {
        self.menubar_object_path.is_some() || self.app_menu_object_path.is_some()
    }
}

/// Where the menus of a window can be read from; the bar process does the reading.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuAddress {
    /// DBusMenu tree, announced through the application menu address protocol.
    DbusMenu { service: String, path: String },
    /// GMenuModel menus, announced through the toolkit shell protocol.
    Gtk(GtkMenuProps),
}

/// The kind of a menu address. A window keeps one checked address of each kind; the bar is told the GMenuModel one
/// when there is one, since its items name their actions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MenuKind {
    Gtk,
    DbusMenu,
}

impl MenuAddress {
    pub fn kind(&self) -> MenuKind {
        match self {
            MenuAddress::DbusMenu { .. } => MenuKind::DbusMenu,
            MenuAddress::Gtk(_) => MenuKind::Gtk,
        }
    }

    /// The bus name the menus are read from.
    pub fn bus_name(&self) -> &str {
        match self {
            MenuAddress::DbusMenu { service, .. } => service,
            MenuAddress::Gtk(props) => &props.bus_name,
        }
    }

    /// Whether the address is a bus name of at most [`MAX_BUS_NAME_BYTES`], object paths of at most
    /// [`MAX_OBJECT_PATH_BYTES`] and an application id of at most [`MAX_APPLICATION_ID_BYTES`], so the menu event that
    /// carries it fits a message whatever the owner check and the absent strings add.
    pub fn within_limits(&self) -> bool {
        match self {
            MenuAddress::DbusMenu { service, path } => is_bus_name(service) && is_object_path(path),
            MenuAddress::Gtk(props) => {
                let paths = [
                    &props.application_object_path,
                    &props.window_object_path,
                    &props.menubar_object_path,
                    &props.app_menu_object_path,
                ];
                is_bus_name(&props.bus_name)
                    && props
                        .application_id
                        .as_ref()
                        .is_none_or(|id| id.len() <= MAX_APPLICATION_ID_BYTES)
                    && paths
                        .iter()
                        .all(|p| p.as_deref().is_none_or(is_object_path))
            }
        }
    }

    /// The address read from the connection `unique` instead.
    pub fn with_bus_name(mut self, unique: String) -> Self {
        match &mut self {
            MenuAddress::DbusMenu { service, .. } => *service = unique,
            MenuAddress::Gtk(props) => props.bus_name = unique,
        }
        self
    }
}

/// What a bar shows for the active window; one group of events carries all of it.
#[derive(Clone, Debug, PartialEq)]
struct Feed {
    app_id: String,
    title: String,
    menu: Option<MenuAddress>,
    can_zoom: bool,
    can_minimize: bool,
    /// The window is in full screen (version 16).
    full_screen: bool,
    /// 1-based workspace of the window, 0 for none.
    workspace: u32,
    workspaces: u32,
    /// The window list (version 10), top of the stack first.
    windows: Vec<AppWindow>,
    /// The process behind the window's connection, 0 when unknown, and whether it is the X11 bridge (version 24).
    client: (u32, bool),
    /// How the server passes chords to the window (version 25).
    keys: ClientClass,
}

/// One row of the window list.
#[derive(Clone, Debug, PartialEq)]
struct AppWindow {
    /// The window id, as the dock names it.
    id: u32,
    title: String,
    flags: AppWindowFlags,
}

impl Feed {
    /// Send the group: `active_app`, one menu event, `window_state`, the window list, `done`.
    fn send(&self, bar: &CupertinoMenuBarV1) {
        bar.active_app(
            cut_text(&self.app_id).to_owned(),
            cut_text(&self.title).to_owned(),
        );
        match &self.menu {
            None => bar.menu_none(),
            Some(MenuAddress::DbusMenu { service, path }) => {
                bar.menu_dbusmenu(service.clone(), path.clone())
            }
            Some(MenuAddress::Gtk(p)) => {
                let path = |p: &Option<String>| p.clone().unwrap_or_default();
                bar.menu_gtk(
                    p.bus_name.clone(),
                    path(&p.application_id),
                    path(&p.application_object_path),
                    path(&p.window_object_path),
                    path(&p.menubar_object_path),
                    path(&p.app_menu_object_path),
                );
            }
        }
        bar.window_state(u32::from(self.can_zoom), self.workspace, self.workspaces);
        // version 15; an older bar keeps both enabled, and the model refuses what is disabled
        if bar.version() >= 15 {
            let mut enabled = WindowControl::empty();
            enabled.set(WindowControl::Minimize, self.can_minimize);
            enabled.set(WindowControl::Zoom, self.can_zoom);
            bar.window_controls(enabled);
        }
        if bar.version() >= 16 {
            bar.window_full_screen(u32::from(self.full_screen));
        }
        if bar.version() >= 24 {
            let (pid, x11) = self.client;
            bar.active_client(
                pid,
                if x11 {
                    ClientKind::X11
                } else {
                    ClientKind::Wayland
                },
            );
        }
        if bar.version() >= 25 {
            bar.active_client_keys(match self.keys {
                ClientClass::FirstParty => KeyClass::Toolkit,
                ClientClass::Terminal => KeyClass::Terminal,
                ClientClass::Other => KeyClass::Other,
            });
        }
        for w in &self.windows {
            bar.app_window(w.id, w.title.clone(), w.flags);
        }
        bar.done();
    }
}

/// The shortcut group (version 10): the chord of every bound action and the primary modifier.
#[derive(Clone, Debug, PartialEq)]
struct Shortcuts {
    chords: Vec<(BoundAction, String)>,
    primary: ModifierKey,
}

impl Shortcuts {
    fn send(&self, bar: &CupertinoMenuBarV1) {
        // help_search came with version 12
        for (action, chord) in self
            .chords
            .iter()
            .filter(|(action, _)| *action != BoundAction::HelpSearch || bar.version() >= 12)
        {
            bar.shortcut(*action, chord.clone());
        }
        bar.primary_modifier(self.primary);
        bar.done();
    }
}

/// `text` cut to at most [`MAX_TEXT_BYTES`] at a character boundary.
pub(crate) fn cut_text(text: &str) -> &str {
    &text[..text.floor_char_boundary(MAX_TEXT_BYTES)]
}

/// The chord in the shortcut file's syntax. The bindings hold the physical keys, so this is the text of the chord itself.
fn physical_chord(chord: Chord) -> String {
    let m = chord.mods;
    let modifiers = Modifiers {
        shift: m.shift,
        ctrl: m.ctrl,
        alt: m.alt,
        super_key: m.logo,
    };
    KeyCombo::new(modifiers, chord.sym).to_string()
}

/// The physical chord the Edit item `action` sends to a client of `class` under `profile` (`opted_out`: the keyboard
/// rewrite is off for it): modifiers and Latin keysym. A terminal gets only copy and paste, as Ctrl+Shift, so the menu
/// never sends it a control character; a client that reads the raw modifiers (first-party, or opted out) gets the
/// profile's primary modifier, as the person would press it; any other gets the Ctrl chord it expects. `None` sends
/// nothing.
fn edit_chord(
    action: EditAction,
    class: ClientClass,
    opted_out: bool,
    profile: &Profile,
) -> Option<(RawMods, u32)> {
    let (sym, shift) = match action {
        EditAction::Undo => (keysyms::KEY_z, false),
        EditAction::Redo => (keysyms::KEY_z, true),
        EditAction::Cut => (keysyms::KEY_x, false),
        EditAction::Copy => (keysyms::KEY_c, false),
        EditAction::Paste => (keysyms::KEY_v, false),
        EditAction::SelectAll => (keysyms::KEY_a, false),
        _ => return None,
    };
    let primary = match class {
        ClientClass::Terminal => {
            return matches!(action, EditAction::Copy | EditAction::Paste)
                .then_some((RawMods::SHIFT | RawMods::CTRL, sym));
        }
        ClientClass::FirstParty => profile.primary().into(),
        ClientClass::Other if opted_out => profile.primary().into(),
        ClientClass::Other => RawMods::CTRL,
    };
    Some((
        if shift {
            primary | RawMods::SHIFT
        } else {
            primary
        },
        sym,
    ))
}

struct Bar {
    resource: CupertinoMenuBarV1,
    surface: WlSurface,
    output: OutputId,
    /// The last group sent; `None` until the first one.
    sent: Option<Feed>,
    /// The screen capture state last sent (version 3); `None` until the first one.
    capture_sent: Option<bool>,
    /// The session agent last named (version 8); `None` until the first time.
    agent_sent: Option<LoginTrust>,
    /// The shortcut group last sent (version 10); `None` until the first one.
    shortcuts_sent: Option<Shortcuts>,
    /// The backdrop lightness last sent (version 14 to 20); `None` until the first one.
    backdrop_sent: Option<u16>,
    /// The backdrop bands last sent (version 21); `None` until the first ones.
    bands_sent: Option<BarBands>,
}

/// Where the bar of a surface is drawn: offset from its layout position and opacity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BarLook {
    /// Vertical offset in logical points, 0 when shown and the bar's height above the output when hidden.
    pub dy: f64,
    pub alpha: f32,
}

/// An `edit` request waiting for the keyboard to come back to its window once the menu has closed.
struct PendingEdit {
    action: EditAction,
    /// The surface of the bar whose popup grab was active when the request arrived.
    bar: WlSurface,
    /// The key window when the request arrived.
    window: WindowId,
    /// Model time of the arrival.
    since: f64,
    /// The first keyboard focus change after the grab ended gave the keyboard to `window`.
    returned: bool,
}

/// The well-known name the menu bar process owns while it reads menus (the registrar `cupertino-menubar` claims); the
/// owner, once checked to be the bar's process, is the reader the windows' menus answer (`menu_reader`).
pub const MENU_BAR_BUS_NAME: &str = "com.canonical.AppMenu.Registrar";
/// The first wait before a check of the reader refused for a passing reason is made again; it doubles up to
/// [`READER_RETRY_MAX`].
const READER_RETRY_FIRST: Duration = Duration::from_secs(1);
const READER_RETRY_MAX: Duration = Duration::from_secs(30);
/// White, the top of the backdrop lightness scale (CIE L*/100 in thousandths).
pub const BACKDROP_LIGHTNESS_MAX: u16 = 1000;
/// How far the backdrop lightness moves, in thousandths, before a bar is told again: more than this.
pub const BACKDROP_LIGHTNESS_STEP: u16 = 10;
/// The version of `cupertino_menu_bar_v1` that is told the backdrop's bands in place of its lightness, and the most
/// bands it carries.
pub const BACKDROP_BANDS_VERSION: u32 = 21;
pub const BACKDROP_BANDS_MAX: usize = 256;

/// The `menu_bar` value of a menu object of `version`: 1 while a bar runs, and from version 3 only while the reader is
/// verified too, so a window hides its own menus only while the bar can read them.
fn shown(bars: bool, reader: bool, version: u32) -> u32 {
    u32::from(bars && (version < 3 || reader))
}

/// A `cupertino_app_menu_v1` of an application window.
struct AppMenu {
    resource: CupertinoAppMenuV1,
    surface: WlSurface,
    /// The object announced an address, which its destruction withdraws.
    announced: bool,
    /// The `menu_bar` value last sent.
    shown: u32,
}

/// The pauses before the retries of the owner check of an announcement refused for a reason that may pass
/// ([`retryable`]), each counted from the refusal; their number is the most retries an announcement gets.
pub const OWNER_RETRY_DELAYS: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
];

/// Whether an owner check refused with `refusal` may pass when asked again: the bus did not answer in time, or the
/// identification threads were all taken. Every other refusal is final.
pub fn retryable(refusal: &OwnerRefusal) -> bool {
    matches!(refusal, OwnerRefusal::TimedOut | OwnerRefusal::Busy)
}

/// Why an announcement refused for a reason that may pass has no check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Parked {
    /// A retry is due.
    Retry,
    /// The retries ran out: a check waits for the window to become the key window.
    Recheck,
}

/// What follows a refused owner check of an announcement.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// The refusal stands.
    Final,
    /// Retried after the pause.
    RetryAfter(Duration),
    /// No retry left: checked again when the window becomes the key window; `warn` only the first time.
    RanOut { warn: bool },
}

/// The retries of the owner check of one announcement.
#[derive(Debug)]
pub struct OwnerRetry {
    /// Which announcement, of all the server's.
    announcement: u64,
    /// The retries since the announcement or its last re-check.
    retries: usize,
    parked: Option<Parked>,
    warned: bool,
}

impl OwnerRetry {
    pub fn new(announcement: u64) -> Self {
        Self {
            announcement,
            retries: 0,
            parked: None,
            warned: false,
        }
    }

    pub fn announcement(&self) -> u64 {
        self.announcement
    }

    /// The check was refused, for a reason that may pass when `retryable`.
    pub fn refused(&mut self, retryable: bool) -> Refused {
        if !retryable {
            return Refused::Final;
        }
        if let Some(&after) = OWNER_RETRY_DELAYS.get(self.retries) {
            self.retries += 1;
            self.parked = Some(Parked::Retry);
            return Refused::RetryAfter(after);
        }
        self.parked = Some(Parked::Recheck);
        Refused::RanOut {
            warn: !std::mem::replace(&mut self.warned, true),
        }
    }

    /// The retry timer of announcement `announcement` fired: whether to check again, only if this is that
    /// announcement and its retry is due.
    pub fn due(&mut self, announcement: u64) -> bool {
        let due = self.announcement == announcement && self.parked == Some(Parked::Retry);
        if due {
            self.parked = None;
        }
        due
    }

    /// The retry could not be timed: the check waits for the window to become the key window, as when the retries ran
    /// out.
    pub fn untimed(&mut self) {
        if self.parked == Some(Parked::Retry) {
            self.parked = Some(Parked::Recheck);
        }
    }

    /// The window became the key window: whether to check again, only if the retries ran out, which are then counted
    /// afresh.
    pub fn recheck(&mut self) -> bool {
        let recheck = self.parked == Some(Parked::Recheck);
        if recheck {
            self.parked = None;
            self.retries = 0;
        }
        recheck
    }
}

/// An address announced and waiting for the check of its owner.
pub struct Announced {
    pub surface: WlSurface,
    pub address: MenuAddress,
    pub retry: OwnerRetry,
    /// The timer of the retry that is due.
    pub timer: Option<RegistrationToken>,
}

/// Presentation model, the bars of the menu bar process and the menu addresses of windows.
pub struct MenuBars {
    pub set: MenuBarSet,
    bars: Vec<Bar>,
    /// The colour of the wallpaper under the bar of each output whose wallpaper the server has measured, band by band
    /// across the output.
    backdrops: HashMap<OutputId, BarBands>,
    /// Addresses announced by windows, by window id and kind.
    addresses: HashMap<(u64, MenuKind), MenuAddress>,
    /// Addresses announced before the window mapped.
    pending: Vec<(WlSurface, MenuAddress)>,
    /// Addresses announced and waiting for the check of their owner, by announcing surface and kind.
    announced: HashMap<(ObjectId, MenuKind), Announced>,
    /// How many announcements were made.
    announcements: u64,
    /// Surfaces whose announcement over the limits was logged: a client repeating it is logged once.
    over_limits: HashSet<ObjectId>,
    /// The last `edit` request still to be sent; a newer one replaces it.
    edit: Option<PendingEdit>,
    app_menus: Vec<AppMenu>,
    /// The unique bus name of the menu bar's connection, checked to be the bar's process; `None` while there is none.
    reader: Option<String>,
    /// The timer of the next check of the reader after a refusal for a passing reason, and the wait after it.
    reader_retry: Option<RegistrationToken>,
    reader_backoff: Duration,
    /// The keyboard is on a bar or one of its menus ([`Cupertino::keep_activated_under_menu_bar`]).
    holds_keyboard: bool,
    /// The process behind the connections of the latest active windows and whether it is the X11 bridge, latest
    /// last: read once per connection, not on each group ([`Cupertino::window_process`]).
    clients: Vec<(ClientId, (u32, bool))>,
}

impl MenuBars {
    /// When a pending edit is dropped for waiting too long.
    pub fn edit_deadline(&self) -> Option<f64> {
        self.edit.as_ref().map(|e| e.since + EDIT_WAIT)
    }

    pub fn new(bar_height: f64) -> Self {
        Self {
            set: MenuBarSet::new(BarSetConfig {
                bar_height,
                ..BarSetConfig::default()
            }),
            bars: Vec::new(),
            backdrops: HashMap::new(),
            addresses: HashMap::new(),
            pending: Vec::new(),
            announced: HashMap::new(),
            announcements: 0,
            over_limits: HashSet::new(),
            edit: None,
            app_menus: Vec::new(),
            reader: None,
            reader_retry: None,
            reader_backoff: READER_RETRY_FIRST,
            holds_keyboard: false,
            clients: Vec::new(),
        }
    }

    /// `surface` announced an address over the limits: whether this is the first time, the one to log.
    pub fn over_limits(&mut self, surface: ObjectId) -> bool {
        self.over_limits.insert(surface)
    }

    /// `surface` is gone: a later surface of the same id is logged afresh.
    pub fn forget_over_limits(&mut self, surface: &ObjectId) {
        self.over_limits.remove(surface);
    }

    pub fn has_app_menu(&self, surface: &WlSurface) -> bool {
        self.app_menus.iter().any(|m| &m.surface == surface)
    }

    /// Track a new menu object and tell it, from version 3, which connection reads its menus and whether a bar shows
    /// them.
    pub fn add_app_menu(&mut self, resource: CupertinoAppMenuV1, surface: WlSurface) {
        if resource.version() >= 3 {
            resource.menu_reader(self.reader.clone().unwrap_or_default());
        }
        let shown = shown(
            !self.bars.is_empty(),
            self.reader.is_some(),
            resource.version(),
        );
        resource.menu_bar(shown);
        self.app_menus.push(AppMenu {
            resource,
            surface,
            announced: false,
            shown,
        });
    }

    /// Send `menu_bar` to every menu object whose value changed.
    fn send_shown(&mut self) {
        let (bars, reader) = (!self.bars.is_empty(), self.reader.is_some());
        for menu in &mut self.app_menus {
            let shown = shown(bars, reader, menu.resource.version());
            if shown != menu.shown {
                menu.shown = shown;
                menu.resource.menu_bar(shown);
            }
        }
    }

    pub fn has_surface(&self, surface: &WlSurface) -> bool {
        self.bars.iter().any(|b| &b.surface == surface)
    }

    pub fn is_empty(&self) -> bool {
        self.bars.is_empty()
    }

    pub fn has_bar(&self, output: OutputId) -> bool {
        self.bars.iter().any(|b| b.output == output)
    }

    /// Height the bar of `output` takes from windows: none without a bar or while it is auto-hidden.
    pub fn reserved_height(&self, output: OutputId) -> f64 {
        if self.has_bar(output) {
            self.set.reserved_height(output).unwrap_or(0.0)
        } else {
            0.0
        }
    }

    /// Presentation of `surface` when it is a menu bar. `output_y` is the top of its output.
    pub fn look(&self, surface: &WlSurface, output_y: f64, now: f64) -> Option<BarLook> {
        let bar = self.bars.iter().find(|b| &b.surface == surface)?;
        let p = self.set.presentation(bar.output, now)?;
        Some(BarLook {
            dy: p.rect.y - output_y,
            alpha: (p.visible * (1.0 - INACTIVE_DIM * p.inactive)) as f32,
        })
    }

    pub fn remove_output(&mut self, output: OutputId, now: f64) {
        self.set.remove_output(output, now);
        self.bars.retain(|b| b.output != output);
        self.backdrops.remove(&output);
    }

    /// Announce (or withdraw with `None`) the address of kind `kind` of window `key`.
    pub fn set_address(&mut self, key: u64, kind: MenuKind, address: Option<MenuAddress>) {
        match address {
            Some(a) => self.addresses.insert((key, kind), a),
            None => self.addresses.remove(&(key, kind)),
        };
    }

    /// The address the bar is told for window `key`: the GMenuModel one when it has a menu bar, else the DBusMenu one,
    /// else a GMenuModel application menu alone (which must not hide a whole DBusMenu menu bar).
    pub fn address(&self, key: u64) -> Option<&MenuAddress> {
        let gtk = self.addresses.get(&(key, MenuKind::Gtk));
        let has_menu_bar =
            |a: &&MenuAddress| matches!(a, MenuAddress::Gtk(p) if p.menubar_object_path.is_some());
        gtk.filter(has_menu_bar)
            .or_else(|| self.addresses.get(&(key, MenuKind::DbusMenu)))
            .or(gtk)
    }

    /// Window `key`, whose root surface is `root`, is gone: its addresses go with it.
    pub fn forget_window(&mut self, key: u64, root: &WlSurface) {
        self.addresses.retain(|(window, _), _| *window != key);
        self.pending.retain(|(surface, _)| surface != root);
    }

    /// Keep an address of kind `kind` announced before its window mapped (`None` withdraws).
    pub fn set_pending(
        &mut self,
        surface: &WlSurface,
        kind: MenuKind,
        address: Option<MenuAddress>,
    ) {
        self.pending
            .retain(|(s, a)| s != surface || a.kind() != kind);
        if let Some(a) = address {
            self.pending.push((surface.clone(), a));
        }
    }

    /// Addresses announced for `root` before its window mapped, oldest first.
    pub fn take_pending(&mut self, root: &WlSurface) -> Vec<MenuAddress> {
        let (mine, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.pending)
            .into_iter()
            .partition(|(s, _)| s == root);
        self.pending = rest;
        mine.into_iter().map(|(_, a)| a).collect()
    }

    /// Keep `address` of `surface` until the owner of its bus name is checked (`crate::owner_check`); a newer
    /// announcement of the same kind replaces it, and is returned.
    pub fn announce(&mut self, surface: &WlSurface, address: MenuAddress) -> Option<Announced> {
        self.announcements += 1;
        let announced = Announced {
            surface: surface.clone(),
            address,
            retry: OwnerRetry::new(self.announcements),
            timer: None,
        };
        self.announced
            .insert((surface.id(), announced.address.kind()), announced)
    }

    /// The address of kind `kind` `surface` announced and is being checked, taken: withdrawn, or the check returned.
    pub fn take_announced(&mut self, surface: &ObjectId, kind: MenuKind) -> Option<Announced> {
        self.announced.remove(&(surface.clone(), kind))
    }

    /// The address of kind `kind` `surface` announced and is being checked.
    pub fn announced(&self, surface: &ObjectId, kind: MenuKind) -> Option<&Announced> {
        self.announced.get(&(surface.clone(), kind))
    }

    pub fn announced_mut(&mut self, surface: &ObjectId, kind: MenuKind) -> Option<&mut Announced> {
        self.announced.get_mut(&(surface.clone(), kind))
    }

    /// The surfaces with an address announced and not checked yet, with its kind.
    pub fn announced_surfaces(&self) -> impl Iterator<Item = (&WlSurface, MenuKind)> {
        self.announced
            .iter()
            .map(|((_, kind), announced)| (&announced.surface, *kind))
    }

    /// How many announcements wait for a retry of their owner check.
    pub fn owner_retries_due(&self) -> usize {
        self.announced
            .values()
            .filter(|a| a.timer.is_some())
            .count()
    }
}

/// Per-object data of a menu bar. `output` is `None` for an object the server rejected with a
/// protocol error: it stays inert until the client is disconnected.
pub struct MenuBarData {
    output: Option<OutputId>,
}

mod dispatch;
use dispatch::{is_bus_name, is_object_path};

pub(crate) use dispatch::is_unique_bus_name;
pub use dispatch::{AppMenuData, MAX_APPLICATION_ID_BYTES};
