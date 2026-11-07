mod support;

use cupertino_geometry::{Point, Rect, Size};
use cupertino_ui_controls::{Sidebar, SidebarAccessory, SidebarItem, SidebarSection};
use cupertino_ui_core::{
    AccessibilityNode, Action, Application, Command, Element, Key, NodeId, Role, Window,
};
use cupertino_ui_layout::ScrollState;
use cupertino_ui_render::{Paint, PaintNodeId, PaintOp, Shape};
use cupertino_ui_theme::{ColorRole, InteractionVariant};
use std::collections::HashMap;
use support::{color, window, Harness, TAB};

const UP: u32 = 0xff52;
const DOWN: u32 = 0xff54;
const HOME: u32 = 0xff50;
const END: u32 = 0xff57;

#[derive(Clone, Debug, PartialEq)]
enum Message {
    Scroll(ScrollState),
    Select(usize),
}

struct Model {
    scroll: ScrollState,
    selected: Option<usize>,
    reported: Vec<usize>,
}

fn item(label: &str) -> SidebarItem {
    SidebarItem {
        key: Key::from(label.to_owned()),
        label: label.into(),
        icon: Some("symbol.folder".into()),
        description: None,
        accessory: None,
        busy: false,
    }
}

/// A row without a heading, then two sections: six rows in a 200×100 window.
struct Places;

impl Application for Places {
    type Model = Model;
    type Message = Message;

    fn init() -> (Model, Vec<Command<Message>>) {
        let scroll = ScrollState {
            offset: Point::ZERO,
            content: Size::ZERO,
            viewport: Size::ZERO,
        };
        let model = Model {
            scroll,
            selected: None,
            reported: Vec::new(),
        };
        (model, Vec::new())
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        match message {
            Message::Scroll(scroll) => model.scroll = scroll,
            Message::Select(index) => {
                model.selected = Some(index);
                model.reported.push(index);
            }
        }
        Vec::new()
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        places(&PLACES, model)
    }
}

const PLACES: [&str; 6] = [
    "Recents",
    "Desktop",
    "Documents",
    "Downloads",
    "Computer",
    "Network",
];

/// `names` as Recents without a heading, three Favourites and the rest under Locations.
fn places(names: &[&str], model: &Model) -> Vec<Window<Message>> {
    let section = |title: Option<&str>, names: &[&str]| SidebarSection {
        title: title.map(Into::into),
        items: names.iter().map(|name| item(name)).collect(),
    };
    window(Element::from(Sidebar {
        label: "Places".into(),
        sections: vec![
            section(None, &names[..1]),
            section(Some("Favourites"), &names[1..4]),
            section(Some("Locations"), &names[4..]),
        ],
        selected: model.selected,
        scroll: model.scroll,
        on_scroll: Box::new(Message::Scroll),
        on_select: Some(Box::new(Message::Select)),
        on_accessory: None,
    }))
}

fn row<A: Application>(h: &Harness<A>, name: &str) -> NodeId {
    h.rt.find(h.window, &Key::from(name.to_owned())).unwrap()
}

fn row_rect(h: &Harness<Places>, index: usize) -> Rect {
    let id = row(h, PLACES[index]);
    h.snapshot()
        .nodes
        .into_iter()
        .find(|n| n.id == id)
        .unwrap()
        .rect
}

fn focused_row(h: &Harness<Places>) -> Option<usize> {
    let focus = h.snapshot().focus?;
    (0..6).find(|&i| row(h, PLACES[i]) == focus)
}

#[test]
fn up_down_home_and_end_move_the_selection_across_sections_and_report_each_change_once() {
    let mut h = Harness::<Places>::new();
    h.tap(TAB);
    assert_eq!(focused_row(&h), Some(0), "the first row is the Tab stop");
    for key in [DOWN, DOWN, DOWN, DOWN, UP, END, END, DOWN, HOME, UP] {
        h.tap(key);
        assert_eq!(focused_row(&h), h.model().selected, "the focus follows");
    }
    assert_eq!(
        h.model().reported,
        [0, 1, 2, 3, 2, 5, 0],
        "a key that leaves the selection where it is reports nothing"
    );
}

#[test]
fn a_click_or_assistive_click_selects_a_row_once() {
    let mut h = Harness::<Places>::assistive();
    let r = row_rect(&h, 1);
    let centre = Point::new(r.min_x() + r.width() / 2.0, r.min_y() + r.height() / 2.0);
    h.click(centre);
    h.click(centre);
    h.ask(&Key::from("Downloads"), Action::Click);
    h.ask(&Key::from("Downloads"), Action::Click);
    assert_eq!(h.model().reported, [1, 3]);
    assert_eq!(
        focused_row(&h),
        Some(1),
        "assistive technology's click leaves the focus"
    );
}

