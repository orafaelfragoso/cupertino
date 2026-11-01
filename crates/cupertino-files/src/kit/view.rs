//! What the window shows for its [`Model`].

use cupertino_kit::controls::{
    Column, ColumnWidth, NavigationStack, Selection, Sidebar, SidebarItem, SidebarSection,
    SortOrder, Spacer, SplitView, Stack, TableView, Text, TextField, TextRole,
};
use cupertino_kit::core::{CommandRegistry, Key, Menus, WindowBackground, WindowKey};
use cupertino_kit::geometry::{Insets, Size};
use cupertino_kit::layout::{Align, Axis};
use cupertino_kit::platform::{DecorationMode, WindowSpec};
use cupertino_kit::prelude::*;
use cupertino_kit::text::{AttributedText, ContentPurpose, TextStyle};

use super::{Message, Model, COLUMNS};
use crate::metrics::{list_view, sidebar, window as window_metrics};
use crate::places::{window_title, PlaceGroup};
use crate::render::{human_size, relative_date};
use crate::settings::{default_columns, ColumnId};
use crate::window::{metrics, path_chain, status_text};
use crate::{mime, Entry, SortKey};

pub(super) const SIDEBAR_WIDTH: f64 = sidebar::DEFAULT_WIDTH;
/// A list row: the icon size over the row padding, as the list view's rows.
const ROW_HEIGHT: f64 = list_view::ICON_SIZE_DEFAULT as f64 + list_view::ROW_PADDING;
const BAR_INSETS: Insets = Insets {
    top: 4.0,
    left: metrics::PATH_BAR_INSET,
    bottom: 4.0,
    right: metrics::PATH_BAR_INSET,
};

pub(super) fn window(model: &Model) -> Window<Message> {
    let browser = &model.browser;
    let title = window_title(browser.dir(), &model.places);
    let state = model.menu_state();
    let (width, height) = window_metrics::DEFAULT_SIZE;
    Window {
        key: WindowKey(Key::from("browser")),
        spec: WindowSpec {
            title,
            app_id: super::APP_ID.into(),
            size: Size::new(width, height),
            min_size: Some(Size::new(metrics::WINDOW_MIN.0, metrics::WINDOW_MIN.1)),
            max_size: None,
            decorations: DecorationMode::Preferred,
            foreign_parent: None,
        },
        shortcuts: CommandRegistry::new(),
        on_close: None,
        background: WindowBackground::Theme,
        popup: None,
        menus: Some(Menus {
            bar: crate::menu_bar::menu_bar(&state),
            disabled: model.disabled(&state),
            on_activate: Box::new(Message::Menu),
        }),
        content: Element::from(SplitView {
            label: "Sidebar".into(),
            axis: Axis::Horizontal,
            leading: places(model),
            trailing: content(model),
            layout: model.split,
            min_leading: metrics::SIDEBAR_MIN,
            min_trailing: metrics::BROWSER_MIN.0,
            on_change: Some(Box::new(Message::Split)),
        })
        .key("browser"),
    }
}

fn places(model: &Model) -> Element<Message> {
    let dir = model.browser.dir();
    let groups = [PlaceGroup::Favorites, PlaceGroup::Locations];
    // the sidebar counts its items across its sections, in this order
    let shown: Vec<usize> = groups
        .iter()
        .flat_map(|&group| (0..model.places.len()).filter(move |&i| model.places[i].group == group))
        .collect();
    let sections = groups
        .into_iter()
        .map(|group| SidebarSection {
            title: Some(group.title().into()),
            items: shown
                .iter()
                .map(|&i| &model.places[i])
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
        .position(|&i| model.places[i].folder() == Some(dir));
    Element::from(Sidebar {
        label: "Sidebar".into(),
        sections,
        selected,
        scroll: model.sidebar_scroll,
        on_scroll: Box::new(Message::SidebarScroll),
        on_select: Some(Box::new(move |row| Message::Place(shown[row]))),
        on_accessory: None,
    })
    .key("places")
}

fn content(model: &Model) -> Element<Message> {
    let history = model.browser.history();
    let mut children = vec![Element::from(NavigationStack {
        content: list(model),
        toolbar: toolbar(model),
        can_go_back: history.back_target().is_some(),
        can_go_forward: history.forward_target().is_some(),
        on_navigate: Some(Box::new(Message::Navigate)),
    })
    .key("navigation")
    .fill()];
    if model.path_bar {
        children.push(path_bar(model));
    }
    if model.status_bar {
        children.push(
            bar(vec![
                Element::new(Spacer { min: 0.0 }),
                text(
                    status_text(model.browser.item_count(), model.available),
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

fn toolbar(model: &Model) -> Vec<Element<Message>> {
    vec![
        text(
            window_title(model.browser.dir(), &model.places),
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
            value: model.search.clone(),
            revision: model.search_revision,
            purpose: ContentPurpose::Normal,
            enabled: true,
            on_change: Some(Box::new(Message::Search)),
            on_submit: None,
            selection: None,
            initial: false,
        })
        .key("search")
        .width(window_metrics::TOOLBAR_SEARCH_FIELD.0),
    ]
}

fn list(model: &Model) -> Element<Message> {
    let browser = &model.browser;
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
    let now = model.now;
    let rows: Vec<[String; 4]> = browser
        .entries()
        .iter()
        .map(|e| COLUMNS.map(|id| cell_text(e, id, now)))
        .collect();
    let names: Vec<String> = browser.entries().iter().map(|e| e.name.clone()).collect();
    let label = window_title(browser.dir(), &model.places);
    Element::from(TableView {
        label,
        columns,
        sort,
        on_sort: Some(Box::new(Message::Sort)),
        count: rows.len(),
        row_height: ROW_HEIGHT,
        scroll: model.scroll,
        selection: selection(browser.selection()),
        multiple: true,
        cell: Box::new(move |row, column, selected| {
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
        on_scroll: Box::new(Message::Scroll),
        on_select: Some(Box::new(Message::Select)),
        on_activate: Some(Box::new(Message::Open)),
    })
    .key("items")
}

fn path_bar(model: &Model) -> Element<Message> {
    let chain = path_chain(model.browser.dir(), &model.places);
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
