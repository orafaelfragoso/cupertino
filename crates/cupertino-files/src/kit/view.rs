//! What a browser window shows for its [`BrowserWindow`].

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::rc::Rc;

use cupertino_menu_model::{ActionName, MenuBar};

use cupertino_kit::controls::{
    striped_element, unobscured_insets, Column, ColumnWidth, CompactButton, ContextMenu, Dialog,
    DialogAction, DialogButton, DialogKind, Icon, IconDirection, Image, ImageFit, ImageSource,
    MenuAnchor, Modal, NavigationStack, OutlineTable, Progress, ScrollView, Scroller, SearchField,
    SecureField, Selection, Sidebar, SidebarAccessory, SidebarItem, SidebarLook, SidebarSection,
    SidebarWindow, SortOrder, Spacer, Stack, TableView, Text, TextField, TextRole, ToolbarGlyph,
    ToolbarGroup, ToolbarItem, ToolbarMenuButton, ToolbarPicker, ToolbarSearch, ToolbarTitle,
    TreeItem,
};
use cupertino_kit::core::{CommandRegistry, Key, Menus, Shortcut, WindowBackground};
use cupertino_kit::geometry::{Insets, Size};
use cupertino_kit::layout::{Align, Axis};
use cupertino_kit::platform::{DecorationMode, Keysym, Mods, WindowSpec};
use cupertino_kit::prelude::*;
use cupertino_kit::text::SecureBuffer;
use cupertino_kit::text::{AttributedText, ContentPurpose, TextStyle};
use cupertino_kit::theme::{ColorRole, ControlSize};

use super::announce::Announcer;
use super::columns;
use super::context::Secondary;
use super::drops;
use super::gallery;
use super::grid::{self, IconGrid, Item};
use super::jobs::Job;
use super::list::{self, Gestures, RowCell};
use super::progress::{self, ShowsProgress};
use super::search::{no_results, SearchSlot};
use super::sidebar::FocusWatch;
use super::toolbar::{PathBar, PathSegment};
use super::{window_key, BrowserWindow, Event, Message, Popup, Session, Unlock, COLUMNS};
use crate::format::{human_size, relative_date};
use crate::layout::icon::{self, Flow, FontMetrics, IconLayout, IconParams, SectionSpec};
use crate::layout::list::RowSpec;
use crate::menu_bar::MenuAction;
use crate::metrics::{list_view, sidebar, trash, window as window_metrics};
use crate::places::{window_title, PlaceGroup};
use crate::places::{PlaceKind, Volume};
use crate::settings::{default_columns, ColumnId, IconViewSettings};
use crate::udisks::Passphrase;
use crate::window::{metrics, path_chain, status_text, ViewMode};
use crate::{mime, Entry, SortKey};

pub(super) const SIDEBAR_WIDTH: f64 = sidebar::DEFAULT_WIDTH;
/// The side of a list row's icon, in points.
const ROW_ICON: f64 = list_view::ICON_SIZE_DEFAULT as f64;
/// From a name cell's leading edge to its text: the item's icon and the gap after it.
pub(super) const NAME_TEXT_X: f64 = ROW_ICON + metrics::PATH_ICON_GAP;
/// A list row: the icon size over the row padding, as the list view's rows.
const ROW_HEIGHT: f64 = ROW_ICON + list_view::ROW_PADDING;
/// How far beyond the viewport items count as shown, in points: their thumbnails are made before they scroll in.
const SHOWN_MARGIN: f64 = 200.0;
/// The symbol of a folder of the path bar that is not a place of the sidebar.
const FOLDER_SYMBOL: &str = "symbol.folder";
/// The heading of the sidebar's tags, and the symbol of each.
const TAGS_TITLE: &str = "Tags";
const TAG_SYMBOL: &str = "symbol.tag.fill";
/// The key of the field that edits an item's name in its row of window `id`; one per window, as focus goes to the
/// first window holding the key.
pub(super) fn name_field_key(id: u64) -> Key {
    Key::from(format!("name-field/{id}"))
}
const ESCAPE: Keysym = Keysym(0xff1b);
const SPACE: Keysym = Keysym(0x20);
const DOWN: Keysym = Keysym(0xff54);
/// The symbol of a mounted removable file system's eject button.
const EJECT_SYMBOL: &str = "symbol.eject";
/// The width of a running transfer's progress bar, and the room between the parts of its row.
/// The side of the chevron between two folders of the path bar.
const PATH_SEPARATOR: f64 = 8.0;
const TRANSFER_PROGRESS_WIDTH: f64 = 160.0;
const TRANSFER_SPACING: f64 = 8.0;
const BAR_INSETS: Insets = Insets {
    top: 4.0,
    left: metrics::PATH_BAR_INSET,
    bottom: 4.0,
    right: metrics::PATH_BAR_INSET,
};

pub(super) fn window(session: &Session, id: u64, win: &BrowserWindow) -> Window<Message> {
    let title = win.title(session);
    let state = win.menu_state(session);
    let bar = crate::menu_bar::menu_bar(&state);
    let disabled = BrowserWindow::disabled(&state);
    let (width, height) = window_metrics::DEFAULT_SIZE;
    Window {
        key: window_key(id),
        spec: WindowSpec {
            title,
            app_id: super::APP_ID.into(),
            size: Size::new(width, height),
            min_size: Some(Size::new(metrics::WINDOW_MIN.0, metrics::WINDOW_MIN.1)),
            max_size: None,
            decorations: DecorationMode::Sidebar,
            foreign_parent: None,
            input_region: None,
        },
        shortcuts: shortcuts(id, win, &bar, &disabled),
        on_close: Some(Message::Window(id, Event::Close)),
        // the sidebar window paints the sidebar's glass and the folder's background itself
        background: WindowBackground::None,
        popup: None,
        menus: Some(Menus {
            bar,
            disabled,
            on_activate: send(id, Event::Menu),
        }),
        content: Element::from(Modal {
            content: Element::from(SidebarWindow {
                label: "Sidebar".into(),
                layout: win.split,
                min_sidebar: metrics::SIDEBAR_MIN,
                min_content: metrics::BROWSER_MIN.0,
                on_change: Some(send(id, Event::Split)),
                sidebar_header: None,
                sidebar: Element::new(FocusWatch {
                    on_focus: send(id, Event::SidebarFocus),
                    children: vec![places(session, id, win)],
                }),
                content: content(session, id, win),
            })
            .key("browser")
            .fill(),
            dialog: win
                .unlock
                .as_ref()
                .map(|unlock| unlock_sheet(id, unlock))
                .or_else(|| win.confirm_empty.then(|| empty_trash_alert(id))),
        }),
    }
}

