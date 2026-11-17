//! Typed wrapper of `cupertino_menu_bar_v1`: the menu-bar role of a layer surface.

use cupertino_protocols::client::cupertino_menu_bar_v1::{
    self, AgentTrust, AppWindowFlags, BoundAction as WireBoundAction, ClientKind, Command,
    CupertinoMenuBarV1, EditAction as WireEditAction, KeyClass as WireKeyClass, ModifierKey,
    SessionAction as WireSessionAction, WindowControl,
};
use std::collections::BTreeMap;
use wayland_client::{Proxy, WEnum};

/// The version that adds the window list, the shortcuts and the commands of the default menus.
const DEFAULT_MENUS_VERSION: u32 = 10;

/// The version that adds the edit request.
const EDIT_VERSION: u32 = 11;

/// The version that adds the force_quit_app request.
const FORCE_QUIT_APP_VERSION: u32 = 13;

/// Application id (the desktop entry id when known) and title of the active window; both are
/// empty when no window is active.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveApp {
    pub app_id: String,
    pub title: String,
}

/// Where a window exports its GMenuModel menus; an absent path is `None`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GtkMenu {
    pub bus_name: String,
    pub application_id: Option<String>,
    pub application_object_path: Option<String>,
    pub window_object_path: Option<String>,
    pub menubar_object_path: Option<String>,
    pub app_menu_object_path: Option<String>,
}

/// How the active window exports its menus.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WindowMenu {
    /// The window exports none.
    None,
    DbusMenu {
        service: String,
        path: String,
    },
    Gtk(GtkMenu),
}

/// State of the active window for the window menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowState {
    /// Whether the window's minimize is enabled; from version 15, else always `true`.
    pub can_minimize: bool,
    pub can_zoom: bool,
    /// Whether the window is in full screen; from version 16, else always `false`.
    pub full_screen: bool,
    /// 1-based workspace of the window, `0` for none.
    pub workspace: u32,
    /// Number of workspaces of the output.
    pub workspaces: u32,
}

/// The process that shows the active window (version 24), for a window that registers its menus on the session bus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActiveClient {
    /// The window's own process; `0` when no window is active or the process is not known.
    Wayland(u32),
    /// The X11 bridge, which shows the windows of every X11 program; its process id.
    X11(u32),
}

/// How the server passes the person's chords to the active window (version 25).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KeyClass {
    /// The window gets the control chord of each command-key chord.
    #[default]
    Other,
    /// The window uses the desktop's toolkit and reads the command key itself.
    Toolkit,
    /// A terminal's window: it gets the control and shift chord.
    Terminal,
}

/// A window of the active application, for the window list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppWindow {
    /// The server's window id, stable for the window's lifetime.
    pub id: u32,
    pub title: String,
    pub active: bool,
    pub minimized: bool,
}

/// A server action whose chord the menus show.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BoundAction {
    CloseWindow,
    Minimize,
    Zoom,
    ToggleFullscreen,
    HideApp,
    HideOthers,
    QuitApp,
    TileLeft,
    TileRight,
    TileFill,
    ForceQuit,
    Lock,
    LogOut,
    /// The Help search shortcut (version 12).
    HelpSearch,
}

impl BoundAction {
    fn from_wire(action: WEnum<WireBoundAction>) -> Option<Self> {
        Some(match action {
            WEnum::Value(WireBoundAction::CloseWindow) => BoundAction::CloseWindow,
            WEnum::Value(WireBoundAction::Minimize) => BoundAction::Minimize,
            WEnum::Value(WireBoundAction::Zoom) => BoundAction::Zoom,
            WEnum::Value(WireBoundAction::ToggleFullscreen) => BoundAction::ToggleFullscreen,
            WEnum::Value(WireBoundAction::HideApp) => BoundAction::HideApp,
            WEnum::Value(WireBoundAction::HideOthers) => BoundAction::HideOthers,
            WEnum::Value(WireBoundAction::QuitApp) => BoundAction::QuitApp,
            WEnum::Value(WireBoundAction::TileLeft) => BoundAction::TileLeft,
            WEnum::Value(WireBoundAction::TileRight) => BoundAction::TileRight,
            WEnum::Value(WireBoundAction::TileFill) => BoundAction::TileFill,
            WEnum::Value(WireBoundAction::ForceQuit) => BoundAction::ForceQuit,
            WEnum::Value(WireBoundAction::Lock) => BoundAction::Lock,
            WEnum::Value(WireBoundAction::LogOut) => BoundAction::LogOut,
            WEnum::Value(WireBoundAction::HelpSearch) => BoundAction::HelpSearch,
            WEnum::Value(_) | WEnum::Unknown(_) => return None,
        })
    }
}

