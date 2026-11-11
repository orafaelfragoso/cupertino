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
use cupertino_input::{KeyCombo, Modifiers};
use cupertino_keyboard::rewrite::ClientClass;
use cupertino_modifiers::{Physical, Profile, RawMods};
use cupertino_protocols::server::app_effects::cupertino_app_menu_v1::{
    self, CupertinoAppMenuV1, Error as AppMenuError,
};
use cupertino_protocols::server::cupertino_menu_bar_v1::{
    self, AgentTrust, AppWindowFlags, BoundAction, Command as BarCommand, CupertinoMenuBarV1,
    EditAction, Error as BarError, ModifierKey, SessionAction, WindowControl,
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
    /// The backdrop lightness last sent (version 14); `None` until the first one.
    backdrop_sent: Option<u16>,
    /// The backdrop spread last sent (version 21); `None` until the first one.
    spread_sent: Option<Vec<u16>>,
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
/// The version of `cupertino_menu_bar_v1` that adds `backdrop_spread`, and the most bands it carries.
pub const BACKDROP_SPREAD_VERSION: u32 = 21;
pub const BACKDROP_SPREAD_MAX_BANDS: usize = 256;

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
    /// The lightness of the wallpaper under the bar of each output whose wallpaper the server has measured.
    backdrops: HashMap<OutputId, u16>,
    /// How unevenly light the wallpaper is under the bar of each output whose wallpaper the server has measured, band
    /// by band across the output.
    spreads: HashMap<OutputId, Vec<u16>>,
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
            spreads: HashMap::new(),
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
        self.spreads.remove(&output);
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

impl Cupertino {
    /// Drop the bars whose connection no longer holds the MenuBar role.
    pub(crate) fn release_revoked_menu_bars(&mut self) {
        let revoked: Vec<_> = self
            .menu_bars
            .bars
            .iter()
            .filter(|b| {
                b.resource
                    .client()
                    .is_some_and(|c| !authorized(&c, ShellRole::MenuBar))
            })
            .map(|b| (b.resource.clone(), b.output))
            .collect();
        for (resource, output) in revoked {
            self.menu_bars.bars.retain(|b| b.resource != resource);
            self.menu_bar_gone(output);
        }
    }

    /// `cupertino_shell_v1.get_menu_bar`: validate the role, register the bar and send its first group.
    pub fn create_menu_bar(
        &mut self,
        shell: &CupertinoShellV1,
        permit: Option<Permit<role::MenuBar>>,
        id: New<CupertinoMenuBarV1>,
        surface: WlSurface,
        output: &WlOutput,
        init: &mut DataInit<'_, Self>,
    ) {
        if permit.is_none() {
            init.init(id, MenuBarData { output: None });
            refuse(shell, ShellRole::MenuBar);
            return;
        }
        let target = Output::from_resource(output)
            .and_then(|o| self.output_by_wl(&o).map(|e| (e.id, e.output.clone())));
        let rejection = match &target {
            None => Some("the output is gone".to_string()),
            Some((_, o))
                if layer_map_for_output(o)
                    .layer_for_surface(&surface, WindowSurfaceType::TOPLEVEL)
                    .is_none() =>
            {
                Some("the surface is not a layer surface of the output".to_string())
            }
            Some(_) if self.menu_bars.has_surface(&surface) || self.docks.has_surface(&surface) => {
                Some("the surface already has a shell role".to_string())
            }
            Some((out, _)) if self.menu_bars.has_bar(*out) => {
                Some("the output already has a menu bar".to_string())
            }
            Some(_) => None,
        };
        if let Some(reason) = rejection {
            init.init(id, MenuBarData { output: None });
            shell.post_error(ShellError::Role, reason);
            return;
        }
        let (out, _) = target.expect("a rejected target returned above");
        let resource = init.init(id, MenuBarData { output: Some(out) });
        self.menu_bars.bars.push(Bar {
            resource,
            surface,
            output: out,
            sent: None,
            capture_sent: None,
            agent_sent: None,
            shortcuts_sent: None,
            backdrop_sent: None,
            spread_sent: None,
        });
        self.refresh_usable_area(out);
        self.refresh_menu_bars();
        self.refresh_capture_indicator();
        self.refresh_session_agent();
        self.refresh_menu_bar_shortcuts();
        self.refresh_backdrop_lightness();
        self.refresh_backdrop_spread();
        self.refresh_gtk_capabilities();
        self.refresh_app_menus();
        if self.menu_bars.bars.len() == 1 {
            self.check_menu_reader();
        }
    }

    /// The layer surface of a bar went away: the role ends with it.
    pub fn menu_bar_surface_gone(&mut self, surface: &WlSurface) {
        let outputs: Vec<OutputId> = self
            .menu_bars
            .bars
            .iter()
            .filter(|b| &b.surface == surface)
            .map(|b| b.output)
            .collect();
        self.menu_bars.bars.retain(|b| &b.surface != surface);
        for output in outputs {
            self.menu_bar_gone(output);
        }
    }

    /// A bar of `output` went away: a menu it had open closes with it, so its reveal does not
    /// outlive it, and its reservation goes.
    fn menu_bar_gone(&mut self, output: OutputId) {
        let now = self.now();
        self.menu_bars.set.set_menu_open(output, false, now);
        self.refresh_usable_area(output);
        self.refresh_gtk_capabilities();
        self.refresh_app_menus();
    }

    /// Tell the windows' menu objects whether a bar shows their menus, when that changed: while any bar runs it
    /// reads the address of the active window.
    pub fn refresh_app_menus(&mut self) {
        if self.menu_bars.is_empty() {
            self.check_menu_reader();
        }
        self.menu_bars.send_shown();
    }

    /// Follow the owner of [`MENU_BAR_BUS_NAME`] on the D-Bus worker: every change checks the reader again. Call once,
    /// when the worker starts.
    pub fn watch_menu_reader(&mut self) {
        let Some(dbus) = &self.dbus else { return };
        let (changed, rx) = channel::channel::<()>();
        let inserted = self.loop_handle.insert_source(rx, |event, _, state| {
            if let ChannelEvent::Msg(()) = event {
                state.check_menu_reader();
            }
        });
        if let Err(e) = inserted {
            tracing::warn!(error = %e, "cannot follow the menu bar's bus name");
            return;
        }
        // the loop is gone only while the server shuts down
        dbus.watch_owner(
            MENU_BAR_BUS_NAME,
            Box::new(move || {
                let _ = changed.send(());
            }),
        );
    }

    /// Check that the menu bar process owns [`MENU_BAR_BUS_NAME`]; with no bar there is no reader.
    fn check_menu_reader(&mut self) {
        if let Some(token) = self.menu_bars.reader_retry.take() {
            self.loop_handle.remove(token);
        }
        let client = self.menu_bars.bars.first().and_then(|b| b.surface.client());
        match client {
            Some(client) => self.check_owner(OwnerCheck {
                key: OwnerKey::MenuReader,
                name: MENU_BAR_BUS_NAME.to_owned(),
                client,
            }),
            None => {
                self.cancel_owner_check(&OwnerKey::MenuReader);
                self.menu_bars.reader_backoff = READER_RETRY_FIRST;
                self.set_menu_reader(None);
            }
        }
    }

    /// The answer to the check of the reader. A refusal for a passing reason (the identification threads are in use, or
    /// the bus is slow) keeps the reader so far and asks again after a wait that doubles; any other names no reader.
    pub(crate) fn menu_reader_checked(&mut self, verdict: Result<String, OwnerRefusal>) {
        match verdict {
            Ok(reader) => {
                self.menu_bars.reader_backoff = READER_RETRY_FIRST;
                self.set_menu_reader(Some(reader));
            }
            Err(
                refusal @ (OwnerRefusal::Busy | OwnerRefusal::TimedOut | OwnerRefusal::Stalled),
            ) => {
                let wait = self.menu_bars.reader_backoff;
                self.menu_bars.reader_backoff = (wait * 2).min(READER_RETRY_MAX);
                tracing::debug!(
                    ?refusal,
                    ?wait,
                    "the menu bar's bus name is checked again later"
                );
                let timer =
                    self.loop_handle
                        .insert_source(Timer::from_duration(wait), |_, _, state| {
                            state.menu_bars.reader_retry = None;
                            state.check_menu_reader();
                            TimeoutAction::Drop
                        });
                match timer {
                    Ok(token) => self.menu_bars.reader_retry = Some(token),
                    Err(e) => {
                        tracing::warn!(error = %e, "cannot time the next check of the menu bar's bus name")
                    }
                }
            }
            Err(refusal) => {
                tracing::info!(?refusal, "the menu bar's bus name is not the menu bar process's: the windows' menus answer no one");
                self.set_menu_reader(None);
            }
        }
    }

    /// Tell the version 3 menu objects the reader, when it changed: a new reader before `menu_bar(1)`, so a window
    /// hides its own menus only once its menus answer the bar, and `menu_bar(0)` before the reader goes. A new reader
    /// also sends every bar its group again, unchanged: a bar whose load an application refused while it had no reader
    /// loads again once the group announces the menus again.
    fn set_menu_reader(&mut self, reader: Option<String>) {
        if reader == self.menu_bars.reader {
            return;
        }
        if reader.is_none() {
            self.menu_bars.reader = None;
            self.menu_bars.send_shown();
        }
        let name = reader.clone().unwrap_or_default();
        for menu in &self.menu_bars.app_menus {
            if menu.resource.version() >= 3 {
                menu.resource.menu_reader(name.clone());
            }
        }
        if reader.is_some() {
            self.menu_bars.reader = reader;
            self.menu_bars.send_shown();
            for bar in &mut self.menu_bars.bars {
                bar.sent = None;
            }
            self.refresh_menu_bars();
        }
    }

    /// The group the bars show now: the key window and the menu it exports.
    fn menu_feed(&self) -> Feed {
        let window = self.wm.key_window().and_then(|id| self.wm.window(id));
        let Some(window) = window else {
            return Feed {
                app_id: String::new(),
                title: String::new(),
                menu: None,
                can_zoom: false,
                can_minimize: false,
                full_screen: false,
                workspace: 0,
                workspaces: 0,
                windows: Vec::new(),
            };
        };
        let workspaces = self.wm.workspaces(window.output);
        let windows = self
            .listed_windows(window.app)
            .take(MAX_APP_WINDOWS)
            .map(|w| {
                let mut flags = AppWindowFlags::empty();
                flags.set(AppWindowFlags::Active, w.id == window.id);
                flags.set(AppWindowFlags::Minimized, w.state().is_minimized());
                AppWindow {
                    id: w.id.0 as u32,
                    title: cut_text(&w.title).to_string(),
                    flags,
                }
            })
            .collect();
        Feed {
            app_id: self
                .wm
                .app(window.app)
                .map(|a| a.identity.clone())
                .unwrap_or_default(),
            title: window.title.clone(),
            menu: self.menu_bars.address(window.id.0).cloned(),
            can_zoom: window.can_zoom(),
            can_minimize: window.can_minimize(),
            full_screen: window.state().is_fullscreen(),
            workspace: workspaces
                .iter()
                .position(|w| w.id == window.workspace)
                .map_or(0, |i| i as u32 + 1),
            workspaces: workspaces.len() as u32,
            windows,
        }
    }

    /// The windows of `app` the window list names, top of the stack first: the ones the dock lists.
    fn listed_windows(&self, app: AppId) -> impl Iterator<Item = &Window> {
        self.wm
            .stacking_order()
            .iter()
            .rev()
            .filter_map(|id| self.wm.window(*id))
            .filter(move |w| {
                w.app == app
                    && !matches!(
                        w.state(),
                        LifecycleState::Created
                            | LifecycleState::Closing
                            | LifecycleState::Destroyed
                    )
            })
    }

    /// Send the current group to every bar that has not seen it. Call after anything the group
    /// derives from may have changed: focus, titles, window state, workspaces, menu addresses.
    pub fn refresh_menu_bars(&mut self) {
        if self.menu_bars.bars.is_empty() {
            return;
        }
        let feed = self.menu_feed();
        // a bar before version 10 has no window list: a change of the list alone is nothing new for it
        let listless = Feed {
            windows: Vec::new(),
            ..feed.clone()
        };
        for bar in &mut self.menu_bars.bars {
            let feed = if bar.resource.version() >= 10 {
                &feed
            } else {
                &listless
            };
            if bar.sent.as_ref() != Some(feed) {
                feed.send(&bar.resource);
                bar.sent = Some(feed.clone());
            }
        }
    }

    /// The chords of the bound actions as the bindings hold them now, in the physical keys of the person. An action
    /// bound to several chords shows the one bound last: a `keybindings` entry of the configuration over the shortcut
    /// tables of the profile.
    fn menu_bar_shortcuts(&self) -> Shortcuts {
        let profile = self.config.keyboard.modifier_profile();
        let entries = self.key_router.bindings.entries();
        let chords = BOUND_ACTIONS
            .iter()
            .map(|(bound, action)| {
                let chord = entries
                    .iter()
                    .rev()
                    .find(|(_, a)| a == action || runs_as(*action, *a))
                    .map(|(c, _)| physical_chord(*c))
                    .unwrap_or_default();
                (*bound, chord)
            })
            .collect();
        let primary = if profile.primary() == Physical::Ctrl {
            ModifierKey::Ctrl
        } else {
            ModifierKey::Super
        };
        Shortcuts { chords, primary }
    }

    /// Tell the bars of version 10 or later the chords of the bound actions and the primary modifier, each one when
    /// that changed for it. Call after the key bindings or the modifier profile changed.
    pub fn refresh_menu_bar_shortcuts(&mut self) {
        if self.menu_bars.bars.is_empty() {
            return;
        }
        let shortcuts = self.menu_bar_shortcuts();
        for bar in
            self.menu_bars.bars.iter_mut().filter(|b| {
                b.resource.version() >= 10 && b.shortcuts_sent.as_ref() != Some(&shortcuts)
            })
        {
            shortcuts.send(&bar.resource);
            bar.shortcuts_sent = Some(shortcuts.clone());
        }
    }

    /// Tell the bars of version 3 or later whether a capture session runs or a desktop-control run may capture, each one
    /// when that changed for it.
    pub fn refresh_capture_indicator(&mut self) {
        let active = self.capture_running() || self.control_capturing();
        for bar in self
            .menu_bars
            .bars
            .iter_mut()
            .filter(|b| b.resource.version() >= 3 && b.capture_sent != Some(active))
        {
            bar.resource.screen_capture(u32::from(active));
            bar.resource.done();
            bar.capture_sent = Some(active);
        }
    }

    /// Tell the bars of version 8 or later which process is the session agent, each one when that changed for it.
    pub fn refresh_session_agent(&mut self) {
        let trust = self.login_trust();
        for bar in self
            .menu_bars
            .bars
            .iter_mut()
            .filter(|b| b.resource.version() >= 8 && b.agent_sent != Some(trust))
        {
            let (kind, pid) = match trust {
                LoginTrust::Nobody => (AgentTrust::None, 0),
                LoginTrust::Agent(pid) => (AgentTrust::Process, pid),
                LoginTrust::Any => (AgentTrust::Any, 0),
            };
            bar.resource.session_agent(kind, pid);
            bar.resource.done();
            bar.agent_sent = Some(trust);
        }
    }

    /// The lightness of the wallpaper under the bar of `output` (CIE L*/100 in thousandths, clamped to
    /// [`BACKDROP_LIGHTNESS_MAX`]). A value equal to the stored one does nothing. `None` forgets the output's measure
    /// when the output goes and sends nothing: a value sent to a bar is never withdrawn.
    pub fn set_backdrop_lightness(&mut self, output: OutputId, lightness: Option<u16>) {
        let Some(lightness) = lightness.map(|l| l.min(BACKDROP_LIGHTNESS_MAX)) else {
            self.menu_bars.backdrops.remove(&output);
            return;
        };
        if self.menu_bars.backdrops.insert(output, lightness) != Some(lightness) {
            self.refresh_backdrop_lightness();
        }
    }

    /// The lightness of the wallpaper under the bar of `output`, as last measured.
    pub fn backdrop_lightness(&self, output: OutputId) -> Option<u16> {
        self.menu_bars.backdrops.get(&output).copied()
    }

    /// Tell the bars of version 14 or later the lightness of the wallpaper under them, each one the first time and
    /// when it moved by more than [`BACKDROP_LIGHTNESS_STEP`] since.
    fn refresh_backdrop_lightness(&mut self) {
        let backdrops = &self.menu_bars.backdrops;
        for bar in self
            .menu_bars
            .bars
            .iter_mut()
            .filter(|b| b.resource.version() >= 14)
        {
            let Some(&lightness) = backdrops.get(&bar.output) else {
                continue;
            };
            if bar
                .backdrop_sent
                .is_some_and(|sent| sent.abs_diff(lightness) <= BACKDROP_LIGHTNESS_STEP)
            {
                continue;
            }
            bar.resource.backdrop_luminance(u32::from(lightness));
            bar.resource.done();
            bar.backdrop_sent = Some(lightness);
        }
    }

    /// How unevenly light the wallpaper is under the bar of `output`, band by band across it (each band's value clamped
    /// to [`BACKDROP_LIGHTNESS_MAX`]). An equal profile does nothing. `None` forgets the output's measure when the
    /// output goes and sends nothing.
    pub fn set_backdrop_spread(&mut self, output: OutputId, spread: Option<Vec<u16>>) {
        let Some(spread) = spread else {
            self.menu_bars.spreads.remove(&output);
            return;
        };
        let spread: Vec<u16> = spread
            .into_iter()
            .take(BACKDROP_SPREAD_MAX_BANDS)
            .map(|s| s.min(BACKDROP_LIGHTNESS_MAX))
            .collect();
        if self.menu_bars.spreads.get(&output) != Some(&spread) {
            self.menu_bars.spreads.insert(output, spread);
            self.refresh_backdrop_spread();
        }
    }

    /// Tell the bars of version 21 or later how unevenly light the wallpaper is under them, each one the first time
    /// and when a band moved by more than [`BACKDROP_LIGHTNESS_STEP`] since.
    fn refresh_backdrop_spread(&mut self) {
        let spreads = &self.menu_bars.spreads;
        for bar in self
            .menu_bars
            .bars
            .iter_mut()
            .filter(|b| b.resource.version() >= BACKDROP_SPREAD_VERSION)
        {
            let Some(spread) = spreads.get(&bar.output) else {
                continue;
            };
            let close = bar.spread_sent.as_ref().is_some_and(|sent| {
                sent.len() == spread.len()
                    && sent
                        .iter()
                        .zip(spread)
                        .all(|(a, b)| a.abs_diff(*b) <= BACKDROP_LIGHTNESS_STEP)
            });
            if close {
                continue;
            }
            let bytes = spread.iter().flat_map(|s| s.to_le_bytes()).collect();
            bar.resource.backdrop_spread(bytes);
            bar.resource.done();
            bar.spread_sent = Some(spread.clone());
        }
    }

    /// The menu-bar shortcut: the bar of the active output opens its first menu.
    pub fn focus_menu_bar(&mut self) {
        let Some(active) = self.menu_bars.set.active_output() else {
            return;
        };
        // the bar answers by asking for exclusive keyboard interactivity, which gives it the
        // keyboard on its next commit (`layer_keyboard_exclusivity`), even when a press released it
        self.released_layer = None;
        for bar in self.menu_bars.bars.iter().filter(|b| b.output == active) {
            bar.resource.engage();
            bar.resource.done();
        }
    }

    /// The Help search shortcut: the bar of the active output opens its Help menu with the search field focused, taking
    /// the keyboard as for `engage`. Bars below version 12 are not told; nothing is sent while the session is locked or
    /// a lock is pending, whatever asks for it.
    pub fn focus_help_search(&mut self) {
        if self.session_lock.is_active() {
            return;
        }
        let Some(active) = self.menu_bars.set.active_output() else {
            return;
        };
        self.released_layer = None;
        for bar in self
            .menu_bars
            .bars
            .iter()
            .filter(|b| b.output == active && b.resource.version() >= 12)
        {
            bar.resource.engage_help();
            bar.resource.done();
        }
    }

    /// The output under the pointer is the active one; the other bars are dimmed.
    pub fn track_active_menu_bar(
        &mut self,
        pos: smithay::utils::Point<f64, smithay::utils::Logical>,
        now: f64,
    ) {
        let Some(active) = self.output_at(pos).map(|o| o.id) else {
            return;
        };
        if self.menu_bars.set.active_output() != Some(active) {
            self.menu_bars.set.set_active_output(Some(active), now);
            self.request_redraw();
        }
    }

    /// Timers of the bars: full-screen state per output and the animations.
    pub fn tick_menu_bars(&mut self, now: f64) {
        let ids: Vec<OutputId> = self.outputs.iter().map(|o| o.id).collect();
        for id in ids {
            let fullscreen = self
                .wm
                .windows()
                .any(|w| w.output == id && w.state() == LifecycleState::FullScreen);
            if !self
                .menu_bars
                .set
                .set_fullscreen(id, fullscreen, now)
                .is_empty()
            {
                self.refresh_usable_area(id);
            }
        }
        self.menu_bars.set.tick(now);
        if let Some(ev) = self.bar_edge.tick(now) {
            self.apply_bar_edge(ev, now);
        }
        self.tick_menu_bar_edit(now);
    }

    /// Reveal or hide the auto-hidden bar after the pointer dwelled at (or left) the top edge.
    pub fn apply_bar_edge(&mut self, ev: cupertino_input::EdgeEvent, now: f64) {
        let (idx, shown) = match ev {
            cupertino_input::EdgeEvent::Reveal { output } => (output, true),
            cupertino_input::EdgeEvent::Hide { output } => (output, false),
        };
        if let Some(id) = self.outputs.get(idx).map(|o| o.id) {
            self.menu_bars.set.set_revealed(id, shown, now);
            self.request_redraw();
        }
    }

    pub fn menu_bars_want_tick(&self, now: f64) -> bool {
        self.outputs.iter().any(|o| {
            self.menu_bars
                .set
                .presentation(o.id, now)
                .is_some_and(|p| p.animating)
        }) || self.bar_edge.next_deadline().is_some()
    }

    /// Run a window-management command of the fallback menus on the active application or window; ignored while
    /// the session is locked. False when the server does not know the command, or the bar's `version` does not have it.
    fn menu_bar_window_command(&mut self, command: BarCommand, version: u32) -> bool {
        // the entries from toggle_fullscreen on came with version 10
        if version < 10 && u32::from(command) >= u32::from(BarCommand::ToggleFullscreen) {
            return false;
        }
        if self.session_lock.is_active() {
            return true;
        }
        let ctx = self.action_context();
        let window = ctx.key_window;
        match command {
            BarCommand::HideApp => {
                if let Some(app) = ctx.front_app {
                    self.command(Command::HideApp { app });
                }
            }
            BarCommand::HideOthers => {
                if let Some(app) = ctx.front_app {
                    self.command(Command::HideOthers { app });
                }
            }
            BarCommand::ShowAll => self.command(Command::ShowAll),
            BarCommand::QuitApp => {
                for window in ctx.front_app_windows {
                    self.command(Command::RequestClose { window });
                }
            }
            BarCommand::Minimize => {
                if let Some(window) = window {
                    let target = self.minimize_target(window);
                    self.command(Command::Minimize { window, target });
                }
            }
            BarCommand::Zoom => {
                if let Some(window) = window {
                    self.command(Command::ToggleZoom { window });
                }
            }
            BarCommand::Close => {
                if let Some(window) = window {
                    self.command(Command::RequestClose { window });
                }
            }
            BarCommand::ToggleFullscreen => {
                let fullscreen = window
                    .and_then(|w| self.wm.window(w))
                    .is_some_and(|w| w.state().is_fullscreen());
                match window {
                    Some(window) if fullscreen => self.command(Command::ExitFullscreen { window }),
                    _ => self.execute_plan(translate_action(Action::ToggleFullscreen, &ctx)),
                }
            }
            BarCommand::TileLeft => self.execute_plan(translate_action(Action::TileLeft, &ctx)),
            BarCommand::TileRight => self.execute_plan(translate_action(Action::TileRight, &ctx)),
            BarCommand::TileFill => self.execute_plan(translate_action(Action::TileFill, &ctx)),
            BarCommand::BringAllToFront | BarCommand::CascadeAll => {
                if let Some(app) = ctx.front_app {
                    let cascade = command == BarCommand::CascadeAll;
                    self.command(Command::BringAllToFront { app, cascade });
                }
            }
            BarCommand::MinimizeAll => {
                for window in ctx.front_app_windows {
                    if self
                        .wm
                        .window(window)
                        .is_some_and(|w| !w.state().is_minimized() && !w.state().is_dead())
                    {
                        self.command(Command::Minimize {
                            window,
                            target: None,
                        });
                    }
                }
            }
            BarCommand::ZoomAll => {
                for window in ctx.front_app_windows {
                    let zoom = self.wm.window(window).is_some_and(|w| {
                        w.can_zoom()
                            && !matches!(
                                w.state(),
                                LifecycleState::Zooming | LifecycleState::Zoomed
                            )
                    });
                    if zoom {
                        self.command(Command::ToggleZoom { window });
                    }
                }
            }
            BarCommand::ForceQuitDialog => {
                self.execute_plan(translate_action(Action::ForceQuit, &ctx))
            }
            _ => return false,
        }
        true
    }

    /// `activate_window`: show the window `id` names when it is a window of the active application, as the window
    /// list names them now; anything else, and every request while the session is locked, is ignored.
    fn menu_bar_activate_window(&mut self, id: u32) {
        if self.session_lock.is_active() {
            return;
        }
        let Some(app) = self
            .wm
            .key_window()
            .and_then(|k| self.wm.window(k))
            .map(|w| w.app)
        else {
            return;
        };
        let window = WindowId(u64::from(id));
        if self.listed_windows(app).any(|w| w.id == window) {
            // unminimizes it, switches to its workspace, raises and focuses it
            self.focus_window_by_input(window);
        }
    }

    /// Move the key window to workspace `n` (1-based) of its output and switch there; ignored while the session is
    /// locked.
    fn menu_bar_move_to_workspace(&mut self, n: u32) {
        if self.session_lock.is_active() {
            return;
        }
        let Some(window) = self.wm.key_window() else {
            return;
        };
        let Some(output) = self.wm.window(window).map(|w| w.output) else {
            return;
        };
        let target = usize::try_from(n)
            .ok()
            .and_then(|n| n.checked_sub(1))
            .and_then(|i| self.wm.workspaces(output).get(i).map(|w| w.id));
        if let Some(workspace) = target {
            self.command(Command::MoveWindowToWorkspace { window, workspace });
            self.command(Command::SwitchWorkspace { output, workspace });
        }
    }

    /// `force_quit_app` of the bar: force quit the active application as the dock's Force Quit dialog does, only when
    /// `app_id` names it now. Anything else, and every request while the session is locked or a lock is pending, is
    /// ignored, so a focus change between the bar's label and the request ends nothing.
    fn menu_bar_force_quit_app(&mut self, app_id: &str) {
        if self.session_lock.is_active() {
            tracing::info!("menu bar force quit: ignored while the session is locked");
            return;
        }
        if app_id.len() > MAX_APPLICATION_ID_BYTES {
            tracing::warn!(
                len = app_id.len(),
                "menu bar force quit: the application id is too long, ignored"
            );
            return;
        }
        let active = self.key_app_id();
        if active.is_empty() || active != app_id {
            tracing::info!(
                app = app_id,
                active,
                "menu bar force quit: not the active application, ignored"
            );
            return;
        }
        self.force_quit_app(app_id);
    }

    /// The key window when an edit may go to it: the xdg toplevel of a client of the public socket, while no lock
    /// (active or pending), overview, application switcher or move or resize is under way. The server runs no
    /// Xwayland: every window it manages is an xdg toplevel (`WindowEntry`), so no X11 window is ever returned, nor a
    /// layer, lock or shell surface.
    fn edit_target(&self) -> Option<WindowId> {
        if self.session_lock.is_active()
            || self.shell.overview.is_some()
            || self.wm.switcher().is_some()
            || self.pointer_mode != PointerMode::Normal
        {
            return None;
        }
        let window = self.wm.key_window()?;
        let client = self.entry(window)?.surface().client()?;
        capabilities(&client).is_empty().then_some(window)
    }

    /// `edit` of the bar on `bar`: kept for the key window when the request comes from a choice in the bar's open
    /// menu (its popup grab is active); ignored otherwise. A newer request replaces a pending one.
    fn menu_bar_edit(&mut self, bar: &WlSurface, action: EditAction) {
        if self.popup_grab_root() != Some(bar) {
            return;
        }
        let Some(window) = self.edit_target() else {
            return;
        };
        self.menu_bars.edit = Some(PendingEdit {
            action,
            bar: bar.clone(),
            window,
            since: self.now(),
            returned: false,
        });
    }

    /// The keyboard went to the menu bar: its layer surface (the bar's keyboard mode) or one of its menus. The window
    /// whose menus the bar shows is still the window in use, so it keeps its activated state, and since its client heard
    /// the keyboard leave it is told so again: an application whose menu is open in the bar keeps its active look. The
    /// keyboard coming back, or another window becoming the key window, changes nothing here.
    pub(crate) fn keep_activated_under_menu_bar(&mut self, focused: Option<&WlSurface>) {
        let on_bar =
            focused.is_some_and(|f| self.menu_bars.has_surface(&self.shell_surface_root(f)));
        // moving between the bar and its menus is no new leave for the window
        if std::mem::replace(&mut self.menu_bars.holds_keyboard, on_bar) || !on_bar {
            return;
        }
        if let Some(entry) = self.activated_window().and_then(|w| self.entry(w)) {
            entry.restate_activated();
        }
    }

    /// The keyboard focus moved to `focused`. The first move after the bar's popup grab ended decides on the pending
    /// edit: it goes out at the next tick when the keyboard came back to its window, and is dropped when it went
    /// anywhere else, so it never reaches whatever has the keyboard later.
    pub(crate) fn menu_bar_edit_focus(&mut self, focused: Option<&WlSurface>) {
        let Some(edit) = &self.menu_bars.edit else {
            return;
        };
        if edit.returned || self.popup_grab_root() == Some(&edit.bar) {
            return;
        }
        let back =
            focused.is_some_and(|s| self.entry(edit.window).is_some_and(|e| e.has_surface(s)));
        match self.menu_bars.edit.as_mut() {
            Some(edit) if back => edit.returned = true,
            _ => self.menu_bars.edit = None,
        }
    }

    /// Send the pending edit once the keyboard is back on its window, checking every guard again; drop it
    /// [`EDIT_WAIT`] after its arrival.
    fn tick_menu_bar_edit(&mut self, now: f64) {
        let Some(edit) = &self.menu_bars.edit else {
            return;
        };
        if now - edit.since > EDIT_WAIT {
            self.menu_bars.edit = None;
            return;
        }
        if !edit.returned {
            return;
        }
        let Some(edit) = self.menu_bars.edit.take() else {
            return;
        };
        let keyboard = self.seat.get_keyboard().unwrap();
        let on_window = keyboard
            .current_focus()
            .is_some_and(|f| self.entry(edit.window).is_some_and(|e| e.has_surface(&f)));
        if !on_window
            || self.edit_target() != Some(edit.window)
            || self.popup_grab_root().is_some()
            || keyboard.is_grabbed()
            || self.pointer.is_grabbed()
        {
            return;
        }
        let kb = &self.config.keyboard;
        let app = self.key_app_id();
        if let Some((mods, sym)) = edit_chord(
            edit.action,
            kb.client_class(&app),
            kb.rewrite_opted_out(&app),
            &kb.modifier_profile(),
        ) {
            self.forward_chord(sym, mods);
        }
    }
}

impl Dispatch<CupertinoMenuBarV1, MenuBarData> for Cupertino {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &CupertinoMenuBarV1,
        request: cupertino_menu_bar_v1::Request,
        data: &MenuBarData,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let Some(output) = data.output else { return };
        if !require(client, ShellRole::MenuBar, resource) {
            return;
        }
        match request {
            cupertino_menu_bar_v1::Request::SetMenuOpen { open } => {
                let now = state.now();
                state.menu_bars.set.set_menu_open(output, open != 0, now);
                state.request_redraw();
            }
            cupertino_menu_bar_v1::Request::WindowCommand { command } => {
                let version = resource.version();
                let known =
                    matches!(command, WEnum::Value(c) if state.menu_bar_window_command(c, version));
                if !known {
                    resource.post_error(
                        BarError::InvalidCommand,
                        format!("{command:?} is not a window command"),
                    );
                }
            }
            cupertino_menu_bar_v1::Request::MoveToWorkspace { workspace } => {
                state.menu_bar_move_to_workspace(workspace)
            }
            cupertino_menu_bar_v1::Request::ActivateWindow { id } => {
                state.menu_bar_activate_window(id)
            }
            // a bar below version 11 cannot send it: the wire rejects a request newer than the object
            cupertino_menu_bar_v1::Request::Edit { action } => match action {
                WEnum::Value(action) => {
                    let bar = state
                        .menu_bars
                        .bars
                        .iter()
                        .find(|b| &b.resource == resource)
                        .map(|b| b.surface.clone());
                    if let Some(bar) = bar {
                        state.menu_bar_edit(&bar, action);
                    }
                }
                WEnum::Unknown(_) => resource.post_error(
                    BarError::InvalidEditAction,
                    format!("{action:?} is not an edit action"),
                ),
            },
            // a bar below version 13 cannot send it: the wire rejects a request newer than the object
            cupertino_menu_bar_v1::Request::ForceQuitApp { app_id } => {
                state.menu_bar_force_quit_app(&app_id)
            }
            cupertino_menu_bar_v1::Request::SessionAction { action } => {
                if action != WEnum::Value(SessionAction::RefreshCapabilities)
                    && !state.control_session_action_allowed(client)
                {
                    return;
                }
                let request = match action {
                    WEnum::Value(SessionAction::Sleep) => Some(DbusRequest::Sleep),
                    WEnum::Value(SessionAction::Restart) => Some(DbusRequest::Restart),
                    WEnum::Value(SessionAction::ShutDown) => Some(DbusRequest::ShutDown),
                    WEnum::Value(SessionAction::Lock) => {
                        state.lock_session_now();
                        return;
                    }
                    WEnum::Value(SessionAction::LogOut) => Some(DbusRequest::LogOut),
                    WEnum::Value(SessionAction::SwitchUser) => Some(DbusRequest::SwitchUser),
                    WEnum::Value(SessionAction::RefreshCapabilities) => {
                        Some(DbusRequest::RefreshCapabilities)
                    }
                    WEnum::Value(_) | WEnum::Unknown(_) => None,
                };
                match request {
                    Some(request) => state.menu_bar_session_action(request),
                    None => resource.post_error(
                        BarError::InvalidSessionAction,
                        format!("{action:?} is not a session action"),
                    ),
                }
            }
            // `Destroy` is handled in `destroyed`
            _ => {}
        }
    }

    /// The role ends with the object; the bar's reservation goes with it.
    fn destroyed(state: &mut Self, _: ClientId, resource: &CupertinoMenuBarV1, data: &MenuBarData) {
        let Some(output) = data.output else { return };
        state.menu_bars.bars.retain(|b| &b.resource != resource);
        state.menu_bar_gone(output);
    }
}