/// The chords of the menus' enabled items, Primary+Down to open the selection, and Escape, once a field has passed it
/// on: it ends a rename without renaming, else it clears the search.
fn shortcuts(
    id: u64,
    win: &BrowserWindow,
    bar: &MenuBar,
    disabled: &BTreeSet<ActionName>,
) -> CommandRegistry<Message> {
    let mut registry = CommandRegistry::new();
    for (chord, action) in crate::menu_bar::chords(bar) {
        let Some(keysym) = crate::menu_bar::keysym(chord.key.name()) else {
            continue;
        };
        if !disabled.contains(&action.name()) {
            let shortcut = Shortcut {
                mods: chord.modifiers,
                keysym: Keysym(keysym),
            };
            registry.insert(shortcut, Message::Window(id, Event::Menu(action.name())));
        }
    }
    if !disabled.contains(&MenuAction::Open.name()) {
        let open_down = Shortcut {
            mods: Mods::PRIMARY,
            keysym: DOWN,
        };
        registry.insert(
            open_down,
            Message::Window(id, Event::Menu(MenuAction::Open.name())),
        );
    }
    // Space opens the preview of the selection, or closes it; a field takes it first
    let peek = win.peek.as_ref().is_some_and(|look| !look.closing);
    if win.rename.is_none() && (peek || !disabled.contains(&MenuAction::Peek.name())) {
        let space = Shortcut {
            mods: Mods::NONE,
            keysym: SPACE,
        };
        registry.insert(
            space,
            Message::Window(id, Event::Menu(MenuAction::Peek.name())),
        );
    }
    let escape = Shortcut {
        mods: Mods::NONE,
        keysym: ESCAPE,
    };
    if win.rename.is_some() {
        registry.insert(escape, Message::Window(id, Event::NameCancelled));
    } else if peek {
        registry.insert(
            escape,
            Message::Window(id, Event::Menu(MenuAction::Peek.name())),
        );
    } else if !win.search.is_empty() {
        registry.insert(escape, Message::Window(id, Event::SearchCleared));
    }
    registry
}

fn places(session: &Session, id: u64, win: &BrowserWindow) -> Element<Message> {
    let dir = win.browser.dir();
    let groups = [PlaceGroup::Favorites, PlaceGroup::Locations];
    // the sidebar counts its items across its sections, in this order
    let shown: Vec<usize> = groups
        .iter()
        .flat_map(|&group| {
            (0..session.places.len()).filter(move |&i| session.places[i].group == group)
        })
        .collect();
    let mut sections: Vec<SidebarSection> = groups
        .into_iter()
        .map(|group| SidebarSection {
            title: Some(group.title().into()),
            items: shown
                .iter()
                .copied()
                .filter(|&i| session.places[i].group == group)
                .map(|i| place_item(session, i))
                .collect(),
        })
        .filter(|s: &SidebarSection| !s.items.is_empty())
        .collect();
    // the Favorites' symbols are the accent, the Locations' the secondary label colour
    let accent = sections
        .iter()
        .map(|s| s.title.as_deref() == Some(PlaceGroup::Favorites.title()))
        .collect();
    // the Tags section, always there and empty until an item listed carries a tag: the colour tags first, in their
    // order, then the others
    let tags = sidebar_tags(session);
    sections.push(SidebarSection {
        title: Some(TAGS_TITLE.into()),
        items: tags
            .iter()
            .map(|name| SidebarItem {
                key: Key::from(format!("sidebar-tag/{name}")),
                label: name.clone(),
                icon: Some(TAG_SYMBOL.into()),
                description: None,
                accessory: None,
                busy: false,
            })
            .collect(),
    });
    // a place about to spring open under a drag flashes as the selected one
    let flashing = win.springing.flashing().map(PathBuf::as_path);
    let selected = shown
        .iter()
        .position(|&i| flashing.is_some() && session.places[i].folder() == flashing)
        .or_else(|| {
            shown
                .iter()
                .position(|&i| session.places[i].folder() == Some(dir))
        });
    // the row a context menu is open on, among the shown ones
    let ringed = win
        .context
        .as_ref()
        .and_then(|menu| match menu.ring {
            Some(super::context::Ringed::Place(place)) => Some(place),
            _ => None,
        })
        .and_then(|place| shown.iter().position(|&i| i == place));
    let pressed: Vec<usize> = shown.clone();
    let sidebar = Sidebar {
        label: "Sidebar".into(),
        sections,
        selected,
        scroll: win.sidebar_scroll,
        on_scroll: send(id, Event::SidebarScroll),
        on_select: Some(send(id, move |row: usize| match shown.get(row) {
            Some(&place) => Event::Place(place),
            None => Event::TagPlace(tags[row - shown.len()].clone()),
        })),
        on_accessory: Some(send(id, Event::Accessory)),
    }
    .with_context(
        SidebarLook::Neutral { accent },
        ringed,
        Some(send(id, move |row: usize| match pressed.get(row) {
            Some(&place) => Event::PlacePress(place),
            // a tag has no menu
            None => Event::PlacePress(usize::MAX),
        })),
    )
    .key("places");
    // a secondary press on a row says which place it was on first, then the sidebar opens the menu there
    let sidebar = Element::new(ContextMenu {
        content: sidebar,
        on_open: Some(send(id, Event::SidebarMenu)),
    })
    .fill();
    drops::sidebar(session, id, sidebar)
}