/// The physical key the modifier profile makes the primary modifier.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PrimaryModifier {
    #[default]
    Super,
    Ctrl,
}

/// The chords the server binds to its actions, and the primary modifier of the profile.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Shortcuts {
    /// The physical chord of each bound action in the shortcut file's syntax (`Ctrl+Alt+Shift+Super+key`); an
    /// unbound action has no entry.
    pub chords: BTreeMap<BoundAction, String>,
    pub primary: PrimaryModifier,
}

impl Shortcuts {
    /// The chord bound to `action`, `None` when it is unbound.
    pub fn chord(&self, action: BoundAction) -> Option<&str> {
        self.chords.get(&action).map(String::as_str)
    }
}

/// One group of events, applied at once. A field is `None` when the group does not mention it;
/// the first group after the object is created mentions everything.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MenuBarUpdate {
    pub active_app: Option<ActiveApp>,
    pub menu: Option<WindowMenu>,
    pub window_state: Option<WindowState>,
    /// The menu-bar shortcut was pressed.
    pub engage: bool,
    /// The Help search shortcut was pressed (version 12): open the Help menu with its search field focused.
    pub engage_help: bool,
    /// Whether a screen or window capture runs (the screen capture indicator).
    pub screen_capture: Option<bool>,
    /// Which process may answer as the session agent.
    pub session_agent: Option<SessionAgent>,
    /// The lightness of the wallpaper under the bar (CIE L*/100 in thousandths, at most 1000), from version 14 once
    /// the server has measured it.
    pub backdrop_lightness: Option<u16>,
    /// The mean encoded sRGB colour of the wallpaper under the bar, band by band from the output's left edge to its right
    /// (channels from 0 to 65535; at most [`MAX_BACKDROP_BANDS`] bands), from version 21 once the server has measured
    /// it, in place of [`MenuBarUpdate::backdrop_lightness`].
    pub backdrop_bands: Option<Vec<[u16; 3]>>,
    /// The windows of the active application, in stacking order from the top; a group with `window_state` carries
    /// the complete list from version 10.
    pub windows: Option<Vec<AppWindow>>,
    /// The chords of the server's actions, from version 10.
    pub shortcuts: Option<Shortcuts>,
    /// The process that shows the active window, from version 24 in every group with `window_state`.
    pub active_client: Option<ActiveClient>,
    /// How the server passes chords to the active window, with [`MenuBarUpdate::active_client`]: from version 25 the
    /// server's class, from a version 24 server the default.
    pub key_class: Option<KeyClass>,
}

/// The most bands of [`MenuBarUpdate::backdrop_bands`] a bar reads; the rest of a longer array is dropped.
pub const MAX_BACKDROP_BANDS: usize = 256;

/// The process whose ownership of the session agent's bus name is trusted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionAgent {
    /// No session agent runs.
    None,
    /// The owner of the name is the agent when it is this process.
    Process(u32),
    /// A development run: whoever owns the name.
    Any,
}

/// A window-management command of the fallback menus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowCommand {
    HideApp,
    HideOthers,
    ShowAll,
    QuitApp,
    Minimize,
    Zoom,
    Close,
    ToggleFullscreen,
    TileLeft,
    TileRight,
    TileFill,
    /// Raise every window of the active application, keeping their order and the focus.
    BringAllToFront,
    /// Bring all to front and cascade the windows from the active one.
    CascadeAll,
    MinimizeAll,
    ZoomAll,
    ForceQuitDialog,
}

