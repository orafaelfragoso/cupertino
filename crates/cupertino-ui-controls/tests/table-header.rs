//! A table's sorting headers by keyboard: Tab reaches each column's header before the rows, and Space or Return sorts
//! by that column, flipping the order of the column already sorted. The sorting header tells assistive technology its
//! order. A key on a header never reaches the rows' handler; one on a row does.

mod support;

use cupertino_geometry::{Point, Size};
use cupertino_ui_controls::{Column, ColumnWidth, Selection, SortOrder, TableView, Text, TextRole};
use cupertino_ui_core::{Application, Command, Element, NodeId, Role, Window};
use cupertino_ui_layout::ScrollState;
use cupertino_ui_text::{AttributedText, TextStyle};
use support::{window, Harness, RETURN, SPACE, TAB};

#[derive(Clone, Debug, PartialEq)]
enum Message {
    Scroll(ScrollState),
    Sort((usize, SortOrder)),
    /// A key a row passed on: the row the keyboard was on, and the key.
    RowKey(usize, u32),
}

struct Model {
    scroll: ScrollState,
    sort: (usize, SortOrder),
    sorted: Vec<(usize, SortOrder)>,
    row_keys: Vec<(usize, u32)>,
}

struct Sorted;

impl Application for Sorted {
    type Model = Model;
    type Message = Message;

    fn init() -> (Model, Vec<Command<Message>>) {
        let model = Model {
            scroll: ScrollState {
                offset: Point::ZERO,
                content: Size::ZERO,
                viewport: Size::ZERO,
            },
            sort: (0, SortOrder::Ascending),
            sorted: Vec::new(),
            row_keys: Vec::new(),
        };
        (model, Vec::new())
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        match message {
            Message::Scroll(scroll) => model.scroll = scroll,
            Message::Sort(sort) => {
                model.sort = sort;
                model.sorted.push(sort);
            }
            Message::RowKey(row, keysym) => model.row_keys.push((row, keysym)),
        }
        Vec::new()
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        let column = |title: &str, width| Column {
            title: title.into(),
            width,
        };
        window(Element::from(TableView {
            label: "Files".into(),
            columns: vec![
                column("Name", ColumnWidth::Fill),
                column("Size", ColumnWidth::Fixed(60.0)),
            ],
            sort: Some(model.sort),
            on_sort: Some(Box::new(Message::Sort)),
            count: 3,
            row_height: 20.0,
            scroll: model.scroll,
            selection: Selection::default(),
            multiple: false,
            cell: Box::new(|row, column, selected| {
                Element::new(Text {
                    content: AttributedText::plain(format!("{row}.{column}"), TextStyle::new(13.0)),
                    role: TextRole::Body,
                    wrap: false,
                    secondary: false,
                    selected,
                })
            }),
            type_select: None,
            on_scroll: Box::new(Message::Scroll),
            on_select: None,
            on_activate: None,
            // the rows keep every key but Tab, which goes on to the next control
            on_row_key: Some(Box::new(|row, key| {
                (key.keysym.0 != TAB).then_some(Message::RowKey(row, key.keysym.0))
            })),
            header_top: 0.0,
        }))
    }
}

/// The header of `column`.
fn header(h: &Harness<Sorted>, column: usize) -> NodeId {
    h.snapshot()
        .accessibility
        .into_iter()
        .find(|(_, n)| n.role == Role::ColumnHeader && n.column_index == Some(column))
        .unwrap()
        .0
}

/// Each header's sort order, by column.
fn sorts(h: &Harness<Sorted>) -> Vec<Option<SortOrder>> {
    let mut headers: Vec<_> = h
        .snapshot()
        .accessibility
        .into_iter()
        .filter(|(_, n)| n.role == Role::ColumnHeader)
        .map(|(_, n)| (n.column_index, n.sort))
        .collect();
    headers.sort_by_key(|&(column, _)| column);
    headers.into_iter().map(|(_, sort)| sort).collect()
}

#[test]
fn each_header_is_named_by_its_column_title() {
    let h = Harness::<Sorted>::new();
    let names: Vec<_> = (0..2)
        .map(|column| {
            let header = header(&h, column);
            h.snapshot()
                .accessibility
                .into_iter()
                .find(|(id, _)| *id == header)
                .unwrap()
                .1
                .name
        })
        .collect();
    assert_eq!(names, [Some("Name".to_owned()), Some("Size".to_owned())]);
}

#[test]
fn only_the_sorting_header_tells_its_order_which_follows_each_sort() {
    let mut h = Harness::<Sorted>::new();
    assert_eq!(sorts(&h), [Some(SortOrder::Ascending), None]);
    h.tap(TAB);
    h.tap(SPACE);
    assert_eq!(sorts(&h), [Some(SortOrder::Descending), None]);
    h.tap(TAB);
    h.tap(RETURN);
    assert_eq!(sorts(&h), [None, Some(SortOrder::Ascending)]);
}

#[test]
fn tab_reaches_each_header_and_space_or_return_sorts_by_its_column() {
    let mut h = Harness::<Sorted>::new();
    h.tap(TAB);
    assert_eq!(h.snapshot().focus, Some(header(&h, 0)));
    h.tap(SPACE);
    h.tap(RETURN);
    h.tap(TAB);
    assert_eq!(h.snapshot().focus, Some(header(&h, 1)));
    h.tap(RETURN);
    assert_eq!(
        h.model().sorted,
        [
            (0, SortOrder::Descending),
            (0, SortOrder::Ascending),
            (1, SortOrder::Ascending)
        ]
    );
}

#[test]
fn a_key_on_a_header_never_reaches_the_rows_handler_and_one_on_a_row_does() {
    const X: u32 = 0x78;
    let mut h = Harness::<Sorted>::new();
    h.tap(TAB);
    assert_eq!(h.snapshot().focus, Some(header(&h, 0)));
    h.tap(X);
    h.tap(TAB);
    assert_eq!(h.snapshot().focus, Some(header(&h, 1)));
    h.tap(X);
    assert_eq!(
        h.model().row_keys,
        [],
        "the headers' keys stay off the rows"
    );
    // Tab into the rows, then a key the table does not take itself
    h.tap(TAB);
    h.tap(X);
    assert_eq!(h.model().row_keys, [(0, X)]);
    // the handler let Tab go: it leaves the table, and comes back to the headers
    h.tap(TAB);
    assert_eq!(h.snapshot().focus, Some(header(&h, 0)));
    assert_eq!(h.model().row_keys, [(0, X)]);
}