/// The tags the sidebar lists: those the windows' items carry, the colour tags first in their order.
fn sidebar_tags(session: &Session) -> Vec<String> {
    let known = session.known_tags.borrow();
    let colours = crate::context_menu::TAG_ROW_ORDER
        .iter()
        .map(|c| c.title())
        .filter(|title| known.iter().any(|k| k.eq_ignore_ascii_case(title)))
        .map(str::to_owned);
    let others = known
        .iter()
        .filter(|k| crate::tags::TagColor::from_title(k).is_none())
        .cloned();
    colours.chain(others).collect()
}

/// The tags popover of window `id`, while it is open.
pub(super) fn tag_popover(
    session: &Session,
    id: u64,
    win: &BrowserWindow,
) -> Option<Window<Message>> {
    let editor = win.tag_editor.as_ref()?;
    let entries = win.browser.entries();
    let selected: Vec<&Entry> = win
        .browser
        .selection()
        .indices()
        .filter_map(|i| entries.get(i))
        .collect();
    let subject = match selected.as_slice() {
        [] => return None,
        [one] => format!("“{}”", one.name),
        many => format!("{} items", many.len()),
    };
    let known: Vec<String> = session.known_tags.borrow().iter().cloned().collect();
    let on = |name: &str| {
        selected
            .iter()
            .all(|e| e.tags.iter().any(|t| t.eq_ignore_ascii_case(name)))
    };
    let tags = super::tag_editor::listed(&known, on, &editor.filter);
    Some(super::tag_editor::window(
        id,
        editor,
        &subject,
        tags,
        &session.tag_colors,
    ))
}

/// The share popover of window `id`, while it is open.
pub(super) fn share_popover(
    session: &Session,
    id: u64,
    win: &BrowserWindow,
) -> Option<Window<Message>> {
    let state = win.share.as_ref()?;
    let entries = win.browser.entries();
    let selected: Vec<&Entry> = win
        .browser
        .selection()
        .indices()
        .filter_map(|i| entries.get(i))
        .collect();
    let first = selected.first()?;
    let subject = match selected.as_slice() {
        [one] => {
            let kind = crate::mime::kind_description(&crate::mime::entry_mime(one));
            let mut chars = kind.chars();
            let kind = chars
                .next()
                .map(|c| c.to_uppercase().chain(chars).collect())
                .unwrap_or_default();
            super::share_popover::Subject {
                name: one.name.clone(),
                kind,
                icon: session.icons.icon(one, 36.0),
            }
        }
        many => super::share_popover::Subject {
            name: format!("{} items", many.len()),
            kind: String::new(),
            icon: session.icons.icon(first, 36.0),
        },
    };
    Some(super::share_popover::window(
        id,
        state,
        subject,
        &win.selection_apps(session),
    ))
}

/// The key of place `index`'s row: a folder's by its name, a removable file system's by its udisks object, so a row
/// keeps its focus and accessible object while others come and go.
pub(super) fn place_key(session: &Session, index: usize) -> Key {
    let device = index
        .checked_sub(session.standard_places)
        .and_then(|row| session.media.device(row));
    match device {
        Some((device, _)) => Key::from(format!("device-{}", device.object.as_str())),
        None => Key::from(format!("place-{}", session.places[index].name)),
    }
}

/// The row of place `index`. A removable file system is described for assistive technology ("Removable", and whether
/// it is mounted, locked or read-only), a mounted one has an eject button, and one with a call under way is busy.
fn place_item(session: &Session, index: usize) -> SidebarItem {
    let place = &session.places[index];
    let device = index
        .checked_sub(session.standard_places)
        .and_then(|row| session.media.device(row));
    let description = device.map(|(device, _)| {
        let mut words = vec!["Removable"];
        if device.locked {
            words.push("locked");
        } else if device.mount.is_none() {
            words.push("not mounted");
        }
        if device.read_only {
            words.push("read-only");
        }
        words.join(", ")
    });
    SidebarItem {
        key: place_key(session, index),
        label: place.name.clone(),
        icon: Some(place.kind.sidebar_symbol().into()),
        description,
        accessory: place.ejectable().then(|| SidebarAccessory {
            symbol: EJECT_SYMBOL.into(),
            label: format!("Eject {}", place.name),
        }),
        busy: matches!(place.kind, PlaceKind::Removable(Volume::Busy, _)),
    }
}

/// The sheet that asks for the passphrase of a locked container.
fn unlock_sheet(id: u64, unlock: &Unlock) -> Dialog<Message> {
    Dialog {
        kind: DialogKind::Dialog,
        icon: None,
        title: format!("Unlock “{}”", unlock.name),
        message: "Enter the passphrase of this encrypted volume.".into(),
        body: Some(
            Element::new(SecureField {
                label: "Passphrase".into(),
                placeholder: "Passphrase".into(),
                enabled: true,
                on_change: Some(Box::new(move |typed: &SecureBuffer| {
                    let passphrase = typed.with_secret(Passphrase::new);
                    Message::Window(id, Event::Passphrase(passphrase))
                })),
                on_submit: Some(Box::new(move |_: &SecureBuffer| {
                    Message::Window(id, Event::Unlock)
                })),
                ..SecureField::default()
            })
            .key("passphrase"),
        ),
        buttons: vec![
            DialogButton {
                label: "Cancel".into(),
                action: DialogAction::Cancel,
                on_press: Message::Window(id, Event::UnlockCancelled),
            },
            DialogButton {
                label: "Unlock".into(),
                action: DialogAction::Default,
                on_press: Message::Window(id, Event::Unlock),
            },
        ],
    }
}

/// The name of the full trash can picture the empty-trash alert shows.
const FULL_TRASH_ICON: &str = "trash.full";

/// The question before the trash is emptied: an alert whose default button deletes everything for good.
fn empty_trash_alert(id: u64) -> Dialog<Message> {
    Dialog {
        kind: DialogKind::Alert,
        icon: Some(FULL_TRASH_ICON.into()),
        title: cupertino_trash::EMPTY_TITLE.into(),
        message: cupertino_trash::EMPTY_MESSAGE.into(),
        body: None,
        buttons: vec![
            DialogButton {
                label: cupertino_trash::CANCEL_BUTTON.into(),
                action: DialogAction::Cancel,
                on_press: Message::Window(id, Event::EmptyTrashCancelled),
            },
            DialogButton {
                label: cupertino_trash::EMPTY_BUTTON.into(),
                action: DialogAction::Default,
                on_press: Message::Window(id, Event::EmptyTrash),
            },
        ],
    }
}

