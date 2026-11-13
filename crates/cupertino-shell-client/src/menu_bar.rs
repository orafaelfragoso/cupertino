//! Typed wrapper of `cupertino_menu_bar_v1`: the menu-bar role of a layer surface.

use cupertino_protocols::client::cupertino_menu_bar_v1::{
    self, AgentTrust, AppWindowFlags, BoundAction as WireBoundAction, Command, CupertinoMenuBarV1,
    EditAction as WireEditAction, ModifierKey, SessionAction as WireSessionAction, WindowControl,
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
                let update = std::mem::take(&mut self.update);
                return (update != MenuBarUpdate::default()).then_some(update);
            }
            _ => {}
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cupertino_menu_bar_v1::Event;

    fn feed(events: Vec<Event>) -> Vec<MenuBarUpdate> {
        let mut group = GroupBuilder::default();
        events.into_iter().filter_map(|e| group.push(e)).collect()
    }

    #[test]
    fn a_group_is_applied_at_its_done_event() {
        let updates = feed(vec![
            Event::ActiveApp {
                app_id: "org.example.editor".into(),
                title: "notes.txt".into(),
            },
            Event::MenuDbusmenu {
                service: ":1.42".into(),
                path: "/MenuBar".into(),
            },
            Event::WindowState {
                can_zoom: 1,
                workspace: 2,
                workspaces: 4,
            },
            Event::Done,
        ]);
        assert_eq!(
            updates,
            vec![MenuBarUpdate {
                active_app: Some(ActiveApp {
                    app_id: "org.example.editor".into(),
                    title: "notes.txt".into()
                }),
                menu: Some(WindowMenu::DbusMenu {
                    service: ":1.42".into(),
                    path: "/MenuBar".into()
                }),
                window_state: Some(WindowState {
                    can_minimize: true,
                    can_zoom: true,
                    full_screen: false,
                    workspace: 2,
                    workspaces: 4
                }),
                engage: false,
                engage_help: false,
                screen_capture: None,
                session_agent: None,
                backdrop_lightness: None,
                backdrop_bands: None,
                windows: None,
                shortcuts: None,
            }]
        );
    }

    fn feed_version_10(events: Vec<Event>) -> Vec<MenuBarUpdate> {
        let mut group = GroupBuilder::new(10);
        events.into_iter().filter_map(|e| group.push(e)).collect()
    }

    fn window_state() -> Event {
        Event::WindowState {
            can_zoom: 1,
            workspace: 1,
            workspaces: 1,
        }
    }

    #[test]
    fn the_window_list_comes_with_the_window_state() {
        let updates = feed_version_10(vec![
            window_state(),
            Event::AppWindow {
                id: 7,
                title: "notes.txt".into(),
                flags: WEnum::Value(AppWindowFlags::Active),
            },
            Event::AppWindow {
                id: 3,
                title: "draft.txt".into(),
                flags: WEnum::Value(AppWindowFlags::Minimized),
            },
            Event::Done,
            window_state(),
            Event::Done,
            Event::Engage,
            Event::Done,
        ]);
        assert_eq!(
            updates[0].windows,
            Some(vec![
                AppWindow {
                    id: 7,
                    title: "notes.txt".into(),
                    active: true,
                    minimized: false,
                },
                AppWindow {
                    id: 3,
                    title: "draft.txt".into(),
                    active: false,
                    minimized: true,
                },
            ])
        );
        assert_eq!(updates[1].windows, Some(vec![]), "the app has no window");
        assert_eq!(updates[2].windows, None, "no window state, no list");
        assert_eq!(
            feed(vec![window_state(), Event::Done])[0].windows,
            None,
            "a version 9 server lists no windows"
        );
    }

    #[test]
    fn the_shortcut_group_names_the_chords_and_the_primary_modifier() {
        let updates = feed_version_10(vec![
            Event::Shortcut {
                action: WEnum::Value(WireBoundAction::QuitApp),
                chord: "Ctrl+q".into(),
            },
            Event::Shortcut {
                action: WEnum::Value(WireBoundAction::TileLeft),
                chord: "Ctrl+Super+Left".into(),
            },
            Event::Shortcut {
                action: WEnum::Value(WireBoundAction::Zoom),
                chord: String::new(),
            },
            Event::PrimaryModifier {
                key: WEnum::Value(ModifierKey::Ctrl),
            },
            Event::Done,
        ]);
        let shortcuts = updates[0].shortcuts.as_ref().unwrap();
        assert_eq!(shortcuts.chord(BoundAction::QuitApp), Some("Ctrl+q"));
        assert_eq!(
            shortcuts.chord(BoundAction::TileLeft),
            Some("Ctrl+Super+Left")
        );
        assert_eq!(shortcuts.chord(BoundAction::Zoom), None, "unbound");
        assert_eq!(shortcuts.chord(BoundAction::Minimize), None, "not sent");
        assert_eq!(shortcuts.primary, PrimaryModifier::Ctrl);
        assert_eq!(updates[0].windows, None);
    }

    #[test]
    fn unknown_enum_values_are_ignored() {
        let updates = feed_version_10(vec![
            Event::Shortcut {
                action: WEnum::Unknown(99),
                chord: "Super+x".into(),
            },
            Event::PrimaryModifier {
                key: WEnum::Unknown(7),
            },
            window_state(),
            Event::AppWindow {
                id: 1,
                title: "a".into(),
                flags: WEnum::Unknown(0b101),
            },
            Event::Done,
        ]);
        assert_eq!(updates[0].shortcuts, Some(Shortcuts::default()));
        assert_eq!(
            updates[0].windows,
            Some(vec![AppWindow {
                id: 1,
                title: "a".into(),
                active: true,
                minimized: false,
            }]),
            "unknown flag bits are dropped"
        );
    }

    #[test]
    fn groups_do_not_leak_into_each_other() {
        let updates = feed(vec![
            Event::MenuNone,
            Event::Done,
            Event::Engage,
            Event::Done,
            Event::ScreenCapture { active: 1 },
            Event::Done,
        ]);
        assert_eq!(updates[0].menu, Some(WindowMenu::None));
        assert_eq!(
            updates[1],
            MenuBarUpdate {
                engage: true,
                ..MenuBarUpdate::default()
            }
        );
        assert_eq!(
            updates[2],
            MenuBarUpdate {
                screen_capture: Some(true),
                ..MenuBarUpdate::default()
            }
        );
    }

    #[test]
    fn engage_help_and_the_help_search_chord_arrive_in_their_groups() {
        let updates = feed_version_10(vec![
            Event::EngageHelp,
            Event::Done,
            Event::Shortcut {
                action: WEnum::Value(WireBoundAction::HelpSearch),
                chord: "Shift+Super+slash".into(),
            },
            Event::PrimaryModifier {
                key: WEnum::Value(ModifierKey::Super),
            },
            Event::Done,
        ]);
        assert_eq!(
            updates[0],
            MenuBarUpdate {
                engage_help: true,
                ..MenuBarUpdate::default()
            }
        );
        assert_eq!(
            updates[1]
                .shortcuts
                .as_ref()
                .and_then(|s| s.chord(BoundAction::HelpSearch)),
            Some("Shift+Super+slash")
        );
    }

    #[test]
    fn the_backdrop_lightness_arrives_in_its_group_capped_at_white() {
        let updates = feed(vec![
            Event::BackdropLuminance { luminance: 620 },
            Event::Done,
            Event::BackdropLuminance { luminance: 70_000 },
            Event::Done,
        ]);
        assert_eq!(
            updates,
            [
                MenuBarUpdate {
                    backdrop_lightness: Some(620),
                    ..MenuBarUpdate::default()
                },
                MenuBarUpdate {
                    backdrop_lightness: Some(1000),
                    ..MenuBarUpdate::default()
                }
            ]
        );
    }

    #[test]
    fn the_backdrop_bands_arrive_in_their_group_three_channels_a_band_and_capped_in_number() {
        let bands = |values: &[[u16; 3]]| {
            values
                .iter()
                .flat_map(|b| b.iter().flat_map(|c| c.to_le_bytes()))
                .collect::<Vec<u8>>()
        };
        let mut long = bands(&[[7, 8, 9]; 300]);
        long.extend([1, 2, 3]);
        let updates = feed(vec![
            Event::BackdropBands {
                bands: bands(&[[0, 1, 2], [65_535, 300, 45]]),
            },
            Event::Done,
            Event::BackdropBands { bands: long },
            Event::Done,
        ]);
        assert_eq!(
            updates,
            [
                MenuBarUpdate {
                    backdrop_bands: Some(vec![[0, 1, 2], [65_535, 300, 45]]),
                    ..MenuBarUpdate::default()
                },
                MenuBarUpdate {
                    backdrop_bands: Some(vec![[7, 8, 9]; MAX_BACKDROP_BANDS]),
                    ..MenuBarUpdate::default()
                }
            ]
        );
    }

    #[test]
    fn whether_the_window_is_in_full_screen_comes_with_its_window_state() {
        let full = |on: u32| Event::WindowFullScreen { full_screen: on };
        let updates = feed(vec![
            full(1),
            window_state(),
            Event::Done,
            window_state(),
            full(0),
            Event::Done,
            window_state(),
            Event::Done,
        ]);
        assert_eq!(
            updates
                .iter()
                .map(|u| u.window_state.map(|s| s.full_screen))
                .collect::<Vec<_>>(),
            [Some(true), Some(false), Some(false)],
            "in either order; not in full screen without the event"
        );
    }

    #[test]
    fn the_window_controls_set_what_the_window_state_enables() {
        let controls = |enabled| Event::WindowControls {
            enabled: WEnum::Value(enabled),
        };
        let enabled = |updates: &[MenuBarUpdate]| {
            updates
                .iter()
                .map(|u| u.window_state.map(|s| (s.can_minimize, s.can_zoom)))
                .collect::<Vec<_>>()
        };
        let updates = feed(vec![
            controls(WindowControl::Zoom),
            window_state(),
            Event::Done,
            window_state(),
            controls(WindowControl::Minimize),
            Event::Done,
            Event::WindowState {
                can_zoom: 0,
                workspace: 1,
                workspaces: 1,
            },
            controls(WindowControl::Minimize | WindowControl::Zoom),
            Event::Done,
            window_state(),
            Event::Done,
        ]);
        assert_eq!(
            enabled(&updates),
            [
                Some((false, true)),
                Some((true, false)),
                Some((true, false)),
                Some((true, true))
            ],
            "in either order; zoom only while both allow it; both without the event"
        );
        let updates = feed(vec![
            controls(WindowControl::empty()),
            Event::Done,
            window_state(),
            Event::Done,
        ]);
        assert_eq!(
            enabled(&updates),
            [Some((true, true))],
            "a group without window_state drops the controls"
        );
    }

    #[test]
    fn empty_strings_are_absent_gtk_paths() {
        let updates = feed(vec![
            Event::MenuGtk {
                bus_name: ":1.7".into(),
                application_id: "org.example.app".into(),
                application_object_path: "/org/example/app".into(),
                window_object_path: String::new(),
                menubar_object_path: "/org/example/app/menus/menubar".into(),
                app_menu_object_path: String::new(),
            },
            Event::Done,
        ]);
        assert_eq!(
            updates[0].menu,
            Some(WindowMenu::Gtk(GtkMenu {
                bus_name: ":1.7".into(),
                application_id: Some("org.example.app".into()),
                application_object_path: Some("/org/example/app".into()),
                window_object_path: None,
                menubar_object_path: Some("/org/example/app/menus/menubar".into()),
                app_menu_object_path: None,
            }))
        );
    }

    #[test]
    fn a_group_without_content_yields_nothing() {
        assert_eq!(feed(vec![Event::Done]), vec![]);
        assert_eq!(
            feed(vec![Event::MenuNone, Event::Done])[0].menu,
            Some(WindowMenu::None)
        );
    }
}
