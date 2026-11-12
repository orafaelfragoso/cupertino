//! Menus: a context menu opens at a secondary click or from the keyboard, a pop-up button's menu below the button.
//! The keyboard moves between the enabled items and opens and closes submenus, an item is chosen by Return or a
//! click, and Escape or a press outside closes the menu.

use crate::support;

use cupertino_geometry::{Insets, Point, Rect};
use cupertino_ui_controls::{
    Button, ButtonRole, ButtonStyle, ContextMenu, Menu, MenuAnchor, MenuItem, PopUpButton, Stack,
};
use cupertino_ui_core::{
    AccessibilityNode, AccessibleValue, Action, Application, Checked, Command, Element, Key,
    NodeId, Role, Window, WindowKey,
};
use cupertino_ui_layout::{Align, Axis, LayoutDirection};
use cupertino_ui_platform::{
    Mods, MouseButton, PointerEvent, PopupEdge, WindowEvent, WindowId, WindowStates,
};
use cupertino_ui_render::{PaintNodeId, PaintOp};
use support::{window, Harness, RETURN, SPACE, TAB};

const LEFT: u32 = 0xff51;
const UP: u32 = 0xff52;
const RIGHT: u32 = 0xff53;
const DOWN: u32 = 0xff54;
const HOME: u32 = 0xff50;
const END: u32 = 0xff57;
const ESCAPE: u32 = 0xff1b;
const MENU: u32 = 0xff67;
const F10: u32 = 0xffc7;

const SIZES: [&str; 3] = ["Small", "Medium", "Large"];

#[derive(Clone, Debug, PartialEq)]
enum Message {
    Context(Point),
    Sizes,
    Open(Vec<usize>),
    Close,
    Choose(&'static str),
    Size(usize),
}

#[derive(Clone, Debug, PartialEq)]
enum Open {
    Context(Point),
    Sizes,
}

#[derive(Default)]
struct Model {
    menu: Option<Open>,
    submenus: Vec<usize>,
    chosen: Vec<&'static str>,
    closes: usize,
    size: usize,
}

struct Editor;

fn item(label: &'static str, enabled: bool) -> MenuItem<Message> {
    MenuItem::Item {
        label: label.into(),
        enabled,
        checked: false,
        on_select: Message::Choose(label),
    }
}

fn shortcut(label: &'static str, keys: &str, enabled: bool) -> MenuItem<Message> {
    MenuItem::Shortcut {
        label: label.into(),
        shortcut: keys.into(),
        enabled,
        checked: false,
        on_select: Message::Choose(label),
    }
}

fn sizes(model: &Model) -> PopUpButton<Message> {
    PopUpButton {
        label: "Size".into(),
        options: SIZES.iter().map(|s| (*s).to_owned()).collect(),
        selected: model.size,
        open: model.menu == Some(Open::Sizes),
        enabled: true,
        on_open: Some(Message::Sizes),
    }
}

impl Application for Editor {
    type Model = Model;
    type Message = Message;

