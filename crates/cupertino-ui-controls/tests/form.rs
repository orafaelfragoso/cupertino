//! The grouped settings form: icon tiles, groups of rows, navigation rows, the header card, the search field and the
//! tiled sidebar of a settings window.

mod support;

use cupertino_geometry::{Insets, Point, Size};
use cupertino_ui_controls::{
    FormBackground, FormGroup, FormHeader, FormRow, IconTile, NavigationRow, PopUpButton,
    SearchField, Stack, Text, TextChange, TextField, TextRole, TileColor, TileSidebar,
    TileSidebarItem, Toggle, FORM_ROW_HEIGHT, FORM_TILE_ROW_HEIGHT, TILE_SIDEBAR_ROW_HEIGHT,
};
use cupertino_ui_core::{
    AccessibilityNode, Action, Application, Command, Element, Key, NodeId, Role, Window,
};
use cupertino_ui_layout::{Align, Axis, ScrollState};
use cupertino_ui_render::{Paint, PaintOp, Shape};
use cupertino_ui_text::{AttributedText, ContentPurpose, TextStyle};
use cupertino_ui_theme::{ColorRole, ControlSize, InteractionVariant};
use support::{color, window, Harness, RETURN, SPACE, TAB};

const UP: u32 = 0xff52;
const DOWN: u32 = 0xff54;
const END: u32 = 0xff57;

#[derive(Clone, Debug, PartialEq)]
enum Message {
    Scroll(ScrollState),
    Select(usize),
    Open,
    Search(TextChange),
}

struct Model {
    scroll: ScrollState,
    selected: Option<usize>,
    reported: Vec<usize>,
    opened: usize,
    query: String,
    revision: u64,
}

const ROWS: [&str; 5] = ["Network", "Sound", "Displays", "Keyboard", "Mouse"];

fn tile(size: f64) -> IconTile {
    IconTile {
        symbol: "symbol.gear".into(),
        color: TileColor::Gray,
        size,
    }
}

fn label(text: &str) -> Element<Message> {
    Element::new(Text {
        content: AttributedText::plain(text, TextStyle::new(13.0)),
        role: TextRole::Body,
        wrap: true,
        secondary: false,
        selected: false,
    })
}

/// A tiled sidebar with a search field over two groups of rows, beside a page: a header card and a group of a plain
/// row, a tiled row and two navigation rows, one of them without a page to open.
struct Settings;

impl Application for Settings {
    type Model = Model;
    type Message = Message;