/// The longest bus name, unique or well-known, the D-Bus specification allows.
pub const MAX_BUS_NAME_BYTES: usize = 255;

/// The longest application id a window's menu announcement may carry.
pub const MAX_APPLICATION_ID_BYTES: usize = 255;

/// Whether `name` is a unique bus name such as `:1.42`.
pub(crate) fn is_unique_bus_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix(':') else {
        return false;
    };
    name.len() <= MAX_BUS_NAME_BYTES
        && rest.split('.').count() >= 2
        && rest.split('.').all(|e| {
            !e.is_empty()
                && e.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        })
}

/// The longest object path a window's menu announcement may carry: the server forwards the four paths, the bus name and
/// an application id in one `menu_gtk` event, and four paths of this length still fit the 4096-byte message limit.
pub const MAX_OBJECT_PATH_BYTES: usize = 512;

/// Whether `name` is a bus name, unique or well-known, no longer than [`MAX_BUS_NAME_BYTES`].
fn is_bus_name(name: &str) -> bool {
    name.len() <= MAX_BUS_NAME_BYTES && zbus::names::BusName::try_from(name).is_ok()
}

/// Whether `path` is a D-Bus object path no longer than [`MAX_OBJECT_PATH_BYTES`].
fn is_object_path(path: &str) -> bool {
    path.len() <= MAX_OBJECT_PATH_BYTES
        && (path == "/"
            || path.strip_prefix('/').is_some_and(|rest| {
                rest.split('/').all(|e| {
                    !e.is_empty() && e.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                })
            }))
}

