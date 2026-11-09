mod support;

use cupertino_geometry::{Point, Rect, Size};
use cupertino_ui_controls::{
    Column, ColumnWidth, ListView, Selection, SortOrder, TableView, Text, TextRole,
};
use cupertino_ui_core::{
    AccessibilityNode, Action, Application, Command, Element, Key, NodeId, Role, Window,
};
use cupertino_ui_layout::ScrollState;
use cupertino_ui_platform::Mods;
use cupertino_ui_render::{Paint, PaintNodeId, PaintOp, Shape};
use cupertino_ui_text::{AttributedText, TextStyle};
use cupertino_ui_theme::{ColorRole, InteractionVariant};
use std::cell::Cell;
use std::collections::HashMap;
use support::{color, window, Harness, RETURN, TAB};

const UP: u32 = 0xff52;
const DOWN: u32 = 0xff54;
const PAGE_DOWN: u32 = 0xff56;
const HOME: u32 = 0xff50;
const END: u32 = 0xff57;
const ROW: f64 = 20.0;
const FRUITS: [&str; 6] = ["apricot", "banana", "blueberry", "cherry", "date", "fig"];

#[derive(Clone, Copy)]
struct Setup {
    count: usize,
    multiple: bool,
    table: bool,
    initial: bool,
}

thread_local! {
    static SETUP: Cell<Setup> = const {
        Cell::new(Setup {
            count: 1000,
            multiple: true,
            table: false,
            initial: false,
        })
    };
}

thread_local! {
    /// The list's selection inset in place of the standard one ([`ListView::inset_element`]).
    static INSET: Cell<Option<f64>> = const { Cell::new(None) };
}

#[derive(Clone, Debug, PartialEq)]
enum Message {
    Scroll(ScrollState),
    Select(Selection),
    Activate(usize),
    Sort((usize, SortOrder)),
}

struct Model {
    scroll: ScrollState,
    selection: Selection,
    activated: Vec<usize>,
    sorted: Vec<(usize, SortOrder)>,
}

fn name(index: usize) -> String {
    FRUITS
        .get(index)
        .map_or_else(|| format!("Item {index}"), |f| (*f).to_owned())
}

fn text(content: String, selected: bool) -> Element<Message> {
    Element::new(Text {
        content: AttributedText::plain(content, TextStyle::new(13.0)),
        role: TextRole::Body,
        wrap: false,
        secondary: false,
        selected,
    })
}

/// A list or a table of `count` rows of 20 filling a 200×100 window.
struct Listed;

impl Application for Listed {
    type Model = Model;
    type Message = Message;

    fn init() -> (Model, Vec<Command<Message>>) {
        let scroll = ScrollState {
            offset: Point::ZERO,
            content: Size::ZERO,
            viewport: Size::ZERO,
        };
        (
            Model {
                scroll,
                selection: Selection::default(),
                activated: Vec::new(),
                sorted: Vec::new(),
            },
            Vec::new(),
        )
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        match message {
            Message::Scroll(state) => model.scroll = state,
            Message::Select(selection) => model.selection = selection,
            Message::Activate(i) => model.activated.push(i),
            Message::Sort(sort) => model.sorted.push(sort),
        }
        Vec::new()
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        let setup = SETUP.get();
        let type_select: Option<Box<dyn Fn(usize) -> String>> = Some(Box::new(name));
        if setup.table {
            return window(Element::from(TableView {
                label: "Fruit".into(),
                columns: vec![
                    Column {
                        title: "Name".into(),
                        width: ColumnWidth::Fill,
                    },
                    Column {
                        title: "Size".into(),
                        width: ColumnWidth::Fixed(60.0),
                    },
                ],
                sort: Some((0, SortOrder::Ascending)),
                on_sort: Some(Box::new(Message::Sort)),
                count: setup.count,
                row_height: ROW,
                scroll: model.scroll,
                selection: model.selection.clone(),
                multiple: setup.multiple,
                cell: Box::new(|i, column, selected| {
                    let content = if column == 0 { name(i) } else { "1 KB".into() };
                    text(content, selected)
                }),
                type_select,
                on_scroll: Box::new(Message::Scroll),
                on_select: Some(Box::new(Message::Select)),
                on_activate: Some(Box::new(Message::Activate)),
                on_row_key: None,
                header_top: 0.0,
            }));
        }
        let list = ListView {
            label: "Fruit".into(),
            count: setup.count,
            row_height: ROW,
            scroll: model.scroll,
            selection: model.selection.clone(),
            multiple: setup.multiple,
            initial: setup.initial,
            row: Box::new(|i, selected| text(name(i), selected)),
            key: None,
            type_select,
            on_scroll: Box::new(Message::Scroll),
            on_select: Some(Box::new(Message::Select)),
            on_activate: Some(Box::new(Message::Activate)),
        };
        window(match INSET.get() {
            Some(inset) => list.inset_element(inset),
            None => Element::from(list),
        })
    }
}