    fn init() -> (Model, Vec<Command<Message>>) {
        (Model::default(), Vec::new())
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        match message {
            Message::Context(at) => model.menu = Some(Open::Context(at)),
            Message::Sizes => model.menu = Some(Open::Sizes),
            Message::Open(path) => model.submenus = path,
            Message::Close => {
                model.menu = None;
                model.submenus.clear();
                model.closes += 1;
            }
            Message::Choose(label) => model.chosen.push(label),
            Message::Size(size) => model.size = size,
        }
        Vec::new()
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        let parent = WindowKey("test".into());
        let popup = sizes(model);
        let mut windows = window(Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::uniform(10.0),
            spacing: 10.0,
            align: Align::Start,
            children: vec![
                Element::new(sizes(model)).key("size"),
                Element::new(ContextMenu {
                    content: Element::new(Button {
                        label: "Canvas".into(),
                        role: ButtonRole::Normal,
                        style: ButtonStyle::Bordered,
                        enabled: true,
                        busy: false,
                        on_press: None,
                    })
                    .key("canvas"),
                    on_open: Some(Box::new(Message::Context)),
                })
                .key("region"),
            ],
        }));
        match model.menu {
            Some(Open::Context(at)) => windows.extend(
                Menu {
                    parent,
                    anchor: MenuAnchor::At(at),
                    label: "Canvas".into(),
                    items: vec![
                        item("Cut", true),
                        shortcut("Copy", "\u{2318}C", true),
                        shortcut("Paste", "\u{2318}V", false),
                        MenuItem::Separator,
                        MenuItem::Submenu {
                            label: "Share".into(),
                            enabled: true,
                            items: vec![item("Mail", true), item("Messages", true)],
                        },
                    ],
                    current: None,
                    open: model.submenus.clone(),
                    on_open: Box::new(Message::Open),
                    on_close: Message::Close,
                }
                .windows(WindowKey("menu".into())),
            ),
            Some(Open::Sizes) => windows.extend(
                popup
                    .menu(
                        parent,
                        "size".into(),
                        Box::new(Message::Size),
                        Message::Close,
                    )
                    .windows(WindowKey("sizes".into())),
            ),
            None => {}
        }
        windows
    }
}

fn popup(h: &Harness<Editor>, key: &'static str) -> Option<WindowId> {
    h.rt.windows()
        .into_iter()
        .find(|(k, _)| *k == WindowKey(key.into()))
        .map(|(_, id)| id)
}

/// Moves the harness onto the open popup `key`: input and queries go to it from now on.
fn enter(h: &mut Harness<Editor>, key: &'static str) -> WindowId {
    let from = h.window;
    let id = popup(h, key).expect("the popup is open");
    h.rt.window_system_mut()
        .configure(id, None, WindowStates::default())
        .unwrap();
    h.window = id;
    h.settle();
    from
}

fn row(h: &Harness<Editor>, index: u64) -> NodeId {
    h.rt.find(h.window, &Key::from(index)).unwrap()
}

fn semantic(h: &Harness<Editor>, id: NodeId) -> AccessibilityNode {
    h.snapshot()
        .accessibility
        .into_iter()
        .find(|(n, _)| *n == id)
        .unwrap()
        .1
}

fn on_row(h: &Harness<Editor>, index: u64) -> bool {
    h.snapshot().focus == Some(row(h, index))
}

fn highlighted(h: &Harness<Editor>, index: u64) -> bool {
    let id = PaintNodeId::from(row(h, index));
    h.snapshot()
        .display_list
        .nodes
        .into_iter()
        .find(|n| n.id == id)
        .is_some_and(|n| n.ops.iter().any(|op| matches!(op, PaintOp::Fill { .. })))
}

fn row_rect(h: &Harness<Editor>, index: u64) -> Rect {
    let id = row(h, index);
    h.snapshot()
        .nodes
        .into_iter()
        .find(|n| n.id == id)
        .unwrap()
        .rect
}

fn secondary_click(h: &mut Harness<Editor>, position: Point) {
    h.pointer(PointerEvent::Motion { position });
    for pressed in [true, false] {
        h.pointer(PointerEvent::Button {
            position,
            button: MouseButton::Right,
            pressed,
            serial: 1,
            mods: Mods::NONE,
        });
    }
}

/// Opens the context menu with a secondary click on the canvas and moves onto it.
fn context(h: &mut Harness<Editor>) -> WindowId {
    secondary_click(h, h.center("canvas"));
    enter(h, "menu")
}

#[test]
fn a_secondary_click_opens_the_context_menu_at_the_pointer() {
    let mut h = Harness::<Editor>::new();
    let at = h.center("canvas");
    secondary_click(&mut h, at);
    assert_eq!(h.model().menu, Some(Open::Context(at)));
    let id = popup(&h, "menu").expect("open");
    let system = h.rt.window_system();
    assert_eq!(system.parent(id).unwrap(), Some(h.window));
    let placed = system.placement(id).unwrap().unwrap();
    assert_eq!(placed.anchor_rect, Rect::new(at.x, at.y, 1.0, 1.0));
    assert_eq!(
        (placed.anchor, placed.gravity),
        (PopupEdge::BottomRight, PopupEdge::BottomRight)
    );
    assert!(placed.grab);
}

