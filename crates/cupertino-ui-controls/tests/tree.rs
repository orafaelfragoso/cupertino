mod support;

use cupertino_geometry::{Point, Size};
use cupertino_ui_controls::{
    Column, ColumnWidth, OutlineTable, Selection, TableView, Text, TextRole, TreeItem, TreeToggle,
    TreeView,
};
use cupertino_ui_core::{Action, Application, Command, Element, Key, NodeId, Role, Window};
use cupertino_ui_layout::{LayoutDirection, ScrollState};
use cupertino_ui_platform::Mods;
use cupertino_ui_text::{AttributedText, TextStyle};
use std::cell::Cell;
use std::collections::BTreeSet;
use support::{window, Harness, TAB};

const LEFT: u32 = 0xff51;
const RIGHT: u32 = 0xff53;
const ROW: f64 = 20.0;
/// Every folder holds this many files.
const FILES: usize = 9;

#[derive(Clone, Copy)]
struct Setup {
    folders: usize,
    open: bool,
    /// Folder 0 holds no files.
    empty: bool,
    /// The rows are a table's, with a Size column after the name.
    table: bool,
}

thread_local! {
    static SETUP: Cell<Setup> = const { Cell::new(Setup { folders: 3, open: false, empty: false, table: false }) };
}

#[derive(Clone, Debug, PartialEq)]
enum Message {
    Scroll(ScrollState),
    Select(Selection),
    Toggle(TreeToggle),
}

struct Model {
    folders: usize,
    empty: bool,
    open: BTreeSet<usize>,
    scroll: ScrollState,
    selection: Selection,
    toggles: Vec<TreeToggle>,
}

/// The shown rows: each folder, then its files while it is open, as (folder, file).
fn rows(model: &Model) -> Vec<(usize, Option<usize>)> {
    let mut rows = Vec::new();
    for folder in 0..model.folders {
        rows.push((folder, None));
        if model.open.contains(&folder) && !(model.empty && folder == 0) {
            rows.extend((0..FILES).map(|file| (folder, Some(file))));
        }
    }
    rows
}

struct Folders;

impl Application for Folders {
    type Model = Model;
    type Message = Message;

    fn init() -> (Model, Vec<Command<Message>>) {
        let setup = SETUP.get();
        let model = Model {
            folders: setup.folders,
            empty: setup.empty,
            open: if setup.open {
                (0..setup.folders).collect()
            } else {
                BTreeSet::new()
            },
            scroll: ScrollState {
                offset: Point::ZERO,
                content: Size::ZERO,
                viewport: Size::ZERO,
            },
            selection: Selection::default(),
            toggles: Vec::new(),
        };
        (model, Vec::new())
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        match message {
            Message::Scroll(scroll) => model.scroll = scroll,
            Message::Select(selection) => model.selection = selection,
            Message::Toggle(toggle) => {
                let (folder, _) = rows(model)[toggle.index];
                if toggle.expanded {
                    model.open.insert(folder);
                } else {
                    model.open.remove(&folder);
                }
                model.toggles.push(toggle);
            }
        }
        Vec::new()
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        let shown = rows(model);
        let open = model.open.clone();
        let folders = model.folders;
        let names = shown.clone();
        let count = shown.len();
        let item = Box::new(move |index| match shown[index] {
            (folder, None) => TreeItem {
                level: 0,
                expanded: Some(open.contains(&folder)),
                position: folder,
                siblings: folders,
            },
            (_, Some(file)) => TreeItem {
                level: 1,
                expanded: None,
                position: file,
                siblings: FILES,
            },
        });
        let name = move |index: usize, selected| {
            let name = match names[index] {
                (folder, None) => format!("Folder {folder}"),
                (folder, Some(file)) => format!("File {folder}.{file}"),
            };
            Element::new(Text {
                content: AttributedText::plain(name, TextStyle::new(13.0)),
                role: TextRole::Body,
                wrap: false,
                secondary: false,
                selected,
            })
        };
        if SETUP.get().table {
            let columns = vec![
                Column {
                    title: "Name".into(),
                    width: ColumnWidth::Fill,
                },
                Column {
                    title: "Size".into(),
                    width: ColumnWidth::Fixed(60.0),
                },
            ];
            return window(Element::from(OutlineTable {
                table: TableView {
                    label: "Folders".into(),
                    columns,
                    sort: None,
                    on_sort: None,
                    count,
                    row_height: ROW,
                    scroll: model.scroll,
                    selection: model.selection.clone(),
                    multiple: false,
                    cell: Box::new(move |index, _, selected| name(index, selected)),
                    type_select: None,
                    on_scroll: Box::new(Message::Scroll),
                    on_select: Some(Box::new(Message::Select)),
                    on_activate: None,
                    on_row_key: None,
                    header_top: 0.0,
                },
                item,
                key: None,
                on_toggle: Some(Box::new(Message::Toggle)),
            }));
        }
        window(Element::from(TreeView {
            label: "Folders".into(),
            count,
            row_height: ROW,
            scroll: model.scroll,
            selection: model.selection.clone(),
            multiple: false,
            item,
            row: Box::new(name),
            type_select: None,
            on_scroll: Box::new(Message::Scroll),
            on_select: Some(Box::new(Message::Select)),
            on_activate: None,
            on_toggle: Some(Box::new(Message::Toggle)),
        }))
    }
}