/// The strip under the toolbar in the trash: how many items it holds and the Empty button, which asks before it
/// deletes and is off while there is nothing to delete.
fn trash_strip(id: u64, win: &BrowserWindow) -> Element<Message> {
    let items = win.browser.item_count();
    let label = match items {
        1 => "1 item".to_owned(),
        n => format!("{n} items"),
    };
    let top = if win.tab_bar_shown() {
        0.0
    } else {
        unobscured_insets(DecorationMode::Sidebar).top
    };
    let strip = Element::new(Stack {
        axis: Axis::Vertical,
        padding: Insets::ZERO,
        spacing: 0.0,
        align: Align::Stretch,
        children: vec![
            Element::new(Stack {
                axis: Axis::Horizontal,
                padding: Insets {
                    top: 0.0,
                    left: trash::STRIP_INSET,
                    bottom: 0.0,
                    right: trash::BUTTON_INSET,
                },
                spacing: 0.0,
                align: Align::Center,
                children: vec![
                    Element::new(Text {
                        content: AttributedText::plain(
                            label,
                            TextStyle {
                                weight: cupertino_kit::text::FontWeight::BOLD,
                                ..TextStyle::new(13.0)
                            },
                        ),
                        role: TextRole::Body,
                        wrap: false,
                        secondary: true,
                        selected: false,
                    }),
                    Element::new(Spacer { min: 0.0 }),
                    Element::new(CompactButton {
                        label: "Empty".into(),
                        enabled: items > 0,
                        on_press: Some(Message::Window(id, Event::AskEmptyTrash)),
                    })
                    .key("empty-trash"),
                ],
            })
            .height(trash::STRIP_HEIGHT - 1.0),
            Element::new(Separator {
                axis: Axis::Horizontal,
            }),
        ],
    })
    .height(trash::STRIP_HEIGHT);
    Element::new(Stack {
        axis: Axis::Vertical,
        padding: Insets {
            top,
            left: 0.0,
            bottom: 0.0,
            right: 0.0,
        },
        spacing: 0.0,
        align: Align::Stretch,
        children: vec![strip],
    })
}

/// How far the toolbar row covers the top of the folder's view, which fills the pane under the row; none with the tab
/// bar shown, which puts the view below the row and the bar, and none in the trash, whose strip lies under the row.
pub(super) fn under_row(win: &BrowserWindow) -> f64 {
    if win.tab_bar_shown() || win.in_trash() {
        0.0
    } else {
        unobscured_insets(DecorationMode::Sidebar).top
    }
}

fn content(session: &Session, id: u64, win: &BrowserWindow) -> Element<Message> {
    let history = win.browser.history();
    let folder = match (win.view, win.browser.no_results()) {
        (ViewMode::List, false) => list(session, id, win),
        (ViewMode::List, true) => no_results(
            list(session, id, win),
            under_row(win) + list_view::HEADER_HEIGHT,
        ),
        (ViewMode::Icons, false) => icons(session, id, win),
        (ViewMode::Icons, true) => no_results(icons(session, id, win), under_row(win)),
        (ViewMode::Columns, false) => columns::view(session, id, win),
        (ViewMode::Columns, true) => no_results(columns::view(session, id, win), under_row(win)),
        (ViewMode::Gallery, false) => gallery::view(session, id, win),
        (ViewMode::Gallery, true) => no_results(gallery::view(session, id, win), under_row(win)),
    };
    // Space on the items opens their preview
    let peek = !disabled_peek(session, win);
    let folder = Element::new(super::peek::SpaceKey {
        message: peek.then(|| Message::Window(id, Event::Menu(MenuAction::Peek.name()))),
        children: vec![folder.fill()],
    })
    .fill();
    // the window follows whether the keyboard is on the items, so a folder closed over it hands it to the folder
    let folder = Element::new(FocusWatch {
        on_focus: send(id, Event::ItemsFocus),
        children: vec![folder.fill()],
    })
    .fill();
    // a secondary press anywhere in the folder opens a context menu: an item's cell says it was on the item first
    let folder = Element::new(Secondary {
        message: Message::Window(id, Event::SecondaryPress(None)),
        children: vec![folder.fill()],
    })
    .fill();
    let folder = drops::folder(session, id, win, folder);
    // the tab bar and the trash's strip, under the toolbar
    let mut above = Vec::new();
    if win.tab_bar_shown() {
        above.push(tab_bar(session, id, win));
    }
    if win.in_trash() {
        above.push(trash_strip(id, win));
    }
    let folder = match above.is_empty() {
        false => {
            above.push(folder.fill());
            Element::new(Stack {
                axis: Axis::Vertical,
                padding: Insets::ZERO,
                spacing: 0.0,
                align: Align::Stretch,
                children: above,
            })
            .fill()
        }
        true => folder,
    };
    let mut children = vec![Element::from(NavigationStack {
        content: Element::new(ContextMenu {
            content: folder,
            on_open: Some(send(id, Event::ContextMenu)),
        })
        .key("folder")
        .fill(),
        toolbar: toolbar(session, id, win),
        can_go_back: history.back_target().is_some(),
        can_go_forward: history.forward_target().is_some(),
        on_navigate: Some(send(id, Event::Navigate)),
        // the folder fills the pane and scrolls under the toolbar row; with the tab bar, it lies below the bar
        under_toolbar: !win.tab_bar_shown(),
    })
    .key("navigation")
    .fill()];
    let started_here = session.jobs.running.iter().filter(|job| job.origin == id);
    children.extend(started_here.map(transfer_row));
    if win.path_bar {
        children.push(path_bar(session, id, win));
    }
    if win.status_bar {
        children.push(
            bar(vec![
                Element::new(Spacer { min: 0.0 }),
                text(
                    status_text(win.browser.item_count(), win.available),
                    TextRole::Caption,
                    true,
                ),
                Element::new(Spacer { min: 0.0 }),
            ])
            .key("status-bar"),
        );
    }
    children.push(
        // each message is a node of its own, which a screen reader hears as it appears
        Element::new(Announcer {
            text: win.announcement.text.clone(),
        })
        .key(format!("announcer/{}", win.announcement.serial)),
    );
    Element::new(Stack {
        axis: Axis::Vertical,
        padding: Insets::ZERO,
        spacing: 0.0,
        align: Align::Stretch,
        children,
    })
    .fill()
}