#[test]
fn the_menu_key_and_shift_f10_open_it_at_the_region() {
    let mut h = Harness::<Editor>::new();
    let region = h.rect("region");
    h.tap(TAB);
    h.tap(TAB);
    assert!(h.focused("canvas"));
    h.tap(MENU);
    assert_eq!(h.model().menu, Some(Open::Context(region.origin)));
    h.send(WindowEvent::PopupDone);
    h.key_event(F10, true, false, Mods::SHIFT);
    assert_eq!(h.model().menu, Some(Open::Context(region.origin)));
}

#[test]
fn arrows_home_end_and_typed_characters_move_between_the_enabled_items() {
    let mut h = Harness::<Editor>::new();
    context(&mut h);
    h.tap(DOWN);
    assert!(on_row(&h, 0), "the first item");
    h.tap(DOWN);
    assert!(on_row(&h, 1));
    h.tap(DOWN);
    assert!(on_row(&h, 4), "past the disabled item and the separator");
    h.tap(DOWN);
    assert!(on_row(&h, 0), "around to the first");
    h.tap(UP);
    assert!(on_row(&h, 4));
    h.tap(HOME);
    assert!(on_row(&h, 0));
    h.tap(END);
    assert!(on_row(&h, 4));
    h.type_text("c");
    assert!(on_row(&h, 0));
    h.type_text("C");
    assert!(on_row(&h, 1), "the next item starting with it");
    h.type_text("p");
    assert!(on_row(&h, 1), "a disabled item is not typed to");
    // after a pause, a new search
    h.clock().advance_by(1_100_000_000);
    h.type_text("s");
    assert!(on_row(&h, 4));
}

#[test]
fn a_menu_opened_with_the_pointer_highlights_nothing_until_the_pointer_moves_and_from_the_keyboard_its_first_item(
) {
    let mut h = Harness::<Editor>::new();
    let main = context(&mut h);
    assert!(on_row(&h, 0));
    assert!(!highlighted(&h, 0));
    let copy = row_rect(&h, 1);
    h.pointer(PointerEvent::Motion {
        position: Point::new(copy.min_x() + 20.0, copy.min_y() + 10.0),
    });
    h.settle();
    assert_eq!((highlighted(&h, 0), highlighted(&h, 1)), (false, true));
    h.send(WindowEvent::PopupDone);
    h.window = main;
    h.tap(TAB);
    h.tap(TAB);
    h.tap(MENU);
    enter(&mut h, "menu");
    assert!(on_row(&h, 0));
    assert!(highlighted(&h, 0));
}

#[test]
fn in_a_menu_opened_with_the_pointer_return_and_space_choose_nothing_and_up_goes_to_the_last_item()
{
    let mut h = Harness::<Editor>::new();
    context(&mut h);
    h.tap(RETURN);
    h.tap(SPACE);
    assert!(h.model().chosen.is_empty());
    assert_eq!(h.model().closes, 0);
    assert!(popup(&h, "menu").is_some());
    h.tap(UP);
    assert!(on_row(&h, 4));
    assert!(highlighted(&h, 4));
}

#[test]
fn return_or_a_click_chooses_an_item_and_closes_the_menu() {
    let mut h = Harness::<Editor>::new();
    let main = context(&mut h);
    h.tap(DOWN);
    h.tap(DOWN);
    h.key(RETURN, true);
    assert_eq!(h.model().chosen, ["Copy"]);
    assert_eq!(h.model().closes, 1);
    assert_eq!(popup(&h, "menu"), None);
    h.window = main;
    context(&mut h);
    let cut = row_rect(&h, 0);
    h.click(Point::new(cut.min_x() + 20.0, cut.min_y() + 10.0));
    assert_eq!(h.model().chosen, ["Copy", "Cut"]);
    assert_eq!(h.model().closes, 2);
}