impl WindowCommand {
    /// The version of `cupertino_menu_bar_v1` that adds the command.
    fn since(self) -> u32 {
        match self {
            WindowCommand::HideApp
            | WindowCommand::HideOthers
            | WindowCommand::ShowAll
            | WindowCommand::QuitApp
            | WindowCommand::Minimize
            | WindowCommand::Zoom
            | WindowCommand::Close => 1,
            WindowCommand::ToggleFullscreen
            | WindowCommand::TileLeft
            | WindowCommand::TileRight
            | WindowCommand::TileFill
            | WindowCommand::BringAllToFront
            | WindowCommand::CascadeAll
            | WindowCommand::MinimizeAll
            | WindowCommand::ZoomAll
            | WindowCommand::ForceQuitDialog => DEFAULT_MENUS_VERSION,
        }
    }
}

impl From<WindowCommand> for Command {
    fn from(c: WindowCommand) -> Self {
        match c {
            WindowCommand::HideApp => Command::HideApp,
            WindowCommand::HideOthers => Command::HideOthers,
            WindowCommand::ShowAll => Command::ShowAll,
            WindowCommand::QuitApp => Command::QuitApp,
            WindowCommand::Minimize => Command::Minimize,
            WindowCommand::Zoom => Command::Zoom,
            WindowCommand::Close => Command::Close,
            WindowCommand::ToggleFullscreen => Command::ToggleFullscreen,
            WindowCommand::TileLeft => Command::TileLeft,
            WindowCommand::TileRight => Command::TileRight,
            WindowCommand::TileFill => Command::TileFill,
            WindowCommand::BringAllToFront => Command::BringAllToFront,
            WindowCommand::CascadeAll => Command::CascadeAll,
            WindowCommand::MinimizeAll => Command::MinimizeAll,
            WindowCommand::ZoomAll => Command::ZoomAll,
            WindowCommand::ForceQuitDialog => Command::ForceQuitDialog,
        }
    }
}

/// What an entry of the system menu asks of the session agent; the server forwards it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionAction {
    Sleep,
    /// The confirmation panel of a restart.
    Restart,
    ShutDown,
    Lock,
    LogOut,
    /// Lock the session and show the login window, where another person can log in.
    SwitchUser,
    /// The menu is about to open: ask the agent to read what the machine allows again; it answers by changing
    /// `Capabilities`. Not an entry of the menu.
    RefreshCapabilities,
}

impl From<SessionAction> for WireSessionAction {
    fn from(a: SessionAction) -> Self {
        match a {
            SessionAction::Sleep => WireSessionAction::Sleep,
            SessionAction::Restart => WireSessionAction::Restart,
            SessionAction::ShutDown => WireSessionAction::ShutDown,
            SessionAction::Lock => WireSessionAction::Lock,
            SessionAction::LogOut => WireSessionAction::LogOut,
            SessionAction::SwitchUser => WireSessionAction::SwitchUser,
            SessionAction::RefreshCapabilities => WireSessionAction::RefreshCapabilities,
        }
    }
}

/// An item of the Edit menu, for the server to send as a key chord to the key window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditAction {
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
}

impl From<EditAction> for WireEditAction {
    fn from(a: EditAction) -> Self {
        match a {
            EditAction::Undo => WireEditAction::Undo,
            EditAction::Redo => WireEditAction::Redo,
            EditAction::Cut => WireEditAction::Cut,
            EditAction::Copy => WireEditAction::Copy,
            EditAction::Paste => WireEditAction::Paste,
            EditAction::SelectAll => WireEditAction::SelectAll,
        }
    }
}

/// The menu bar of one output. Dropping it gives up the role.
pub struct MenuBar {
    proxy: CupertinoMenuBarV1,
}

impl MenuBar {
    pub(crate) fn new(proxy: CupertinoMenuBarV1) -> Self {
        Self { proxy }
    }

    /// Tell the server a menu of this bar is open; it keeps the bar revealed meanwhile.
    pub fn set_menu_open(&self, open: bool) {
        self.proxy.set_menu_open(u32::from(open));
    }

    /// Run a command on the active application or window at the time the server handles it. A command the
    /// server's version lacks is not sent.
    pub fn window_command(&self, command: WindowCommand) {
        if self.proxy.version() >= command.since() {
            self.proxy.window_command(command.into());
        }
    }

