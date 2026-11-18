//! Lists, tables, outlines and sidebars as a person selects in them: the rows a press and the keys select, what
//! opens a row, typing to select, sorting by a column header, opening and closing outline rows, and a selection's own
//! arithmetic.

#[allow(dead_code)]
mod support;

use cupertino_geometry::{Point, Size};
use cupertino_ui_controls::{
    Column, ColumnWidth, ListView, Selection, Sidebar, SidebarAccessory, SidebarItem,
    SidebarSection, SortOrder, TableView, Text, TextRole, TreeItem, TreeToggle, TreeView,
};
use cupertino_ui_core::layout::ScrollState;
use cupertino_ui_core::platform::{Keysym, Mods};
use cupertino_ui_core::{Element, Key, Role};
use cupertino_ui_text::{AttributedText, TextStyle};
use support::{Scene, View, DOWN, END, HOME, LEFT, RETURN, RIGHT, SIZE, UP};

const NAMES: [&str; 8] = [
    "apple",
    "banana",
    "blueberry",
    "cherry",
    "date",
    "elder",
    "fig",
    "grape",
];
const ROW: f64 = 24.0;

fn text(content: &str, selected: bool) -> Element<Msg> {
    Element::new(Text {
        content: AttributedText::plain(content, TextStyle::new(13.0)),
        role: TextRole::Body,
        wrap: false,
        secondary: false,
        selected,
    })
}