/// The tab bar of window `id`: each tab named after its folder.
fn tab_bar(session: &Session, id: u64, win: &BrowserWindow) -> Element<Message> {
    let active = win.tabs.active_id();
    let tabs = win
        .tabs
        .order()
        .iter()
        .map(|&tab| {
            let dir = match win.parked.get(&tab) {
                Some(parked) => parked.browser.dir(),
                None => win.browser.dir(),
            };
            super::tabs::Shown {
                id: tab,
                title: window_title(dir, &session.places),
                active: tab == active,
            }
        })
        .collect();
    super::tabs::bar(id, tabs)
}

/// Whether Space on the items does nothing: a name is edited, or nothing is selected and no preview is open.
fn disabled_peek(session: &Session, win: &BrowserWindow) -> bool {
    let open = win.peek.as_ref().is_some_and(|look| !look.closing);
    let state = win.menu_state(session);
    let enabled = crate::menu_bar::enabled(&state).contains(&(MenuAction::Peek, true));
    win.rename.is_some() || !(open || enabled)
}

/// The folder's title, which takes the room the other items leave; the view-mode choice; the View menu; the share,
/// Tags and Action group; then the search field, which rests as a round button and grows into the title's room while
/// it has the keyboard or text ([`crate::search_field`]).
fn toolbar(session: &Session, id: u64, win: &BrowserWindow) -> Vec<Element<Message>> {
    // the open field is a capsule as tall as the toolbar's controls
    let field = Element::new(SearchField {
        field: TextField {
            label: "Search".into(),
            placeholder: "Search".into(),
            value: win.search.clone(),
            revision: win.search_revision,
            purpose: ContentPurpose::Normal,
            enabled: true,
            on_change: Some(send(id, Event::Search)),
            on_submit: None,
            selection: None,
            initial: false,
        },
        size: Some(ControlSize::ExtraLarge),
    })
    .key("search");
    let expanded = |popup: Popup| {
        win.context
            .as_ref()
            .is_some_and(|menu| menu.anchor == MenuAnchor::Below(popup_key(popup)))
    };
    let popup = |popup: Popup| Some(Message::Window(id, Event::Popup(popup)));
    let menu = |action: MenuAction| Some(Message::Window(id, Event::Menu(action.name())));
    let item = |key: Option<Key>, symbol: &str, label: &str, on_press, menu| ToolbarItem {
        key,
        glyph: ToolbarGlyph::Symbol(symbol.into()),
        label: label.into(),
        on_press,
        menu,
    };
    let view_modes = ToolbarGroup {
        label: "View As".into(),
        items: vec![
            item(
                None,
                "symbol.grid",
                "Icons",
                menu(MenuAction::View(ViewMode::Icons)),
                None,
            ),
            item(
                None,
                "symbol.list",
                "List",
                menu(MenuAction::View(ViewMode::List)),
                None,
            ),
            item(
                None,
                "symbol.columns",
                "Columns",
                menu(MenuAction::View(ViewMode::Columns)),
                None,
            ),
            item(
                None,
                "symbol.gallery",
                "Gallery",
                menu(MenuAction::View(ViewMode::Gallery)),
                None,
            ),
        ],
        selected: Some(match win.view {
            ViewMode::Icons => 0,
            ViewMode::List => 1,
            ViewMode::Columns => 2,
            ViewMode::Gallery => 3,
        }),
        dividers: true,
    };
    let actions = ToolbarGroup {
        label: String::new(),
        items: vec![
            // a share popover for the selection; without one the button is off
            ToolbarItem {
                key: Some(popup_key(Popup::Share)),
                ..item(
                    None,
                    "symbol.share",
                    "Share",
                    popup(Popup::Share).filter(|_| !win.browser.selection().is_empty()),
                    Some(win.share.is_some()),
                )
            },
            // an authored tag: the host's symbol themes may have none, and stand a different glyph in for it
            ToolbarItem {
                glyph: ToolbarGlyph::Tag,
                ..item(
                    Some(popup_key(Popup::Tags)),
                    "",
                    "Tags",
                    popup(Popup::Tags),
                    Some(expanded(Popup::Tags) || win.tag_editor.is_some()),
                )
            },
            ToolbarItem {
                glyph: ToolbarGlyph::More,
                ..item(
                    Some(popup_key(Popup::Action)),
                    "",
                    "Action",
                    popup(Popup::Action),
                    Some(expanded(Popup::Action)),
                )
            },
        ],
        selected: None,
        dividers: false,
    };
    // while the search field is open the title keeps its room: the four views fold into one pop-up that shows the
    // current one, and the View button, which offers the same, steps aside
    let views = if win.search_width.is_open() {
        vec![Element::from(ToolbarPicker {
            key: Some(popup_key(Popup::View)),
            symbol: match win.view {
                ViewMode::Icons => "symbol.grid",
                ViewMode::List => "symbol.list",
                ViewMode::Columns => "symbol.columns",
                ViewMode::Gallery => "symbol.gallery",
            }
            .into(),
            label: "View As".into(),
            on_press: popup(Popup::View),
            expanded: expanded(Popup::View),
        })]
    } else {
        vec![
            Element::from(view_modes),
            Element::from(ToolbarMenuButton {
                key: Some(popup_key(Popup::View)),
                symbol: "symbol.grid".into(),
                label: "View".into(),
                on_press: popup(Popup::View),
                expanded: expanded(Popup::View),
            }),
        ]
    };
    let mut items = vec![Element::new(ToolbarTitle {
        text: win.title(session),
    })
    .key("title")];
    items.extend(views);
    items.extend([
        // keyed, so it keeps its buttons, and the keyboard on one, when the views fold or unfold before it
        Element::from(actions).key("actions"),
        Element::new(SearchSlot {
            window: id,
            animating: win.search_width.animating(),
            children: vec![Element::from(ToolbarSearch {
                field,
                collapsed: !win.search_width.is_open() && !win.search_width.animating(),
            })],
        })
        .key("search-slot")
        .width(win.search_width.width()),
    ]);
    items
}