#[test]
fn assistive_technology_reads_a_list_of_named_sections_of_rows_with_the_selected_one() {
    let mut h = Harness::<Places>::new();
    h.click({
        let r = row_rect(&h, 1);
        Point::new(r.min_x() + 20.0, r.min_y() + 10.0)
    });
    let nodes: HashMap<NodeId, AccessibilityNode> =
        h.snapshot().accessibility.into_iter().collect();
    let list = nodes.values().find(|n| n.role == Role::List).unwrap();
    assert_eq!(
        (list.name.as_deref(), list.item_count),
        (Some("Places"), Some(6))
    );
    let outline: Vec<(Role, Option<&str>, Vec<&str>)> = list
        .children
        .iter()
        .map(|id| {
            let node = &nodes[id];
            let rows = node
                .children
                .iter()
                .filter_map(|id| nodes[id].name.as_deref())
                .collect();
            (node.role, node.name.as_deref(), rows)
        })
        .collect();
    assert_eq!(
        outline,
        [
            (Role::ListItem, Some("Recents"), vec![]),
            (
                Role::Group,
                Some("Favourites"),
                vec!["Desktop", "Documents", "Downloads"]
            ),
            (Role::Group, Some("Locations"), vec!["Computer", "Network"]),
        ]
    );
    let rows: Vec<(Option<usize>, bool, bool)> = (0..6)
        .map(|i| {
            let node = &nodes[&row(&h, PLACES[i])];
            assert_eq!(node.role, Role::ListItem);
            (node.item_index, node.states.selected, node.states.focusable)
        })
        .collect();
    let mut expected: Vec<_> = (0..6).map(|i| (Some(i), false, false)).collect();
    expected[1] = (Some(1), true, true);
    assert_eq!(
        rows, expected,
        "only the selected row is selected and the Tab stop"
    );
}

#[test]
fn a_selected_row_fills_inset_from_the_sidebar_edges_with_the_selected_control_colour() {
    let mut h = Harness::<Places>::new();
    h.tap(TAB);
    h.tap(DOWN);
    let id = PaintNodeId::from(row(&h, "Recents"));
    let ops = h
        .snapshot()
        .display_list
        .nodes
        .into_iter()
        .find(|n| n.id == id)
        .unwrap()
        .ops;
    assert_eq!(
        ops.first(),
        Some(&PaintOp::Fill {
            shape: Shape::RoundedRect {
                rect: Rect::new(10.0, 0.0, 180.0, 28.0),
                radius: 5.0,
            },
            paint: Paint::Solid(color(
                ColorRole::ControlFill,
                InteractionVariant::Normal,
                true
            )),
        })
    );
}

/// Places where selecting Network mounts a disk above it, as a removable disk can appear while a row has the focus.
struct Mounting;

impl Application for Mounting {
    type Model = (Model, bool);
    type Message = Message;

    fn init() -> ((Model, bool), Vec<Command<Message>>) {
        ((Places::init().0, false), Vec::new())
    }

    fn update((model, mounted): &mut (Model, bool), message: Message) -> Vec<Command<Message>> {
        let selected_network = message == Message::Select(5);
        Places::update(model, message);
        if selected_network && !*mounted {
            *mounted = true;
            model.selected = Some(6);
        }
        Vec::new()
    }

    fn view((model, mounted): &(Model, bool)) -> Vec<Window<Message>> {
        let mut names = PLACES.to_vec();
        if *mounted {
            names.insert(5, "Disk");
        }
        places(&names, model)
    }
}

#[test]
fn the_focus_and_the_accessible_row_stay_on_their_item_when_a_row_is_inserted_above() {
    let mut h = Harness::<Mounting>::new();
    let named = |h: &Harness<Mounting>, name: &str| {
        h.snapshot()
            .accessibility
            .into_iter()
            .find(|(_, n)| n.name.as_deref() == Some(name))
            .map(|(id, n)| (id, n.item_index, n.states.selected))
    };
    let (network, _, _) = named(&h, "Network").unwrap();
    h.tap(TAB);
    h.tap(END);
    assert_eq!(named(&h, "Network"), Some((network, Some(6), true)));
    assert_eq!(
        h.snapshot().focus,
        Some(network),
        "the focus stays on Network"
    );
    assert!(named(&h, "Disk").is_some_and(|(id, _, _)| id != network));
}

#[derive(Clone, Debug, PartialEq)]
enum Device {
    Scroll(ScrollState),
    Select(usize),
    Eject(Key),
}

/// Three devices: Backup is mounted and can be ejected, Card is mid-eject, and Stick is not mounted.
struct Devices;

impl Application for Devices {
    type Model = (ScrollState, Vec<Device>);
    type Message = Device;

    fn init() -> ((ScrollState, Vec<Device>), Vec<Command<Device>>) {
        ((Places::init().0.scroll, Vec::new()), Vec::new())
    }

    fn update(
        (scroll, reported): &mut (ScrollState, Vec<Device>),
        message: Device,
    ) -> Vec<Command<Device>> {
        match message {
            Device::Scroll(new) => *scroll = new,
            other => reported.push(other),
        }
        Vec::new()
    }