fn harness(folders: usize, open: bool, direction: LayoutDirection) -> Harness<Folders> {
    SETUP.set(Setup {
        folders,
        open,
        empty: false,
        table: false,
    });
    Harness::with_direction(direction)
}

fn row(h: &Harness<Folders>, index: usize) -> Option<NodeId> {
    h.rt.find(h.window, &Key::Int(index as u64))
}

fn focused_row(h: &Harness<Folders>) -> Option<usize> {
    let focus = h.snapshot().focus?;
    (0..rows(h.model()).len()).find(|&i| row(h, i) == Some(focus))
}

fn chord(h: &mut Harness<Folders>, keysym: u32, mods: Mods) {
    h.key_event(keysym, true, false, mods);
    h.key_event(keysym, false, false, mods);
}

#[test]
fn right_opens_a_row_then_moves_to_its_first_child_and_left_moves_to_the_parent_then_closes_it() {
    let mut h = harness(3, false, LayoutDirection::Ltr);
    h.tap(TAB);
    assert_eq!(focused_row(&h), Some(0));
    h.tap(RIGHT);
    assert_eq!(rows(h.model()).len(), 3 + FILES, "folder 0 opened");
    assert_eq!(focused_row(&h), Some(0));
    h.tap(RIGHT);
    assert_eq!(focused_row(&h), Some(1), "to the first file");
    h.tap(RIGHT);
    assert_eq!(focused_row(&h), Some(1), "a file has nothing to open");
    h.tap(LEFT);
    assert_eq!(focused_row(&h), Some(0), "back to its folder");
    h.tap(LEFT);
    assert_eq!(rows(h.model()).len(), 3, "folder 0 closed");
    h.tap(LEFT);
    assert_eq!(focused_row(&h), Some(0), "a top-level row has no parent");

    let mut h = harness(3, false, LayoutDirection::Rtl);
    h.tap(TAB);
    h.tap(LEFT);
    assert_eq!(rows(h.model()).len(), 3 + FILES, "left opens right to left");
}

#[test]
fn primary_with_an_arrow_opens_or_closes_everything_under_the_row() {
    let mut h = harness(3, false, LayoutDirection::Ltr);
    h.tap(TAB);
    chord(&mut h, RIGHT, Mods::PRIMARY);
    chord(&mut h, LEFT, Mods::PRIMARY);
    let toggles: Vec<(bool, bool)> = h
        .model()
        .toggles
        .iter()
        .map(|t| (t.expanded, t.recursive))
        .collect();
    assert_eq!(toggles, [(true, true), (false, true)]);
}

#[test]
fn a_press_on_the_triangle_opens_the_row_without_selecting_it_and_a_press_on_the_row_selects() {
    let mut h = harness(3, false, LayoutDirection::Ltr);
    let r = h
        .snapshot()
        .accessibility
        .into_iter()
        .find(|(id, _)| Some(*id) == row(&h, 1));
    let bounds = r.expect("row 1").1.bounds;
    let y = bounds.min_y() + ROW / 2.0;
    h.click(Point::new(bounds.min_x() + 18.0, y));
    assert_eq!(rows(h.model()).len(), 3 + FILES, "folder 1 opened");
    assert!(h.model().selection.is_empty());
    h.click(Point::new(bounds.min_x() + 80.0, y));
    assert!(h.model().selection.contains(1));
}

