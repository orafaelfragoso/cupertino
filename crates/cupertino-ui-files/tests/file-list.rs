//! The file list as a program uses it: what a press, a key and the band select, what opens a row, renames it, drags
//! it, asks for its menu, sorts the list and opens a folder in place, each reported through the program's function,
//! and the program's field standing in the name being edited.

use crate::support;

use std::cell::Cell;
use std::rc::Rc;

use cupertino_geometry::{Point, Rect, Size};
use cupertino_ui_controls::{
    Column, ColumnWidth, Selection, SortOrder, Text, TextRole, TreeItem, TreeToggle,
};
use cupertino_ui_core::layout::ScrollState;
use cupertino_ui_core::platform::{Mods, MouseButton};
use cupertino_ui_core::{Command, Element, Role, Window};
use cupertino_ui_files::entry::Entry;
use cupertino_ui_files::list::{FileList, Presses, ROW_HEIGHT};
use cupertino_ui_files::ViewEvent;
use cupertino_ui_text::{AttributedText, TextStyle};

use support::{View, RETURN, RIGHT, TAB};

/// The window's size: the rows end well above its bottom.
const SIZE: Size = Size::new(400.0, 240.0);
/// The rows in order, which is not the order of their names; the first is a folder.
const NAMES: [&str; 5] = ["delta", "alpha", "charlie", "bravo", "echo"];
/// The text of the field that stands in a name being edited.
const FIELD: &str = "the name being edited";
/// The width of the list's second column.
const SIZE_COLUMN: f64 = 80.0;

thread_local! {
    /// The row whose name is edited in the next list the thread starts.
    static RENAMING: Cell<Option<usize>> = const { Cell::new(None) };
    /// The next list the thread starts has no message for opening a row or sorting, as a panel that only picks.
    static PICKS_ONLY: Cell<bool> = const { Cell::new(false) };
}

struct Model {
    items: Rc<[Entry]>,
    selection: Selection,
    band: Option<Rect>,
    presses: Presses,
    scroll: ScrollState,
    renaming: Option<usize>,
    picks_only: bool,
    events: Vec<ViewEvent>,
}

struct List;

impl cupertino_ui_core::Application for List {
    type Model = Model;
    type Message = ViewEvent;

    fn init() -> (Model, Vec<Command<ViewEvent>>) {
        let items: Vec<Entry> = NAMES
            .iter()
            .enumerate()
            .map(|(i, name)| Entry::synthetic(name, i == 0, 1))
            .collect();
        let model = Model {
            items: items.into(),
            selection: Selection::default(),
            band: None,
            presses: Presses::default(),
            scroll: ScrollState {
                offset: Point::ZERO,
                content: Size::new(SIZE.width, NAMES.len() as f64 * ROW_HEIGHT),
                viewport: SIZE,
            },
            renaming: RENAMING.with(Cell::get),
            picks_only: PICKS_ONLY.with(Cell::get),
            events: Vec::new(),
        };
        (model, Vec::new())
    }

    fn update(model: &mut Model, event: ViewEvent) -> Vec<Command<ViewEvent>> {
        match &event {
            ViewEvent::Select(selection) => model.selection = selection.clone(),
            ViewEvent::Band(band) => model.band = *band,
            // the table reports its scroll as it lays out: kept, and not among the events a test looks at
            ViewEvent::Scroll(scroll) => {
                model.scroll = *scroll;
                return Vec::new();
            }
            _ => {}
        }
        model.events.push(event);
        Vec::new()
    }

    fn view(model: &Model) -> Vec<Window<ViewEvent>> {
        let outline: Vec<TreeItem> = (0..NAMES.len())
            .map(|i| TreeItem {
                level: 0,
                expanded: (i == 0).then_some(false),
                position: i,
                siblings: NAMES.len(),
            })
            .collect();
        let field: Rc<dyn Fn() -> Element<ViewEvent>> = Rc::new(|| {
            Element::new(Text {
                content: AttributedText::plain(FIELD, TextStyle::new(13.0)),
                role: TextRole::Body,
                wrap: false,
                secondary: false,
                selected: false,
            })
        });
        let on_event: Rc<dyn Fn(ViewEvent) -> Option<ViewEvent>> = if model.picks_only {
            Rc::new(|event| match event {
                ViewEvent::Open(_) | ViewEvent::Sort(_) => None,
                event => Some(event),
            })
        } else {
            Rc::new(Some)
        };
        let list = FileList {
            window: 1,
            label: "Items".into(),
            items: Rc::clone(&model.items),
            outline: outline.into(),
            columns: vec![
                Column {
                    title: "Name".into(),
                    width: ColumnWidth::Fill,
                },
                Column {
                    title: "Size".into(),
                    width: ColumnWidth::Fixed(SIZE_COLUMN),
                },
            ],
            text: Rc::new(|entry, _| entry.size.to_string()),
            sort: Some((0, SortOrder::Ascending)),
            icon: Rc::new(|_| Vec::new()),
            multiple: true,
            scroll: model.scroll,
            selection: model.selection.clone(),
            ringed: Selection::default(),
            band: model.band,
            lead: 0.0,
            name_field: model.renaming.map(|row| (row, Rc::clone(&field))),
            presses: model.presses.clone(),
            on_event,
        };
        vec![support::window(SIZE, Element::from(list))]
    }
}