#[test]
fn escape_or_a_press_outside_closes_it_without_choosing() {
    let mut h = Harness::<Editor>::new();
    h.tap(TAB);
    h.tap(TAB);
    h.tap(MENU);
    let main = enter(&mut h, "menu");
    h.key(ESCAPE, true);
    assert_eq!((h.model().closes, popup(&h, "menu")), (1, None));
    h.window = main;
    assert!(
        h.focused("canvas"),
        "the focus is back where the menu opened"
    );
    context(&mut h);
    h.send(WindowEvent::PopupDone);
    assert_eq!((h.model().closes, popup(&h, "menu")), (2, None));
    assert!(h.model().chosen.is_empty());
}

#[test]
fn the_trailing_arrow_opens_a_submenu_beside_its_item_and_the_leading_arrow_or_escape_closes_it() {
    let mut h = Harness::<Editor>::new();
    context(&mut h);
    let menu = h.window;
    h.tap(END);
    let share = row_rect(&h, 4);
    h.tap(RIGHT);
    assert_eq!(h.model().submenus, [4]);
    let id = popup(&h, "menu/1").expect("the submenu is open");
    let system = h.rt.window_system();
    assert_eq!(system.parent(id).unwrap(), Some(menu));
    let placed = system.placement(id).unwrap().unwrap();
    assert_eq!(placed.anchor_rect, share);
    assert_eq!(
        (placed.anchor, placed.gravity, placed.offset),
        (
            PopupEdge::TopRight,
            PopupEdge::BottomRight,
            Point::new(-2.0, -5.0)
        )
    );
    enter(&mut h, "menu/1");
    assert!(on_row(&h, 0), "the submenu's first item");
    h.key(LEFT, true);
    assert_eq!(h.model().submenus, Vec::<usize>::new());
    assert_eq!(popup(&h, "menu/1"), None);
    h.window = menu;
    assert!(on_row(&h, 4), "back on the submenu's item");
    h.tap(RETURN);
    enter(&mut h, "menu/1");
    h.key(ESCAPE, true);
    assert_eq!(h.model().submenus, Vec::<usize>::new());
    assert!(
        popup(&h, "menu").is_some(),
        "Escape closes only the submenu"
    );
    h.window = menu;
    h.tap(RIGHT);
    enter(&mut h, "menu/1");
    h.tap(DOWN);
    h.key(RETURN, true);
    assert_eq!(h.model().chosen, ["Messages"]);
    assert_eq!((popup(&h, "menu"), popup(&h, "menu/1")), (None, None));
}

#[test]
fn in_right_to_left_the_left_arrow_opens_a_submenu_on_the_left() {
    let mut h = Harness::<Editor>::with_direction(LayoutDirection::Rtl);
    context(&mut h);
    let menu = h.window;
    h.tap(END);
    h.tap(RIGHT);
    assert!(
        h.model().submenus.is_empty(),
        "Right leads in right-to-left"
    );
    h.tap(LEFT);
    assert_eq!(h.model().submenus, [4]);
    let id = popup(&h, "menu/1").expect("open");
    let placed = h.rt.window_system().placement(id).unwrap().unwrap();
    assert_eq!(
        (placed.anchor, placed.gravity, placed.offset),
        (
            PopupEdge::TopLeft,
            PopupEdge::BottomLeft,
            Point::new(2.0, -5.0)
        )
    );
    enter(&mut h, "menu/1");
    h.key(RIGHT, true);
    assert!(h.model().submenus.is_empty());
    h.window = menu;
    assert!(on_row(&h, 4));
}