#[test]
fn rows_are_tree_items_with_their_level_place_among_siblings_and_whether_they_are_open() {
    let h = harness(3, true, LayoutDirection::Ltr);
    let nodes = h.snapshot().accessibility;
    let tree = nodes
        .iter()
        .find(|(_, n)| n.role == Role::Tree)
        .expect("a tree")
        .1
        .clone();
    assert_eq!(tree.name.as_deref(), Some("Folders"));
    assert_eq!(tree.item_count, None);
    let item = |index| {
        let id = row(&h, index).expect("built");
        nodes
            .iter()
            .find(|(n, _)| *n == id)
            .expect("a node")
            .1
            .clone()
    };
    let folder = item(0);
    assert_eq!(folder.role, Role::TreeItem);
    assert_eq!(
        (
            folder.level,
            folder.item_index,
            folder.item_count,
            folder.states.expanded
        ),
        (Some(1), Some(0), Some(3), Some(true))
    );
    assert!(folder.actions.contains(&Action::Collapse));
    let file = item(3);
    assert_eq!(
        (
            file.level,
            file.item_index,
            file.item_count,
            file.states.expanded
        ),
        (Some(2), Some(2), Some(FILES), None)
    );
}

#[test]
fn an_open_tree_of_ten_thousand_rows_builds_as_many_nodes_as_one_of_a_thousand() {
    let nodes = |folders| {
        let h = harness(folders, true, LayoutDirection::Ltr);
        assert_eq!(rows(h.model()).len(), folders * (1 + FILES));
        h.snapshot().nodes.len()
    };
    assert_eq!(nodes(100), nodes(1000));
}

#[test]
fn right_on_an_open_row_without_children_stays_on_it() {
    SETUP.set(Setup {
        folders: 3,
        open: false,
        empty: true,
        table: false,
    });
    let mut h = Harness::<Folders>::new();
    h.tap(TAB);
    h.tap(RIGHT);
    assert!(h.model().open.contains(&0), "the empty folder opened");
    assert_eq!(rows(h.model()).len(), 3);
    h.tap(RIGHT);
    assert_eq!(focused_row(&h), Some(0), "not to the next folder");
}

#[test]
fn a_screen_reader_opens_and_closes_a_row_with_its_expand_and_collapse_actions() {
    SETUP.set(Setup {
        folders: 3,
        open: false,
        empty: false,
        table: false,
    });
    let mut h = Harness::<Folders>::assistive();
    h.ask(&Key::Int(1), Action::Expand);
    h.ask(&Key::Int(1), Action::Expand);
    h.ask(&Key::Int(1), Action::Collapse);
    let toggle = |expanded| TreeToggle {
        index: 1,
        expanded,
        recursive: false,
    };
    assert_eq!(
        h.model().toggles,
        [toggle(true), toggle(false)],
        "an Expand on an open row changes nothing"
    );
}

#[test]
fn an_outline_table_opens_its_rows_as_a_tree_does_and_reads_each_rows_level() {
    SETUP.set(Setup {
        folders: 3,
        open: false,
        empty: false,
        table: true,
    });
    let mut h = Harness::<Folders>::new();
    h.tap(TAB);
    h.tap(RIGHT);
    assert_eq!(rows(h.model()).len(), 3 + FILES, "Right opened folder 0");
    h.tap(RIGHT);
    assert_eq!(focused_row(&h), Some(1), "to its first file");
    h.tap(LEFT);
    h.tap(LEFT);
    assert_eq!(rows(h.model()).len(), 3, "closed again");

    let nodes = h.snapshot().accessibility;
    let bounds = nodes
        .iter()
        .find(|(id, _)| Some(*id) == row(&h, 1))
        .expect("row 1")
        .1
        .bounds;
    let y = bounds.min_y() + ROW / 2.0;
    h.click(Point::new(bounds.min_x() + 18.0, y));
    assert_eq!(
        rows(h.model()).len(),
        3 + FILES,
        "the triangle opened folder 1"
    );
    assert_eq!(
        h.model().selection.len(),
        1,
        "and kept the selection on folder 0"
    );
    assert!(h.model().selection.contains(0));

    let nodes = h.snapshot().accessibility;
    let read = |index| {
        let id = row(&h, index).expect("built");
        let node = &nodes.iter().find(|(n, _)| *n == id).expect("a node").1;
        (node.role, node.level, node.states.expanded)
    };
    assert_eq!(read(1), (Role::Row, Some(1), Some(true)));
    assert_eq!(read(2), (Role::Row, Some(2), None));
    assert!(nodes.iter().any(|(_, n)| n.role == Role::ColumnHeader));
}