/// The list in its window, the name of row `renaming` edited.
fn open(renaming: Option<usize>) -> View<List> {
    RENAMING.with(|r| r.set(renaming));
    View::open(SIZE)
}

/// How far into a name's text a press on the name lands.
const INTO_NAME: f64 = 6.0;

/// A point on row `index`'s name, and one of its blank part past the name.
fn name_of(view: &View<List>, index: usize) -> Point {
    let text = view
        .bounds_of(Some(Role::StaticText), NAMES[index])
        .expect("the row's name is shown");
    Point::new(text.min_x() + INTO_NAME, text.center().y)
}

fn blank_of(view: &View<List>, index: usize) -> Point {
    let text = view
        .bounds_of(Some(Role::StaticText), NAMES[index])
        .expect("the row's name is shown");
    Point::new(text.max_x() - INTO_NAME, text.center().y)
}

fn selected(selection: &Selection) -> Vec<usize> {
    selection.ranges().iter().flat_map(Clone::clone).collect()
}

fn index_of(name: &str) -> usize {
    NAMES.iter().position(|n| *n == name).unwrap()
}

#[test]
fn a_press_on_a_row_selects_as_its_modifiers_say() {
    let rows: [(Mods, Vec<usize>); 3] = [
        (Mods::NONE, vec![3]),
        (Mods::SHIFT, vec![1, 2, 3]),
        (Mods::PRIMARY, vec![1, 3]),
    ];
    for (mods, expected) in rows {
        let mut view = open(None);
        let (first, second) = (name_of(&view, 1), name_of(&view, 3));
        view.click(first, Mods::NONE);
        view.wait(1.0);
        view.click(second, mods);
        assert_eq!(selected(&view.model().selection), expected, "{mods:?}");
    }
}

#[test]
fn a_double_click_and_return_open_a_row() {
    let mut view = open(None);
    let at = name_of(&view, 2);
    view.click(at, Mods::NONE);
    view.click(at, Mods::NONE);
    assert!(view.model().events.contains(&ViewEvent::Open(2)));

    let mut view = open(None);
    let at = name_of(&view, 4);
    view.click(at, Mods::NONE);
    view.key(RETURN, None, Mods::NONE);
    assert_eq!(view.model().events.last(), Some(&ViewEvent::Open(4)));
}

#[test]
fn a_function_with_no_message_for_an_event_gets_nothing_sent_for_it() {
    PICKS_ONLY.with(|p| p.set(true));
    let mut view = open(None);
    PICKS_ONLY.with(|p| p.set(false));
    let at = name_of(&view, 2);
    view.click(at, Mods::NONE);
    view.click(at, Mods::NONE);
    view.key(RETURN, None, Mods::NONE);
    let header = view
        .center_of(Some(Role::ColumnHeader), "Size")
        .expect("the size column's header is shown");
    view.click(header, Mods::NONE);
    assert_eq!(
        selected(&view.model().selection),
        [2],
        "the press still selects"
    );
    let events = &view.model().events;
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, ViewEvent::Open(_) | ViewEvent::Sort(_))),
        "{events:?}"
    );
}

