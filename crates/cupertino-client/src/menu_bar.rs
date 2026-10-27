//! Typed wrapper of `cupertino_menu_bar_v1`: the menu-bar role of a layer surface.

use cupertino_protocols::client::cupertino_menu_bar_v1::{
    self, Command, CupertinoMenuBarV1, SessionAction as WireSessionAction,
};

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
    pub can_zoom: bool,
    /// 1-based workspace of the window, `0` for none.
    pub workspace: u32,
    /// Number of workspaces of the output.
    pub workspaces: u32,
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
    /// Whether a screen or window capture runs (the screen capture indicator).
    pub screen_capture: Option<bool>,
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

    /// Run a command on the active application or window at the time the server handles it.
    pub fn window_command(&self, command: WindowCommand) {
        self.proxy.window_command(command.into());
    }

    /// Ask the session agent through the server (it answers only the server).
    pub fn session_action(&self, action: SessionAction) {
        self.proxy.session_action(action.into());
    }

    /// Move the active window to a workspace of its output (1-based).
    pub fn move_to_workspace(&self, workspace: u32) {
        self.proxy.move_to_workspace(workspace);
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
}

impl GroupBuilder {
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
                    can_zoom: can_zoom != 0,
                    workspace,
                    workspaces,
                })
            }
            Event::Engage => self.update.engage = true,
            Event::ScreenCapture { active } => self.update.screen_capture = Some(active != 0),
            Event::Done => {
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
                    can_zoom: true,
                    workspace: 2,
                    workspaces: 4
                }),
                engage: false,
                screen_capture: None,
            }]
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
