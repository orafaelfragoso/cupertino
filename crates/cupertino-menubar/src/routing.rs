//! From input and controller effects to what the process does: key mapping and the action a
//! controller effect stands for, given what the bar shows.

use crate::controller::{Button, Effect, Key, ScrollAxis, SYSTEM_MENU_ROOT};
use crate::dbus::dbusmenu::MenuEvent;
use crate::dbus::sni::{Orientation, StatusAction, StatusItem};
use crate::extras::{ExtraAction, ExtraKind, SystemAction};
use crate::fallback::{self, FallbackAction};
use crate::feed::{BarFeed, Shared};
use crate::item::{ItemId, Origin};
use crate::layout::{BarLayout, SlotKey};
use crate::source::MenuSource;
use crate::worker::Request;
use cupertino_apps::Launch;
use cupertino_kit_shell::bus::Panel;
use cupertino_kit_shell::menu_bar::{EditAction, WindowCommand};
use std::path::PathBuf;

/// The evdev codes of the keys menus take (`linux/input-event-codes.h`), as a key event carries them.
mod key {
    pub const ESCAPE: u32 = 1;
    pub const ENTER: u32 = 28;
    pub const SPACE: u32 = 57;
    pub const KEYPAD_ENTER: u32 = 96;
    pub const HOME: u32 = 102;
    pub const UP: u32 = 103;
    pub const LEFT: u32 = 105;
    pub const RIGHT: u32 = 106;
    pub const END: u32 = 107;
    pub const DOWN: u32 = 108;
}

/// The control panel, which Settings… of the system menu starts.
const SETTINGS_PROGRAM: &str = "cupertino-settings";

/// Start the control panel on the pane named `pane`.
fn settings_pane(pane: &str) -> Action {
    Action::Launch(Launch::Program(vec![
        SETTINGS_PROGRAM.to_owned(),
        "--pane".to_owned(),
        pane.to_owned(),
    ]))
}

