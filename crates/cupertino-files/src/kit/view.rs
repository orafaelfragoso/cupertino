//! What a browser window shows for its [`BrowserWindow`].

use std::collections::BTreeSet;
use std::rc::Rc;

use cupertino_menu_model::{ActionName, MenuBar};

use cupertino_kit::controls::{
    Column, ColumnWidth, ContextMenu, Dialog, DialogAction, DialogButton, DialogKind, Modal,
    NavigationStack, ScrollView, Scroller, SecureField, Selection, Sidebar, SidebarAccessory,
    SidebarItem, SidebarSection, SortOrder, Spacer, SplitView, Stack, TableView, Text, TextField,
    TextRole,
};
use cupertino_kit::core::{CommandRegistry, Key, Menus, Shortcut, WindowBackground};
use cupertino_kit::geometry::{Insets, Size};
use cupertino_kit::layout::{Align, Axis};
use cupertino_kit::platform::{DecorationMode, Keysym, Mods, WindowSpec};
use cupertino_kit::prelude::*;
use cupertino_kit::text::SecureBuffer;
use cupertino_kit::text::{AttributedText, ContentPurpose, TextStyle};

use super::context::Secondary;
use super::grid::{self, IconGrid, Item};
use super::search::{no_results, SearchSlot};
use super::{window_key, BrowserWindow, Event, Message, Session, Unlock, COLUMNS};
use crate::layout::icon::{self, Flow, FontMetrics, IconParams, SectionSpec};
use crate::menu_bar::MenuAction;
use crate::metrics::{list_view, sidebar, window as window_metrics};
use crate::places::{window_title, PlaceGroup};
use crate::places::{PlaceKind, Volume};
use crate::render::{human_size, relative_date};
use crate::settings::{default_columns, ColumnId, IconViewSettings};
use crate::udisks::Passphrase;
use crate::window::{metrics, path_chain, status_text, ViewMode};
use crate::{mime, Entry, SortKey};

pub(super) const SIDEBAR_WIDTH: f64 = sidebar::DEFAULT_WIDTH;
/// A list row: the icon size over the row padding, as the list view's rows.
const ROW_HEIGHT: f64 = list_view::ICON_SIZE_DEFAULT as f64 + list_view::ROW_PADDING;
/// The key of the field that edits an item's name in its row of window `id`; one per window, as focus goes to the
/// first window holding the key.
pub(super) fn name_field_key(id: u64) -> Key {
    Key::from(format!("name-field/{id}"))
}
const ESCAPE: Keysym = Keysym(0xff1b);
const DOWN: Keysym = Keysym(0xff54);
/// The icons of a folder and of a file in the grid.
const FOLDER_SYMBOL: &str = "symbol.folder";
const DOCUMENT_SYMBOL: &str = "symbol.document";
/// The symbol of a mounted removable file system's eject button.
const EJECT_SYMBOL: &str = "symbol.eject";
const BAR_INSETS: Insets = Insets {
    top: 4.0,
    left: metrics::PATH_BAR_INSET,
    bottom: 4.0,
    right: metrics::PATH_BAR_INSET,
};