fn harness(count: usize, multiple: bool, table: bool) -> Harness<Listed> {
    SETUP.set(Setup {
        count,
        multiple,
        table,
        initial: false,
    });
    Harness::new()
}

fn row(h: &Harness<Listed>, index: usize) -> Option<NodeId> {
    h.rt.find(h.window, &Key::Int(index as u64))
}

fn row_rect(h: &Harness<Listed>, index: usize) -> Rect {
    let id = row(h, index).unwrap();
    h.snapshot()
        .nodes
        .into_iter()
        .find(|n| n.id == id)
        .unwrap()
        .rect
}

fn row_center(h: &Harness<Listed>, index: usize) -> Point {
    let r = row_rect(h, index);
    Point::new(r.min_x() + r.width() / 2.0, r.min_y() + r.height() / 2.0)
}

/// The indices of the rows that exist as nodes, in order.
fn built(h: &Harness<Listed>) -> Vec<usize> {
    (0..SETUP.get().count)
        .filter(|&i| row(h, i).is_some())
        .collect()
}

fn focused_row(h: &Harness<Listed>) -> Option<usize> {
    let focus = h.snapshot().focus?;
    (0..SETUP.get().count).find(|&i| row(h, i) == Some(focus))
}

/// The selected ranges as `(start, end)`.
fn pairs(selection: &Selection) -> Vec<(usize, usize)> {
    selection
        .ranges()
        .iter()
        .map(|r| (r.start, r.end))
        .collect()
}

fn selected(h: &Harness<Listed>) -> Vec<(usize, usize)> {
    pairs(&h.model().selection)
}

fn chord(h: &mut Harness<Listed>, keysym: u32, mods: Mods) {
    h.key_event(keysym, true, false, mods);
    h.key_event(keysym, false, false, mods);
}

fn roles(h: &Harness<Listed>, role: Role) -> usize {
    h.snapshot()
        .accessibility
        .iter()
        .filter(|(_, n)| n.role == role)
        .count()
}

#[test]
fn a_list_builds_the_rows_near_its_viewport_and_the_cursor_row_and_end_reveals_the_last() {
    let mut h = harness(1000, true, false);
    assert_eq!(h.model().scroll.viewport, Size::new(200.0, 100.0));
    assert_eq!(h.model().scroll.content.height, 20_000.0);
    assert_eq!(
        built(&h),
        (0..8).collect::<Vec<_>>(),
        "five rows in view and half a viewport below"
    );

    h.tap(TAB);
    assert_eq!(focused_row(&h), Some(0));
    h.tap(END);
    assert_eq!(selected(&h), [(999, 1000)]);
    assert_eq!(focused_row(&h), Some(999));
    assert_eq!(h.model().scroll.offset.y, 19_900.0, "the last row shows");
    assert_eq!(built(&h), (992..1000).collect::<Vec<_>>());
    assert_eq!(row_rect(&h, 999).max_y(), 100.0);
}