fn list(session: &Session, id: u64, win: &BrowserWindow) -> Element<Message> {
    let browser = &win.browser;
    let defaults = default_columns();
    let columns = COLUMNS
        .iter()
        .map(|&id| {
            let desc = defaults.iter().find(|c| c.id == id);
            Column {
                title: column_title(id).into(),
                width: match id {
                    ColumnId::Name => ColumnWidth::Fill,
                    _ => ColumnWidth::Fixed(desc.map_or(100.0, |c| c.width)),
                },
            }
        })
        .collect();
    let spec = browser.sort();
    let sort = COLUMNS
        .iter()
        .position(|id| id.sort_key() == spec.key && spec.key != SortKey::None)
        .map(|column| {
            let order = if spec.ascending {
                SortOrder::Ascending
            } else {
                SortOrder::Descending
            };
            (column, order)
        });
    let now = win.now;
    // the table asks only for the rows it shows, so their text and icons are made then
    let entries: Rc<[crate::Entry]> = browser.entries().into();
    let names: Rc<[String]> = browser.entries().iter().map(|e| e.name.clone()).collect();
    let type_names = Rc::clone(&names);
    let item_icons = Rc::clone(&session.icons);
    let paths = Rc::clone(&entries);
    let items = tree_items(browser.outline());
    let selected = shown_selection(win);
    let cell_selection = selected.clone();
    let presses = Rc::clone(&win.list_presses);
    let cell_presses = Rc::clone(&win.list_presses);
    let label = window_title(browser.dir(), &session.places);
    let rename = win.rename.as_ref().map(|r| {
        (
            r.edit.index(),
            r.edit.text().to_owned(),
            r.revision,
            r.selection.clone(),
        )
    });
    let table = TableView {
        label,
        columns,
        sort,
        on_sort: Some(send(id, Event::Sort)),
        count: entries.len(),
        row_height: ROW_HEIGHT,
        scroll: win.scroll,
        selection: selected.clone(),
        multiple: true,
        cell: Box::new(move |row, column, selected| {
            if let Some((_, value, revision, selection)) =
                rename.as_ref().filter(|r| r.0 == row && column == 0)
            {
                return name_field(id, value.clone(), *revision, selection.clone());
            }
            // a stack that fills the cell gives the text its width, so a long one ends in an ellipsis
            let cell = Element::new(Stack {
                axis: Axis::Vertical,
                padding: Insets::ZERO,
                spacing: 0.0,
                align: Align::Stretch,
                children: vec![
                    Element::new(Spacer { min: 0.0 }),
                    cell_text_element(
                        cell_text(&entries[row], COLUMNS[column], now),
                        column,
                        selected,
                    ),
                    Element::new(Spacer { min: 0.0 }),
                ],
            })
            .fill();
            // the name follows the item's icon
            let cell = match column {
                0 => Element::new(Stack {
                    axis: Axis::Horizontal,
                    padding: Insets::ZERO,
                    spacing: metrics::PATH_ICON_GAP,
                    align: Align::Center,
                    children: vec![
                        item_image(item_icons.icon(&entries[row], ROW_ICON), ROW_ICON),
                        cell,
                    ],
                })
                .fill(),
                _ => cell,
            };
            Element::new(RowCell {
                window: id,
                index: row,
                name: column == 0,
                label: (column == 0).then(|| entries[row].name.clone()),
                selection: cell_selection.clone(),
                presses: Rc::clone(&cell_presses),
                children: vec![cell],
            })
            .fill()
        }),
        type_select: Some(Box::new(move |row| type_names[row].clone())),
        on_scroll: send(id, Event::Scroll),
        on_select: Some(send(id, Event::Select)),
        on_activate: Some(send(id, Event::Open)),
        on_row_key: Some(Box::new(list::tab(id, names, selected.cursor()))),
        header_top: under_row(win),
    };
    let count = table.count;
    let ringed = ringed_items(win);
    let table = striped_element(
        OutlineTable {
            table,
            // the outline follows the entries; a row it does not know yet is a top-level one
            item: Box::new(move |row| {
                items.get(row).copied().unwrap_or(TreeItem {
                    level: 0,
                    expanded: None,
                    position: row,
                    siblings: count,
                })
            }),
            key: Some(Box::new(move |row| row_key(id, &paths[row].path))),
            on_toggle: Some(send(id, Event::Toggle)),
        },
        ringed,
    )
    .key("items");
    Element::new(Gestures {
        window: id,
        count,
        row_height: ROW_HEIGHT,
        scroll: win.scroll,
        selection: selected,
        presses,
        band: win.band,
        lead: under_row(win) + list_view::HEADER_HEIGHT,
        children: vec![table],
    })
    .key("list")
}

/// The key of the list's row for the item at `path` in window `id`: its focus follows the item while folders open
/// and close above it.
pub(super) fn row_key(id: u64, path: &std::path::Path) -> Key {
    Key::from(format!("row/{id}/{}", path.display()))
}

/// The key of the icon grid's cell for item `index` in window `id`.
pub(super) fn item_key(id: u64, index: usize) -> Key {
    grid::cell_key(id, index)
}

/// Each row of `outline` as the table reads it: its level, whether it is open, and its place among its siblings.
fn tree_items(outline: &[RowSpec]) -> Vec<TreeItem> {
    // the sibling groups met so far, by how many rows each holds, and those open along the current row's parents
    let mut groups: Vec<usize> = Vec::new();
    let mut open: Vec<(u32, usize)> = Vec::new();
    let mut placed = Vec::with_capacity(outline.len());
    for row in outline {
        while open.last().is_some_and(|&(level, _)| level > row.level) {
            open.pop();
        }
        let group = match open.last() {
            Some(&(level, group)) if level == row.level => group,
            _ => {
                groups.push(0);
                open.push((row.level, groups.len() - 1));
                groups.len() - 1
            }
        };
        placed.push((group, groups[group]));
        groups[group] += 1;
    }
    outline
        .iter()
        .zip(placed)
        .map(|(row, (group, position))| TreeItem {
            level: row.level as usize,
            expanded: row.expandable.then_some(row.expanded),
            position,
            siblings: groups[group],
        })
        .collect()
}