    fn init() -> (Model, Vec<Command<Message>>) {
        let model = Model {
            scroll: ScrollState {
                offset: Point::ZERO,
                content: Size::ZERO,
                viewport: Size::ZERO,
            },
            selected: None,
            reported: Vec::new(),
            opened: 0,
            query: String::new(),
            revision: 0,
        };
        (model, Vec::new())
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        match message {
            Message::Scroll(scroll) => model.scroll = scroll,
            Message::Select(i) => {
                model.selected = Some(i);
                model.reported.push(i);
            }
            Message::Open => model.opened += 1,
            Message::Search(change) => {
                model.query = change.text;
                model.revision = change.revision;
            }
        }
        Vec::new()
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        let item = |name: &&str| TileSidebarItem {
            key: Key::from(name.to_string()),
            label: name.to_string(),
            symbol: "symbol.gear".into(),
            tile: TileColor::Blue,
        };
        let search = Element::new(SearchField {
            field: TextField {
                label: "Search".into(),
                placeholder: "Search".into(),
                value: model.query.clone(),
                revision: model.revision,
                purpose: ContentPurpose::Normal,
                enabled: true,
                on_change: Some(Box::new(Message::Search)),
                on_submit: None,
                selection: None,
                initial: false,
            },
            size: Some(ControlSize::Large),
        })
        .key("search");
        let sidebar = Element::from(TileSidebar {
            label: "Pages".into(),
            header: Some(search),
            groups: vec![
                ROWS[..2].iter().map(item).collect(),
                ROWS[2..].iter().map(item).collect(),
            ],
            selected: model.selected,
            scroll: model.scroll,
            on_scroll: Box::new(Message::Scroll),
            on_select: Some(Box::new(Message::Select)),
        })
        .key("sidebar")
        .width(200.0);
        let page = Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::ZERO,
            spacing: 10.0,
            align: Align::Stretch,
            children: vec![
                Element::new(FormHeader {
                    tile: tile(48.0),
                    title: "General".into(),
                    description: "Everything at once.".into(),
                })
                .key("header"),
                Element::from(FormGroup {
                    rows: vec![
                        Element::from(FormRow {
                            tile: None,
                            content: label("Plain"),
                        })
                        .key("plain"),
                        Element::from(FormRow {
                            tile: Some(tile(24.0)),
                            content: label("Tiled"),
                        })
                        .key("tiled"),
                        Element::from(FormRow {
                            tile: None,
                            content: Element::new(Toggle {
                                label: "Switch".into(),
                                on: true,
                                enabled: false,
                                on_toggle: None,
                            }),
                        })
                        .key("switch"),
                        Element::from(FormRow {
                            tile: None,
                            content: Element::new(PopUpButton {
                                label: "Pop-up".into(),
                                options: vec!["When Unlocked".into()],
                                selected: 0,
                                open: false,
                                enabled: false,
                                on_open: None,
                            }),
                        })
                        .key("popup"),
                        Element::new(NavigationRow {
                            label: "About".into(),
                            tile: Some(tile(24.0)),
                            on_open: Some(Message::Open),
                        })
                        .key("open"),
                        Element::new(NavigationRow {
                            label: "Storage".into(),
                            tile: None,
                            on_open: None,
                        })
                        .key("closed"),
                    ],
                })
                .key("group"),
            ],
        })
        .fill();
        let page = Element::from(FormBackground { content: page }).key("pane");
        window(Element::new(Stack {
            axis: Axis::Horizontal,
            padding: Insets::ZERO,
            spacing: 0.0,
            align: Align::Stretch,
            children: vec![sidebar, page],
        }))
    }
}

fn harness() -> Harness<Settings> {
    let mut h = Harness::<Settings>::assistive();
    h.resize(Size::new(640.0, 480.0));
    h
}

fn node(h: &Harness<Settings>, key: &'static str) -> AccessibilityNode {
    h.accessible(key)
}

fn row(h: &Harness<Settings>, name: &str) -> NodeId {
    h.rt.find(h.window, &Key::from(name.to_owned())).unwrap()
}

fn focused(h: &Harness<Settings>) -> Option<usize> {
    let focus = h.snapshot().focus?;
    (0..ROWS.len()).find(|&i| row(h, ROWS[i]) == focus)
}

#[test]
fn the_sidebar_moves_its_selection_across_groups_and_reports_each_change_once() {
    let mut h = harness();
    h.tap(TAB);
    h.tap(TAB);
    assert_eq!(
        focused(&h),
        Some(0),
        "after the search field, the first row"
    );
    for key in [DOWN, DOWN, DOWN, UP, END, END] {
        h.tap(key);
        assert_eq!(focused(&h), h.model().selected);
    }
    assert_eq!(h.model().reported, [0, 1, 2, 1, 4]);
    h.ask(&Key::from("Sound"), Action::Click);
    assert_eq!(h.model().selected, Some(1));
}

#[test]
fn a_press_on_the_selected_row_reports_it_once_more_and_keys_that_stay_on_it_report_nothing() {
    let mut h = harness();
    h.click(h.center("Displays"));
    assert_eq!(h.model().reported, [2], "one press, one report");
    h.click(h.center("Displays"));
    assert_eq!(h.model().reported, [2, 2], "the selected row again, once");
    h.ask(&Key::from("Displays"), Action::Click);
    assert_eq!(h.model().reported, [2, 2, 2]);
    assert_eq!(focused(&h), Some(2), "the press keeps the focus on the row");
    h.tap(END);
    h.tap(END);
    assert_eq!(
        h.model().reported,
        [2, 2, 2, 4],
        "a key that leaves the selection where it is reports nothing"
    );
}

