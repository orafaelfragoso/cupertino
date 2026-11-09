mod support;

use cupertino_geometry::{Point, Rect, Size};
use cupertino_ui_controls::{Sidebar, SidebarAccessory, SidebarItem, SidebarLook, SidebarSection};
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

/// Three devices: Backup is mounted and can be ejected, Card is mid-eject, and Stick is not mounted. Card's activity
/// indicator turns without end, so the harnesses show one more frame per settle.
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
    let mut h = Harness::<Devices>::animated();
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
    let mut h = Harness::<Devices>::animated();
    let r = device_rect(&h, "Card");
    h.click(Point::new(r.max_x() - 11.0, r.min_y() + r.height() / 2.0));
    assert_eq!(h.model().1, [Device::Select(1)]);
    assert!(named(&h, "Eject Card").is_none());
    assert!(named(&h, "Card").unwrap().states.busy);
}

#[test]
fn assistive_technology_reads_the_description_and_clicks_the_eject_button() {
    let mut h = Harness::<Devices>::assistive_animated();
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
fn an_inactive_windows_row_icons_take_the_inactive_sidebar_text_colour() {
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
    let inactive_text = support::theme()
        .colors
        .get(
            ColorRole::InactiveSidebarText,
            cupertino_ui_theme::StateVariant::NORMAL,
        )
        .unwrap();
    assert!(
        asked.iter().all(|(_, c)| *c == inactive_text),
        "every row's icon in the inactive sidebar text colour: {asked:?}"
    );
}

#[test]
fn a_busy_row_turns_an_activity_indicator_where_its_eject_button_was() {
    let h = Harness::<Devices>::animated();
    let spokes = |h: &Harness<Devices>, name: &str| -> Vec<Rect> {
        let id = PaintNodeId::from(row(h, name));
        h.snapshot()
            .display_list
            .nodes
            .into_iter()
            .find(|n| n.id == id)
            .unwrap()
            .ops
            .into_iter()
            .filter_map(|op| match op {
                PaintOp::Stroke {
                    shape: Shape::Path { verbs, .. },
                    ..
                } => {
                    let points: Vec<Point> = verbs
                        .iter()
                        .filter_map(|v| match v {
                            cupertino_ui_render::PathVerb::MoveTo(p)
                            | cupertino_ui_render::PathVerb::LineTo(p) => Some(*p),
                            _ => None,
                        })
                        .collect();
                    let (a, b) = (points[0], points[1]);
                    Some(Rect::new(
                        a.x.min(b.x),
                        a.y.min(b.y),
                        (a.x - b.x).abs(),
                        (a.y - b.y).abs(),
                    ))
                }
                _ => None,
            })
            .collect()
    };
    assert!(spokes(&h, "Backup").is_empty(), "a row at rest shows none");
    let card = spokes(&h, "Card");
    assert_eq!(card.len(), 8);
    let row_width = device_rect(&h, "Card").width();
    // every spoke lies in the trailing slot the eject button takes on a row at rest
    for spoke in &card {
        assert!(
            spoke.min_x() >= row_width - 10.0 - 4.0 - 24.0
                && spoke.max_x() <= row_width - 10.0 - 4.0,
            "{spoke:?} in the slot of a {row_width} pt row"
        );
    }
}

#[test]
fn with_motion_reduced_a_busy_row_s_indicator_stands_still_and_asks_for_no_frames() {
    let mut reduced = support::theme();
    reduced.motion.reduce = true;
    // a harness that is not animated settles only once the window stops asking for frames
    let mut h = Harness::<Devices>::with_theme(reduced);
    let strokes = |h: &Harness<Devices>| -> Vec<f32> {
        let id = PaintNodeId::from(row(h, "Card"));
        h.snapshot()
            .display_list
            .nodes
            .into_iter()
            .find(|n| n.id == id)
            .unwrap()
            .ops
            .into_iter()
            .filter_map(|op| match op {
                PaintOp::Stroke {
                    shape: Shape::Path { .. },
                    paint: Paint::Solid(c),
                    ..
                } => Some(c.a),
                _ => None,
            })
            .collect()
    };
    let still = strokes(&h);
    assert_eq!(still.len(), 8, "the indicator is drawn: {still:?}");
    assert!(
        still[0] > still[1..].iter().copied().fold(0.0, f32::max),
        "its first spoke is the brightest: {still:?}"
    );
    h.settle();
    assert_eq!(strokes(&h), still, "and it does not turn");
}

/// [`Places`] in the neutral look, Computer selected and only the Favourites marked for the accent.
struct Neutral;

impl Application for Neutral {
    type Model = Model;
    type Message = Message;

    fn init() -> (Model, Vec<Command<Message>>) {
        let (mut model, commands) = Places::init();
        model.selected = Some(4);
        (model, commands)
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        Places::update(model, message)
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        let section = |title: Option<&str>, names: &[&str]| SidebarSection {
            title: title.map(Into::into),
            items: names
                .iter()
                .map(|name| SidebarItem {
                    icon: Some(format!("symbol.{name}")),
                    ..item(name)
                })
                .collect(),
        };
        window(
            Sidebar {
                label: "Places".into(),
                sections: vec![
                    section(None, &PLACES[..1]),
                    section(Some("Favourites"), &PLACES[1..4]),
                    section(Some("Locations"), &PLACES[4..]),
                ],
                selected: model.selected,
                scroll: model.scroll,
                on_scroll: Box::new(Message::Scroll),
                on_select: Some(Box::new(Message::Select)),
                on_accessory: None,
            }
            .with_look(SidebarLook::Neutral {
                accent: vec![false, true],
            }),
        )
    }
}

#[test]
fn the_neutral_look_fills_the_selected_row_grey_and_tints_symbols_by_section() {
    let tints = std::sync::Arc::new(Tints::default());
    let mut h =
        Harness::<Neutral>::with_icons(cupertino_ui_layout::LayoutDirection::Ltr, tints.clone());
    // tall enough for every row
    h.rt.window_system_mut()
        .configure(
            h.window,
            Some(Size::new(200.0, 300.0)),
            cupertino_ui_platform::WindowStates {
                activated: true,
                ..Default::default()
            },
        )
        .unwrap();
    h.settle();
    let snapshot = h.snapshot();
    let rect = |name: &str| {
        let id = row(&h, name);
        snapshot.nodes.iter().find(|n| n.id == id).unwrap().rect
    };
    assert_eq!(
        rect("Desktop").height(),
        32.0,
        "taller rows than the accent look's"
    );
    assert_eq!(rect("Documents").min_y() - rect("Desktop").min_y(), 32.0);

    let id = PaintNodeId::from(row(&h, "Computer"));
    let ops = snapshot
        .display_list
        .nodes
        .into_iter()
        .find(|n| n.id == id)
        .unwrap()
        .ops;
    let grey = support::theme()
        .colors
        .get(
            ColorRole::SidebarSelection,
            cupertino_ui_theme::StateVariant::NORMAL,
        )
        .unwrap();
    assert_eq!(
        ops.first(),
        Some(&PaintOp::Fill {
            shape: Shape::RoundedRect {
                rect: Rect::new(10.0, 0.0, 180.0, 32.0),
                radius: 10.0,
            },
            paint: Paint::Solid(grey),
        }),
        "the selection is the neutral grey, not the accent"
    );

    let accent = color(ColorRole::ControlFill, InteractionVariant::Normal, true);
    let secondary = support::theme()
        .colors
        .get(
            ColorRole::SecondaryText,
            cupertino_ui_theme::StateVariant::NORMAL,
        )
        .unwrap();
    let asked: HashMap<String, _> = tints.asked.lock().unwrap().iter().cloned().collect();
    let tint = |name: &str| asked.get(&format!("symbol.{name}")).copied();
    // the first section is not marked, the second is, and the third is past the marks
    assert_eq!(tint("Recents"), Some(secondary));
    assert_eq!(tint("Desktop"), Some(accent));
    assert_eq!(tint("Downloads"), Some(accent));
    assert_eq!(tint("Network"), Some(secondary));
    assert_eq!(
        tint("Computer"),
        Some(accent),
        "the selected row's symbol is the accent"
    );
}

#[test]
fn the_neutral_looks_selected_symbol_turns_grey_with_the_others_when_the_window_is_not_active() {
    let tints = std::sync::Arc::new(Tints::default());
    let mut h =
        Harness::<Neutral>::with_icons(cupertino_ui_layout::LayoutDirection::Ltr, tints.clone());
    h.rt.window_system_mut()
        .configure(
            h.window,
            Some(Size::new(200.0, 300.0)),
            cupertino_ui_platform::WindowStates::default(),
        )
        .unwrap();
    h.settle();
    let normal = cupertino_ui_theme::StateVariant::NORMAL;
    let theme = support::theme();
    let secondary = theme
        .colors
        .get(ColorRole::InactiveSidebarText, normal)
        .unwrap();
    let selection = theme
        .colors
        .get(ColorRole::SidebarSelection, inactive())
        .unwrap();
    let asked: HashMap<String, _> = tints.asked.lock().unwrap().iter().cloned().collect();
    for name in PLACES {
        assert_eq!(
            asked.get(&format!("symbol.{name}")).copied(),
            Some(secondary),
            "{name}: every symbol is the inactive sidebar text colour, the marked sections' and the selected row's too"
        );
    }
    assert_ne!(
        secondary, selection,
        "the selected symbol stands out from the selection behind it"
    );
}

fn inactive() -> cupertino_ui_theme::StateVariant {
    cupertino_ui_theme::StateVariant {
        window_active: false,
        ..cupertino_ui_theme::StateVariant::NORMAL
    }
}

/// The window drawn active, then again after it lost the activation, node by node. Every op of each node is the same
/// but for its colour; the colours that changed are returned as (active, inactive) pairs. A symbol's image is told
/// apart by its place only, since its pixels carry its tint.
fn active_then_inactive<A: Application>(h: &mut Harness<A>, size: Size) -> Vec<(Paint, Paint)> {
    let configure = |h: &mut Harness<A>, activated| {
        h.rt.window_system_mut()
            .configure(
                h.window,
                Some(size),
                cupertino_ui_platform::WindowStates {
                    activated,
                    ..Default::default()
                },
            )
            .unwrap();
        h.settle();
        h.snapshot().display_list.nodes
    };
    let active = configure(h, true);
    let inactive = configure(h, false);
    assert_eq!(active.len(), inactive.len(), "the same nodes");
    let mut changed = Vec::new();
    for (a, b) in active.iter().zip(&inactive) {
        assert_eq!((a.id, a.bounds, &a.clip), (b.id, b.bounds, &b.clip));
        assert_eq!(a.ops.len(), b.ops.len(), "{:?}: the same ops", a.id);
        for (x, y) in a.ops.iter().zip(&b.ops) {
            match (x, y) {
                (
                    PaintOp::Fill { shape, paint },
                    PaintOp::Fill {
                        shape: shape2,
                        paint: paint2,
                    },
                ) => {
                    assert_eq!(shape, shape2);
                    if paint != paint2 {
                        changed.push((paint.clone(), paint2.clone()));
                    }
                }
                (
                    PaintOp::Glyphs { origin, run, color },
                    PaintOp::Glyphs {
                        origin: origin2,
                        run: run2,
                        color: color2,
                    },
                ) => {
                    assert_eq!(
                        (origin, run),
                        (origin2, run2),
                        "the same text in the same place"
                    );
                    if color != color2 {
                        changed.push((Paint::Solid(*color), Paint::Solid(*color2)));
                    }
                }
                (
                    PaintOp::Image { src, dst, .. },
                    PaintOp::Image {
                        src: src2,
                        dst: dst2,
                        ..
                    },
                ) => {
                    assert_eq!(
                        (src, dst),
                        (src2, dst2),
                        "the same symbol in the same place"
                    );
                }
                _ => assert_eq!(x, y),
            }
        }
    }
    changed
}

/// [`Places`] with Computer selected and every row wearing a symbol.
struct AccentSelected;

impl Application for AccentSelected {
    type Model = Model;
    type Message = Message;

    fn init() -> (Model, Vec<Command<Message>>) {
        let (mut model, commands) = Places::init();
        model.selected = Some(4);
        (model, commands)
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        Places::update(model, message)
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        window(Element::from(Sidebar {
            label: "Places".into(),
            sections: vec![SidebarSection {
                title: Some("Favourites".into()),
                items: PLACES
                    .iter()
                    .map(|name| SidebarItem {
                        icon: Some(format!("symbol.{name}")),
                        ..item(name)
                    })
                    .collect(),
            }],
            selected: model.selected,
            scroll: model.scroll,
            on_scroll: Box::new(Message::Scroll),
            on_select: Some(Box::new(Message::Select)),
            on_accessory: None,
        }))
    }
}

/// A window that is not active draws its sidebar as an active one does but for the documented roles: every label,
/// symbol and heading takes the inactive sidebar text colour and the selection turns from the accent (or the neutral
/// selection) to the fainter inactive neutral selection; the geometry and the text stay.
#[test]
fn an_inactive_window_s_sidebar_differs_only_in_its_labels_symbols_and_selection() {
    let theme = support::theme();
    let normal = cupertino_ui_theme::StateVariant::NORMAL;
    let get = |role, state| theme.colors.get(role, state).unwrap();
    let secondary = Paint::Solid(get(ColorRole::InactiveSidebarText, normal));
    let faint = Paint::Solid(get(ColorRole::SidebarSelection, inactive()));
    let accent = Paint::Solid(color(
        ColorRole::ControlFill,
        InteractionVariant::Normal,
        true,
    ));
    let neutral = Paint::Solid(get(ColorRole::SidebarSelection, normal));
    let size = Size::new(200.0, 300.0);
    for (look, changed, selection) in [
        (
            "accent",
            active_then_inactive(&mut Harness::<AccentSelected>::new(), size),
            accent,
        ),
        (
            "neutral",
            active_then_inactive(&mut Harness::<Neutral>::new(), size),
            neutral,
        ),
    ] {
        assert!(
            changed.contains(&(selection.clone(), faint.clone())),
            "{look}: the selection fades to the inactive neutral selection: {changed:?}"
        );
        for (before, after) in &changed {
            if *before == selection {
                assert_eq!(*after, faint, "{look}");
            } else {
                assert_eq!(
                    *after, secondary,
                    "{look}: a label turns secondary, from {before:?}"
                );
            }
        }
        assert!(
            changed
                .iter()
                .filter(|(_, after)| *after == secondary)
                .count()
                >= PLACES.len(),
            "{look}: every row's label turns secondary: {changed:?}"
        );
    }
}

/// Every symbol of a window that is not active, the selected row's too, takes the inactive sidebar text colour in either
/// look, and the selected row's label is that colour on the faint selection.
#[test]
fn an_inactive_window_s_selected_row_reads_secondary_on_the_faint_selection() {
    let tints = std::sync::Arc::new(Tints::default());
    let mut h = Harness::<AccentSelected>::with_icons(
        cupertino_ui_layout::LayoutDirection::Ltr,
        tints.clone(),
    );
    h.rt.window_system_mut()
        .configure(
            h.window,
            Some(Size::new(200.0, 300.0)),
            cupertino_ui_platform::WindowStates::default(),
        )
        .unwrap();
    tints.asked.lock().unwrap().clear();
    h.settle();
    let theme = support::theme();
    let secondary = theme
        .colors
        .get(
            ColorRole::InactiveSidebarText,
            cupertino_ui_theme::StateVariant::NORMAL,
        )
        .unwrap();
    let asked: HashMap<String, _> = tints.asked.lock().unwrap().iter().cloned().collect();
    assert_eq!(asked.get("symbol.Computer"), Some(&secondary));
    let id = PaintNodeId::from(row(&h, "Computer"));
    let ops = h
        .snapshot()
        .display_list
        .nodes
        .into_iter()
        .find(|n| n.id == id)
        .unwrap()
        .ops;
    assert!(
        matches!(ops.first(), Some(PaintOp::Fill { paint: Paint::Solid(c), .. })
            if *c == theme.colors.get(ColorRole::SidebarSelection, inactive()).unwrap()),
        "{ops:?}"
    );
    assert!(
        ops.iter()
            .any(|op| matches!(op, PaintOp::Glyphs { color, .. } if *color == secondary)),
        "the label is secondary"
    );
}