/// The icon grid's parameters at the icon view's default settings, and its layout as wide as the viewport.
fn icon_layout(win: &BrowserWindow) -> (IconParams, IconLayout) {
    let settings = IconViewSettings::default();
    let params = IconParams::new(
        settings.icon_size,
        settings.grid_spacing,
        settings.text_size,
        settings.label_on_bottom,
        settings.show_item_info,
        FontMetrics::estimate(f64::from(settings.text_size)),
    );
    let layout = icon::layout(
        &params,
        win.scroll.viewport.width,
        &[SectionSpec {
            title: None,
            count: win.browser.entries().len(),
        }],
        Flow::Leading,
    );
    (params, layout)
}

/// The items in view or near it, the topmost first, and the side of their icons in points.
pub(super) fn shown(win: &BrowserWindow) -> (Vec<usize>, f64) {
    let scroll = &win.scroll;
    if scroll.viewport.height <= 0.0 {
        return (Vec::new(), 0.0);
    }
    // the view's content starts with the part the toolbar row covers at rest, and the list's with its header too
    let lead = under_row(win)
        + match win.view {
            ViewMode::List => list_view::HEADER_HEIGHT,
            _ => 0.0,
        };
    let (top, bottom) = (
        scroll.offset.y - lead - SHOWN_MARGIN,
        scroll.offset.y - lead + scroll.viewport.height + SHOWN_MARGIN,
    );
    match win.view {
        ViewMode::List => {
            let count = win.browser.entries().len();
            let first = (top.max(0.0) / ROW_HEIGHT).floor() as usize;
            let last = ((bottom.max(0.0) / ROW_HEIGHT).ceil() as usize).min(count);
            ((first.min(last)..last).collect(), ROW_ICON)
        }
        ViewMode::Columns => (
            win.columns
                .shown(&win.browser, SHOWN_MARGIN, under_row(win)),
            ROW_ICON,
        ),
        ViewMode::Gallery => gallery::shown(win, SHOWN_MARGIN),
        ViewMode::Icons => {
            let (params, layout) = icon_layout(win);
            let shown = layout
                .cells
                .iter()
                .filter(|c| c.frame.y + c.frame.h >= top && c.frame.y <= bottom)
                .map(|c| c.index)
                .collect();
            (shown, params.icon_size)
        }
    }
}

/// An item's icon or thumbnail, `side` points square.
fn item_image(sources: Vec<ImageSource>, side: f64) -> Element<Message> {
    Element::new(Image {
        sources,
        size: Size::new(side, side),
        fit: ImageFit::Contain,
        radius: 0.0,
        label: None,
    })
}

/// The folder as a grid of icons with their names, at the icon view's default settings, as wide as its viewport.
fn icons(session: &Session, id: u64, win: &BrowserWindow) -> Element<Message> {
    let browser = &win.browser;
    let (params, layout) = icon_layout(win);
    let text_size = params.text_size;
    // the whole lines the layout gives a name; a name past them ends in an ellipsis
    let lines = icon::max_title_lines(&params).floor().max(1.0) as u32;
    let entries = browser.entries();
    let items: Rc<[Item]> = entries
        .iter()
        .map(|e| Item {
            name: e.name.clone(),
            icon: session.icons.icon(e, params.icon_size),
        })
        .collect();
    let rename = win.rename.as_ref().map(|r| {
        (
            r.edit.index(),
            r.edit.text().to_owned(),
            r.revision,
            r.selection.clone(),
        )
    });
    let names = Rc::clone(&items);
    let content = Element::new(IconGrid {
        window: id,
        label: window_title(browser.dir(), &session.places),
        layout: Rc::new(layout),
        params,
        items,
        scroll: win.scroll,
        selection: shown_selection(win),
        ringed: ringed_items(win),
        band: win.band,
        lead: under_row(win),
        name: Box::new(
            move |index, selected| match rename.as_ref().filter(|r| r.0 == index) {
                Some((_, value, revision, selection)) => {
                    name_field(id, value.clone(), *revision, selection.clone())
                }
                None => grid::name(names[index].name.clone(), text_size, selected, Some(lines)),
            },
        ),
        strip: false,
    });
    // the grid starts below the toolbar row, and scrolls under it
    let content = Element::new(Stack {
        axis: Axis::Vertical,
        padding: Insets {
            top: under_row(win),
            ..Insets::ZERO
        },
        spacing: 0.0,
        align: Align::Stretch,
        children: vec![content],
    });
    Element::from(ScrollView {
        content,
        state: win.scroll,
        horizontal: false,
        vertical: true,
        scroller: Scroller::Overlay,
        on_scroll: send(id, Event::Scroll),
    })
    .key("icons")
}

pub(super) fn name_field(
    id: u64,
    value: String,
    revision: u64,
    selection: std::ops::Range<usize>,
) -> Element<Message> {
    Element::new(TextField {
        label: "Name".into(),
        placeholder: String::new(),
        value,
        revision,
        purpose: ContentPurpose::Normal,
        enabled: true,
        on_change: Some(send(id, Event::NameChanged)),
        on_submit: Some(Message::Window(id, Event::NameSubmitted)),
        selection: Some(selection),
        initial: false,
    })
    .key(name_field_key(id))
    .fill()
}

/// The key of the toolbar's pop-up button `popup`, which its menu opens below.
pub(super) fn popup_key(popup: Popup) -> Key {
    Key::from(match popup {
        Popup::View => "popup-view",
        Popup::Tags => "popup-tags",
        Popup::Share => "popup-share",
        Popup::Action => "popup-action",
        Popup::More => "popup-more",
    })
}