/// Whether a bar opens the menu a source asks for (DBusMenu `ItemActivationRequested`): not while the bar gives way to
/// an overview (`hidden`), where nothing of it shows or takes input, nor while the session is locked, where the lock
/// screen covers it.
pub fn opens_requested_menu(hidden: bool, locked: bool) -> bool {
    !hidden && !locked
}

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
    /// Open or close a panel of another component on the bar's output, over the host bus.
    TogglePanel(Panel),
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
/// menus. An exporter is told before one of its menus shows, a status item's menu and its submenus included. An item
/// is sent to an exporter only while the tree the bar shows holds it: a choice of a row the exporter has since
/// removed does nothing.
pub fn resolve(effect: Effect, cx: &Context) -> Option<Action> {
    match effect {
        Effect::Redraw => Some(Action::Redraw),
        Effect::Opened { .. } => Some(Action::SetMenuOpen(true)),
        Effect::Closed => Some(Action::SetMenuOpen(false)),
        // an exporter's menu, or a submenu of the exporter's items the App menu took in
        Effect::AboutToShow {
            slot: slot @ (SlotKey::Menu(_) | SlotKey::AppMenu),
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
        // the Wi-Fi panel asks the network service for a fresh scan as it opens
        Effect::AboutToShow {
            slot: SlotKey::Status(id),
            item: ItemId(0),
        } if ExtraKind::from_slot_id(&id) == Some(ExtraKind::Wifi) => {
            Some(Action::Extra(ExtraAction::ScanWifi))
        }
        // a status item's menu, or one of its submenus
        Effect::AboutToShow {
            slot: SlotKey::Status(id),
            item,
        } => {
            let status = cx.shared.status.iter().find(|s| s.item.id == id)?;
            status.menu.as_ref()?.find(item)?;
            Some(Action::Request(Request::AboutToShow {
                menu: status.menu_source()?,
                item: item.0,
            }))
        }
        Effect::AboutToShow { .. } => None,
        Effect::Activate {
            slot: SlotKey::Status(id),
            item,
            ..
        } if ExtraKind::from_slot_id(&id).is_some() => {
            let kind = ExtraKind::from_slot_id(&id)?;
            match cx.shared.extras.menu(kind).action(item)? {
                ExtraAction::System(SystemAction::WifiSettings) => Some(settings_pane("wifi")),
                ExtraAction::System(SystemAction::BluetoothSettings) => {
                    Some(settings_pane("bluetooth"))
                }
                action => Some(Action::Extra(action.clone())),
            }
        }
        Effect::Activate {
            slot: SlotKey::Status(id),
            item,
            time,
        } => {
            let status = cx.shared.status.iter().find(|s| s.item.id == id)?;
            // the row is one the exporter still has, and can be chosen
            if !status.menu.as_ref()?.find(item)?.is_selectable() {
                return None;
            }
            Some(Action::Request(Request::ActivateDbusMenu {
                menu: status.menu_source()?,
                item: item.0,
                time,
            }))
        }
        Effect::Activate {
            slot: slot @ (SlotKey::Menu(_) | SlotKey::AppMenu),
            item,
            time,
        } => menu_action(&slot, item, time, cx),
        Effect::MenuOpened { slot, item, time } => {
            menu_event(&slot, item, MenuEvent::Opened, time, cx)
        }
        Effect::MenuClosed { slot, item, time } => {
            menu_event(&slot, item, MenuEvent::Closed, time, cx)
        }
        Effect::Activate {
            slot: SlotKey::System,
            item,
            ..
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
        Effect::SlotActivated { slot, button, at } => match (slot, button) {
            (SlotKey::Status(id), button) => {
                let status = cx.shared.status.iter().find(|s| s.item.id == id)?;
                // where the pointer is, in the compositor space
                let (x, y) = (
                    at.x.round() as i32 + cx.origin.0,
                    at.y.round() as i32 + cx.origin.1,
                );
                // as the specification has them: the primary button activates the item (the controller opened the
                // menu of one that is only a menu, `StatusItem::primary_action`); an item that is only a menu and
                // has none the bar could read, and the secondary button on an item without a menu, show its own
                let action = match button {
                    Button::Primary if !status.item.item_is_menu => StatusAction::Activate { x, y },
                    Button::Primary | Button::Secondary => StatusAction::ContextMenu { x, y },
                    Button::Middle => StatusAction::SecondaryActivate { x, y },
                };
                Some(status_action(&status.item, action))
            }
            (SlotKey::ControlPanel, Button::Primary) => {
                Some(Action::TogglePanel(Panel::ControlPanel))
            }
            (SlotKey::Clock, Button::Primary) => {
                Some(Action::TogglePanel(Panel::NotificationCenter))
            }
            _ => None,
        },
        Effect::SlotScrolled {
            slot: SlotKey::Status(id),
            steps,
            axis,
        } => {
            let status = cx.shared.status.iter().find(|s| s.item.id == id)?;
            let orientation = match axis {
                ScrollAxis::Vertical => Orientation::Vertical,
                ScrollAxis::Horizontal => Orientation::Horizontal,
            };
            let action = StatusAction::Scroll {
                delta: steps,
                orientation,
            };
            Some(status_action(&status.item, action))
        }
        Effect::SlotScrolled { .. } => None,
    }
}

/// Send `action` to the status item `item`.
fn status_action(item: &StatusItem, action: StatusAction) -> Action {
    Action::Request(Request::StatusAction {
        service: item.service.clone(),
        path: item.path.clone(),
        action,
    })
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
fn menu_action(slot: &SlotKey, item: ItemId, time: u32, cx: &Context) -> Option<Action> {
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
            time,
        })),
        MenuSource::Gtk(props) => Some(Action::Request(Request::ActivateGtk {
            props,
            item: target.0,
            // the component asks for the activation token before it sends the request
            token: None,
        })),
        MenuSource::Synthetic => None,
    }
}

/// The DBusMenu event that tells an exporter the menu of `item` of `slot` opened or closed. An exporter is told of its
/// own menus only: an application's menus and the submenus of its items the App menu took in, a status item's menu and
/// its submenus. A menu that closed because its item left the tree is told under the id it had: the follower sends it
/// only to an exporter that was told it opened or still has the id, so a bar that changed applications under an open
/// menu tells the new one nothing.
fn menu_event(
    slot: &SlotKey,
    item: ItemId,
    event: MenuEvent,
    time: u32,
    cx: &Context,
) -> Option<Action> {
    let menu = match slot {
        SlotKey::Menu(_) | SlotKey::AppMenu => {
            if fallback::is_system(item) {
                return None;
            }
            match shown_origin(slot, item, cx) {
                Some(Origin::Bar) => return None,
                None if event == MenuEvent::Opened => return None,
                _ => {}
            }
            match cx.feed.source(cx.shared) {
                MenuSource::Kde(menu) => menu,
                _ => return None,
            }
        }
        SlotKey::Status(id) => {
            let status = cx.shared.status.iter().find(|s| &s.item.id == id)?;
            status.menu_source()?
        }
        SlotKey::System | SlotKey::ControlPanel | SlotKey::Clock => return None,
    };
    Some(Action::Request(Request::MenuEvent {
        menu,
        item: item.0,
        event,
        time,
    }))
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