#[test]
fn arrows_pages_and_home_move_the_cursor_shift_extends_and_primary_a_selects_all_in_one_range() {
    let mut h = harness(1000, true, false);
    h.tap(TAB);
    h.tap(DOWN);
    assert_eq!(
        selected(&h),
        [(0, 1)],
        "the first Down selects the cursor row"
    );
    h.tap(DOWN);
    chord(&mut h, DOWN, Mods::SHIFT);
    chord(&mut h, DOWN, Mods::SHIFT);
    assert_eq!(selected(&h), [(1, 4)]);
    chord(&mut h, UP, Mods::SHIFT);
    assert_eq!(
        selected(&h),
        [(1, 3)],
        "the extension follows the cursor back"
    );
    assert_eq!(focused_row(&h), Some(2));
    h.tap(PAGE_DOWN);
    assert_eq!(selected(&h), [(7, 8)], "a page is the rows in view");
    assert_eq!(focused_row(&h), Some(7));
    assert_eq!(h.model().scroll.offset.y, 60.0);
    h.tap(HOME);
    assert_eq!((selected(&h), focused_row(&h)), (vec![(0, 1)], Some(0)));
    assert_eq!(h.model().scroll.offset.y, 0.0);
    chord(&mut h, 0x61, Mods::PRIMARY);
    assert_eq!(selected(&h), [(0, 1000)]);
}

#[test]
fn a_click_selects_shift_extends_from_the_anchor_and_primary_toggles_without_scrolling() {
    let mut h = harness(1000, true, false);
    h.click(row_center(&h, 1));
    assert_eq!(selected(&h), [(1, 2)]);
    assert_eq!(focused_row(&h), Some(1));
    h.click_with(row_center(&h, 3), Mods::PRIMARY);
    assert_eq!(selected(&h), [(1, 2), (3, 4)]);
    h.click_with(row_center(&h, 4), Mods::SHIFT);
    assert_eq!(
        selected(&h),
        [(1, 2), (3, 5)],
        "the toggled row stays selected"
    );
    h.click_with(row_center(&h, 3), Mods::PRIMARY);
    assert_eq!(selected(&h), [(1, 2), (4, 5)]);
    assert_eq!(h.model().scroll.offset.y, 0.0);
}

#[test]
fn a_single_selection_list_ignores_shift_and_primary() {
    let mut h = harness(1000, false, false);
    h.click(row_center(&h, 1));
    h.click_with(row_center(&h, 3), Mods::SHIFT);
    assert_eq!(selected(&h), [(3, 4)]);
    chord(&mut h, 0x61, Mods::PRIMARY);
    assert_eq!(selected(&h), [(3, 4)]);
}

#[test]
fn typing_selects_the_next_row_that_starts_with_it_and_return_activates_the_cursor_row() {
    let mut h = harness(6, false, false);
    h.tap(TAB);
    h.type_text("b");
    assert_eq!(selected(&h), [(1, 2)]);
    h.type_text("l");
    assert_eq!(selected(&h), [(2, 3)], "the search goes on from the cursor");
    h.clock().advance_by(1_000_000_000);
    h.type_text("b");
    assert_eq!(
        selected(&h),
        [(1, 2)],
        "a new search starts after the cursor and wraps"
    );
    h.tap(RETURN);
    assert_eq!(h.model().activated, [1]);
}

#[test]
fn an_initial_list_takes_the_focus_when_its_window_opens_so_the_arrows_work_at_once() {
    assert_eq!(focused_row(&harness(1000, false, false)), None);
    SETUP.set(Setup {
        count: 1000,
        multiple: false,
        table: false,
        initial: true,
    });
    let mut h = Harness::<Listed>::new();
    assert_eq!(focused_row(&h), Some(0));
    chord(&mut h, DOWN, Mods::NONE);
    assert_eq!(selected(&h), vec![(0, 1)]);
}

#[test]
fn only_the_cursor_row_is_a_tab_stop_and_rows_say_whether_they_are_selected() {
    let mut h = harness(1000, true, false);
    let list = h
        .snapshot()
        .accessibility
        .into_iter()
        .find(|(_, n)| n.role == Role::List)
        .unwrap()
        .1;
    assert_eq!(list.name.as_deref(), Some("Fruit"));
    assert!(list.states.multiselectable);
    h.click(row_center(&h, 2));
    let items: Vec<(bool, bool)> = h
        .snapshot()
        .accessibility
        .into_iter()
        .filter(|(_, n)| n.role == Role::ListItem)
        .map(|(_, n)| (n.states.selected, n.states.focusable))
        .collect();
    let mut expected = vec![(false, false); 8];
    expected[2] = (true, true);
    assert_eq!(items, expected);
}