#[test]
fn assistive_technology_reads_a_menu_of_items_and_clicks_one() {
    let mut h = Harness::<Editor>::assistive();
    context(&mut h);
    let nodes = h.snapshot().accessibility;
    let menu = nodes.iter().find(|(_, n)| n.role == Role::Menu).unwrap();
    assert_eq!(menu.1.name.as_deref(), Some("Canvas"));
    let node = |index: u64| semantic(&h, row(&h, index));
    let items: Vec<_> = [0, 1, 2, 4]
        .into_iter()
        .map(|i| {
            let n = node(i);
            (
                n.role,
                n.name.unwrap(),
                n.states.disabled,
                n.actions.contains(&Action::Click),
            )
        })
        .collect();
    assert_eq!(
        items,
        [
            (Role::MenuItem, "Cut".to_owned(), false, true),
            (Role::MenuItem, "Copy".to_owned(), false, true),
            (Role::MenuItem, "Paste".to_owned(), true, false),
            (Role::MenuItem, "Share".to_owned(), false, true),
        ]
    );
    assert_eq!(node(3).role, Role::Separator);
    assert_eq!(node(4).states.expanded, Some(false));
    assert_eq!(node(0).states.expanded, None);
    h.ask(&Key::from(1u64), Action::Click);
    assert_eq!(h.model().chosen, ["Copy"]);
}