/// The folders from the top of the folder shown down to it, each its icon and name, which a click goes to.
fn path_bar(session: &Session, id: u64, win: &BrowserWindow) -> Element<Message> {
    let chain = path_chain(win.browser.dir(), &session.places);
    let last = chain.len().saturating_sub(1);
    let folders: Vec<PathBuf> = chain.iter().map(|(path, _)| path.clone()).collect();
    let flashing = win.springing.flashing();
    let children = chain
        .into_iter()
        .enumerate()
        .flat_map(|(i, (path, name))| {
            let recents = path == crate::recents::location();
            let place = session.places.iter().find(|p| p.path == path);
            let symbol = match place {
                _ if recents => PlaceKind::Recents.symbol(),
                Some(p) => p.kind.symbol(),
                None => FOLDER_SYMBOL,
            };
            let segment = Element::new(PathSegment {
                name,
                symbol: symbol.into(),
                accent: recents,
                // a folder about to spring open under a drag flashes as the folder shown is drawn
                current: i == last || flashing == Some(&path),
                on_press: Message::Window(id, Event::PathSegment(path)),
            })
            .key(Key::Int(i as u64));
            let separator = (i != last).then(|| {
                Element::new(Icon {
                    name: "symbol.chevron.right".into(),
                    size: PATH_SEPARATOR,
                    direction: IconDirection::Semantic,
                    label: None,
                    tint: Some(ColorRole::SecondaryText),
                })
            });
            std::iter::once(segment).chain(separator)
        })
        .collect();
    let bar = Element::new(PathBar {
        children: vec![bar(children)],
    });
    drops::path_bar(session, id, &folders, bar).key("path-bar")
}

/// A running file operation this window started: what it does and how far it came, which a click shows in the
/// progress window, and its Stop button.
fn transfer_row(job: &Job) -> Element<Message> {
    let transfer = &job.transfer;
    let title = transfer.title();
    let progress = Element::new(Progress {
        label: title.clone(),
        value: Some(transfer.fraction()),
    })
    .width(TRANSFER_PROGRESS_WIDTH);
    Element::new(Stack {
        axis: Axis::Horizontal,
        padding: BAR_INSETS,
        spacing: TRANSFER_SPACING,
        align: Align::Center,
        children: vec![
            Element::new(ShowsProgress {
                spacing: TRANSFER_SPACING,
                children: vec![text(title, TextRole::Caption, true).fill(), progress],
            }),
            progress::stop(job, "stop"),
        ],
    })
    .key(Key::from(format!("transfer-{}", transfer.id)))
}

/// A handler that sends `event` of the window `id`.
fn send<T>(id: u64, event: impl Fn(T) -> Event + 'static) -> Box<dyn Fn(T) -> Message> {
    Box::new(move |value| Message::Window(id, event(value)))
}

fn bar(children: Vec<Element<Message>>) -> Element<Message> {
    Element::new(Stack {
        axis: Axis::Horizontal,
        padding: BAR_INSETS,
        spacing: metrics::PATH_ICON_GAP,
        align: Align::Center,
        children,
    })
}

fn text(content: String, role: TextRole, secondary: bool) -> Element<Message> {
    Element::new(Text {
        content: AttributedText::plain(content, TextStyle::new(13.0)),
        role,
        wrap: false,
        secondary,
        selected: false,
    })
}

/// A list cell's text: the name in the label colour and the other columns in the secondary one, and every column of a
/// selected row in the theme's selected control text colour, which reads on the selection's fill.
fn cell_text_element(content: String, column: usize, selected: bool) -> Element<Message> {
    Element::new(Text {
        content: AttributedText::plain(content, TextStyle::new(13.0)),
        role: TextRole::Body,
        wrap: false,
        secondary: column != 0,
        selected,
    })
}

fn column_title(id: ColumnId) -> &'static str {
    match id {
        ColumnId::Name => "Name",
        ColumnId::DateModified => "Date Modified",
        ColumnId::DateCreated => "Date Created",
        ColumnId::Size => "Size",
        ColumnId::Kind => "Kind",
        ColumnId::Label => "Tags",
        ColumnId::Version => "Version",
        ColumnId::Comments => "Comments",
    }
}

/// What the list shows of `e` in column `id`, with dates worded relative to `now`.
fn cell_text(e: &Entry, id: ColumnId, now: std::time::SystemTime) -> String {
    match id {
        ColumnId::Name => e.name.clone(),
        ColumnId::DateModified => e
            .modified
            .map_or_else(|| "--".into(), |t| relative_date(t, now)),
        ColumnId::Size if e.is_dir => "--".into(),
        ColumnId::Size => human_size(e.size),
        ColumnId::Kind => mime::kind_description(&mime::entry_mime(e)),
        _ => String::new(),
    }
}

/// The selection the views show: the browser's, with the folder about to spring open under a drag flashing as selected.
/// The items a context menu is open on: the selection while a menu opened on items is up, else none.
pub(super) fn ringed_items(win: &BrowserWindow) -> Selection {
    match win.context.as_ref().and_then(|menu| menu.ring) {
        Some(super::context::Ringed::Items) => selection(win.browser.selection()),
        _ => Selection::default(),
    }
}

pub(super) fn shown_selection(win: &BrowserWindow) -> Selection {
    let selected = win.browser.selection();
    let flashing = win
        .springing
        .flashing()
        .and_then(|path| win.browser.entries().iter().position(|e| &e.path == path))
        .filter(|&i| !selected.contains(i));
    match flashing {
        Some(index) => {
            let mut flashed = selected.clone();
            flashed.restore(
                selected.indices().chain([index]).collect::<Vec<_>>(),
                selected.anchor(),
                selected.focus(),
            );
            selection(&flashed)
        }
        None => selection(selected),
    }
}

/// The browser's selection as the list takes it: its items in runs.
fn selection(selected: &crate::Selection) -> Selection {
    let mut ranges: Vec<std::ops::Range<usize>> = Vec::new();
    for i in selected.indices() {
        match ranges.last_mut() {
            Some(run) if run.end == i => run.end += 1,
            _ => ranges.push(i..i + 1),
        }
    }
    Selection::new(ranges, selected.focus(), selected.anchor())
}