#[test]
fn a_list_and_a_table_tell_assistive_technology_all_their_rows_and_each_built_rows_place() {
    for (table, container, item) in [
        (false, Role::List, Role::ListItem),
        (true, Role::Table, Role::Row),
    ] {
        let mut h = harness(1000, true, table);
        // a table's two sorting headers come before its rows
        for _ in 0..if table { 3 } else { 1 } {
            h.tap(TAB);
        }
        h.tap(END);
        let nodes: HashMap<_, _> = h.snapshot().accessibility.into_iter().collect();
        let rows = nodes.values().find(|n| n.role == container).unwrap();
        let mut stack: Vec<NodeId> = rows.children.iter().rev().copied().collect();
        let mut places = Vec::new();
        while let Some(id) = stack.pop() {
            let node = &nodes[&id];
            if node.role == item {
                places.push(node.item_index);
            }
            stack.extend(node.children.iter().rev());
        }
        assert_eq!(rows.item_count, Some(1000), "{container:?}");
        assert_eq!(built(&h).last(), Some(&999));
        let header = table.then_some(None);
        assert_eq!(
            places,
            header
                .into_iter()
                .chain(built(&h).into_iter().map(Some))
                .collect::<Vec<_>>(),
            "{item:?} in reading order, a table's header row first and without a place"
        );
    }
}

#[test]
fn a_selected_row_fills_inset_with_the_selected_control_colour() {
    let mut h = harness(1000, true, false);
    h.click(row_center(&h, 1));
    let id = PaintNodeId::from(row(&h, 1).unwrap());
    let ops = h
        .snapshot()
        .display_list
        .nodes
        .into_iter()
        .find(|n| n.id == id)
        .unwrap()
        .ops;
    assert_eq!(
        ops,
        [PaintOp::Fill {
            shape: Shape::RoundedRect {
                rect: Rect::new(4.0, 0.0, 192.0, 20.0),
                radius: 5.0,
            },
            paint: Paint::Solid(color(
                ColorRole::ControlFill,
                InteractionVariant::Normal,
                true
            )),
        }]
    );
}

#[test]
fn a_list_built_with_its_own_selection_inset_fills_the_selection_that_far_in() {
    INSET.set(Some(10.0));
    let mut h = harness(1000, true, false);
    h.click(row_center(&h, 1));
    let id = PaintNodeId::from(row(&h, 1).unwrap());
    let ops = h
        .snapshot()
        .display_list
        .nodes
        .into_iter()
        .find(|n| n.id == id)
        .unwrap()
        .ops;
    assert!(
        matches!(
            ops.as_slice(),
            [PaintOp::Fill {
                shape: Shape::RoundedRect { rect, radius },
                ..
            }] if *rect == Rect::new(10.0, 0.0, 180.0, 20.0) && *radius == 5.0
        ),
        "{ops:?}"
    );
}

#[test]
fn a_table_lines_its_cells_up_under_the_header_and_a_title_click_sorts_the_other_way() {
    let mut h = harness(1000, true, true);
    assert_eq!(roles(&h, Role::Table), 1);
    assert_eq!(roles(&h, Role::ColumnHeader), 2);
    assert_eq!(roles(&h, Role::Cell), 2 * built(&h).len());
    let headers: Vec<Rect> = h
        .snapshot()
        .accessibility
        .into_iter()
        .filter(|(_, n)| n.role == Role::ColumnHeader)
        .map(|(_, n)| n.bounds)
        .collect();
    let cells: Vec<Rect> = h
        .snapshot()
        .accessibility
        .into_iter()
        .filter(|(_, n)| n.role == Role::Cell)
        .take(2)
        .map(|(_, n)| n.bounds)
        .collect();
    assert_eq!(headers[1].width(), 60.0);
    for (header, cell) in headers.iter().zip(&cells) {
        assert_eq!(
            (header.min_x(), header.width()),
            (cell.min_x(), cell.width())
        );
    }
    assert_eq!(
        row_rect(&h, 0).min_y(),
        28.0,
        "the rows start under the header"
    );
    let dividers = h
        .snapshot()
        .display_list
        .nodes
        .iter()
        .flat_map(|n| &n.ops)
        .filter(|op| matches!(op, PaintOp::Fill { shape: Shape::Rect(r), .. } if r.width() == 1.0))
        .count();
    assert_eq!(
        dividers, 1,
        "one divider between the two columns, none after the last"
    );
    let title = Point::new(headers[0].min_x() + 10.0, 14.0);
    h.click(title);
    assert_eq!(h.model().sorted, [(0, SortOrder::Descending)]);
}