    /// Show a window of the window list: unminimize, raise and focus it. Not sent before version 10.
    pub fn activate_window(&self, id: u32) {
        if self.proxy.version() >= DEFAULT_MENUS_VERSION {
            self.proxy.activate_window(id);
        }
    }

    /// Ask the session agent through the server (it answers only the server).
    pub fn session_action(&self, action: SessionAction) {
        self.proxy.session_action(action.into());
    }

    /// Move the active window to a workspace of its output (1-based).
    pub fn move_to_workspace(&self, workspace: u32) {
        self.proxy.move_to_workspace(workspace);
    }

    /// An item of the Edit menu was chosen: the server types its chord into the key window once the keyboard is back
    /// there. Send it before the menu closes, while its popup still holds the grab. Not sent before version 11.
    pub fn edit(&self, action: EditAction) {
        if self.can_edit() {
            self.proxy.edit(action.into());
        }
    }

    /// Whether the server takes [`Self::edit`] (version 11).
    pub fn can_edit(&self) -> bool {
        self.proxy.version() >= EDIT_VERSION
    }

    /// Force quit the active application without the dialog, naming it by the id the bar shows: the server ends it only
    /// if it is still the active application. Not sent before version 13.
    pub fn force_quit_app(&self, app_id: &str) {
        if self.can_force_quit_app() {
            self.proxy.force_quit_app(app_id.to_owned());
        }
    }

    /// Whether the server takes [`Self::force_quit_app`] (version 13).
    pub fn can_force_quit_app(&self) -> bool {
        self.proxy.version() >= FORCE_QUIT_APP_VERSION
    }
}

impl Drop for MenuBar {
    fn drop(&mut self) {
        self.proxy.destroy();
    }
}

/// Collects the events of a group until `done`.
#[derive(Debug, Default)]
pub(crate) struct GroupBuilder {
    update: MenuBarUpdate,
    /// The server lists the windows in every group with `window_state`.
    lists_windows: bool,
    /// The enabled controls of the active window, for the group's `window_state` (version 15).
    controls: Option<WindowControl>,
    /// Whether the active window is in full screen, for the group's `window_state` (version 16).
    full_screen: Option<bool>,
}

impl GroupBuilder {
    /// The builder of a menu-bar object of `version`.
    pub(crate) fn new(version: u32) -> Self {
        Self {
            update: MenuBarUpdate::default(),
            lists_windows: version >= DEFAULT_MENUS_VERSION,
            controls: None,
            full_screen: None,
        }
    }

