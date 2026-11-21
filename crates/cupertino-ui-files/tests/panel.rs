//! The open and save panel as a program uses it: what its sidebar, its folder row, its Save As field, its filters,
//! choices and buttons report through the program's function, the question whether to replace a file in place of
//! the controls, and the items of the open folder as a file list that reports through its own function.

#[allow(dead_code)]
mod support;

use std::cell::Cell;
use std::rc::Rc;

use cupertino_geometry::{Point, Size};
use cupertino_ui_controls::{
    Column, ColumnWidth, Selection, SidebarItem, SidebarSection, SplitLayout, TextChange,
};
use cupertino_ui_core::layout::ScrollState;
use cupertino_ui_core::platform::{Keysym, Mods};
use cupertino_ui_core::{Command, Element, Key, Role, Window};
use cupertino_ui_files::entry::Entry;
use cupertino_ui_files::list::{FileList, Presses};
use cupertino_ui_files::panel::{
    ChoiceValue, OpenSavePanel, PanelChoice, PanelEvent, SaveAs, SIDEBAR_WIDTH,
};
use cupertino_ui_files::ViewEvent;

use support::{View, RETURN};

const SIZE: Size = Size::new(720.0, 460.0);
const ITEMS: [&str; 3] = ["alpha", "bravo", "charlie"];
const PLACES: [&str; 3] = ["Home", "Desktop", "Documents"];
const NO_SCROLL: ScrollState = ScrollState {
    offset: Point::ZERO,
    content: Size::ZERO,
    viewport: Size::ZERO,
};

/// What the next panel the thread opens shows.
#[derive(Clone, Copy, Debug, Default)]
struct Shown {
    saving: bool,
    replacing: bool,
    can_accept: bool,
    enclosing: bool,
}

thread_local! {
    static SHOWN: Cell<Shown> = const { Cell::new(Shown { saving: false, replacing: false, can_accept: false, enclosing: false }) };
}

#[derive(Clone, Debug, PartialEq)]
enum Msg {
    Panel(PanelEvent),
    List(ViewEvent),
}

struct Model {
    shown: Shown,
    name: String,
    revision: u64,
    selection: Selection,
    scroll: ScrollState,
    presses: Presses,
    events: Vec<Msg>,
}

struct Panel;

impl cupertino_ui_core::Application for Panel {
    type Model = Model;
    type Message = Msg;

    fn init() -> (Model, Vec<Command<Msg>>) {
        let model = Model {
            shown: SHOWN.with(Cell::get),
            name: "Notes.txt".into(),
            revision: 0,
            selection: Selection::default(),
            scroll: NO_SCROLL,
            presses: Presses::default(),
            events: Vec::new(),
        };
        (model, Vec::new())
    }

    fn update(model: &mut Model, message: Msg) -> Vec<Command<Msg>> {
        match &message {
            // the views report their scroll and layout as they lay out: not among the events a test looks at
            Msg::Panel(PanelEvent::PlacesScroll(_) | PanelEvent::Split(_)) => return Vec::new(),
            Msg::List(ViewEvent::Scroll(scroll)) => {
                model.scroll = *scroll;
                return Vec::new();
            }
            Msg::Panel(PanelEvent::Name(TextChange { text, revision })) => {
                model.name.clone_from(text);
                model.revision = *revision;
            }
            Msg::List(ViewEvent::Select(selection)) => model.selection = selection.clone(),
            _ => {}
        }
        model.events.push(message);
        Vec::new()
    }

    fn view(model: &Model) -> Vec<Window<Msg>> {
        let shown = model.shown;
        let items: Rc<[Entry]> = ITEMS
            .iter()
            .map(|name| Entry::synthetic(name, false, 1))
            .collect();
        let list = FileList {
            window: 1,
            label: "Documents".into(),
            items,
            outline: Rc::from([]),
            columns: vec![Column {
                title: "Name".into(),
                width: ColumnWidth::Fill,
            }],
            text: Rc::new(|_, _| String::new()),
            sort: None,
            icon: Rc::new(|_| Vec::new()),
            multiple: false,
            scroll: model.scroll,
            selection: model.selection.clone(),
            ringed: Selection::default(),
            band: None,
            lead: 0.0,
            name_field: None,
            presses: model.presses.clone(),
            on_event: Rc::new(|event| match event {
                ViewEvent::Rename(_) | ViewEvent::Band(_) => None,
                event => Some(Msg::List(event)),
            }),
        };
        let panel = OpenSavePanel {
            places: vec![SidebarSection {
                title: Some("Favorites".into()),
                items: PLACES
                    .iter()
                    .map(|name| SidebarItem {
                        key: Key::from(*name),
                        label: (*name).into(),
                        icon: Some("symbol.folder".into()),
                        description: None,
                        accessory: None,
                        busy: false,
                    })
                    .collect(),
            }],
            place: Some(2),
            places_scroll: NO_SCROLL,
            split: SplitLayout {
                position: SIDEBAR_WIDTH,
                collapsed: None,
            },
            folder: "/home/u/Documents".into(),
            enclosing: shown.enclosing,
            unreadable: false,
            list,
            save_as: shown.saving.then(|| SaveAs {
                name: model.name.clone(),
                revision: model.revision,
                stem: 5,
            }),
            filters: vec!["Text".into(), "Images".into()],
            filter: Some(0),
            choices: vec![
                PanelChoice::Check {
                    label: "Open Read-Only".into(),
                    checked: false,
                },
                PanelChoice::Pick {
                    label: "Encoding".into(),
                    options: vec!["UTF-8".into(), "Latin-1".into()],
                    selected: Some(0),
                },
            ],
            accept_label: if shown.saving { "Save" } else { "Open" }.into(),
            can_accept: shown.can_accept,
            replace: shown.replacing.then(|| "Notes.txt".to_owned()),
            on_event: Rc::new(Msg::Panel),
        };
        vec![support::window(SIZE, Element::from(panel))]
    }
}

