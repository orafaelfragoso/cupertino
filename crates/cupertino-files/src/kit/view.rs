//! What a browser window shows for its [`BrowserWindow`].

use cupertino_kit::controls::{
    Column, ColumnWidth, NavigationStack, Selection, Sidebar, SidebarItem, SidebarSection,
    SortOrder, Spacer, SplitView, Stack, TableView, Text, TextField, TextRole,
};
use cupertino_kit::core::{CommandRegistry, Key, Menus, Shortcut, WindowBackground};
use cupertino_kit::geometry::{Insets, Size};
use cupertino_kit::layout::{Align, Axis};
use cupertino_kit::platform::{DecorationMode, Keysym, Mods, WindowSpec};
use cupertino_kit::prelude::*;
use cupertino_kit::text::{AttributedText, ContentPurpose, TextStyle};

use super::{window_key, BrowserWindow, Event, Message, Session, COLUMNS};
use crate::metrics::{list_view, sidebar, window as window_metrics};
use crate::places::{window_title, PlaceGroup};
use crate::render::{human_size, relative_date};
use crate::settings::{default_columns, ColumnId};
use crate::window::{metrics, path_chain, status_text};
use crate::{mime, Entry, SortKey};

pub(super) const SIDEBAR_WIDTH: f64 = sidebar::DEFAULT_WIDTH;
/// A list row: the icon size over the row padding, as the list view's rows.
const ROW_HEIGHT: f64 = list_view::ICON_SIZE_DEFAULT as f64 + list_view::ROW_PADDING;
/// The key of the field that edits an item's name in its row.
pub(super) const NAME_FIELD: &str = "name-field";
const ESCAPE: Keysym = Keysym(0xff1b);
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
        shortcuts: shortcuts(id, win),
        on_close: Some(Message::Window(id, Event::Close)),
        background: WindowBackground::Theme,
        popup: None,
        menus: Some(Menus {
            bar: crate::menu_bar::menu_bar(&state),
            disabled: BrowserWindow::disabled(&state),
            on_activate: send(id, Event::Menu),
        }),
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
    }
}

/// Escape ends a rename without renaming, once the field has passed the key on.
fn shortcuts(id: u64, win: &BrowserWindow) -> CommandRegistry<Message> {
    let registry = CommandRegistry::new();
    match win.rename {
        Some(_) => registry.with(
            Shortcut {
                mods: Mods::NONE,
                keysym: ESCAPE,
            },
            Message::Window(id, Event::NameCancelled),
        ),
        None => registry,
    }
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
                .map(|&i| &session.places[i])
                .filter(|p| p.group == group)
                .map(|p| SidebarItem {
                    key: Key::from(format!("place-{}", p.name)),
                    label: p.name.clone(),
                    icon: Some(p.kind.symbol().into()),
                    description: None,
                    accessory: None,
                    busy: false,
                })
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
        on_accessory: None,
    })
    .key("places")
}

fn content(session: &Session, id: u64, win: &BrowserWindow) -> Element<Message> {
    let history = win.browser.history();
    let mut children = vec![Element::from(NavigationStack {
        content: list(session, id, win),
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

fn toolbar(session: &Session, id: u64, win: &BrowserWindow) -> Vec<Element<Message>> {
    vec![
        text(
            window_title(win.browser.dir(), &session.places),
            TextRole::Section,
            false,
        )
        .key("title"),
        Element::new(Spacer {
            min: metrics::TITLE_TRAILING_GAP,
        }),
        Element::new(TextField {
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
        .width(window_metrics::TOOLBAR_SEARCH_FIELD.0),
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
            Element::new(Stack {
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
            .fill()
        }),
        type_select: Some(Box::new(move |row| names[row].clone())),
        on_scroll: send(id, Event::Scroll),
        on_select: Some(send(id, Event::Select)),
        on_activate: Some(send(id, Event::Open)),
    })
    .key("items")
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
    .key(NAME_FIELD)
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
