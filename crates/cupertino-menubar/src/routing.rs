//! From input and controller effects to what the process does: key mapping and the action a
//! controller effect stands for, given what the bar shows.

use crate::controller::{Effect, Key, SYSTEM_MENU_ROOT};
use crate::extras::{ExtraAction, ExtraKind, SystemAction};
use crate::fallback::{self, FallbackAction};
use crate::feed::{BarFeed, Shared};
use crate::item::{ItemId, Origin};
use crate::layout::{BarLayout, SlotKey};
use crate::source::{KdeAppMenu, MenuSource};
use crate::worker::Request;
use cupertino_apps::Launch;
use cupertino_shell_client::{EditAction, WindowCommand};
use cupertino_wayland_client::key;
use std::path::PathBuf;

/// The control panel, which Settings… of the system menu starts.
const SETTINGS_PROGRAM: &str = "cupertino-settings";

/// The menu key for an evdev key code; `None` for keys menus ignore. Letters come through
/// [`typed_key`].
pub fn menu_key(code: u32) -> Option<Key> {
    Some(match code {
        key::UP => Key::Up,
        key::DOWN => Key::Down,
        key::LEFT => Key::Left,
        key::RIGHT => Key::Right,
        key::ENTER | key::KEYPAD_ENTER => Key::Enter,
        key::SPACE => Key::Space,
        key::ESCAPE => Key::Escape,
        key::HOME => Key::Home,
        key::END => Key::End,
        _ => return None,
    })
}

/// The character a key typed, as a menu key for type-ahead: printable, and typed without a command modifier
/// (`command`: Ctrl, Alt or the primary modifier held).
pub fn typed_key(text: Option<char>, command: bool) -> Option<Key> {
    text.filter(|c| !command && !c.is_control() && !c.is_whitespace())
        .map(Key::Char)
}

/// What the process does for a controller effect.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Draw the bar and its popups again.
    Redraw,
    /// Tell the server whether a menu of this bar is open.
    SetMenuOpen(bool),
    /// Ask the session bus worker.
    Request(Request),
    WindowCommand(WindowCommand),
    /// Move the active window to a workspace (1-based).
    MoveToWorkspace(u32),
    /// Show a window of the window list by its server id.
    ActivateWindow(u32),
    /// Have the server type the chord of an Edit item into the key window.
    Edit(EditAction),
    /// Have the server force quit the active application when this id still names it.
    ForceQuitApp(String),
    ShowAbout,
    /// Start a program or an application, handing it an activation token.
    Launch(Launch),
    /// Open a file with the default application of its type.
    OpenDocument(PathBuf),
    /// The system menu is about to open: read Recent Items again and, with a session agent, ask it to read what the
    /// machine allows again.
    SystemMenuOpening {
        refresh_capabilities: bool,
    },
    /// Clear Menu of Recent Items.
    ClearRecentItems,
    /// A row of one of the bar's own items.
    Extra(ExtraAction),
}

/// What an effect is resolved against: one bar and what the process shares between bars.
pub struct Context<'a> {
    pub feed: &'a BarFeed,
    pub shared: &'a Shared,
    pub layout: &'a BarLayout,
    /// Position of the bar's output in the compositor space, in points.
    pub origin: (i32, i32),
}

