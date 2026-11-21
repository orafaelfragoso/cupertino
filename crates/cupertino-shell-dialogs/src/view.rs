//! The dialogs' windows in CupertinoKit: a file chooser is the family's open and save panel
//! (`kit::files::panel::OpenSavePanel`) around a `FileList` of the chooser's rows; an app chooser is a prompt window
//! that lists the applications under the question.

use std::rc::Rc;

use cupertino_kit::controls::{
    AppIcon, Button, ButtonRole, ButtonStyle, Column, ColumnWidth, Icon, ListView, Selection,
    SidebarItem, SidebarSection, TextRole, TreeItem, WindowDragArea, PANEL_WINDOW_BACKGROUND,
    SIDEBAR_TOP,
};
use cupertino_kit::core::{CommandRegistry, Key, Shortcut};
use cupertino_kit::files::grid::ItemIcon;
use cupertino_kit::files::item_icons::ItemIcons;
use cupertino_kit::files::list::{FileList, ROW_ICON};
use cupertino_kit::files::mime::{entry_mime, kind_description};
use cupertino_kit::files::panel::{OpenSavePanel, PanelChoice, PanelEvent, SaveAs};
use cupertino_kit::files::places::{Place, PlaceGroup};
use cupertino_kit::files::ViewEvent;
use cupertino_kit::geometry::{Insets, Size};
use cupertino_kit::platform::{DecorationMode, ForeignParent, Keysym, Mods};
use cupertino_kit::prelude::*;
use cupertino_kit::text::{AttributedText, TextStyle};

use crate::apps::{AppDialog, AppEvent};
use crate::ask::Kind;
use crate::chooser::Chooser;
use crate::component::Message;
use crate::file::{FileDialog, FileEvent};

/// The app id of the dialogs' windows.
pub const APP_ID: &str = "org.cupertino.Dialogs";
/// A file chooser's size when it opens, and the smallest it gets.
pub const FILE_SIZE: Size = Size::new(720.0, 460.0);
const FILE_MIN_SIZE: Size = Size::new(560.0, 340.0);
/// The width of the list's Kind column.
const KIND_WIDTH: f64 = 150.0;
/// An app chooser: the row of the window's buttons ([`SIDEBAR_TOP`]) over its content.
pub const APPS_SIZE: Size = Size::new(480.0, 360.0 + SIDEBAR_TOP);
const APPS_MIN_SIZE: Size = Size::new(360.0, 240.0 + SIDEBAR_TOP);
/// A prompt window's title bar is only its control buttons, over the top of its glass panel.
const PROMPT_DECORATIONS: DecorationMode = DecorationMode::Unified;
/// The app chooser's rows and their icons.
const ROW_HEIGHT: f64 = 24.0;
const APP_ICON: f64 = 16.0;
const GAP: f64 = 8.0;
/// Around the app chooser's question and its buttons; an approximate design value.
const BAR_INSETS: Insets = Insets {
    top: 12.0,
    left: 16.0,
    bottom: 12.0,
    right: 16.0,
};
/// Characters of a file name or URI the app chooser's question shows before it ends it with an ellipsis.
const TARGET_CHARS: usize = 80;
const TEXT_SIZE: f32 = 13.0;
const RETURN: Keysym = Keysym(0xff0d);
const ESCAPE: Keysym = Keysym(0xff1b);

/// The window of another application a request names as its parent (`wayland:HANDLE`): the dialog opens as its
/// child, and as its modal dialog when `modal`. Another kind of parent, or none, gives none.
pub fn foreign_parent(parent_window: &str, modal: bool) -> Option<ForeignParent> {
    let handle = parent_window.strip_prefix("wayland:")?;
    (!handle.is_empty()).then(|| ForeignParent {
        handle: handle.to_owned(),
        modal,
    })
}

/// Return and Escape, wherever the focus is.
fn keys(on_return: Message, on_escape: Message) -> CommandRegistry<Message> {
    CommandRegistry::new()
        .with(
            Shortcut {
                mods: Mods::NONE,
                keysym: RETURN,
            },
            on_return,
        )
        .with(
            Shortcut {
                mods: Mods::NONE,
                keysym: ESCAPE,
            },
            on_escape,
        )
}