#[test]
fn the_sidebar_reads_as_a_list_of_its_rows_with_the_selected_one() {
    let mut h = harness();
    h.click(h.center("Displays"));
    assert_eq!(h.model().reported, [2]);
    let list = h
        .snapshot()
        .accessibility
        .into_iter()
        .find(|(_, n)| n.role == Role::List)
        .unwrap()
        .1;
    assert_eq!(list.name.as_deref(), Some("Pages"));
    let rows: Vec<(Role, Option<String>, bool)> = ROWS
        .iter()
        .map(|name| {
            let id = row(&h, name);
            let (_, n) = h
                .snapshot()
                .accessibility
                .into_iter()
                .find(|(i, _)| *i == id)
                .unwrap();
            (n.role, n.name, n.states.selected)
        })
        .collect();
    let expected: Vec<_> = ROWS
        .iter()
        .enumerate()
        .map(|(i, name)| (Role::ListItem, Some(name.to_string()), i == 2))
        .collect();
    assert_eq!(rows, expected);
}

#[test]
fn a_sidebar_row_is_a_row_tall_and_the_groups_stand_apart() {
    let h = harness();
    let pitch = h.rect("Sound").min_y() - h.rect("Network").min_y();
    assert_eq!(h.rect("Network").height(), TILE_SIDEBAR_ROW_HEIGHT);
    assert_eq!(pitch, TILE_SIDEBAR_ROW_HEIGHT);
    let gap = h.rect("Displays").min_y() - h.rect("Sound").max_y();
    assert!(gap > 0.0, "a group's first row starts below a gap, {gap}");
    assert!(
        h.rect("search").max_y() <= h.rect("Network").min_y(),
        "the search field stands above the rows"
    );
}

#[test]
fn the_selected_sidebar_row_fills_with_the_selected_control_colour() {
    let mut h = harness();
    h.click(h.center("Network"));
    let fill = h.ops("Network").into_iter().find_map(|op| match op {
        PaintOp::Fill {
            shape: Shape::RoundedRect { .. },
            paint: Paint::Solid(c),
        } => Some(c),
        _ => None,
    });
    assert_eq!(
        fill,
        Some(color(
            ColorRole::ControlFill,
            InteractionVariant::Normal,
            true
        ))
    );
}

#[test]
fn the_search_field_edits_and_reads_as_a_text_field_at_a_search_fields_height() {
    let mut h = harness();
    h.click(h.center("search"));
    h.type_text("dock");
    assert_eq!(h.model().query, "dock");
    let field = node(&h, "search");
    assert_eq!(
        (field.role, field.name.as_deref()),
        (Role::TextInput, Some("Search"))
    );
    assert_eq!(h.rect("search").height(), 28.0);
}

#[test]
fn form_rows_are_a_rows_height_with_any_regular_control_and_taller_with_a_tile() {
    let h = harness();
    assert_eq!(h.rect("plain").height(), FORM_ROW_HEIGHT);
    assert_eq!(h.rect("tiled").height(), FORM_TILE_ROW_HEIGHT);
    assert_eq!(h.rect("open").height(), FORM_TILE_ROW_HEIGHT);
    assert_eq!(h.rect("closed").height(), FORM_ROW_HEIGHT);
    // a switch, and a pop-up button padded out to the least pointer target, keep their row at the least height
    assert_eq!(h.rect("switch").height(), FORM_ROW_HEIGHT);
    assert_eq!(h.rect("popup").height(), FORM_ROW_HEIGHT);
    let page = h.rect("header");
    for key in ["plain", "tiled", "switch", "popup", "open", "closed"] {
        let r = h.rect(key);
        assert_eq!(
            (r.min_x(), r.width()),
            (page.min_x(), page.width()),
            "{key} runs the group's full width"
        );
    }
}