/// The action for `effect`, `None` when nothing is to be done.
///
/// The bar's own items (the App menu, the synthetic menus and what the bar adds to an exporter's) act through the
/// server; the exporter's items, and the bar's items rebound to them, go to the exporter, as do the items of status
/// menus. A status item's menu is read once when the
/// item is announced, so it is not refreshed before it opens.
pub fn resolve(effect: Effect, cx: &Context) -> Option<Action> {
    match effect {
        Effect::Redraw => Some(Action::Redraw),
        Effect::Opened { .. } => Some(Action::SetMenuOpen(true)),
        Effect::Closed => Some(Action::SetMenuOpen(false)),
        Effect::AboutToShow {
            slot: slot @ SlotKey::Menu(_),
            item,
        } if shown_origin(&slot, item, cx) == Some(Origin::Exporter) => {
            match cx.feed.source(cx.shared) {
                MenuSource::Kde(menu) => {
                    Some(Action::Request(Request::AboutToShow { menu, item: item.0 }))
                }
                _ => None,
            }
        }
        // the system menu asks the session agent to read the machine's policy again as it opens: what it allows can have
        // changed since the bar started (the answer arrives as a change of `Capabilities`); Recent Items is read then too
        Effect::AboutToShow {
            slot: SlotKey::System,
            item: SYSTEM_MENU_ROOT,
        } => Some(Action::SystemMenuOpening {
            refresh_capabilities: cx.shared.extras.session.is_some(),
        }),
        Effect::AboutToShow { .. } => None,
        Effect::Activate {
            slot: SlotKey::Status(id),
            item,
        } if ExtraKind::from_slot_id(&id).is_some() => {
            let kind = ExtraKind::from_slot_id(&id)?;
            cx.shared
                .extras
                .menu(kind)
                .action(item)
                .cloned()
                .map(Action::Extra)
        }
        Effect::Activate {
            slot: SlotKey::Status(id),
            item,
        } => {
            let status = cx.shared.status.iter().find(|s| s.item.id == id)?;
            let path = status.item.menu_path.clone()?;
            Some(Action::Request(Request::ActivateDbusMenu {
                menu: KdeAppMenu {
                    service: status.item.service.clone(),
                    path,
                },
                item: item.0,
            }))
        }
        Effect::Activate {
            slot: slot @ (SlotKey::Menu(_) | SlotKey::AppMenu),
            item,
        } => menu_action(&slot, item, cx),
        Effect::Activate {
            slot: SlotKey::System,
            item,
        } => Some(
            match cx
                .shared
                .extras
                .system_menu(&cx.shared.chords, &cx.shared.recent, cx.feed.front_app())
                .action(item)?
            {
                ExtraAction::System(SystemAction::About) => Action::Launch(Launch::Program(vec![
                    SETTINGS_PROGRAM.to_owned(),
                    "--about".to_owned(),
                ])),
                ExtraAction::System(SystemAction::Settings) => {
                    Action::Launch(Launch::Program(vec![SETTINGS_PROGRAM.to_owned()]))
                }
                ExtraAction::System(SystemAction::ForceQuit) => {
                    Action::WindowCommand(WindowCommand::ForceQuitDialog)
                }
                ExtraAction::System(SystemAction::ForceQuitApp(app_id)) => {
                    Action::ForceQuitApp(app_id.clone())
                }
                ExtraAction::System(SystemAction::OpenRecentApp(row)) => {
                    Action::Launch(Launch::App(cx.shared.recent.apps.get(*row)?.entry.clone()))
                }
                ExtraAction::System(SystemAction::OpenRecentDocument(row)) => {
                    Action::OpenDocument(cx.shared.recent.documents.get(*row)?.path.clone())
                }
                ExtraAction::System(SystemAction::ClearRecentItems) => Action::ClearRecentItems,
                action => Action::Extra(action.clone()),
            },
        ),
        Effect::Activate {
            slot: SlotKey::ControlPanel | SlotKey::Clock,
            ..
        } => None,
        Effect::SlotActivated { slot } => match slot {
            SlotKey::Status(id) => {
                let status = cx.shared.status.iter().find(|s| s.item.id == id)?;
                let rect = cx.layout.slot(&SlotKey::Status(id))?.rect;
                // the item anchors what it shows at the middle of its slot
                let (x, y) = (
                    (rect.x + rect.w / 2.0).round() as i32 + cx.origin.0,
                    (rect.y + rect.h / 2.0).round() as i32 + cx.origin.1,
                );
                Some(Action::Request(Request::ActivateStatus {
                    service: status.item.service.clone(),
                    path: status.item.path.clone(),
                    x,
                    y,
                }))
            }
            SlotKey::ControlPanel => Some(Action::Request(Request::TogglePanel)),
            SlotKey::Clock => Some(Action::Request(Request::ToggleCentre)),
            SlotKey::System | SlotKey::AppMenu | SlotKey::Menu(_) => None,
        },
    }
}

/// The origin of `item` as the bar shows it in the menu of `slot`; `None` when it does not show there.
fn shown_origin(slot: &SlotKey, item: ItemId, cx: &Context) -> Option<Origin> {
    let content = cx.feed.content(cx.shared);
    let menus = match slot {
        SlotKey::AppMenu => content.app_menu,
        SlotKey::Menu(id) => content.menus.into_iter().filter(|m| m.id == *id).collect(),
        _ => return None,
    };
    menus.iter().find_map(|m| m.find(item)).map(|i| i.origin)
}

/// The action of an item of the App menu or of the application's menus, by the item's origin: the bar's own items act
/// through the server, unless About, Settings… or Quit is rebound to the exporter's item of its role; the exporter's
/// items go to the exporter.
fn menu_action(slot: &SlotKey, item: ItemId, cx: &Context) -> Option<Action> {
    let target = match shown_origin(slot, item, cx)? {
        Origin::Exporter => item,
        Origin::Bar => {
            match fallback::action_for(item).and_then(|a| cx.feed.rebound(cx.shared, a)) {
                Some(target) => target,
                None => return fallback_action(item, cx.feed),
            }
        }
    };
    match cx.feed.source(cx.shared) {
        MenuSource::Kde(menu) => Some(Action::Request(Request::ActivateDbusMenu {
            menu,
            item: target.0,
        })),
        MenuSource::Gtk(props) => Some(Action::Request(Request::ActivateGtk {
            props,
            item: target.0,
        })),
        MenuSource::Synthetic => None,
    }
}