#[test]
fn assistive_technology_reads_one_table_with_its_header_row_first_and_every_row_under_it() {
    let h = harness(1000, true, true);
    let nodes: HashMap<_, _> = h.snapshot().accessibility.into_iter().collect();
    let (&table, node) = nodes.iter().find(|(_, n)| n.role == Role::Table).unwrap();
    assert_eq!(
        (node.name.as_deref(), node.states.multiselectable),
        (Some("Fruit"), true)
    );
    let header = &nodes[&node.children[0]];
    let titles: Vec<Role> = header.children.iter().map(|c| nodes[c].role).collect();
    assert_eq!(
        (header.role, titles),
        (Role::Row, vec![Role::ColumnHeader; 2])
    );
    let parents: HashMap<NodeId, NodeId> = nodes
        .iter()
        .flat_map(|(&p, n)| n.children.iter().map(move |&c| (c, p)))
        .collect();
    let in_table = |mut id: NodeId| loop {
        match parents.get(&id) {
            Some(&p) if p == table => break true,
            Some(&p) => id = p,
            None => break false,
        }
    };
    let rows: Vec<bool> = nodes
        .iter()
        .filter(|(_, n)| n.role == Role::Row)
        .map(|(&id, _)| in_table(id))
        .collect();
    assert_eq!(rows, vec![true; built(&h).len() + 1]);
    let headers: Vec<Option<usize>> = header
        .children
        .iter()
        .map(|c| nodes[c].column_index)
        .collect();
    assert_eq!(headers, [Some(0), Some(1)]);
    let cells: Vec<Option<usize>> = nodes
        .values()
        .filter(|n| {
            n.role == Role::Row
                && !n.children.is_empty()
                && nodes[&n.children[0]].role == Role::Cell
        })
        .flat_map(|n| n.children.iter().map(|c| nodes[c].column_index))
        .collect();
    assert_eq!(
        cells.len(),
        2 * built(&h).len(),
        "every cell carries its column"
    );
    assert!(cells.chunks(2).all(|pair| pair == headers), "{cells:?}");
}

#[test]
fn extending_replaces_only_the_anchors_range_and_clamp_drops_rows_past_the_end() {
    let mut s = Selection::default();
    s.select_only(2);
    s.toggle(6);
    s.extend_to(8);
    assert_eq!(pairs(&s), [(2, 3), (6, 9)]);
    s.extend_to(7);
    assert_eq!(pairs(&s), [(2, 3), (6, 8)]);
    s.extend_to(1);
    assert_eq!(
        pairs(&s),
        [(1, 7)],
        "the new extension merges with what it reaches"
    );
    assert_eq!((s.anchor(), s.cursor()), (Some(6), Some(1)));
    s.toggle(3);
    assert_eq!(pairs(&s), [(1, 3), (4, 7)]);
    assert_eq!(s.len(), 5);
    s.clamp(5);
    assert_eq!(pairs(&s), [(1, 3), (4, 5)]);
    assert_eq!((s.anchor(), s.cursor()), (Some(3), Some(3)));
    assert_eq!(
        pairs(&Selection::new([4..5, 0..0, 1..3, 2..4], None, None)),
        [(1, 5)],
        "built from overlapping, touching and empty ranges"
    );
    s.clamp(3);
    assert_eq!((pairs(&s), s.cursor()), (vec![(1, 3)], None));
}