fn open(shown: Shown) -> View<Panel> {
    SHOWN.with(|s| s.set(shown));
    View::open(SIZE)
}

fn at(view: &View<Panel>, role: Option<Role>, name: &str) -> Point {
    view.center_of(role, name)
        .unwrap_or_else(|| panic!("{name} is shown"))
}

fn panel_events(view: &View<Panel>) -> Vec<PanelEvent> {
    view.model()
        .events
        .iter()
        .filter_map(|m| match m {
            Msg::Panel(event) => Some(event.clone()),
            Msg::List(_) => None,
        })
        .collect()
}

#[test]
fn the_sidebar_the_folder_row_the_filters_the_choices_and_the_buttons_report_what_was_chosen() {
    let shown = Shown {
        can_accept: true,
        enclosing: true,
        ..Shown::default()
    };
    let presses: [(Option<Role>, &str, PanelEvent); 6] = [
        (None, "Desktop", PanelEvent::Place(1)),
        (Some(Role::Button), "Enclosing Folder", PanelEvent::Parent),
        (None, "Images", PanelEvent::Filter(1)),
        (
            None,
            "Open Read-Only",
            PanelEvent::Choice(0, ChoiceValue::Checked(true)),
        ),
        (
            None,
            "Latin-1",
            PanelEvent::Choice(1, ChoiceValue::Option(1)),
        ),
        (Some(Role::Button), "Open", PanelEvent::Accept),
    ];
    for (role, name, expected) in presses {
        let mut view = open(shown);
        let point = at(&view, role, name);
        view.click(point, Mods::NONE);
        assert_eq!(panel_events(&view), [expected], "{name}");
    }
    let mut view = open(shown);
    let cancel = at(&view, Some(Role::Button), "Cancel");
    view.click(cancel, Mods::NONE);
    assert_eq!(panel_events(&view), [PanelEvent::Cancel]);
    assert!(
        view.bounds_of(None, "Save As").is_none(),
        "no Save As field when opening"
    );
}

#[test]
fn a_button_that_cannot_be_pressed_now_reports_nothing() {
    let mut view = open(Shown::default());
    for name in ["Open", "Enclosing Folder"] {
        let point = at(&view, Some(Role::Button), name);
        view.click(point, Mods::NONE);
    }
    assert_eq!(panel_events(&view), []);
}

#[test]
fn the_save_as_field_reports_the_typed_name_and_return_in_it_accepts() {
    let mut view = open(Shown {
        saving: true,
        can_accept: true,
        ..Shown::default()
    });
    assert_eq!(view.model().name, "Notes.txt");
    // the field takes the focus with the stem selected: typing replaces it
    view.key(Keysym('x' as u32), Some('x'), Mods::NONE);
    assert_eq!(view.model().name, "x.txt");
    view.key(RETURN, None, Mods::NONE);
    assert_eq!(panel_events(&view).last(), Some(&PanelEvent::Accept));
}

#[test]
fn the_question_whether_to_replace_a_file_stands_in_the_controls_and_its_answers_report() {
    for (name, expected) in [
        ("Replace", PanelEvent::Replace),
        ("Cancel", PanelEvent::KeepEditing),
    ] {
        let mut view = open(Shown {
            saving: true,
            replacing: true,
            can_accept: true,
            ..Shown::default()
        });
        assert!(view
            .bounds_of(
                None,
                "“Notes.txt” already exists. Do you want to replace it?"
            )
            .is_some());
        assert!(view.bounds_of(None, "Images").is_none(), "no filters");
        assert!(view.bounds_of(Some(Role::Button), "Save").is_none());
        let point = at(&view, Some(Role::Button), name);
        view.click(point, Mods::NONE);
        assert_eq!(panel_events(&view).last(), Some(&expected), "{name}");
    }
}

#[test]
fn the_items_are_a_file_list_that_reports_through_its_own_function() {
    let mut view = open(Shown::default());
    let bravo = at(&view, Some(Role::StaticText), "bravo");
    view.click(bravo, Mods::NONE);
    view.click(bravo, Mods::NONE);
    let list: Vec<&Msg> = view
        .model()
        .events
        .iter()
        .filter(|m| matches!(m, Msg::List(_)))
        .collect();
    assert_eq!(view.model().selection.cursor(), Some(1));
    assert!(list.contains(&&Msg::List(ViewEvent::Open(1))), "{list:?}");
}

#[test]
fn an_open_panel_opens_with_the_keyboard_on_its_items() {
    let mut view = open(Shown::default());
    // no click first: the rows have the focus, so Down selects the first
    view.key(Keysym(0xff54), None, Mods::NONE);
    assert_eq!(view.model().selection.cursor(), Some(0));
}