fn scroll(rows: usize) -> ScrollState {
    ScrollState {
        offset: Point::ZERO,
        content: Size::new(SIZE.width, rows as f64 * ROW),
        viewport: SIZE,
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Msg {
    Select(Selection),
    Open(usize),
    Scroll(ScrollState),
    Sort((usize, SortOrder)),
    Toggle(TreeToggle),
    Place(usize),
    Eject(Key),
}

struct State {
    selection: Selection,
    scroll: ScrollState,
    opened: Vec<usize>,
    sort: Option<(usize, SortOrder)>,
    toggles: Vec<TreeToggle>,
    placed: Vec<usize>,
    ejected: Vec<Key>,
    place: Option<usize>,
}

fn state(rows: usize) -> State {
    State {
        selection: Selection::default(),
        scroll: scroll(rows),
        opened: Vec::new(),
        sort: None,
        toggles: Vec::new(),
        placed: Vec::new(),
        ejected: Vec::new(),
        place: Some(0),
    }
}

fn update(state: &mut State, message: Msg) {
    match message {
        Msg::Select(selection) => state.selection = selection,
        Msg::Open(row) => state.opened.push(row),
        Msg::Scroll(scroll) => state.scroll = scroll,
        Msg::Sort(sort) => state.sort = Some(sort),
        Msg::Toggle(toggle) => state.toggles.push(toggle),
        Msg::Place(index) => {
            state.placed.push(index);
            state.place = Some(index);
        }
        Msg::Eject(key) => state.ejected.push(key),
    }
}

fn selected(state: &State) -> Vec<usize> {
    state
        .selection
        .ranges()
        .iter()
        .flat_map(Clone::clone)
        .collect()
}

/// A list of the eight names; `MULTIPLE` lets it select several rows.
struct Names<const MULTIPLE: bool>;

impl<const MULTIPLE: bool> Scene for Names<MULTIPLE> {
    type State = State;
    type Msg = Msg;

    fn state() -> State {
        state(NAMES.len())
    }

    fn update(state: &mut State, message: Msg) {
        update(state, message);
    }

    fn view(state: &State) -> Element<Msg> {
        Element::from(ListView {
            label: "Fruit".into(),
            count: NAMES.len(),
            row_height: ROW,
            scroll: state.scroll,
            selection: state.selection.clone(),
            multiple: MULTIPLE,
            initial: false,
            row: Box::new(|i, selected| text(NAMES[i], selected)),
            key: None,
            type_select: Some(Box::new(|i| NAMES[i].to_string())),
            on_scroll: Box::new(Msg::Scroll),
            on_select: Some(Box::new(Msg::Select)),
            on_activate: Some(Box::new(Msg::Open)),
            double_click_activates: true,
        })
    }
}

fn row<S: Scene>(view: &View<S>, name: &str) -> Point {
    view.center_of(Role::StaticText, name)
}

#[test]
fn a_press_on_a_row_selects_as_its_modifiers_say_and_only_a_multiple_list_extends_or_toggles() {
    // after a press on row 1 (banana), a press on row 3 (cherry) with the modifiers
    let rows = [
        ("plain", Mods::NONE, true, vec![3]),
        ("shift", Mods::SHIFT, true, vec![1, 2, 3]),
        ("primary", Mods::PRIMARY, true, vec![1, 3]),
        ("shift in a single list", Mods::SHIFT, false, vec![3]),
        ("primary in a single list", Mods::PRIMARY, false, vec![3]),
    ];
    for (what, mods, multiple, expected) in rows {
        let (first, second);
        let state = if multiple {
            let mut view = View::<Names<true>>::open();
            (first, second) = (row(&view, "banana"), row(&view, "cherry"));
            view.click(first, Mods::NONE);
            view.wait(1.0);
            view.click(second, mods);
            selected(view.state())
        } else {
            let mut view = View::<Names<false>>::open();
            (first, second) = (row(&view, "banana"), row(&view, "cherry"));
            view.click(first, Mods::NONE);
            view.wait(1.0);
            view.click(second, mods);
            selected(view.state())
        };
        assert_eq!(state, expected, "{what}");
    }
}

#[test]
fn the_arrow_keys_home_and_end_move_the_selection_and_shift_extends_it_in_a_multiple_list() {
    let rows = [
        ("down", DOWN, Mods::NONE, vec![3]),
        ("up", UP, Mods::NONE, vec![1]),
        ("home", HOME, Mods::NONE, vec![0]),
        ("end", END, Mods::NONE, vec![7]),
        ("shift down", DOWN, Mods::SHIFT, vec![2, 3]),
        ("shift up", UP, Mods::SHIFT, vec![1, 2]),
        ("shift end", END, Mods::SHIFT, vec![2, 3, 4, 5, 6, 7]),
        (
            "select all",
            Keysym('a' as u32),
            Mods::PRIMARY,
            vec![0, 1, 2, 3, 4, 5, 6, 7],
        ),
    ];
    for (what, key, mods, expected) in rows {
        let mut view = View::<Names<true>>::open();
        let at = row(&view, "blueberry");
        view.click(at, Mods::NONE);
        view.key(key, None, mods);
        assert_eq!(selected(view.state()), expected, "{what}");
    }

    let mut view = View::<Names<false>>::open();
    let at = row(&view, "blueberry");
    view.click(at, Mods::NONE);
    view.key(DOWN, None, Mods::SHIFT);
    assert_eq!(selected(view.state()), [3], "shift in a single list");
    view.key(Keysym('a' as u32), None, Mods::PRIMARY);
    assert_eq!(selected(view.state()), [3], "select all in a single list");
    for _ in 0..8 {
        view.key(DOWN, None, Mods::NONE);
    }
    assert_eq!(selected(view.state()), [7], "stops at the last row");
}

#[test]
fn return_and_a_double_click_open_the_row_under_the_keyboard_or_the_pointer() {
    let mut view = View::<Names<true>>::open();
    let at = row(&view, "date");
    view.click(at, Mods::NONE);
    view.key(RETURN, None, Mods::NONE);
    assert_eq!(view.state().opened, [4]);

    let mut view = View::<Names<true>>::open();
    let at = row(&view, "fig");
    view.click(at, Mods::NONE);
    view.click(at, Mods::NONE);
    assert_eq!(view.state().opened, [6]);
}

#[test]
fn typing_selects_the_next_row_that_starts_with_it_and_a_pause_starts_the_search_over() {
    let mut view = View::<Names<true>>::open();
    let at = row(&view, "apple");
    view.click(at, Mods::NONE);
    let steps = [
        ("b", 0.0, 1),
        ("l", 0.0, 2),
        ("e", 0.0, 2),
        ("b", 2.0, 1),
        ("b", 0.0, 1),
        ("z", 2.0, 1),
        ("a", 2.0, 0),
    ];
    for (typed, pause, expected) in steps {
        view.wait(pause);
        let c = typed.chars().next().unwrap();
        view.key(Keysym(c as u32), Some(c), Mods::NONE);
        assert_eq!(
            view.state().selection.cursor(),
            Some(expected),
            "{typed} after {pause} s"
        );
    }
}

#[test]
fn a_selection_extends_toggles_and_gives_up_rows_the_way_a_list_needs() {
    let mut selection = Selection::default();
    selection.select_only(2);
    selection.extend_to(5);
    assert_eq!(selection.ranges(), only(2..6));
    selection.extend_to(3);
    assert_eq!(
        selection.ranges(),
        only(2..4),
        "an extension takes back what it selected before"
    );

    selection.toggle(7);
    selection.toggle(2);
    assert_eq!(selection.ranges(), [3..4, 7..8]);
    assert_eq!((selection.cursor(), selection.anchor()), (Some(2), Some(2)));
    assert!(selection.contains(7) && !selection.contains(4));
    assert_eq!(selection.len(), 2);

    selection.select_all(10);
    assert_eq!(selection.ranges(), only(0..10));
    selection.clamp(4);
    assert_eq!(selection.ranges(), only(0..4));
    assert_eq!(selection.cursor(), Some(2));
    selection.clamp(2);
    assert_eq!(selection.cursor(), None);

    let rebuilt = Selection::new([5..7, 1..3, 3..4, 9..9], Some(1), None);
    assert_eq!(
        rebuilt.ranges(),
        [1..4, 5..7],
        "sorted, joined where they touch, without an empty one"
    );
    assert!(Selection::default().is_empty());
}

/// A table of the names with a size column, sorted as the application says.
struct Table;

impl Scene for Table {
    type State = State;
    type Msg = Msg;

    fn state() -> State {
        state(NAMES.len())
    }

    fn update(state: &mut State, message: Msg) {
        update(state, message);
    }

    fn view(state: &State) -> Element<Msg> {
        Element::from(TableView {
            label: "Fruit".into(),
            columns: vec![
                Column {
                    title: "Name".into(),
                    width: ColumnWidth::Fill,
                },
                Column {
                    title: "Letters".into(),
                    width: ColumnWidth::Fixed(80.0),
                },
            ],
            sort: state.sort,
            on_sort: Some(Box::new(Msg::Sort)),
            count: NAMES.len(),
            row_height: ROW,
            scroll: state.scroll,
            selection: state.selection.clone(),
            multiple: true,
            cell: Box::new(|i, column, selected| match column {
                0 => text(NAMES[i], selected),
                _ => text(&NAMES[i].len().to_string(), selected),
            }),
            type_select: None,
            on_scroll: Box::new(Msg::Scroll),
            on_select: Some(Box::new(Msg::Select)),
            on_activate: Some(Box::new(Msg::Open)),
            double_click_activates: true,
            on_row_key: None,
            header_top: 0.0,
        })
    }
}

#[test]
fn a_column_header_asks_to_sort_by_its_column_and_the_other_way_when_it_is_the_sorted_one() {
    let mut view = View::<Table>::open();
    let name = view.center_of(Role::ColumnHeader, "Name");
    let letters = view.center_of(Role::ColumnHeader, "Letters");
    let asked = [
        (name, (0, SortOrder::Ascending)),
        (name, (0, SortOrder::Descending)),
        (letters, (1, SortOrder::Ascending)),
        (name, (0, SortOrder::Ascending)),
    ];
    for (header, expected) in asked {
        view.click(header, Mods::NONE);
        assert_eq!(view.state().sort, Some(expected));
    }
    let name = view.node(Role::ColumnHeader, "Name").unwrap();
    assert_eq!(name.sort, Some(SortOrder::Ascending));

    let at = row(&view, "date");
    view.click(at, Mods::NONE);
    assert_eq!(
        selected(view.state()),
        [4],
        "the rows select as a list's do"
    );
}

/// An outline: `a` holds `a1` and `a2`, `b` holds `b1`; the rows shown follow what the application keeps open.
struct Outline;

const OUTLINE: [(&str, usize, Option<bool>); 5] = [
    ("a", 0, Some(true)),
    ("a1", 1, None),
    ("a2", 1, None),
    ("b", 0, Some(false)),
    ("c", 0, None),
];

impl Scene for Outline {
    type State = State;
    type Msg = Msg;

    fn state() -> State {
        state(OUTLINE.len())
    }

    fn update(state: &mut State, message: Msg) {
        update(state, message);
    }

    fn view(state: &State) -> Element<Msg> {
        Element::from(TreeView {
            label: "Tree".into(),
            count: OUTLINE.len(),
            row_height: ROW,
            scroll: state.scroll,
            selection: state.selection.clone(),
            multiple: false,
            item: Box::new(|i| TreeItem {
                level: OUTLINE[i].1,
                expanded: OUTLINE[i].2,
                position: 0,
                siblings: 1,
            }),
            row: Box::new(|i, selected| text(OUTLINE[i].0, selected)),
            type_select: None,
            on_scroll: Box::new(Msg::Scroll),
            on_select: Some(Box::new(Msg::Select)),
            on_activate: None,
            double_click_activates: false,
            on_toggle: Some(Box::new(Msg::Toggle)),
        })
    }
}

#[test]
fn right_and_left_open_and_close_an_outline_row_or_move_to_its_first_child_or_its_parent() {
    // the row the keyboard starts on, the key, what the outline then asks for, and the row it is on
    let toggle = |index, expanded, recursive| {
        Some(TreeToggle {
            index,
            expanded,
            recursive,
        })
    };
    let rows = [
        ("a", RIGHT, Mods::NONE, None, 1),
        ("a", LEFT, Mods::NONE, toggle(0, false, false), 0),
        ("b", RIGHT, Mods::NONE, toggle(3, true, false), 3),
        ("b", LEFT, Mods::NONE, None, 3),
        ("a2", LEFT, Mods::NONE, None, 0),
        ("a1", RIGHT, Mods::NONE, None, 1),
        ("a", LEFT, Mods::PRIMARY, toggle(0, false, true), 0),
        ("b", RIGHT, Mods::PRIMARY, toggle(3, true, true), 3),
        ("c", RIGHT, Mods::PRIMARY, None, 4),
    ];
    for (from, key, mods, asked, to) in rows {
        let mut view = View::<Outline>::open();
        let at = row(&view, from);
        view.click(at, Mods::NONE);
        view.key(key, None, mods);
        assert_eq!(
            view.state().toggles.first().copied(),
            asked,
            "{from} {key:?} {mods:?}"
        );
        assert_eq!(
            view.state().selection.cursor(),
            Some(to),
            "{from} {key:?} {mods:?}"
        );
    }
}

/// Two sections of places; the second has a device that can be ejected.
struct Places;

impl Scene for Places {
    type State = State;
    type Msg = Msg;

    fn state() -> State {
        state(4)
    }

    fn update(state: &mut State, message: Msg) {
        update(state, message);
    }

    fn view(state: &State) -> Element<Msg> {
        let item = |name: &'static str, accessory: bool| SidebarItem {
            key: Key::from(name),
            label: name.into(),
            icon: None,
            description: None,
            accessory: accessory.then(|| SidebarAccessory {
                symbol: "eject".into(),
                label: format!("Eject {name}"),
            }),
            busy: false,
        };
        Element::from(Sidebar {
            label: "Places".into(),
            sections: vec![
                SidebarSection {
                    title: None,
                    items: vec![item("Home", false), item("Documents", false)],
                },
                SidebarSection {
                    title: Some("Devices".into()),
                    items: vec![item("Backup", true), item("Camera", false)],
                },
            ],
            selected: state.place,
            scroll: state.scroll,
            on_scroll: Box::new(Msg::Scroll),
            on_select: Some(Box::new(Msg::Place)),
            on_accessory: Some(Box::new(Msg::Eject)),
        })
    }
}

#[test]
fn a_sidebar_selects_across_its_sections_by_keys_and_press_and_reports_a_row_once() {
    let mut view = View::<Places>::open();
    let home = view.center_of(Role::ListItem, "Home");
    view.click(home, Mods::NONE);
    assert!(
        view.state().placed.is_empty(),
        "the selected row again reports nothing"
    );

    for key in [DOWN, DOWN, DOWN, DOWN, UP, HOME, END] {
        view.key(key, None, Mods::NONE);
    }
    assert_eq!(view.state().placed, [1, 2, 3, 2, 0, 3]);

    let camera = view.center_of(Role::ListItem, "Camera");
    let before = view.state().placed.len();
    view.click(camera, Mods::NONE);
    assert_eq!(view.state().placed.len(), before, "selected already by End");
    let documents = view.center_of(Role::ListItem, "Documents");
    view.click(documents, Mods::NONE);
    assert_eq!(view.state().placed.last(), Some(&1));
}

#[test]
fn a_press_on_a_rows_accessory_reports_the_row_without_selecting_it() {
    let mut view = View::<Places>::open();
    let eject = view.center_of(Role::Button, "Eject Backup");
    view.click(eject, Mods::NONE);
    assert_eq!(view.state().ejected, [Key::from("Backup")]);
    assert!(view.state().placed.is_empty());
}

/// The ranges of a selection of one run of rows.
fn only(range: std::ops::Range<usize>) -> Vec<std::ops::Range<usize>> {
    vec![range]
}