/// The action of an item of the synthetic menus.
fn fallback_action(item: ItemId, feed: &BarFeed) -> Option<Action> {
    Some(match fallback::action_for(item)? {
        FallbackAction::About => Action::ShowAbout,
        FallbackAction::Hide => Action::WindowCommand(WindowCommand::HideApp),
        FallbackAction::HideOthers => Action::WindowCommand(WindowCommand::HideOthers),
        FallbackAction::ShowAll => Action::WindowCommand(WindowCommand::ShowAll),
        FallbackAction::Quit => Action::WindowCommand(WindowCommand::QuitApp),
        FallbackAction::Minimize => Action::WindowCommand(WindowCommand::Minimize),
        FallbackAction::Zoom => Action::WindowCommand(WindowCommand::Zoom),
        FallbackAction::CloseWindow => Action::WindowCommand(WindowCommand::Close),
        FallbackAction::ToggleFullscreen => Action::WindowCommand(WindowCommand::ToggleFullscreen),
        FallbackAction::TileLeft => Action::WindowCommand(WindowCommand::TileLeft),
        FallbackAction::TileRight => Action::WindowCommand(WindowCommand::TileRight),
        FallbackAction::TileFill => Action::WindowCommand(WindowCommand::TileFill),
        FallbackAction::BringAllToFront => Action::WindowCommand(WindowCommand::BringAllToFront),
        FallbackAction::MinimizeAll => Action::WindowCommand(WindowCommand::MinimizeAll),
        FallbackAction::ZoomAll => Action::WindowCommand(WindowCommand::ZoomAll),
        FallbackAction::CloseAll => Action::WindowCommand(WindowCommand::QuitApp),
        FallbackAction::CascadeAll => Action::WindowCommand(WindowCommand::CascadeAll),
        FallbackAction::ShowWindow(row) => Action::ActivateWindow(feed.listed_window(row)?.id),
        FallbackAction::MoveToWorkspace(n) => Action::MoveToWorkspace(n),
        FallbackAction::Edit(action) => Action::Edit(match action {
            fallback::EditAction::Undo => EditAction::Undo,
            fallback::EditAction::Redo => EditAction::Redo,
            fallback::EditAction::Cut => EditAction::Cut,
            fallback::EditAction::Copy => EditAction::Copy,
            fallback::EditAction::Paste => EditAction::Paste,
            fallback::EditAction::SelectAll => EditAction::SelectAll,
        }),
        // disabled until the app's preferences action exists
        FallbackAction::Settings => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printable_characters_typed_without_a_command_modifier_are_type_ahead() {
        assert_eq!(typed_key(Some('r'), false), Some(Key::Char('r')));
        assert_eq!(typed_key(Some('É'), false), Some(Key::Char('É')));
        assert_eq!(typed_key(Some('r'), true), None, "a shortcut");
        assert_eq!(typed_key(Some('\r'), false), None);
        assert_eq!(typed_key(Some(' '), false), None, "Space chooses");
        assert_eq!(typed_key(None, false), None);
    }
    use crate::controller::MenuController;
    use crate::dbus::sni::{ItemStatus, StatusItem};
    use crate::desktop::AppName;
    use crate::extras::{BluetoothState, SystemRow};
    use crate::feed::StatusState;
    use crate::geom::Point;
    use crate::geom::Rect;
    use crate::item::MenuItem;
    use crate::layout::{layout_bar, BarMetrics, MonoMeasure};
    use crate::source::GtkMenuProps;
    use cupertino_shell_client::{
        ActiveApp, AppWindow, GtkMenu, MenuBarUpdate, WindowMenu, WindowState,
    };
    use cupertino_ui_text::{KeyInput, Keymap};
    use std::time::Instant;

    struct Fixture {
        feed: BarFeed,
        shared: Shared,
        layout: BarLayout,
    }

    impl Fixture {
        /// A bar of an application whose window announces `menu`, with one status item that has a menu.
        fn new(menu: WindowMenu) -> Self {
            let mut feed = BarFeed::new(1, true, true);
            let mut shared = Shared::default();
            let update = MenuBarUpdate {
                active_app: Some(ActiveApp {
                    app_id: "org.example.editor".into(),
                    title: String::new(),
                }),
                menu: Some(menu),
                window_state: None,
                engage: false,
                engage_help: false,
                screen_capture: None,
                session_agent: None,
                backdrop_lightness: None,
                backdrop_spread: None,
                windows: None,
                shortcuts: None,
            };
            feed.apply(&update, &mut shared, |_| AppName {
                name: "Editor".into(),
                has_entry: true,
            });
            let item = StatusItem {
                id: ":1.9/StatusNotifierItem".into(),
                service: ":1.9".into(),
                path: "/StatusNotifierItem".into(),
                menu_path: Some("/MenuBar".into()),
                status: ItemStatus::Active,
                ..StatusItem::default()
            };
            shared.set_status(StatusState {
                item,
                icon: None,
                menu: None,
            });
            shared.clock = "10:42".into();
            let layout = layout_bar(
                Rect::new(0.0, 0.0, 1000.0, 800.0),
                &feed.content(&shared),
                &BarMetrics::default(),
                &MonoMeasure::default(),
            );
            Fixture {
                feed,
                shared,
                layout,
            }
        }

        fn resolve(&self, effect: Effect) -> Option<Action> {
            resolve(
                effect,
                &Context {
                    feed: &self.feed,
                    shared: &self.shared,
                    layout: &self.layout,
                    origin: (1920, 0),
                },
            )
        }

        /// The exporter's tree arrived, as a reader gives it.
        fn load(&mut self, tree: MenuItem) {
            self.feed.request_layout(&mut self.shared);
            let source = self.feed.source(&self.shared);
            self.shared.apply_layout(&source, tree.exported());
        }

        /// Activate `item` in the menu that shows it.
        fn click(&self, item: ItemId) -> Option<Action> {
            let content = self.feed.content(&self.shared);
            let slot = content
                .menus
                .iter()
                .find(|m| m.find(item).is_some())
                .map_or(SlotKey::AppMenu, |m| SlotKey::Menu(m.id));
            self.resolve(activate(slot, item.0))
        }
    }

    fn kde_menu() -> WindowMenu {
        WindowMenu::DbusMenu {
            service: ":1.5".into(),
            path: "/MenuBar".into(),
        }
    }

    fn kde() -> KdeAppMenu {
        KdeAppMenu {
            service: ":1.5".into(),
            path: "/MenuBar".into(),
        }
    }

    fn activate(slot: SlotKey, item: i32) -> Effect {
        Effect::Activate {
            slot,
            item: ItemId(item),
        }
    }

    #[test]
    fn keys_map_from_evdev_codes() {
        assert_eq!(menu_key(key::DOWN), Some(Key::Down));
        assert_eq!(menu_key(key::KEYPAD_ENTER), Some(Key::Enter));
        assert_eq!(menu_key(key::ESCAPE), Some(Key::Escape));
        assert_eq!(menu_key(key::HOME), Some(Key::Home));
        // KEY_A
        assert_eq!(menu_key(30), None);
    }

    #[test]
    fn opening_and_closing_tell_the_server() {
        let f = Fixture::new(WindowMenu::None);
        assert_eq!(
            f.resolve(Effect::Opened {
                slot: SlotKey::AppMenu
            }),
            Some(Action::SetMenuOpen(true))
        );
        assert_eq!(f.resolve(Effect::Closed), Some(Action::SetMenuOpen(false)));
        assert_eq!(f.resolve(Effect::Redraw), Some(Action::Redraw));
    }

    #[test]
    fn exporter_menus_are_refreshed_before_they_show_but_the_app_menu_is_not() {
        let mut f = Fixture::new(kde_menu());
        f.load(MenuItem::submenu(
            0,
            "",
            vec![MenuItem::submenu(1, "File", vec![])],
        ));
        let file = SlotKey::Menu(ItemId(1));
        assert_eq!(
            f.resolve(Effect::AboutToShow {
                slot: file,
                item: ItemId(1)
            }),
            Some(Action::Request(Request::AboutToShow {
                menu: kde(),
                item: 1
            }))
        );
        assert_eq!(
            f.resolve(Effect::AboutToShow {
                slot: SlotKey::AppMenu,
                item: ItemId(i32::MIN)
            }),
            None
        );
        assert_eq!(
            f.resolve(Effect::AboutToShow {
                slot: SlotKey::Status(":1.9/StatusNotifierItem".into()),
                item: ItemId(0)
            }),
            None
        );
    }

    #[test]
    fn items_of_an_exporter_go_to_the_exporter() {
        let file = || {
            MenuItem::submenu(
                0,
                "",
                vec![MenuItem::submenu(
                    1,
                    "File",
                    vec![MenuItem::new(2, "New"), MenuItem::new(5, "Open")],
                )],
            )
        };
        let mut f = Fixture::new(kde_menu());
        f.load(file());
        assert_eq!(
            f.resolve(activate(SlotKey::Menu(ItemId(1)), 2)),
            Some(Action::Request(Request::ActivateDbusMenu {
                menu: kde(),
                item: 2
            }))
        );
        let props = GtkMenuProps {
            bus_name: ":1.7".into(),
            menubar_object_path: Some("/menubar".into()),
            ..GtkMenuProps::default()
        };
        let gtk = WindowMenu::Gtk(GtkMenu {
            bus_name: ":1.7".into(),
            application_id: None,
            application_object_path: None,
            window_object_path: None,
            menubar_object_path: Some("/menubar".into()),
            app_menu_object_path: None,
        });
        let mut f = Fixture::new(gtk);
        f.load(file());
        assert_eq!(
            f.resolve(activate(SlotKey::Menu(ItemId(1)), 5)),
            Some(Action::Request(Request::ActivateGtk { props, item: 5 }))
        );
    }

    #[test]
    fn merged_menus_tell_the_bars_items_from_the_exporters() {
        let mut f = Fixture::new(kde_menu());
        let tree = MenuItem::submenu(
            0,
            "",
            vec![
                MenuItem::submenu(
                    1,
                    "Editor",
                    vec![
                        MenuItem::new(2, "About Editor"),
                        MenuItem::new(3, "Check for Updates"),
                    ],
                ),
                MenuItem::submenu(
                    10,
                    "File",
                    vec![MenuItem::new(11, "New"), MenuItem::new(12, "Quit")],
                ),
                MenuItem::submenu(20, "Window", vec![MenuItem::new(21, "Split View")]),
            ],
        );
        f.load(tree);
        let to_exporter = |item| {
            Some(Action::Request(Request::ActivateDbusMenu {
                menu: kde(),
                item,
            }))
        };
        let app =
            |action: FallbackAction| f.resolve(activate(SlotKey::AppMenu, action.item_id().0));
        assert_eq!(app(FallbackAction::About), to_exporter(2), "rebound");
        assert_eq!(app(FallbackAction::Quit), to_exporter(12), "rebound");
        assert_eq!(app(FallbackAction::Settings), None, "nothing to bind");
        assert_eq!(
            app(FallbackAction::Hide),
            Some(Action::WindowCommand(WindowCommand::HideApp))
        );
        assert_eq!(
            f.resolve(activate(SlotKey::AppMenu, 3)),
            to_exporter(3),
            "the application's own item moved into the App menu"
        );
        let window = SlotKey::Menu(fallback::WINDOW_MENU);
        assert_eq!(
            f.resolve(activate(
                window.clone(),
                FallbackAction::Minimize.item_id().0
            )),
            Some(Action::WindowCommand(WindowCommand::Minimize))
        );
        assert_eq!(f.resolve(activate(window.clone(), 21)), to_exporter(21));
        assert_eq!(
            f.resolve(Effect::AboutToShow {
                slot: window,
                item: fallback::WINDOW_MENU
            }),
            None,
            "the bar's own menus load nothing from the exporter"
        );
    }

    #[test]
    fn an_exporters_item_with_an_id_of_the_bars_goes_to_the_exporter() {
        let hide_others = FallbackAction::HideOthers.item_id();
        let first_row = FallbackAction::ShowWindow(0).item_id();
        let mut f = Fixture::new(kde_menu());
        f.load(MenuItem::submenu(
            0,
            "",
            vec![MenuItem::submenu(
                10,
                "File",
                vec![
                    MenuItem::new(hide_others.0, "Hide Others"),
                    MenuItem::new(first_row.0, "Row"),
                ],
            )],
        ));
        let file = SlotKey::Menu(ItemId(10));
        for id in [hide_others, first_row] {
            assert_eq!(
                f.resolve(activate(file.clone(), id.0)),
                Some(Action::Request(Request::ActivateDbusMenu {
                    menu: kde(),
                    item: id.0
                })),
                "{id:?}"
            );
        }
        assert_eq!(
            f.resolve(activate(SlotKey::AppMenu, hide_others.0)),
            Some(Action::WindowCommand(WindowCommand::HideOthers)),
            "the bar's own item of that id still acts through the server"
        );
    }

    #[test]
    fn the_app_menu_and_the_window_menu_act_through_the_server() {
        // the app menu is synthetic even when the window exports its own menus
        let f = Fixture::new(kde_menu());
        assert_eq!(
            f.resolve(activate(SlotKey::AppMenu, FallbackAction::Hide.item_id().0)),
            Some(Action::WindowCommand(WindowCommand::HideApp))
        );
        assert_eq!(
            f.resolve(activate(SlotKey::AppMenu, FallbackAction::Quit.item_id().0)),
            Some(Action::WindowCommand(WindowCommand::QuitApp))
        );
        assert_eq!(
            f.resolve(activate(
                SlotKey::AppMenu,
                FallbackAction::About.item_id().0
            )),
            Some(Action::ShowAbout)
        );
        let mut f = Fixture::new(WindowMenu::None);
        let state = MenuBarUpdate {
            window_state: Some(WindowState {
                can_minimize: true,
                can_zoom: true,
                full_screen: false,
                workspace: 1,
                workspaces: 3,
            }),
            ..MenuBarUpdate::default()
        };
        f.feed.apply(&state, &mut f.shared, |_| {
            unreachable!("the application did not change")
        });
        let window = SlotKey::Menu(fallback::WINDOW_MENU);
        assert_eq!(
            f.resolve(activate(
                window.clone(),
                FallbackAction::Minimize.item_id().0
            )),
            Some(Action::WindowCommand(WindowCommand::Minimize))
        );
        assert_eq!(
            f.resolve(activate(window.clone(), FallbackAction::Zoom.item_id().0)),
            Some(Action::WindowCommand(WindowCommand::Zoom))
        );
        assert_eq!(
            f.resolve(activate(
                window.clone(),
                FallbackAction::CloseWindow.item_id().0
            )),
            Some(Action::WindowCommand(WindowCommand::Close))
        );
        assert_eq!(
            f.resolve(activate(
                window,
                FallbackAction::MoveToWorkspace(3).item_id().0
            )),
            Some(Action::MoveToWorkspace(3))
        );
    }

    #[test]
    fn the_new_items_of_the_synthetic_menus_act_through_the_server() {
        let mut f = Fixture::new(WindowMenu::None);
        let update = MenuBarUpdate {
            windows: Some(vec![AppWindow {
                id: 42,
                title: "notes.txt".into(),
                active: true,
                minimized: false,
            }]),
            ..MenuBarUpdate::default()
        };
        f.feed.apply(&update, &mut f.shared, |_| {
            unreachable!("the application did not change")
        });
        let act = |action: FallbackAction| f.click(action.item_id());
        for (action, command) in [
            (
                FallbackAction::ToggleFullscreen,
                WindowCommand::ToggleFullscreen,
            ),
            (FallbackAction::TileLeft, WindowCommand::TileLeft),
            (FallbackAction::TileRight, WindowCommand::TileRight),
            (FallbackAction::TileFill, WindowCommand::TileFill),
            (
                FallbackAction::BringAllToFront,
                WindowCommand::BringAllToFront,
            ),
        ] {
            assert_eq!(
                act(action),
                Some(Action::WindowCommand(command)),
                "{action:?}"
            );
        }
        assert_eq!(
            act(FallbackAction::ShowWindow(0)),
            Some(Action::ActivateWindow(42))
        );
        assert_eq!(
            act(FallbackAction::ShowWindow(1)),
            None,
            "no window at the row"
        );
        assert_eq!(act(FallbackAction::Settings), None);
        assert_eq!(
            act(FallbackAction::Edit(fallback::EditAction::Copy)),
            Some(Action::Edit(EditAction::Copy))
        );
        assert_eq!(
            act(FallbackAction::Edit(fallback::EditAction::SelectAll)),
            Some(Action::Edit(EditAction::SelectAll))
        );
    }

    #[test]
    fn the_alternates_of_the_window_menu_act_through_the_server_beside_an_exporters_items() {
        let mut f = Fixture::new(kde_menu());
        f.load(MenuItem::submenu(
            0,
            "",
            vec![MenuItem::submenu(
                20,
                "Window",
                vec![MenuItem::new(21, "Split View")],
            )],
        ));
        for (action, command) in [
            (FallbackAction::MinimizeAll, WindowCommand::MinimizeAll),
            (FallbackAction::ZoomAll, WindowCommand::ZoomAll),
            (FallbackAction::CloseAll, WindowCommand::QuitApp),
            (FallbackAction::CascadeAll, WindowCommand::CascadeAll),
        ] {
            assert_eq!(
                f.click(action.item_id()),
                Some(Action::WindowCommand(command)),
                "{action:?}"
            );
        }
        let content = f.feed.content(&f.shared);
        let window = content
            .menus
            .iter()
            .find(|m| m.id == fallback::WINDOW_MENU)
            .unwrap();
        let split = window.find(ItemId(21)).unwrap();
        assert_eq!(split.origin, Origin::Exporter);
        assert_eq!(split.alternate, None, "an exporter's item has no alternate");
    }

    #[test]
    fn a_status_menu_item_goes_to_the_items_menu() {
        let f = Fixture::new(WindowMenu::None);
        let slot = SlotKey::Status(":1.9/StatusNotifierItem".into());
        assert_eq!(
            f.resolve(activate(slot, 4)),
            Some(Action::Request(Request::ActivateDbusMenu {
                menu: KdeAppMenu {
                    service: ":1.9".into(),
                    path: "/MenuBar".into()
                },
                item: 4
            }))
        );
    }

    #[test]
    fn clicking_slots_without_a_menu() {
        let f = Fixture::new(WindowMenu::None);
        assert_eq!(
            f.resolve(Effect::SlotActivated {
                slot: SlotKey::Clock
            }),
            Some(Action::Request(Request::ToggleCentre))
        );
        assert_eq!(
            f.resolve(Effect::SlotActivated {
                slot: SlotKey::ControlPanel
            }),
            Some(Action::Request(Request::TogglePanel))
        );
        // 1000 wide bar: clock 51 and control panel 28 from the right inset of 6, then the 26 wide
        // status slot at 889 with its middle at 902, on the output at x = 1920
        assert_eq!(
            f.resolve(Effect::SlotActivated {
                slot: SlotKey::Status(":1.9/StatusNotifierItem".into())
            }),
            Some(Action::Request(Request::ActivateStatus {
                service: ":1.9".into(),
                path: "/StatusNotifierItem".into(),
                x: 2822,
                y: 12
            }))
        );
    }

    #[test]
    fn rows_of_the_bars_own_items_become_their_actions() {
        let mut f = Fixture::new(WindowMenu::None);
        f.shared.extras.bluetooth = Some(BluetoothState {
            powered: true,
            devices: Vec::new(),
        });
        let slot = SlotKey::Status(ExtraKind::Bluetooth.slot_id().into());
        let toggle = f.shared.extras.menu(ExtraKind::Bluetooth).rows[0].id;
        assert_eq!(
            f.resolve(Effect::Activate {
                slot: slot.clone(),
                item: toggle
            }),
            Some(Action::Extra(ExtraAction::SetBluetoothPowered(false)))
        );
        assert_eq!(
            f.resolve(Effect::Activate {
                slot,
                item: ItemId(99)
            }),
            None
        );
    }

    #[test]
    fn a_system_menu_entry_goes_to_the_session_agent() {
        let mut f = Fixture::new(WindowMenu::None);
        f.shared.extras.session = Some(crate::extras::SessionState::default());
        let menu = f
            .shared
            .extras
            .system_menu(&f.shared.chords, &f.shared.recent, None);
        let restart = menu
            .rows
            .iter()
            .find(|r| r.label.starts_with("Restart"))
            .unwrap();
        assert_eq!(
            f.resolve(activate(SlotKey::System, restart.id.0)),
            Some(Action::Extra(ExtraAction::Session(
                crate::extras::SessionAction::Restart
            )))
        );
        f.shared.extras.session = None;
        assert_eq!(
            f.resolve(activate(SlotKey::System, restart.id.0)),
            None,
            "no agent, nothing to call"
        );
    }

    #[test]
    fn settings_starts_the_control_panel_and_force_quit_opens_the_servers_dialog_without_an_agent()
    {
        let f = Fixture::new(WindowMenu::None);
        assert_eq!(
            f.resolve(activate(SlotKey::System, SystemRow::About.id().0)),
            Some(Action::Launch(Launch::Program(vec![
                "cupertino-settings".to_owned(),
                "--about".to_owned()
            ])))
        );
        assert_eq!(
            f.resolve(activate(SlotKey::System, SystemRow::Settings.id().0)),
            Some(Action::Launch(Launch::Program(vec![
                "cupertino-settings".to_owned()
            ])))
        );
        assert_eq!(
            f.resolve(activate(SlotKey::System, SystemRow::ForceQuit.id().0)),
            Some(Action::WindowCommand(WindowCommand::ForceQuitDialog))
        );
        assert_eq!(
            f.resolve(activate(SlotKey::System, SystemRow::Lock.id().0)),
            Some(Action::Extra(ExtraAction::Session(
                crate::extras::SessionAction::Lock
            )))
        );
    }

    #[test]
    fn the_force_quit_alternate_names_the_active_app_to_the_server_from_version_13_only() {
        let f = Fixture::new(WindowMenu::None);
        let force_quit_app = activate(SlotKey::System, SystemRow::ForceQuitApp.id().0);
        assert_eq!(
            f.resolve(force_quit_app.clone()),
            Some(Action::ForceQuitApp("org.example.editor".into()))
        );
        let resolve_with = |feed: &BarFeed| {
            resolve(
                force_quit_app.clone(),
                &Context {
                    feed,
                    shared: &f.shared,
                    layout: &f.layout,
                    origin: (0, 0),
                },
            )
        };
        let desktop = BarFeed::new(2, true, true);
        assert_eq!(resolve_with(&desktop), None, "no alternate on the desktop");
        let mut below_13 = BarFeed::new(3, true, false);
        let mut shared = Shared::default();
        below_13.apply(
            &MenuBarUpdate {
                active_app: Some(ActiveApp {
                    app_id: "org.example.editor".into(),
                    title: String::new(),
                }),
                menu: None,
                window_state: None,
                engage: false,
                engage_help: false,
                screen_capture: None,
                session_agent: None,
                backdrop_lightness: None,
                backdrop_spread: None,
                windows: None,
                shortcuts: None,
            },
            &mut shared,
            |_| AppName {
                name: "Editor".into(),
                has_entry: true,
            },
        );
        assert_eq!(
            resolve_with(&below_13),
            None,
            "a disabled alternate sends nothing"
        );
    }

    #[test]
    fn opening_the_system_menu_asks_the_session_agent_to_read_its_capabilities_again() {
        let mut f = Fixture::new(WindowMenu::None);
        let about = |slot| Effect::AboutToShow {
            slot,
            item: crate::controller::SYSTEM_MENU_ROOT,
        };
        assert_eq!(
            f.resolve(about(SlotKey::System)),
            Some(Action::SystemMenuOpening {
                refresh_capabilities: false
            }),
            "no agent, no call"
        );
        f.shared.extras.session = Some(crate::extras::SessionState::default());
        assert_eq!(
            f.resolve(about(SlotKey::System)),
            Some(Action::SystemMenuOpening {
                refresh_capabilities: true
            })
        );
        assert_eq!(
            f.resolve(Effect::AboutToShow {
                slot: SlotKey::System,
                item: SystemRow::RecentItems.id(),
            }),
            None,
            "a submenu opening reads nothing"
        );
        assert_eq!(
            f.resolve(about(SlotKey::AppMenu)),
            None,
            "other menus do not ask"
        );
    }

    #[test]
    fn a_recent_item_is_resolved_by_its_position_when_clicked() {
        use crate::extras::{CLEAR_RECENT_ITEMS, RECENT_APP_ROWS, RECENT_DOCUMENT_ROWS};
        use crate::recent::{RecentApp, RecentDocument, RecentItems};
        let mut f = Fixture::new(WindowMenu::None);
        let entry = cupertino_apps::DesktopEntry {
            id: "editor.desktop".into(),
            name: "Editor".into(),
            generic_name: String::new(),
            comment: String::new(),
            keywords: vec![],
            categories: vec![],
            icon: None,
            exec: "editor".into(),
            try_exec: None,
            terminal: false,
            path: None,
            startup_wm_class: None,
            actions: vec![],
            source: None,
        };
        f.shared.recent = RecentItems {
            apps: vec![RecentApp {
                entry: entry.clone(),
                last_used: 0.0,
            }],
            documents: vec![RecentDocument {
                path: "/home/ada/notes.md".into(),
                used: None,
            }],
        };
        assert_eq!(
            f.resolve(activate(SlotKey::System, RECENT_APP_ROWS)),
            Some(Action::Launch(Launch::App(entry)))
        );
        assert_eq!(
            f.resolve(activate(SlotKey::System, RECENT_DOCUMENT_ROWS)),
            Some(Action::OpenDocument("/home/ada/notes.md".into()))
        );
        assert_eq!(
            f.resolve(activate(SlotKey::System, CLEAR_RECENT_ITEMS)),
            Some(Action::ClearRecentItems)
        );
        assert_eq!(
            f.resolve(activate(SlotKey::System, RECENT_APP_ROWS - 1)),
            None,
            "no second application"
        );
        f.shared.recent = RecentItems::default();
        assert_eq!(
            f.resolve(activate(SlotKey::System, RECENT_APP_ROWS)),
            None,
            "the menu was cleared"
        );
    }

    /// Search for `text` in the Help menu of the bar `f` shows and choose the first result with Return.
    fn choose_first_result(f: &Fixture, text: &str) -> Option<Action> {
        let output = Rect::new(0.0, 0.0, 1000.0, 800.0);
        let mut c = MenuController::new(output, Box::new(MonoMeasure::default()));
        c.set_content(f.feed.content(&f.shared));
        let help = c.layout().slot(&SlotKey::Menu(fallback::HELP_MENU))?.rect;
        let p = Point::new(help.x + help.w / 2.0, help.y + help.h / 2.0);
        c.pointer_press(p, 0.0);
        c.pointer_release(p, 0.05);
        let mut keymap = Keymap::standard();
        for ch in text.chars() {
            let input = KeyInput::new(0, false, false, false, false, Some(ch));
            c.search_key(None, &mut keymap, input, Instant::now())?;
        }
        let effect = c
            .key(Key::Enter, 1.0)
            .effects
            .into_iter()
            .find(|e| matches!(e, Effect::Activate { .. }))?;
        f.resolve(effect)
    }

    #[test]
    fn a_help_search_result_goes_where_the_item_it_found_goes_by_its_origin() {
        let mut f = Fixture::new(kde_menu());
        f.load(MenuItem::submenu(
            0,
            "",
            vec![MenuItem::submenu(
                1,
                "File",
                vec![MenuItem::new(2, "New"), MenuItem::new(5, "Open Document")],
            )],
        ));
        assert_eq!(
            choose_first_result(&f, "open doc"),
            Some(Action::Request(Request::ActivateDbusMenu {
                menu: kde(),
                item: 5
            })),
            "the exporter's item goes to the exporter"
        );
        assert_eq!(
            choose_first_result(&f, "minimize"),
            Some(Action::WindowCommand(WindowCommand::Minimize)),
            "the bar's item acts through the server"
        );
    }
}