/// The fruit, each row keyed by its name, where Return on a row puts an avocado at the top and keeps the same fruit
/// selected, as a list whose items come and go does.
struct Grocer;

impl Application for Grocer {
    type Model = (Model, Vec<&'static str>);
    type Message = Message;

    fn init() -> ((Model, Vec<&'static str>), Vec<Command<Message>>) {
        ((Listed::init().0, FRUITS.to_vec()), Vec::new())
    }

    fn update(
        (model, fruit): &mut (Model, Vec<&'static str>),
        message: Message,
    ) -> Vec<Command<Message>> {
        if let Message::Activate(i) = message {
            fruit.insert(0, "avocado");
            model.selection.select_only(i + 1);
        }
        Listed::update(model, message)
    }

    fn view((model, fruit): &(Model, Vec<&'static str>)) -> Vec<Window<Message>> {
        let names = fruit.clone();
        let keys = fruit.clone();
        window(Element::from(ListView {
            label: "Fruit".into(),
            count: fruit.len(),
            row_height: ROW,
            scroll: model.scroll,
            selection: model.selection.clone(),
            multiple: false,
            initial: false,
            row: Box::new(move |i, selected| text(names[i].into(), selected)),
            key: Some(Box::new(move |i| Key::from(keys[i]))),
            type_select: None,
            on_scroll: Box::new(Message::Scroll),
            on_select: Some(Box::new(Message::Select)),
            on_activate: Some(Box::new(Message::Activate)),
        }))
    }
}

/// `id`'s place in the list and whether it is selected, as assistive technology reads them.
fn read(h: &Harness<Grocer>, id: NodeId) -> (Option<usize>, bool) {
    let nodes: HashMap<NodeId, AccessibilityNode> =
        h.snapshot().accessibility.into_iter().collect();
    (nodes[&id].item_index, nodes[&id].states.selected)
}

#[test]
fn the_focus_and_the_accessible_row_stay_on_their_item_when_a_row_is_inserted_above() {
    let mut h = Harness::<Grocer>::new();
    h.tap(TAB);
    h.tap(HOME);
    for _ in 0..3 {
        h.tap(DOWN);
    }
    let cherry = h.snapshot().focus.unwrap();
    assert_eq!(read(&h, cherry), (Some(3), true));
    h.tap(RETURN);
    assert_eq!(
        h.snapshot().focus,
        Some(cherry),
        "the focus stays on cherry"
    );
    assert_eq!(read(&h, cherry), (Some(4), true));
}

#[test]
fn an_assistive_click_after_an_insert_selects_the_row_it_was_asked_of() {
    let mut h = Harness::<Grocer>::assistive();
    h.tap(TAB);
    h.tap(HOME);
    for _ in 0..4 {
        h.tap(DOWN);
    }
    let date = h.snapshot().focus.unwrap();
    h.tap(UP);
    h.tap(RETURN);
    h.ask_node(date, Action::Click);
    assert_eq!(selected_in(&h), [(5, 6)], "date, now under the avocado");
    assert_eq!(read(&h, date), (Some(5), true));
    h.ask_node(date, Action::Click);
    assert_eq!(selected_in(&h), [(5, 6)]);
}

fn selected_in(h: &Harness<Grocer>) -> Vec<(usize, usize)> {
    pairs(&h.model().0.selection)
}

#[test]
fn an_assistive_click_on_a_table_row_selects_it_alone() {
    SETUP.set(Setup {
        count: 1000,
        multiple: true,
        table: true,
        initial: false,
    });
    let mut h = Harness::<Listed>::assistive();
    let third = row(&h, 2).unwrap();
    let actions = |h: &Harness<Listed>| {
        let nodes: HashMap<NodeId, AccessibilityNode> =
            h.snapshot().accessibility.into_iter().collect();
        nodes[&third].actions.clone()
    };
    assert_eq!(actions(&h), [Action::Click], "a table row offers Click");
    h.ask_node(third, Action::Click);
    assert_eq!(selected(&h), [(2, 3)]);
}
