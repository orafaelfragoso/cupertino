//! The open panel's window: the family's open and save panel (Kit's `files::panel::OpenSavePanel`) around a
//! `FileList` of the [`crate::chooser::Chooser`]'s rows, and what the window keeps of it between frames (the rows as
//! the list draws them, their presses and the items' icons).

use std::rc::Rc;
use std::sync::Arc;

use cupertino_kit::controls::{
    Column, ColumnWidth, Selection, SidebarItem, SidebarSection, TreeItem,
};
use cupertino_kit::core::{CommandRegistry, Key, Shortcut};
use cupertino_kit::file_icons::FileIcons;
use cupertino_kit::files::entry::Entry;
use cupertino_kit::files::grid::ItemIcon;
use cupertino_kit::files::item_icons::ItemIcons;
use cupertino_kit::files::list::{FileList, Presses, ROW_ICON};
use cupertino_kit::files::mime::{entry_mime, kind_description};
use cupertino_kit::files::panel::{OpenSavePanel, PanelEvent};
use cupertino_kit::files::places::PlaceGroup;
use cupertino_kit::files::ViewEvent;
use cupertino_kit::geometry::Size;
use cupertino_kit::platform::{DecorationMode, Keysym, Mods};
use cupertino_kit::prelude::*;

use super::{Message, APP_ID};
use crate::chooser::{Chooser, Event};

/// The panel's size when it opens, and the smallest it gets: the family's file chooser's.
const SIZE: Size = Size::new(720.0, 460.0);
const MIN_SIZE: Size = Size::new(560.0, 340.0);
/// The width of the list's Kind column.
const KIND_WIDTH: f64 = 150.0;
const RETURN: Keysym = Keysym(0xff0d);
const ESCAPE: Keysym = Keysym(0xff1b);
/// Tells the list's rows from those of the settings window.
const LIST_WINDOW: u64 = 1;

fn window_key() -> WindowKey {
    WindowKey("chooser".into())
}

/// What the window keeps of the open panel: its rows as the list draws them, taken at each listing, the presses its
/// rows share, and the items' icons once read.
#[derive(Default)]
pub(super) struct PanelRows {
    items: Rc<[Entry]>,
    /// The listing the rows were taken at; `None` without a panel.
    listing: Option<u64>,
    presses: Presses,
    icons: Option<Rc<ItemIcons>>,
}

impl PanelRows {
    /// Follow the panel after an update: its rows when it took a listing, fresh presses when it opened, and the
    /// items' icons once they were read.
    pub(super) fn follow(&mut self, chooser: Option<&Chooser>, icons: Option<FileIcons>) {
        if let Some(icons) = icons {
            self.icons = Some(Rc::new(ItemIcons::new(icons, None, Arc::new(|| {}))));
        }
        match chooser {
            None if self.listing.is_some() => {
                self.items = Rc::from([]);
                self.listing = None;
                self.presses = Presses::default();
            }
            None => {}
            Some(chooser) if self.listing != Some(chooser.listings()) => {
                self.items = Rc::from(chooser.entries());
                self.listing = Some(chooser.listings());
            }
            Some(_) => {}
        }
    }
}

/// Return and Escape, wherever the focus is.
fn keys() -> CommandRegistry<Message> {
    CommandRegistry::new()
        .with(
            Shortcut {
                mods: Mods::NONE,
                keysym: RETURN,
            },
            Message::Chooser(Event::Return),
        )
        .with(
            Shortcut {
                mods: Mods::NONE,
                keysym: ESCAPE,
            },
            Message::Chooser(Event::Cancel),
        )
}

/// The places as the sidebar's one section.
fn sections(chooser: &Chooser) -> Vec<SidebarSection> {
    vec![SidebarSection {
        title: Some(PlaceGroup::Favorites.title().to_owned()),
        items: (chooser.places().iter())
            .map(|p| SidebarItem {
                key: Key::from(p.path.to_string_lossy().into_owned()),
                label: p.name.clone(),
                icon: Some(p.kind.symbol().to_owned()),
                description: None,
                accessory: None,
                busy: false,
            })
            .collect(),
    }]
}

/// The rows as the family's list: the names and the kinds, each with its item's icon; a row selects and opens.
fn list(chooser: &Chooser, rows: &PanelRows) -> FileList<Message> {
    let icon: ItemIcon = match &rows.icons {
        Some(icons) => {
            let icons = Rc::clone(icons);
            Rc::new(move |entry| icons.icon(entry, ROW_ICON))
        }
        None => Rc::new(|_| Vec::new()),
    };
    let count = rows.items.len();
    FileList {
        window: LIST_WINDOW,
        label: chooser
            .dir()
            .file_name()
            .map_or_else(|| "/".to_owned(), |n| n.to_string_lossy().into_owned()),
        items: Rc::clone(&rows.items),
        outline: (0..count)
            .map(|position| TreeItem {
                level: 0,
                expanded: None,
                position,
                siblings: count,
            })
            .collect(),
        columns: vec![
            Column {
                title: "Name".into(),
                width: ColumnWidth::Fill,
            },
            Column {
                title: "Kind".into(),
                width: ColumnWidth::Fixed(KIND_WIDTH),
            },
        ],
        text: Rc::new(|entry, _| kind_description(&entry_mime(entry))),
        sort: None,
        icon,
        multiple: false,
        scroll: chooser.scroll(),
        selection: chooser.selection().clone(),
        ringed: Selection::default(),
        band: None,
        lead: 0.0,
        name_field: None,
        presses: rows.presses.clone(),
        on_event: Rc::new(|event| match event {
            ViewEvent::Select(_) | ViewEvent::Open(_) | ViewEvent::Scroll(_) => {
                Some(Message::Chooser(Event::List(event)))
            }
            _ => None,
        }),
    }
}

/// The panel's window, beside the settings window.
pub(super) fn window(chooser: &Chooser, rows: &PanelRows) -> Window<Message> {
    let dir = chooser.dir();
    let panel = OpenSavePanel {
        places: sections(chooser),
        place: chooser.place(),
        places_scroll: chooser.places_scroll(),
        split: chooser.split(),
        folder: dir.display().to_string(),
        enclosing: dir.parent().is_some(),
        unreadable: chooser.unreadable(),
        list: list(chooser, rows),
        save_as: None,
        filters: Vec::new(),
        filter: None,
        choices: Vec::new(),
        accept_label: chooser.accept_label().to_owned(),
        can_accept: chooser.choice().is_some(),
        replace: None,
        on_event: Rc::new(|event: PanelEvent| Message::Chooser(Event::Panel(event))),
    };
    Window {
        key: window_key(),
        spec: WindowSpec {
            title: chooser.title().to_owned(),
            app_id: APP_ID.into(),
            size: SIZE,
            min_size: Some(MIN_SIZE),
            max_size: None,
            decorations: DecorationMode::None,
            foreign_parent: None,
            input_region: None,
        },
        shortcuts: keys(),
        on_close: Some(Message::Chooser(Event::Cancel)),
        background: WindowBackground::Theme,
        popup: None,
        menus: None,
        content: Element::from(panel),
    }
}
