//! Menus: a context menu opens at a secondary click or from the keyboard, a pop-up button's menu below the button.
//! The keyboard moves between the enabled items and opens and closes submenus, an item is chosen by Return or a
//! click, and Escape or a press outside closes the menu.

mod support;

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
                        item("Copy", true),
                        item("Paste", false),
                        MenuItem::Separator,
                        MenuItem::Submenu {
                            label: "Share".into(),
                            enabled: true,
                            items: vec![item("Mail", true), item("Messages", true)],
                        },
                    ],
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
    h.type_text("s");
    assert!(on_row(&h, 4));
}

#[test]
fn return_or_a_click_chooses_an_item_and_closes_the_menu() {
    let mut h = Harness::<Editor>::new();
    let main = context(&mut h);
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
fn a_pop_up_button_opens_its_options_below_itself_with_the_chosen_one_checked() {
    let mut h = Harness::<Editor>::assistive();
    let button = h.rect("size");
    h.tap(TAB);
    h.tap(SPACE);
    assert_eq!(h.model().menu, Some(Open::Sizes));
    let id = popup(&h, "sizes").expect("open");
    let placed = h.rt.window_system().placement(id).unwrap().unwrap().clone();
    assert_eq!(placed.anchor_rect, button);
    assert_eq!(
        (placed.anchor, placed.offset),
        (PopupEdge::Bottom, Point::new(0.0, 4.0))
    );
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
    h.send(WindowEvent::PopupDone);
    h.click(h.center("size"));
    assert_eq!(h.model().menu, Some(Open::Sizes), "and a click");
}