/// The places as the sidebar's one section.
fn sections(places: &[Place]) -> Vec<SidebarSection> {
    vec![SidebarSection {
        title: Some(PlaceGroup::Favorites.title().to_owned()),
        items: places
            .iter()
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

/// The chooser's rows as the family's list: the names and the kinds, each with its item's icon; no row opens in place.
fn list(d: &FileDialog, icons: Option<&Rc<ItemIcons>>) -> FileList<Message> {
    let id = d.id;
    let multiple = d.chooser().ask().multiple;
    let icon: ItemIcon = match icons {
        Some(icons) => {
            let icons = Rc::clone(icons);
            Rc::new(move |entry| icons.icon(entry, ROW_ICON))
        }
        None => Rc::new(|_| Vec::new()),
    };
    let count = d.items().len();
    FileList {
        window: id,
        label: d
            .chooser()
            .picker()
            .dir()
            .file_name()
            .map_or_else(|| "/".to_owned(), |n| n.to_string_lossy().into_owned()),
        items: Rc::clone(d.items()),
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
        multiple,
        scroll: d.scroll(),
        selection: d.selection().clone(),
        ringed: Selection::default(),
        band: d.band(),
        lead: 0.0,
        name_field: None,
        presses: d.presses().clone(),
        // the rows open and select; a chooser has no renaming, dragging, menu or sorting, and only a chooser of
        // several files draws the band
        on_event: Rc::new(move |event| match event {
            ViewEvent::Select(_) | ViewEvent::Open(_) | ViewEvent::Scroll(_) => {
                Some(Message::File(id, FileEvent::List(event)))
            }
            ViewEvent::Band(_) if multiple => Some(Message::File(id, FileEvent::List(event))),
            _ => None,
        }),
    }
}

/// The panel's choices, at their values.
fn choices(chooser: &Chooser) -> Vec<PanelChoice> {
    (chooser.ask().choices.iter().enumerate())
        .map(|(i, choice)| {
            let value = chooser.choice(i).unwrap_or_default();
            if choice.options.is_empty() {
                PanelChoice::Check {
                    label: choice.label.clone(),
                    checked: value == "true",
                }
            } else {
                PanelChoice::Pick {
                    label: choice.label.clone(),
                    options: choice.options.iter().map(|(_, l)| l.clone()).collect(),
                    selected: choice.options.iter().position(|(o, _)| o == value),
                }
            }
        })
        .collect()
}

/// A file chooser's window: the family's panel, a child of the app's window.
pub fn file_window(
    d: &FileDialog,
    places: &[Place],
    icons: Option<&Rc<ItemIcons>>,
) -> Window<Message> {
    let id = d.id;
    let chooser = d.chooser();
    let ask = chooser.ask();
    let dir = chooser.picker().dir();
    let panel = OpenSavePanel {
        places: sections(places),
        place: places.iter().position(|p| p.path == dir),
        places_scroll: d.places_scroll(),
        split: d.split(),
        folder: dir.display().to_string(),
        enclosing: dir.parent().is_some(),
        unreadable: chooser.picker().unreadable(),
        list: list(d, icons),
        save_as: (ask.kind == Kind::Save).then(|| SaveAs {
            name: chooser.name().to_owned(),
            revision: d.name_revision(),
            stem: Chooser::stem_len(chooser.name()),
        }),
        filters: ask.filters.iter().map(|f| f.name.clone()).collect(),
        filter: chooser.filter(),
        choices: choices(chooser),
        accept_label: chooser.accept_label(),
        can_accept: chooser.accept() != crate::chooser::Accept::Nothing,
        replace: d.replacing().map(|path| {
            path.file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
        }),
        on_event: Rc::new(move |event: PanelEvent| Message::File(id, FileEvent::Panel(event))),
    };
    let title = if ask.title.is_empty() {
        chooser.accept_label()
    } else {
        ask.title.clone()
    };
    Window {
        key: d.window_key(),
        spec: WindowSpec {
            title,
            app_id: APP_ID.into(),
            size: FILE_SIZE,
            min_size: Some(FILE_MIN_SIZE),
            max_size: None,
            decorations: DecorationMode::None,
            foreign_parent: foreign_parent(&ask.parent_window, ask.modal),
            input_region: None,
        },
        shortcuts: keys(
            Message::File(id, FileEvent::Return),
            Message::File(id, FileEvent::Cancel),
        ),
        on_close: Some(Message::File(id, FileEvent::Cancel)),
        background: WindowBackground::Theme,
        popup: None,
        menus: None,
        content: Element::from(panel),
    }
}

fn text(content: String, wrap: bool, selected: bool) -> Element<Message> {
    Element::new(Text {
        content: AttributedText::plain(content, TextStyle::new(TEXT_SIZE)),
        role: TextRole::Body,
        wrap,
        secondary: false,
        selected,
    })
}

fn button(label: &str, role: ButtonRole, enabled: bool, on_press: Message) -> Element<Message> {
    Element::new(Button {
        label: label.to_owned(),
        role,
        style: ButtonStyle::Bordered,
        enabled,
        busy: false,
        on_press: Some(on_press),
    })
}

/// `target` as the app chooser's question names it: at most [`TARGET_CHARS`] characters, the rest left out.
pub fn shortened(target: &str) -> String {
    match target.char_indices().nth(TARGET_CHARS) {
        Some((end, _)) => format!("{}…", &target[..end]),
        None => target.to_owned(),
    }
}

/// The app chooser's question.
pub fn question(target: Option<&str>, any: bool) -> String {
    match (target.map(shortened), any) {
        (Some(target), true) => format!("Open “{target}” with:"),
        (Some(target), false) => format!("There is no app to open “{target}” with."),
        (None, true) => "Choose an app to open it with.".to_owned(),
        (None, false) => "There is no app to open it with.".to_owned(),
    }
}

/// An application's row: its own icon, or the generic one, and its name.
fn app_row(icon: Option<&str>, name: &str, selected: bool) -> Element<Message> {
    let icon = match icon {
        Some(name) => Element::new(AppIcon {
            name: name.to_owned(),
            size: APP_ICON,
        }),
        None => Element::new(Icon {
            name: "symbol.applications".to_owned(),
            size: APP_ICON,
            direction: Default::default(),
            label: None,
            tint: None,
        }),
    };
    let name = text(name.to_owned(), false, selected);
    Element::new(Stack {
        axis: Axis::Horizontal,
        padding: Insets {
            top: 0.0,
            left: GAP,
            bottom: 0.0,
            right: GAP,
        },
        spacing: 6.0,
        align: Align::Center,
        children: vec![icon, name],
    })
}

/// The row of the app chooser's buttons.
fn bar(open: bool, id: u64) -> Element<Message> {
    Element::new(Stack {
        axis: Axis::Horizontal,
        padding: BAR_INSETS,
        spacing: GAP,
        align: Align::Center,
        children: vec![
            Element::new(Spacer { min: GAP }),
            button(
                "Cancel",
                ButtonRole::Normal,
                true,
                Message::App(id, AppEvent::Cancel),
            )
            .key("cancel"),
            button(
                "Open",
                ButtonRole::Default,
                open,
                Message::App(id, AppEvent::Open),
            )
            .key("open"),
        ],
    })
}

/// An app chooser's window: the question, the applications and the buttons, a child of the app's window.
pub fn app_window(a: &AppDialog) -> Window<Message> {
    let id = a.id;
    let ask = a.ask();
    let names: Vec<String> = ask.choices.iter().map(|c| c.name.clone()).collect();
    let rows: Vec<(Option<String>, String)> = (ask.choices.iter())
        .map(|c| (c.icon.clone(), c.name.clone()))
        .collect();
    let list = Element::from(ListView {
        label: "Apps".into(),
        count: rows.len(),
        row_height: ROW_HEIGHT,
        scroll: a.scroll(),
        selection: a.rows().clone(),
        multiple: false,
        initial: true,
        row: Box::new(move |i, selected| app_row(rows[i].0.as_deref(), &rows[i].1, selected)),
        key: Some(Box::new(|i| Key::Int(i as u64))),
        type_select: Some(Box::new(move |i| names[i].clone())),
        on_scroll: Box::new(move |s| Some(Message::App(id, AppEvent::Scroll(s)))),
        on_select: Some(Box::new(move |s| {
            Some(Message::App(id, AppEvent::Select(s)))
        })),
        double_click_activates: true,
        on_activate: Some(Box::new(move |_| Some(Message::App(id, AppEvent::Open)))),
    })
    .key("apps");
    let header = Element::new(Stack {
        axis: Axis::Vertical,
        padding: BAR_INSETS,
        spacing: GAP,
        align: Align::Stretch,
        children: vec![text(
            question(ask.target.as_deref(), !ask.choices.is_empty()),
            true,
            false,
        )
        .key("prompt")],
    });
    let line = || {
        Element::new(Separator {
            axis: Axis::Horizontal,
        })
    };
    // under the row of the window's buttons; a press its controls do not take moves the window
    let content = Element::from(WindowDragArea {
        content: Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets {
                top: SIDEBAR_TOP,
                ..Insets::ZERO
            },
            spacing: 0.0,
            align: Align::Stretch,
            children: vec![header, line(), list, line(), bar(a.chosen().is_some(), id)],
        }),
    });
    Window {
        key: a.window_key(),
        spec: WindowSpec {
            title: "Open With".to_owned(),
            app_id: APP_ID.into(),
            size: APPS_SIZE,
            min_size: Some(APPS_MIN_SIZE),
            max_size: None,
            decorations: PROMPT_DECORATIONS,
            foreign_parent: foreign_parent(&ask.parent_window, false),
            input_region: None,
        },
        shortcuts: keys(
            Message::App(id, AppEvent::Open),
            Message::App(id, AppEvent::Cancel),
        ),
        on_close: Some(Message::App(id, AppEvent::Cancel)),
        background: PANEL_WINDOW_BACKGROUND,
        popup: None,
        menus: None,
        content,
    }
}