#[test]
fn the_band_selects_the_rows_it_crosses_until_it_is_let_go() {
    // from under the rows up to row 1, and from row 0's blank part down to row 2: the rows crossed
    let rows: [(Option<usize>, Vec<usize>); 2] =
        [(None, vec![1, 2, 3, 4]), (Some(0), vec![0, 1, 2])];
    for (from_row, expected) in rows {
        let mut view = open(None);
        let from = match from_row {
            None => Point::new(SIZE.width / 2.0, SIZE.height - 8.0),
            Some(row) => blank_of(&view, row),
        };
        let over = match from_row {
            None => blank_of(&view, 1),
            Some(_) => blank_of(&view, 2),
        };
        view.press(from, MouseButton::Left, Mods::NONE);
        view.move_to(Point::new(from.x, (from.y + over.y) / 2.0));
        view.move_to(over);
        assert!(view.model().band.is_some(), "from {from_row:?}");
        assert_eq!(
            selected(&view.model().selection),
            expected,
            "from {from_row:?}"
        );
        view.release(over, MouseButton::Left);
        assert_eq!(view.model().band, None, "from {from_row:?}");
    }
}

#[test]
fn a_press_on_the_only_selected_name_renames_once_the_interval_passes() {
    let rows = [(0.2, false), (0.6, true)];
    for (waited, renames) in rows {
        let mut view = open(None);
        let at = name_of(&view, 1);
        view.click(at, Mods::NONE);
        view.wait(1.0);
        view.click(at, Mods::NONE);
        view.wait(waited);
        assert_eq!(
            view.model().events.contains(&ViewEvent::Rename(1)),
            renames,
            "after {waited} s"
        );
    }
}

#[test]
fn tab_gives_the_next_row_by_name_the_keyboard_and_passes_on_after_the_last() {
    let rows = [
        ("alpha", Mods::NONE, Some("bravo")),
        ("charlie", Mods::SHIFT, Some("bravo")),
        ("echo", Mods::NONE, None),
    ];
    for (from, mods, to) in rows {
        let mut view = open(None);
        let at = name_of(&view, index_of(from));
        view.click(at, Mods::NONE);
        view.key(TAB, None, mods);
        let focused = view.model().events.iter().find_map(|e| match e {
            ViewEvent::FocusRow(row) => Some(*row),
            _ => None,
        });
        assert_eq!(focused, to.map(index_of), "{from} {mods:?}");
    }
}

#[test]
fn a_press_on_a_name_that_moves_drags_the_rows() {
    let mut view = open(None);
    let at = name_of(&view, 3);
    view.press(at, MouseButton::Left, Mods::NONE);
    view.move_to(Point::new(at.x + 2.0, at.y));
    assert!(!view
        .model()
        .events
        .iter()
        .any(|e| matches!(e, ViewEvent::DragRows(_))));
    view.move_to(Point::new(at.x + 8.0, at.y));
    let Some(ViewEvent::DragRows(grab)) = view.model().events.last() else {
        panic!("no drag: {:?}", view.model().events);
    };
    assert_eq!(grab.indices, vec![3]);
    assert!(!grab.rtl);
}

#[test]
fn a_secondary_press_says_which_row_before_the_menu_opens() {
    let mut view = open(None);
    let at = name_of(&view, 2);
    view.press(at, MouseButton::Right, Mods::NONE);
    assert_eq!(
        view.model().events.first(),
        Some(&ViewEvent::SecondaryPress(Some(2)))
    );

    let mut view = open(None);
    let at = name_of(&view, 2);
    view.press(at, MouseButton::Left, Mods::CTRL);
    assert_eq!(
        view.model().events,
        vec![
            ViewEvent::SecondaryPress(Some(2)),
            ViewEvent::ContextMenu(at)
        ]
    );
}

#[test]
fn a_column_header_sorts_by_its_column() {
    let rows = [
        ("Size", (1, SortOrder::Ascending)),
        ("Name", (0, SortOrder::Descending)),
    ];
    for (title, expected) in rows {
        let mut view = open(None);
        let at = view.center_of(None, title).expect("the header");
        view.click(at, Mods::NONE);
        assert_eq!(
            view.model().events.last(),
            Some(&ViewEvent::Sort(expected)),
            "{title}"
        );
    }
}

#[test]
fn right_on_a_closed_folder_opens_it_in_place() {
    let mut view = open(None);
    let at = name_of(&view, 0);
    view.click(at, Mods::NONE);
    view.key(RIGHT, None, Mods::NONE);
    assert_eq!(
        view.model().events.last(),
        Some(&ViewEvent::Toggle(TreeToggle {
            index: 0,
            expanded: true,
            recursive: false,
        }))
    );
}

#[test]
fn the_programs_field_stands_in_the_name_being_edited() {
    let view = open(Some(1));
    assert!(view.center_of(Some(Role::StaticText), FIELD).is_some());
    assert!(view.center_of(Some(Role::StaticText), NAMES[1]).is_none());
    assert!(view.center_of(Some(Role::StaticText), NAMES[2]).is_some());
}