pub(super) fn window(session: &Session, id: u64, win: &BrowserWindow) -> Window<Message> {
    let browser = &win.browser;
    let title = window_title(browser.dir(), &session.places);
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
            decorations: DecorationMode::Preferred,
            foreign_parent: None,
            input_region: None,
        },
        shortcuts: shortcuts(id, win, &bar, &disabled),
        on_close: Some(Message::Window(id, Event::Close)),
        background: WindowBackground::Theme,
        popup: None,
        menus: Some(Menus {
            bar,
            disabled,
            on_activate: send(id, Event::Menu),
        }),
        content: Element::from(Modal {
            content: Element::from(SplitView {
                label: "Sidebar".into(),
                axis: Axis::Horizontal,
                leading: places(session, id, win),
                trailing: content(session, id, win),
                layout: win.split,
                min_leading: metrics::SIDEBAR_MIN,
                min_trailing: metrics::BROWSER_MIN.0,
                on_change: Some(send(id, Event::Split)),
            })
            .key("browser"),
            dialog: win.unlock.as_ref().map(|unlock| unlock_sheet(id, unlock)),
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
    let escape = Shortcut {
        mods: Mods::NONE,
        keysym: ESCAPE,
    };
    if win.rename.is_some() {
        registry.insert(escape, Message::Window(id, Event::NameCancelled));
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
    let sections = groups
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
    let selected = shown
        .iter()
        .position(|&i| session.places[i].folder() == Some(dir));
    Element::from(Sidebar {
        label: "Sidebar".into(),
        sections,
        selected,
        scroll: win.sidebar_scroll,
        on_scroll: send(id, Event::SidebarScroll),
        on_select: Some(send(id, move |row| Event::Place(shown[row]))),
        on_accessory: Some(send(id, Event::Accessory)),
    })
    .key("places")
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
        icon: Some(place.kind.symbol().into()),
        description,
        accessory: place.ejectable().then(|| SidebarAccessory {
            symbol: EJECT_SYMBOL.into(),
            label: format!("Eject {}", place.name),
        }),
        busy: place.kind == PlaceKind::Removable(Volume::Busy),
    }
}

/// The sheet that asks for the passphrase of a locked container.
fn unlock_sheet(id: u64, unlock: &Unlock) -> Dialog<Message> {
    Dialog {
        kind: DialogKind::Dialog,
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

fn content(session: &Session, id: u64, win: &BrowserWindow) -> Element<Message> {
    let history = win.browser.history();
    let folder = match (win.view, win.browser.no_results()) {
        (ViewMode::List, false) => list(session, id, win),
        (ViewMode::List, true) => no_results(list(session, id, win), list_view::HEADER_HEIGHT),
        (ViewMode::Icons, false) => icons(session, id, win),
        (ViewMode::Icons, true) => no_results(icons(session, id, win), 0.0),
    };
    // a secondary press anywhere in the folder opens a context menu: an item's cell says it was on the item first
    let folder = Element::new(Secondary {
        message: Message::Window(id, Event::SecondaryPress(None)),
        children: vec![folder.fill()],
    })
    .fill();
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
    })
    .key("navigation")
    .fill()];
    if win.path_bar {
        children.push(path_bar(session, win));
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
    Element::new(Stack {
        axis: Axis::Vertical,
        padding: Insets::ZERO,
        spacing: 0.0,
        align: Align::Stretch,
        children,
    })
    .fill()
}

/// The folder's title, which takes the room the other items leave, then the search field, which grows into it while it
/// has the keyboard or text ([`crate::search_field`]).
fn toolbar(session: &Session, id: u64, win: &BrowserWindow) -> Vec<Element<Message>> {
    let field = Element::new(TextField {
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
    })
    .key("search")
    .width(win.search_width.width());
    // a stack that fills the room gives the title its width, so a long one ends in an ellipsis, and centres it
    let title = Element::new(Stack {
        axis: Axis::Vertical,
        padding: Insets::ZERO,
        spacing: 0.0,
        align: Align::Stretch,
        children: vec![
            Element::new(Spacer { min: 0.0 }),
            text(
                window_title(win.browser.dir(), &session.places),
                TextRole::Section,
                false,
            )
            .key("title"),
            Element::new(Spacer { min: 0.0 }),
        ],
    })
    .fill();
    vec![
        title,
        Element::new(SearchSlot {
            window: id,
            animating: win.search_width.animating(),
            children: vec![field],
        })
        .key("search-slot")
        .width(win.search_width.width()),
    ]
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
    let rows: Vec<[String; 4]> = browser
        .entries()
        .iter()
        .map(|e| COLUMNS.map(|id| cell_text(e, id, now)))
        .collect();
    let names: Vec<String> = browser.entries().iter().map(|e| e.name.clone()).collect();
    let label = window_title(browser.dir(), &session.places);
    let rename = win.rename.as_ref().map(|r| {
        (
            r.edit.index(),
            r.edit.text().to_owned(),
            r.revision,
            r.selection.clone(),
        )
    });
    Element::from(TableView {
        label,
        columns,
        sort,
        on_sort: Some(send(id, Event::Sort)),
        count: rows.len(),
        row_height: ROW_HEIGHT,
        scroll: win.scroll,
        selection: selection(browser.selection()),
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
                    text(
                        rows[row][column].clone(),
                        TextRole::Body,
                        column != 0 && !selected,
                    ),
                    Element::new(Spacer { min: 0.0 }),
                ],
            })
            .fill();
            Element::new(Secondary {
                message: Message::Window(id, Event::SecondaryPress(Some(row))),
                children: vec![cell],
            })
            .fill()
        }),
        type_select: Some(Box::new(move |row| names[row].clone())),
        on_scroll: send(id, Event::Scroll),
        on_select: Some(send(id, Event::Select)),
        on_activate: Some(send(id, Event::Open)),
    })
    .key("items")
}

/// The folder as a grid of icons with their names, at the icon view's default settings, as wide as its viewport.
fn icons(session: &Session, id: u64, win: &BrowserWindow) -> Element<Message> {
    let browser = &win.browser;
    let settings = IconViewSettings::default();
    let text_size = f64::from(settings.text_size);
    let params = IconParams::new(
        settings.icon_size,
        settings.grid_spacing,
        settings.text_size,
        settings.label_on_bottom,
        settings.show_item_info,
        FontMetrics::estimate(text_size),
    );
    let entries = browser.entries();
    let layout = icon::layout(
        &params,
        win.scroll.viewport.width,
        &[SectionSpec {
            title: None,
            count: entries.len(),
        }],
        Flow::Leading,
    );
    let items: Rc<[Item]> = entries
        .iter()
        .map(|e| Item {
            name: e.name.clone(),
            symbol: if e.is_dir {
                FOLDER_SYMBOL
            } else {
                DOCUMENT_SYMBOL
            },
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
        selection: selection(browser.selection()),
        name: Box::new(
            move |index, selected| match rename.as_ref().filter(|r| r.0 == index) {
                Some((_, value, revision, selection)) => {
                    name_field(id, value.clone(), *revision, selection.clone())
                }
                None => grid::name(names[index].name.clone(), text_size, selected),
            },
        ),
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

fn name_field(
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

fn path_bar(session: &Session, win: &BrowserWindow) -> Element<Message> {
    let chain = path_chain(win.browser.dir(), &session.places);
    let last = chain.len().saturating_sub(1);
    let children = chain
        .into_iter()
        .enumerate()
        .flat_map(|(i, (_, name))| {
            let segment = text(name, TextRole::Caption, i != last).key(Key::Int(i as u64));
            let separator = (i != last).then(|| text("›".into(), TextRole::Caption, true));
            std::iter::once(segment).chain(separator)
        })
        .collect();
    bar(children).key("path-bar")
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