/// Per-object data of a window's menu object; `surface` is `None` for an object the server rejected with a protocol
/// error.
pub struct AppMenuData {
    pub surface: Option<WlSurface>,
}

impl Dispatch<CupertinoAppMenuV1, AppMenuData> for Cupertino {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &CupertinoAppMenuV1,
        request: cupertino_app_menu_v1::Request,
        data: &AppMenuData,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let Some(surface) = &data.surface else { return };
        if let cupertino_app_menu_v1::Request::SetAddress {
            bus_name,
            application_object_path,
            window_object_path,
            menubar_object_path,
            app_menu_object_path,
        } = request
        {
            let paths = [
                &application_object_path,
                &window_object_path,
                &menubar_object_path,
            ];
            if !is_unique_bus_name(&bus_name)
                || !paths.iter().all(|p| is_object_path(p))
                || !(app_menu_object_path.is_empty() || is_object_path(&app_menu_object_path))
            {
                resource.post_error(
                    AppMenuError::InvalidAddress,
                    "a bus name or object path is malformed",
                );
                return;
            }
            let props = GtkMenuProps {
                bus_name,
                application_id: None,
                application_object_path: Some(application_object_path),
                window_object_path: Some(window_object_path),
                menubar_object_path: Some(menubar_object_path),
                app_menu_object_path: (!app_menu_object_path.is_empty())
                    .then_some(app_menu_object_path),
            };
            if let Some(menu) = state
                .menu_bars
                .app_menus
                .iter_mut()
                .find(|m| &m.resource == resource)
            {
                menu.announced = true;
            }
            state.announce_menu_address(surface, MenuAddress::Gtk(props));
        }
    }

    /// The announcement ends with the object.
    fn destroyed(state: &mut Self, _: ClientId, resource: &CupertinoAppMenuV1, data: &AppMenuData) {
        let Some(i) = state
            .menu_bars
            .app_menus
            .iter()
            .position(|m| &m.resource == resource)
        else {
            return;
        };
        let menu = state.menu_bars.app_menus.remove(i);
        if let (true, Some(surface)) = (menu.announced, &data.surface) {
            if surface.is_alive() {
                state.withdraw_menu_address(surface, MenuKind::Gtk);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::parse_chord;

    #[test]
    fn a_long_title_is_cut_at_a_character_boundary() {
        assert_eq!(cut_text("Notes"), "Notes");
        let exact = "a".repeat(MAX_TEXT_BYTES);
        assert_eq!(cut_text(&exact), exact);
        // the two-byte character spans the limit: it goes as a whole
        let long = format!("{}é{}", "a".repeat(MAX_TEXT_BYTES - 1), "b".repeat(10));
        assert_eq!(cut_text(&long), "a".repeat(MAX_TEXT_BYTES - 1));
    }

    #[test]
    fn a_chord_is_the_text_of_its_physical_keys() {
        assert_eq!(physical_chord(parse_chord("Super+q").unwrap()), "Super+q");
        assert_eq!(
            physical_chord(parse_chord("Super+Ctrl+f").unwrap()),
            "Ctrl+Super+f"
        );
        assert_eq!(physical_chord(parse_chord("Alt+F4").unwrap()), "Alt+F4");
    }

    #[test]
    fn every_edit_chord_follows_the_client_class_and_the_profile() {
        use keysyms::{KEY_a as A, KEY_c as C, KEY_v as V, KEY_x as X, KEY_z as Z};
        use EditAction::*;
        let (shift, ctrl, sup) = (RawMods::SHIFT, RawMods::CTRL, RawMods::SUPER);
        let actions = [Undo, Redo, Cut, Copy, Paste, SelectAll];
        let row = |primary: RawMods| {
            [
                Some((primary, Z)),
                Some((primary | shift, Z)),
                Some((primary, X)),
                Some((primary, C)),
                Some((primary, V)),
                Some((primary, A)),
            ]
        };
        let terminal = [
            None,
            None,
            None,
            Some((shift | ctrl, C)),
            Some((shift | ctrl, V)),
            None,
        ];
        for (profile, primary) in [(Profile::cupertino(), sup), (Profile::classic_pc(), ctrl)] {
            for (class, opted_out, want) in [
                (ClientClass::Other, false, row(ctrl)),
                (ClientClass::Other, true, row(primary)),
                (ClientClass::FirstParty, false, row(primary)),
                (ClientClass::FirstParty, true, row(primary)),
                (ClientClass::Terminal, false, terminal),
                (ClientClass::Terminal, true, terminal),
            ] {
                let got = actions.map(|a| edit_chord(a, class, opted_out, &profile));
                assert_eq!(
                    got, want,
                    "{} {class:?} opted out {opted_out}",
                    profile.name
                );
            }
        }
    }

    #[test]
    fn unique_bus_names_and_object_paths_are_checked() {
        for name in [":1.42", ":1.0.7", ":a-b.c_d"] {
            assert!(is_unique_bus_name(name), "{name}");
        }
        for name in [
            "",
            ":",
            ":1",
            ":1.",
            ":.1",
            "org.example.App",
            ":1.4 2",
            ":1.\u{e9}",
        ] {
            assert!(!is_unique_bus_name(name), "{name}");
        }
        assert!(!is_unique_bus_name(&format!(":1.{}", "2".repeat(254))));
        for path in ["/", "/org/example/Notes", "/a/_1/window/2"] {
            assert!(is_object_path(path), "{path}");
        }
        for path in ["", "org", "//", "/a/", "/a//b", "/a-b", "/a.b"] {
            assert!(!is_object_path(path), "{path}");
        }
        let longest = format!("/{}", "a".repeat(MAX_OBJECT_PATH_BYTES - 1));
        assert!(is_object_path(&longest));
        assert!(!is_object_path(&format!("{longest}a")));
    }

    #[test]
    fn only_a_timed_out_or_busy_owner_check_is_retried() {
        for refusal in [OwnerRefusal::TimedOut, OwnerRefusal::Busy] {
            let mut retry = OwnerRetry::new(1);
            assert_eq!(
                retry.refused(retryable(&refusal)),
                Refused::RetryAfter(OWNER_RETRY_DELAYS[0]),
                "{refusal:?}"
            );
        }
        for refusal in [
            OwnerRefusal::NoBus,
            OwnerRefusal::NoClientProcess,
            OwnerRefusal::NotABusName,
            OwnerRefusal::NoOwner("org.example.Editor".into()),
            OwnerRefusal::Unidentified,
            OwnerRefusal::OtherInstance {
                owner: 1,
                client: 2,
            },
            OwnerRefusal::Stalled,
        ] {
            let mut retry = OwnerRetry::new(1);
            assert_eq!(
                retry.refused(retryable(&refusal)),
                Refused::Final,
                "{refusal:?}"
            );
            assert!(!retry.due(1), "{refusal:?}");
            assert!(!retry.recheck(), "{refusal:?}");
        }
    }

    #[test]
    fn an_announcement_is_retried_at_most_three_times_then_warns_once_and_waits_for_a_recheck() {
        let mut retry = OwnerRetry::new(1);
        let mut delays = Vec::new();
        loop {
            match retry.refused(true) {
                Refused::RetryAfter(after) => {
                    delays.push(after);
                    assert!(!retry.recheck(), "no re-check while a retry is due");
                    assert!(retry.due(1));
                    assert!(!retry.due(1), "one check per retry");
                }
                Refused::RanOut { warn } => {
                    assert!(warn);
                    break;
                }
                Refused::Final => panic!("a refusal that may pass is final"),
            }
            assert!(delays.len() <= OWNER_RETRY_DELAYS.len(), "{delays:?}");
        }
        assert_eq!(delays, OWNER_RETRY_DELAYS);
        assert!(!retry.due(1), "no retry left");

        // one re-check per activation, with its retries counted afresh and no second warning
        assert!(retry.recheck());
        assert!(!retry.recheck(), "the re-check runs");
        for after in OWNER_RETRY_DELAYS {
            assert_eq!(retry.refused(true), Refused::RetryAfter(after));
            assert!(retry.due(1));
        }
        assert_eq!(retry.refused(true), Refused::RanOut { warn: false });
        assert!(retry.recheck());
    }

    #[test]
    fn a_retry_of_an_older_announcement_does_nothing() {
        let mut newer = OwnerRetry::new(2);
        assert!(!newer.due(1), "the newer announcement is being checked");
        assert_eq!(
            newer.refused(true),
            Refused::RetryAfter(OWNER_RETRY_DELAYS[0])
        );
        assert!(
            !newer.due(1),
            "the newer announcement's retry is due, not the older one's"
        );
        assert!(newer.due(2));
    }

    #[test]
    fn a_retry_that_cannot_be_timed_waits_for_a_recheck() {
        let mut retry = OwnerRetry::new(1);
        assert_eq!(
            retry.refused(true),
            Refused::RetryAfter(OWNER_RETRY_DELAYS[0])
        );
        retry.untimed();
        assert!(!retry.due(1), "no timer will fire for it");
        assert!(retry.recheck(), "the window becoming key checks it again");
    }

    #[test]
    fn an_announcement_over_the_limits_is_logged_once_per_surface() {
        let mut bars = MenuBars::new(24.0);
        let surface = ObjectId::null();
        assert!(bars.over_limits(surface.clone()), "the first is logged");
        assert!(!bars.over_limits(surface.clone()), "a repeat is not");
        bars.forget_over_limits(&surface);
        assert!(
            bars.over_limits(surface),
            "a new surface of the id is logged again"
        );
    }

    #[test]
    fn an_announced_address_is_within_limits_up_to_each_cap() {
        let path = format!("/{}", "a".repeat(MAX_OBJECT_PATH_BYTES - 1));
        let name = format!("a.{}", "b".repeat(MAX_BUS_NAME_BYTES - 2));
        let gtk = |name: &str, id: usize, path: &str| {
            MenuAddress::Gtk(GtkMenuProps {
                bus_name: name.into(),
                application_id: Some("a".repeat(id)),
                application_object_path: None,
                window_object_path: Some(path.into()),
                menubar_object_path: Some(path.into()),
                app_menu_object_path: Some(path.into()),
            })
        };
        let dbusmenu = |service: &str, path: &str| MenuAddress::DbusMenu {
            service: service.into(),
            path: path.into(),
        };
        assert!(gtk(&name, MAX_APPLICATION_ID_BYTES, &path).within_limits());
        assert!(dbusmenu(&name, &path).within_limits());
        assert!(dbusmenu(":1.42", "/MenuBar/1").within_limits());
        for refused in [
            gtk(&format!("{name}c"), 0, "/a"),
            gtk(":1.42", MAX_APPLICATION_ID_BYTES + 1, "/a"),
            gtk(":1.42", 0, &format!("{path}a")),
            gtk(":1.42", 0, "/a-b"),
            gtk("", 0, "/a"),
            dbusmenu(&format!("{name}c"), "/a"),
            dbusmenu(":1.42", &format!("{path}a")),
            dbusmenu("org", "/a"),
        ] {
            assert!(!refused.within_limits(), "{refused:?}");
        }
    }
}