#[test]
fn an_items_shortcut_sits_at_its_trailing_edge_in_a_dimmer_colour_and_is_read_as_its_description() {
    let mut h = Harness::<Editor>::assistive();
    context(&mut h);
    let description = |index: u64| semantic(&h, row(&h, index)).description;
    assert_eq!(
        (description(0), description(1), description(2)),
        (
            None,
            Some("\u{2318}C".to_owned()),
            Some("\u{2318}V".to_owned())
        )
    );
    let glyphs = |index: u64| {
        let id = PaintNodeId::from(row(&h, index));
        let node = h
            .snapshot()
            .display_list
            .nodes
            .into_iter()
            .find(|n| n.id == id)
            .unwrap();
        node.ops
            .into_iter()
            .filter_map(|op| match op {
                PaintOp::Glyphs { origin, color, .. } => Some((origin.x, color)),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let copy = glyphs(1);
    assert_eq!(copy.len(), 2, "the label and the keys");
    let width = row_rect(&h, 1).width();
    assert!(
        copy[0].0 < 40.0 && copy[1].0 > width / 2.0,
        "{copy:?} in {width}"
    );
    assert_ne!(copy[0].1, copy[1].1, "the keys are dimmer than the label");
    assert_eq!(glyphs(0).len(), 1);
}

#[test]
fn a_pop_up_button_opens_its_options_over_itself_with_the_chosen_one_checked_on_its_label() {
    let mut h = Harness::<Editor>::assistive();
    let button = h.rect("size");
    h.tap(TAB);
    h.tap(SPACE);
    assert_eq!(h.model().menu, Some(Open::Sizes));
    let id = popup(&h, "sizes").expect("open");
    let placed = h.rt.window_system().placement(id).unwrap().unwrap().clone();
    assert_eq!(placed.anchor_rect, button);
    // the button's label is 10 pt in, a row's 31 in a menu with check marks; the chosen first row's middle is 5 + 12
    // down
    assert_eq!(
        (placed.anchor, placed.gravity, placed.offset),
        (
            PopupEdge::Left,
            PopupEdge::BottomRight,
            Point::new(-21.0, -17.0)
        )
    );
    assert!(!placed.constraint.flip, "it slides to fit and never flips");
    let main = enter(&mut h, "sizes");
    let checked: Vec<_> = (0..3)
        .map(|i| semantic(&h, row(&h, i)).value == Some(AccessibleValue::Toggle(Checked::True)))
        .collect();
    assert_eq!(checked, [true, false, false]);
    h.tap(END);
    h.key(RETURN, true);
    assert_eq!((h.model().size, h.model().menu.clone()), (2, None));
    h.window = main;
    let size = &semantic(&h, h.rt.find(h.window, &Key::from("size")).unwrap());
    assert_eq!(size.role, Role::PopUpButton);
    assert_eq!(size.name.as_deref(), Some("Size"));
    assert!(matches!(&size.value, Some(AccessibleValue::Text { value, .. }) if value == "Large"));
    assert_eq!(size.states.expanded, Some(false));
    h.tap(DOWN);
    assert_eq!(h.model().menu, Some(Open::Sizes), "Down opens it too");
    let dismiss = |h: &mut Harness<Editor>| {
        h.window = popup(h, "sizes").expect("open");
        h.send(WindowEvent::PopupDone);
        h.window = main;
        assert_eq!(h.model().menu, None);
    };
    dismiss(&mut h);
    h.ask(&Key::from("size"), Action::Click);
    assert_eq!(
        (h.model().menu.clone(), h.model().size),
        (Some(Open::Sizes), 2),
        "and assistive technology, choosing nothing"
    );
    dismiss(&mut h);
    h.click(h.center("size"));
    assert_eq!(h.model().menu, Some(Open::Sizes), "and a click");
    let id = popup(&h, "sizes").expect("open");
    let placed = h.rt.window_system().placement(id).unwrap().unwrap();
    assert_eq!(
        placed.offset,
        Point::new(-21.0, -65.0),
        "Large, the third row, is over the button"
    );
    enter(&mut h, "sizes");
    assert!(on_row(&h, 2), "on the chosen option");
    assert!(
        highlighted(&h, 2),
        "highlighted although the pointer opened it"
    );
}

#[test]
fn in_right_to_left_a_pop_up_buttons_menu_lines_up_with_its_right_edge() {
    let mut h = Harness::<Editor>::with_direction(LayoutDirection::Rtl);
    h.tap(TAB);
    h.tap(SPACE);
    let id = popup(&h, "sizes").expect("open");
    let placed = h.rt.window_system().placement(id).unwrap().unwrap();
    assert_eq!(
        (placed.anchor, placed.gravity, placed.offset),
        (
            PopupEdge::Right,
            PopupEdge::BottomLeft,
            Point::new(21.0, -17.0)
        )
    );
}

fn ops(h: &Harness<Editor>, id: NodeId) -> Vec<PaintOp> {
    let id = PaintNodeId::from(id);
    h.snapshot()
        .display_list
        .nodes
        .into_iter()
        .find(|n| n.id == id)
        .map(|n| n.ops)
        .unwrap_or_default()
}

fn label_x(h: &Harness<Editor>, index: u64) -> f64 {
    let ops = ops(h, row(h, index));
    ops.iter()
        .find_map(|op| match op {
            PaintOp::Glyphs { origin, .. } => Some(origin.x),
            _ => None,
        })
        .unwrap_or_else(|| panic!("row {index} has a label: {ops:?}"))
}

#[test]
fn rows_are_24_points_tall_with_labels_15_in_and_separators_from_the_labels_to_15_short_of_the_edge(
) {
    let mut h = Harness::<Editor>::assistive();
    context(&mut h);
    for index in [0, 1, 2, 4] {
        assert_eq!(row_rect(&h, index).height(), 24.0, "row {index}");
    }
    for index in [0, 1, 4] {
        assert!((label_x(&h, index) - 15.0).abs() < 0.5, "row {index}");
    }
    let separator = row_rect(&h, 3);
    assert_eq!(separator.height(), 11.0);
    let line = ops(&h, row(&h, 3))
        .into_iter()
        .find_map(|op| match op {
            PaintOp::Fill {
                shape: cupertino_ui_render::Shape::Rect(r),
                ..
            } => Some(r),
            _ => None,
        })
        .expect("the line");
    assert_eq!(line.x(), 15.0);
    assert_eq!(line.x() + line.width(), separator.width() - 15.0);
    assert_eq!(line.height(), 1.0);
}

#[test]
fn in_a_menu_with_check_marks_the_labels_move_over_to_make_room_for_them() {
    let mut h = Harness::<Editor>::assistive();
    h.tap(TAB);
    h.tap(SPACE);
    enter(&mut h, "sizes");
    for index in 0..SIZES.len() as u64 {
        assert!((label_x(&h, index) - 31.0).abs() < 0.5, "row {index}");
    }
}

#[test]
fn without_the_glass_a_menu_strokes_its_own_edge() {
    let mut h = Harness::<Editor>::assistive();
    context(&mut h);
    let edged = h
        .snapshot()
        .display_list
        .nodes
        .into_iter()
        .any(|n| n.ops.iter().any(|op| matches!(op, PaintOp::Stroke { .. })));
    assert!(edged, "over the opaque fill the panel draws its edge");
}