#[test]
fn a_navigation_row_opens_on_a_click_return_space_or_assistive_click() {
    let mut h = harness();
    h.click(h.center("open"));
    assert_eq!(h.model().opened, 1);
    // the search field, the sidebar's row, then the row
    for _ in 0..3 {
        h.tap(TAB);
    }
    assert_eq!(
        h.snapshot().focus,
        Some(h.rt.find(h.window, &Key::from("open")).unwrap())
    );
    for key in [RETURN, SPACE] {
        h.tap(key);
    }
    assert_eq!(h.model().opened, 3);
    h.ask(&Key::from("open"), Action::Click);
    assert_eq!(h.model().opened, 4);
    let open = node(&h, "open");
    assert_eq!(
        (open.role, open.name.as_deref(), open.states.focusable),
        (Role::Button, Some("About"), true)
    );
    let closed = node(&h, "closed");
    h.click(h.center("closed"));
    assert_eq!(h.model().opened, 4, "a row without a page opens nothing");
    assert!(closed.states.disabled);
}

#[test]
fn the_header_card_reads_its_title_described_by_its_description() {
    let h = harness();
    let header = node(&h, "header");
    assert_eq!(
        (
            header.role,
            header.name.as_deref(),
            header.description.as_deref()
        ),
        (
            Role::StaticText,
            Some("General"),
            Some("Everything at once.")
        )
    );
    let r = h.rect("header");
    assert!(r.height() > 48.0 + 2.0 * 13.0, "{r:?}");
}

#[test]
fn a_forms_pane_is_the_window_background_and_its_groups_keep_their_own_fill() {
    let window_background = color(
        ColorRole::WindowBackground,
        InteractionVariant::Normal,
        false,
    );
    let text_background = color(ColorRole::TextBackground, InteractionVariant::Normal, false);
    assert_ne!(window_background, text_background);

    // painted by the pane where the desktop draws no material
    let h = harness();
    let pane = h.rect("pane");
    let fills: Vec<_> = h
        .ops("pane")
        .into_iter()
        .filter_map(|op| match op {
            PaintOp::Fill {
                shape: Shape::Rect(rect),
                paint: Paint::Solid(c),
            } => Some((rect, c)),
            _ => None,
        })
        .collect();
    assert_eq!(
        fills,
        [(
            cupertino_geometry::Rect::new(0.0, 0.0, pane.width(), pane.height()),
            window_background
        )],
        "the window background over the pane"
    );
    // the groups on it keep their fill, which is not the pane's
    let group: Vec<_> = h
        .ops("group")
        .into_iter()
        .filter_map(|op| match op {
            PaintOp::Fill { paint, .. } => Some(paint),
            _ => None,
        })
        .collect();
    assert!(!group.is_empty(), "the group fills its rows' room");
    assert!(group
        .iter()
        .all(|p| *p != Paint::Solid(window_background) && *p != Paint::Solid(text_background)));

    // drawn by the compositor, as the window background, where it can
    let mut h = Harness::<Settings>::with_effects(support::theme());
    h.resize(Size::new(640.0, 480.0));
    assert!(
        h.ops("pane").is_empty(),
        "the compositor draws it: {:?}",
        h.ops("pane")
    );
    let regions =
        h.rt.window_system()
            .requests(h.window)
            .unwrap()
            .iter()
            .rev()
            .find_map(|r| match r {
                cupertino_ui_platform::WindowRequest::Materials(regions) => Some(regions.clone()),
                _ => None,
            })
            .expect("the window asks for materials");
    let pane = h.rect("pane");
    assert!(
        regions
            .iter()
            .any(|r| r.material == "WindowBackground" && r.rect == pane),
        "{regions:?}"
    );
    assert!(!regions.iter().any(|r| r.material == "TextBackground"));
}