    /// Add an event; the finished group when it was a `done`. An empty group yields nothing.
    pub(crate) fn push(&mut self, event: cupertino_menu_bar_v1::Event) -> Option<MenuBarUpdate> {
        use cupertino_menu_bar_v1::Event;
        let optional = |s: String| Some(s).filter(|s| !s.is_empty());
        match event {
            Event::ActiveApp { app_id, title } => {
                self.update.active_app = Some(ActiveApp { app_id, title })
            }
            Event::MenuNone => self.update.menu = Some(WindowMenu::None),
            Event::MenuDbusmenu { service, path } => {
                self.update.menu = Some(WindowMenu::DbusMenu { service, path })
            }
            Event::MenuGtk {
                bus_name,
                application_id,
                application_object_path,
                window_object_path,
                menubar_object_path,
                app_menu_object_path,
            } => {
                self.update.menu = Some(WindowMenu::Gtk(GtkMenu {
                    bus_name,
                    application_id: optional(application_id),
                    application_object_path: optional(application_object_path),
                    window_object_path: optional(window_object_path),
                    menubar_object_path: optional(menubar_object_path),
                    app_menu_object_path: optional(app_menu_object_path),
                }))
            }
            Event::WindowState {
                can_zoom,
                workspace,
                workspaces,
            } => {
                self.update.window_state = Some(WindowState {
                    can_minimize: true,
                    can_zoom: can_zoom != 0,
                    full_screen: false,
                    workspace,
                    workspaces,
                })
            }
            Event::Engage => self.update.engage = true,
            Event::EngageHelp => self.update.engage_help = true,
            Event::ScreenCapture { active } => self.update.screen_capture = Some(active != 0),
            Event::SessionAgent { trust, pid } => {
                self.update.session_agent = Some(match trust {
                    WEnum::Value(AgentTrust::Process) => SessionAgent::Process(pid),
                    WEnum::Value(AgentTrust::Any) => SessionAgent::Any,
                    WEnum::Value(_) | WEnum::Unknown(_) => SessionAgent::None,
                })
            }
            Event::BackdropLuminance { luminance } => {
                self.update.backdrop_lightness = Some(luminance.min(1000) as u16)
            }
            Event::BackdropBands { bands } => {
                let (bands, _) = bands.as_chunks::<6>();
                self.update.backdrop_bands = Some(
                    bands
                        .iter()
                        .take(MAX_BACKDROP_BANDS)
                        .map(|b| [0, 1, 2].map(|c| u16::from_le_bytes([b[2 * c], b[2 * c + 1]])))
                        .collect(),
                )
            }
            Event::AppWindow { id, title, flags } => {
                let flags = match flags {
                    WEnum::Value(flags) => flags,
                    WEnum::Unknown(bits) => AppWindowFlags::from_bits_truncate(bits),
                };
                self.update
                    .windows
                    .get_or_insert_with(Vec::new)
                    .push(AppWindow {
                        id,
                        title,
                        active: flags.contains(AppWindowFlags::Active),
                        minimized: flags.contains(AppWindowFlags::Minimized),
                    })
            }
            Event::Shortcut { action, chord } => {
                let shortcuts = self.update.shortcuts.get_or_insert_with(Shortcuts::default);
                if let Some(action) = BoundAction::from_wire(action) {
                    if chord.is_empty() {
                        shortcuts.chords.remove(&action);
                    } else {
                        shortcuts.chords.insert(action, chord);
                    }
                }
            }
            Event::PrimaryModifier { key } => {
                let shortcuts = self.update.shortcuts.get_or_insert_with(Shortcuts::default);
                match key {
                    WEnum::Value(ModifierKey::Super) => shortcuts.primary = PrimaryModifier::Super,
                    WEnum::Value(ModifierKey::Ctrl) => shortcuts.primary = PrimaryModifier::Ctrl,
                    WEnum::Value(_) | WEnum::Unknown(_) => {}
                }
            }
            Event::WindowControls { enabled } => {
                self.controls = Some(match enabled {
                    WEnum::Value(enabled) => enabled,
                    WEnum::Unknown(bits) => WindowControl::from_bits_truncate(bits),
                })
            }
            Event::WindowFullScreen { full_screen } => self.full_screen = Some(full_screen != 0),
            Event::ActiveClient { pid, kind } => {
                self.update.active_client = Some(match kind {
                    WEnum::Value(ClientKind::X11) => ActiveClient::X11(pid),
                    WEnum::Value(_) | WEnum::Unknown(_) => ActiveClient::Wayland(pid),
                });
            }
            Event::ActiveClientKeys { keys } => {
                self.update.key_class = Some(match keys {
                    WEnum::Value(WireKeyClass::Toolkit) => KeyClass::Toolkit,
                    WEnum::Value(WireKeyClass::Terminal) => KeyClass::Terminal,
                    WEnum::Value(_) | WEnum::Unknown(_) => KeyClass::Other,
                });
            }
            Event::Done => {
                if self.lists_windows && self.update.window_state.is_some() {
                    self.update.windows.get_or_insert_with(Vec::new);
                }
                if let (Some(state), Some(enabled)) =
                    (self.update.window_state.as_mut(), self.controls.take())
                {
                    state.can_minimize = enabled.contains(WindowControl::Minimize);
                    state.can_zoom &= enabled.contains(WindowControl::Zoom);
                }
                if let (Some(state), Some(full_screen)) =
                    (self.update.window_state.as_mut(), self.full_screen.take())
                {
                    state.full_screen = full_screen;
                }
                // a version 24 server sends the process without the class of its keys: the default class
                if self.update.active_client.is_some() {
                    self.update.key_class.get_or_insert_default();
                }
                let update = std::mem::take(&mut self.update);
                return (update != MenuBarUpdate::default()).then_some(update);
            }
            _ => {}
        }
        None
    }
}