    fn view((scroll, _): &(ScrollState, Vec<Device>)) -> Vec<Window<Device>> {
        let device = |name: &str, ejectable: bool, busy: bool| SidebarItem {
            description: Some("Removable".into()),
            accessory: ejectable.then(|| SidebarAccessory {
                symbol: "symbol.eject".into(),
                label: format!("Eject {name}"),
            }),
            busy,
            ..item(name)
        };
        window(Element::from(Sidebar {
            label: "Places".into(),
            sections: vec![SidebarSection {
                title: Some("Locations".into()),
                items: vec![
                    device("Backup", true, false),
                    device("Card", true, true),
                    device("Stick", false, false),
                ],
            }],
            selected: None,
            scroll: *scroll,
            on_scroll: Box::new(Device::Scroll),
            on_select: Some(Box::new(Device::Select)),
            on_accessory: Some(Box::new(Device::Eject)),
        }))
    }
}

fn named(h: &Harness<Devices>, name: &str) -> Option<AccessibilityNode> {
    h.snapshot()
        .accessibility
        .into_iter()
        .map(|(_, n)| n)
        .find(|n| n.name.as_deref() == Some(name))
}

fn device_rect(h: &Harness<Devices>, name: &str) -> Rect {
    let id = row(h, name);
    h.snapshot()
        .nodes
        .into_iter()
        .find(|n| n.id == id)
        .unwrap()
        .rect
}

#[test]
fn a_press_at_the_trailing_edge_ejects_without_selecting_and_a_press_on_the_name_selects() {
    let mut h = Harness::<Devices>::new();
    let r = device_rect(&h, "Backup");
    let y = r.min_y() + r.height() / 2.0;
    h.click(Point::new(r.max_x() - 11.0, y));
    h.click(Point::new(r.max_x() - 30.0, y));
    h.click(Point::new(r.min_x() + 40.0, y));
    assert_eq!(
        h.model().1,
        [
            Device::Eject(Key::from("Backup")),
            Device::Eject(Key::from("Backup")),
            Device::Select(0)
        ],
        "the slot and the inset after it eject; the name selects"
    );
}

#[test]
fn a_busy_row_has_no_eject_button_and_says_it_is_busy() {
    let mut h = Harness::<Devices>::new();
    let r = device_rect(&h, "Card");
    h.click(Point::new(r.max_x() - 11.0, r.min_y() + r.height() / 2.0));
    assert_eq!(h.model().1, [Device::Select(1)]);
    assert!(named(&h, "Eject Card").is_none());
    assert!(named(&h, "Card").unwrap().states.busy);
}

#[test]
fn assistive_technology_reads_the_description_and_clicks_the_eject_button() {
    let mut h = Harness::<Devices>::assistive();
    let backup = named(&h, "Backup").unwrap();
    assert_eq!(backup.description.as_deref(), Some("Removable"));
    let eject = named(&h, "Eject Backup").unwrap();
    assert_eq!(eject.role, Role::Button);
    assert_eq!(eject.actions, [Action::Click]);
    assert!(
        !eject.states.focusable,
        "the sidebar keeps its one Tab stop"
    );
    assert!(named(&h, "Eject Stick").is_none());

    let id = h
        .snapshot()
        .accessibility
        .into_iter()
        .find(|(_, n)| n.name.as_deref() == Some("Eject Backup"))
        .map(|(id, _)| id)
        .unwrap();
    h.ask_node(id, Action::Click);
    assert_eq!(h.model().1, [Device::Eject(Key::from("Backup"))]);
}

/// Placeholders, with the colour of every symbol asked for kept by name.
#[derive(Debug, Default)]
struct Tints {
    asked: std::sync::Mutex<Vec<(String, cupertino_ui_theme::Color)>>,
    draw: cupertino_ui_core::PlaceholderIcons,
}

impl cupertino_ui_core::Icons for Tints {
    fn symbol(
        &self,
        name: &str,
        px: u32,
        color: cupertino_ui_theme::Color,
        mirrored: bool,
    ) -> cupertino_ui_core::IconImage {
        self.asked.lock().unwrap().push((name.to_owned(), color));
        self.draw.symbol(name, px, color, mirrored)
    }
}

#[test]
fn an_inactive_windows_row_icons_take_the_secondary_label_colour() {
    let tints = std::sync::Arc::new(Tints::default());
    let mut h =
        Harness::<Places>::with_icons(cupertino_ui_layout::LayoutDirection::Ltr, tints.clone());
    let accent = color(ColorRole::ControlFill, InteractionVariant::Normal, true);
    let last = |tints: &Tints| tints.asked.lock().unwrap().last().map(|(_, c)| *c);
    assert_eq!(last(&tints), Some(accent), "the accent while active");
    tints.asked.lock().unwrap().clear();
    h.rt.window_system_mut()
        .configure(
            h.window,
            Some(Size::new(200.0, 100.0)),
            cupertino_ui_platform::WindowStates::default(),
        )
        .unwrap();
    h.settle();
    let asked = tints.asked.lock().unwrap().clone();
    assert!(!asked.is_empty(), "the rows drew their icons again");
    let secondary = support::theme()
        .colors
        .get(
            ColorRole::SecondaryText,
            cupertino_ui_theme::StateVariant::NORMAL,
        )
        .unwrap();
    assert!(
        asked.iter().all(|(_, c)| *c == secondary),
        "every row's icon in the secondary label colour: {asked:?}"
    );
}
