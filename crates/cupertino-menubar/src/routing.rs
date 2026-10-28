//! From input and controller effects to what the process does: key mapping and the action a
//! controller effect stands for, given what the bar shows.

use crate::controller::{Effect, Key, SYSTEM_MENU_ROOT};
use crate::extras::{ExtraAction, ExtraKind, SystemAction};
use crate::fallback::{self, FallbackAction};
use crate::feed::{BarFeed, Shared};
use crate::item::ItemId;
use crate::layout::{BarLayout, SlotKey};
use crate::source::{KdeAppMenu, MenuSource};
use crate::worker::Request;
use cupertino_apps::Launch;
use cupertino_shell_client::WindowCommand;
use cupertino_wayland_client::key;
use std::path::PathBuf;

/// The control panel, which Settings… of the system menu starts.
const SETTINGS_PROGRAM: &str = "cupertino-settings";

/// The menu key for an evdev key code; `None` for keys menus ignore. Letters (type-ahead) need
/// a keymap, which the process does not have.
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
/// The application menu and the synthetic Window menu act through the server; the menus of an
/// exporter and of status items go to their exporter. A status item's menu is read once when the
/// item is announced, so it is not refreshed before it opens.
pub fn resolve(effect: Effect, cx: &Context) -> Option<Action> {
    match effect {
        Effect::Redraw => Some(Action::Redraw),
        Effect::Opened { .. } => Some(Action::SetMenuOpen(true)),
        Effect::Closed => Some(Action::SetMenuOpen(false)),
        Effect::AboutToShow {
            slot: SlotKey::Menu(_),
            item,
        } => match cx.feed.source(cx.shared) {
            MenuSource::Kde(menu) => {
                Some(Action::Request(Request::AboutToShow { menu, item: item.0 }))
            }
            _ => None,
        },
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
            slot: SlotKey::Menu(_),
            item,
        } => match cx.feed.source(cx.shared) {
            MenuSource::Kde(menu) => Some(Action::Request(Request::ActivateDbusMenu {
                menu,
                item: item.0,
            })),
            MenuSource::Gtk(props) => Some(Action::Request(Request::ActivateGtk {
                props,
                item: item.0,
            })),
            MenuSource::Synthetic => fallback_action(item, cx.feed),
        },
        Effect::Activate {
            slot: SlotKey::AppMenu,
            item,
        } => fallback_action(item, cx.feed),
        Effect::Activate {
            slot: SlotKey::System,
            item,
        } => Some(
            match cx
                .shared
                .extras
                .system_menu(&cx.shared.chords, &cx.shared.recent)
                .action(item)?
            {
                ExtraAction::System(SystemAction::Settings) => {
                    Action::Launch(Launch::Program(vec![SETTINGS_PROGRAM.to_owned()]))
                }
                ExtraAction::System(SystemAction::ForceQuit) => {
                    Action::WindowCommand(WindowCommand::ForceQuitDialog)
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
        FallbackAction::ShowWindow(row) => Action::ActivateWindow(feed.listed_window(row)?.id),
        FallbackAction::MoveToWorkspace(n) => Action::MoveToWorkspace(n),
        // disabled until the app's preferences action and the server's edit request exist
        FallbackAction::Settings | FallbackAction::Edit(_) => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dbus::sni::{ItemStatus, StatusItem};
    use crate::desktop::AppName;
    use crate::extras::{BluetoothState, SystemRow};
    use crate::feed::StatusState;
    use crate::geom::Rect;
    use crate::layout::{layout_bar, BarMetrics, MonoMeasure};
    use crate::source::GtkMenuProps;
    use cupertino_shell_client::{ActiveApp, AppWindow, GtkMenu, MenuBarUpdate, WindowMenu};

    struct Fixture {
        feed: BarFeed,
        shared: Shared,
        layout: BarLayout,
    }

    impl Fixture {
        /// A bar of an application whose window announces `menu`, with one status item that has a menu.
        fn new(menu: WindowMenu) -> Self {
            let mut feed = BarFeed::new(1);
            let mut shared = Shared::default();
            let update = MenuBarUpdate {
                active_app: Some(ActiveApp {
                    app_id: "org.example.editor".into(),
                    title: String::new(),
                }),
                menu: Some(menu),
                window_state: None,
                engage: false,
                screen_capture: None,
                session_agent: None,
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
        let f = Fixture::new(kde_menu());
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
        let f = Fixture::new(kde_menu());
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
        let f = Fixture::new(gtk);
        assert_eq!(
            f.resolve(activate(SlotKey::Menu(ItemId(1)), 5)),
            Some(Action::Request(Request::ActivateGtk { props, item: 5 }))
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
        let f = Fixture::new(WindowMenu::None);
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
        let window = SlotKey::Menu(fallback::WINDOW_MENU);
        let act = |action: FallbackAction| f.resolve(activate(window.clone(), action.item_id().0));
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
        assert_eq!(act(FallbackAction::Edit(fallback::EditAction::Copy)), None);
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
            .system_menu(&f.shared.chords, &f.shared.recent);
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
}
