//! Menus follow one pointer path across their windows as the compositor routes it: a press that opens a menu and is
//! dragged onto an item chooses it on release, a quick click leaves the menu open, the way from a row into its submenu
//! keeps the submenu while the pointer heads there and gives it up when the pointer rests or crawls, and typed letters
//! find a row that Return chooses.

mod support;

use cupertino_geometry::{Insets, Point, Rect};
use cupertino_menu_model::tracking::{SUBMENU_AIM_PAUSE, SUBMENU_OPEN_DELAY, TRACKING_INTERVAL};
use cupertino_ui_controls::{
    Button, ButtonRole, ButtonStyle, ContextMenu, Menu, MenuAnchor, MenuItem, PopUpButton, Stack,
};
use cupertino_ui_core::{Application, Command, Element, Key, NodeId, Window, WindowKey};
use cupertino_ui_layout::{Align, Axis};
use cupertino_ui_platform::{Mods, MouseButton, PointerEvent, PopupEdge, WindowId, WindowStates};
use support::{window, Harness, RETURN};

const RIGHT: u32 = 0xff53;
const ESCAPE: u32 = 0xff1b;
const MENU: u32 = 0xff67;
const TAB: u32 = 0xff09;
const MS: u64 = 1_000_000;

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
    sizes_chosen: usize,
}

struct Browser;

fn item(label: &'static str) -> MenuItem<Message> {
    MenuItem::Item {
        label: label.into(),
        enabled: true,
        checked: false,
        on_select: Message::Choose(label),
    }
}

/// Sort By opens a tall submenu; the way from it into the submenu crosses Refresh and Group By, which opens another.
fn items() -> Vec<MenuItem<Message>> {
    vec![
        MenuItem::Submenu {
            label: "Sort By".into(),
            enabled: true,
            items: vec![
                item("Name"),
                item("Kind"),
                item("Date Added"),
                item("Size"),
                item("Tags"),
            ],
        },
        item("Refresh"),
        MenuItem::Submenu {
            label: "Group By".into(),
            enabled: true,
            items: vec![item("None"), item("Application")],
        },
        item("Reload"),
        MenuItem::Separator,
        item("Copy"),
        item("Paste"),
    ]
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

impl Application for Browser {
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
            Message::Size(size) => {
                model.size = size;
                model.sizes_chosen += 1;
            }
        }
        Vec::new()
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        let parent = WindowKey("test".into());
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
                    items: items(),
                    current: None,
                    open: model.submenus.clone(),
                    on_open: Box::new(Message::Open),
                    on_close: Message::Close,
                }
                .windows(WindowKey("menu".into())),
            ),
            Some(Open::Sizes) => windows.extend(
                sizes(model)
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

/// The pointer as the compositor routes it: to the window under it, with Leave and Enter as it changes windows. While
/// a menu holds the pointer, the release of the press that opened it goes to the menu window under the pointer.
struct Pointer {
    main: WindowId,
    over: Option<WindowId>,
    configured: Vec<WindowId>,
    at: Point,
}

impl Pointer {
    fn new(h: &Harness<Browser>) -> Self {
        Pointer {
            main: h.window,
            over: None,
            configured: vec![h.window],
            at: Point::new(0.0, 0.0),
        }
    }

    /// Lets every window lay out, map new popups and run what is due.
    fn settle(&mut self, h: &mut Harness<Browser>) {
        for _ in 0..4 {
            h.window = self.main;
            h.settle();
            let new: Vec<WindowId> =
                h.rt.windows()
                    .into_iter()
                    .map(|(_, id)| id)
                    .filter(|id| !self.configured.contains(id))
                    .collect();
            if new.is_empty() {
                break;
            }
            for id in new {
                // the compositor gives a popup the size it was placed at
                let size = h.rt.window_system().placement(id).unwrap().map(|p| p.size);
                h.rt.window_system_mut()
                    .configure(id, size, WindowStates::default())
                    .unwrap();
                self.configured.push(id);
                h.window = id;
                h.settle();
            }
        }
        for (_, id) in h.rt.windows() {
            h.window = id;
            h.settle();
        }
        h.window = self.main;
    }

    fn wait(&mut self, h: &mut Harness<Browser>, ms: u64) {
        h.clock().advance_by(ms * MS);
        self.settle(h);
    }

    /// The window under `p`, in the main window's coordinates, and `p` in its own.
    fn locate(&self, h: &Harness<Browser>, p: Point) -> (WindowId, Point) {
        let popups: Vec<WindowId> =
            h.rt.windows()
                .into_iter()
                .map(|(_, id)| id)
                .filter(|&id| id != self.main)
                .collect();
        for id in popups.into_iter().rev() {
            let r = rect(h, id);
            if r.contains(p) {
                return (id, Point::new(p.x - r.min_x(), p.y - r.min_y()));
            }
        }
        (self.main, p)
    }

    fn alive(&self, h: &Harness<Browser>, id: WindowId) -> bool {
        h.rt.windows().iter().any(|(_, w)| *w == id)
    }

    fn move_to(&mut self, h: &mut Harness<Browser>, p: Point) {
        self.at = p;
        let (id, local) = self.locate(h, p);
        if self.over != Some(id) {
            if let Some(old) = self.over.filter(|&old| self.alive(h, old)) {
                h.window = old;
                h.pointer(PointerEvent::Leave);
            }
            h.window = id;
            h.pointer(PointerEvent::Enter { position: local });
            self.over = Some(id);
        }
        h.window = id;
        h.pointer(PointerEvent::Motion { position: local });
        self.settle(h);
    }

    /// From where the pointer is to `to` in `steps` moves `ms` apart.
    fn glide(&mut self, h: &mut Harness<Browser>, to: Point, steps: u32, ms: u64) -> Vec<Point> {
        let from = self.at;
        let mut points = Vec::new();
        for step in 1..=steps {
            let k = f64::from(step) / f64::from(steps);
            let p = Point::new(from.x + (to.x - from.x) * k, from.y + (to.y - from.y) * k);
            self.wait(h, ms);
            self.move_to(h, p);
            points.push(p);
        }
        points
    }

    fn button(&mut self, h: &mut Harness<Browser>, button: MouseButton, pressed: bool) {
        let (id, local) = self.locate(h, self.at);
        h.window = id;
        h.pointer(PointerEvent::Button {
            position: local,
            button,
            pressed,
            serial: 1,
            mods: Mods::NONE,
        });
        self.settle(h);
    }
}

/// The rect of the open window `id` in the main window's coordinates, from where it asked to be placed.
fn rect(h: &Harness<Browser>, id: WindowId) -> Rect {
    let system = h.rt.window_system();
    let Some(parent) = system.parent(id).unwrap() else {
        return Rect::new(0.0, 0.0, 200.0, 100.0);
    };
    let spec = system.placement(id).unwrap().unwrap().clone();
    let base = rect(h, parent).origin;
    let a = spec.anchor_rect;
    let ax = match spec.anchor {
        PopupEdge::Left | PopupEdge::TopLeft | PopupEdge::BottomLeft => a.min_x(),
        PopupEdge::Right | PopupEdge::TopRight | PopupEdge::BottomRight => a.max_x(),
        _ => a.min_x() + a.width() / 2.0,
    };
    let ay = match spec.anchor {
        PopupEdge::Top | PopupEdge::TopLeft | PopupEdge::TopRight => a.min_y(),
        PopupEdge::Bottom | PopupEdge::BottomLeft | PopupEdge::BottomRight => a.max_y(),
        _ => a.min_y() + a.height() / 2.0,
    };
    let (w, hh) = (spec.size.width, spec.size.height);
    let x = ax
        + spec.offset.x
        + match spec.gravity {
            PopupEdge::Left | PopupEdge::TopLeft | PopupEdge::BottomLeft => -w,
            PopupEdge::Right | PopupEdge::TopRight | PopupEdge::BottomRight => 0.0,
            _ => -w / 2.0,
        };
    let y = ay
        + spec.offset.y
        + match spec.gravity {
            PopupEdge::Top | PopupEdge::TopLeft | PopupEdge::TopRight => -hh,
            PopupEdge::Bottom | PopupEdge::BottomLeft | PopupEdge::BottomRight => 0.0,
            _ => -hh / 2.0,
        };
    Rect::new(base.x + x, base.y + y, w, hh)
}

fn popup(h: &Harness<Browser>, key: &str) -> Option<WindowId> {
    h.rt.windows()
        .into_iter()
        .find(|(k, _)| *k == WindowKey(key.to_owned().into()))
        .map(|(_, id)| id)
}

fn node(h: &Harness<Browser>, window: WindowId, index: u64) -> NodeId {
    h.rt.find(window, &Key::from(index)).unwrap()
}

/// Row `index` of the menu `key`, in the main window's coordinates.
fn row_rect(h: &Harness<Browser>, key: &str, index: u64) -> Rect {
    let id = popup(h, key).expect("the menu is open");
    let node = node(h, id, index);
    let r =
        h.rt.snapshot(id)
            .unwrap()
            .nodes
            .into_iter()
            .find(|n| n.id == node)
            .unwrap()
            .rect;
    let origin = rect(h, id).origin;
    Rect::new(
        origin.x + r.min_x(),
        origin.y + r.min_y(),
        r.width(),
        r.height(),
    )
}

/// The middle of row `index` of the menu `key`, `along` of its width in.
fn row_point(h: &Harness<Browser>, key: &str, index: u64, along: f64) -> Point {
    let r = row_rect(h, key, index);
    Point::new(r.min_x() + r.width() * along, r.min_y() + r.height() / 2.0)
}

/// The row of the menu `key` with the focus.
fn focused(h: &Harness<Browser>, key: &str) -> Option<u64> {
    let id = popup(h, key)?;
    let focus = h.rt.snapshot(id).unwrap().focus?;
    (0..8).find(|&i| h.rt.find(id, &Key::from(i)) == Some(focus))
}

/// A secondary click on the canvas, quick: the menu opens at the pointer and stays open.
fn context(h: &mut Harness<Browser>, ptr: &mut Pointer) {
    let at = h.center("canvas");
    ptr.move_to(h, at);
    ptr.button(h, MouseButton::Right, true);
    ptr.wait(h, 80);
    ptr.button(h, MouseButton::Right, false);
    assert!(popup(h, "menu").is_some(), "a quick click leaves it open");
}

/// The context menu open with Sort By's submenu, opened by resting on its row.
fn sort_by_open(h: &mut Harness<Browser>, ptr: &mut Pointer) {
    context(h, ptr);
    let sort_by = row_point(h, "menu", 0, 0.5);
    ptr.glide(h, sort_by, 4, 16);
    assert!(h.model().submenus.is_empty(), "not before the delay");
    ptr.wait(h, (SUBMENU_OPEN_DELAY * 1000.0) as u64 + 10);
    assert_eq!(h.model().submenus, [0]);
    assert!(popup(h, "menu/1").is_some());
}

#[test]
fn a_secondary_press_dragged_onto_an_item_chooses_it_once_and_closes_the_menu() {
    let mut h = Harness::<Browser>::new();
    let mut ptr = Pointer::new(&h);
    let canvas = h.center("canvas");
    ptr.move_to(&mut h, canvas);
    ptr.button(&mut h, MouseButton::Right, true);
    let copy = row_point(&h, "menu", 5, 0.5);
    ptr.glide(&mut h, copy, 8, 16);
    assert_eq!(focused(&h, "menu"), Some(5));
    ptr.button(&mut h, MouseButton::Right, false);
    assert_eq!(h.model().chosen, ["Copy"], "chosen once");
    assert_eq!(h.model().closes, 1);
    assert_eq!(popup(&h, "menu"), None);
}

#[test]
fn a_quick_click_leaves_a_menu_open_and_a_held_or_dragged_press_on_a_pop_up_button_chooses() {
    let mut h = Harness::<Browser>::new();
    let mut ptr = Pointer::new(&h);
    context(&mut h, &mut ptr);
    let copy = row_point(&h, "menu", 5, 0.5);
    ptr.glide(&mut h, copy, 4, 16);
    ptr.button(&mut h, MouseButton::Left, true);
    ptr.button(&mut h, MouseButton::Left, false);
    assert_eq!(h.model().chosen, ["Copy"], "then a click chooses");

    // the pop-up button's menu opens over it, the current option under the pointer
    let button = h.center("size");
    ptr.move_to(&mut h, button);
    ptr.button(&mut h, MouseButton::Left, true);
    assert_eq!(h.model().menu, Some(Open::Sizes), "it opens on the press");
    ptr.move_to(&mut h, button);
    ptr.wait(&mut h, 100);
    ptr.button(&mut h, MouseButton::Left, false);
    assert!(
        popup(&h, "sizes").is_some() && h.model().sizes_chosen == 0,
        "a quick click leaves it open over the button"
    );
    h.window = popup(&h, "sizes").unwrap();
    h.send(cupertino_ui_platform::WindowEvent::PopupDone);
    ptr.settle(&mut h);
    assert_eq!(h.model().menu, None);

    ptr.move_to(&mut h, button);
    ptr.button(&mut h, MouseButton::Left, true);
    ptr.move_to(&mut h, button);
    ptr.wait(&mut h, (TRACKING_INTERVAL * 1000.0) as u64 + 50);
    ptr.button(&mut h, MouseButton::Left, false);
    assert_eq!(
        (h.model().sizes_chosen, h.model().size, popup(&h, "sizes")),
        (1, 0, None),
        "held, the release chooses the option under it"
    );

    ptr.move_to(&mut h, button);
    ptr.button(&mut h, MouseButton::Left, true);
    let large = row_point(&h, "sizes", 2, 0.5);
    ptr.glide(&mut h, large, 4, 16);
    ptr.button(&mut h, MouseButton::Left, false);
    assert_eq!(
        (h.model().sizes_chosen, h.model().size, popup(&h, "sizes")),
        (2, 2, None),
        "dragged onto another option, at once"
    );
}

#[test]
fn a_diagonal_into_a_submenu_keeps_it_across_the_rows_it_crosses() {
    let mut h = Harness::<Browser>::new();
    let mut ptr = Pointer::new(&h);
    sort_by_open(&mut h, &mut ptr);
    let menu = popup(&h, "menu").unwrap();
    let rows: Vec<(u64, Rect)> = (1..4).map(|i| (i, row_rect(&h, "menu", i))).collect();
    let size = row_point(&h, "menu/1", 3, 0.3);
    let from = ptr.at;
    let mut crossed = Vec::new();
    for step in 1..=16 {
        let k = f64::from(step) / 16.0;
        let p = Point::new(
            from.x + (size.x - from.x) * k,
            from.y + (size.y - from.y) * k,
        );
        ptr.wait(&mut h, 16);
        ptr.move_to(&mut h, p);
        if ptr.locate(&h, p).0 == menu {
            crossed.extend(rows.iter().filter(|(_, r)| r.contains(p)).map(|(i, _)| *i));
            assert_eq!(h.model().submenus, [0], "Sort By's submenu stays");
            assert_eq!(focused(&h, "menu"), Some(0), "and its row keeps the focus");
        }
    }
    assert!(
        crossed.contains(&1) && crossed.contains(&2),
        "the way crosses Refresh and Group By: {crossed:?}"
    );
    assert_eq!(h.model().submenus, [0], "Sort By's submenu stays");
    assert_eq!(focused(&h, "menu"), Some(0), "and its row keeps the focus");
    assert_eq!(focused(&h, "menu/1"), Some(3), "Size, in the submenu");
    ptr.wait(&mut h, 1000);
    assert_eq!(h.model().submenus, [0]);
}

#[test]
fn a_rest_on_the_way_gives_the_submenu_up_to_the_row_under_the_pointer() {
    let mut h = Harness::<Browser>::new();
    let mut ptr = Pointer::new(&h);
    sort_by_open(&mut h, &mut ptr);
    let from = ptr.at;
    let group = row_point(&h, "menu", 2, 0.5);
    let rest = Point::new(from.x + 30.0, group.y);
    ptr.glide(&mut h, rest, 4, 16);
    assert_eq!(focused(&h, "menu"), Some(0), "heading into the submenu");
    ptr.wait(&mut h, (SUBMENU_AIM_PAUSE * 1000.0) as u64 + 10);
    assert_eq!(focused(&h, "menu"), Some(2), "resting, Group By takes over");
    assert_eq!(h.model().submenus, [0], "until its own delay");
    ptr.wait(&mut h, (SUBMENU_OPEN_DELAY * 1000.0) as u64 + 10);
    assert_eq!(h.model().submenus, [2], "its submenu replaces Sort By's");
}

#[test]
fn a_slow_path_gives_the_submenu_up() {
    let mut h = Harness::<Browser>::new();
    let mut ptr = Pointer::new(&h);
    sort_by_open(&mut h, &mut ptr);
    let size = row_point(&h, "menu/1", 3, 0.3);
    let from = ptr.at;
    let mut switched = false;
    for step in 1..=16 {
        let k = f64::from(step) / 16.0;
        ptr.wait(&mut h, 400);
        ptr.move_to(
            &mut h,
            Point::new(
                from.x + (size.x - from.x) * k,
                from.y + (size.y - from.y) * k,
            ),
        );
        if h.model().submenus == [2] {
            switched = true;
            break;
        }
    }
    assert!(switched, "crawling over Group By opens its submenu instead");
}

#[test]
fn typed_letters_find_a_row_return_chooses_it_and_escape_closes_one_level_then_the_menu() {
    let mut h = Harness::<Browser>::new();
    let mut ptr = Pointer::new(&h);
    h.tap(TAB);
    h.tap(TAB);
    h.tap(MENU);
    ptr.settle(&mut h);
    h.window = popup(&h, "menu").unwrap();
    h.type_text("rel");
    assert_eq!(focused(&h, "menu"), Some(3), "Reload, not Refresh");
    h.key(RETURN, true);
    assert_eq!(h.model().chosen, ["Reload"]);
    assert_eq!(popup(&h, "menu"), None);

    h.window = ptr.main;
    h.tap(MENU);
    ptr.settle(&mut h);
    let menu = popup(&h, "menu").unwrap();
    h.window = menu;
    h.type_text("g");
    assert_eq!(focused(&h, "menu"), Some(2));
    h.tap(RIGHT);
    ptr.settle(&mut h);
    h.window = popup(&h, "menu/1").unwrap();
    h.type_text("a");
    assert_eq!(focused(&h, "menu/1"), Some(1), "Application");
    h.key(ESCAPE, true);
    ptr.settle(&mut h);
    assert_eq!(
        (h.model().submenus.clone(), popup(&h, "menu/1")),
        (Vec::new(), None)
    );
    assert_eq!(focused(&h, "menu"), Some(2), "back on Group By");
    h.window = menu;
    h.key(ESCAPE, true);
    ptr.settle(&mut h);
    assert_eq!((h.model().closes, popup(&h, "menu")), (2, None));
    assert!(h.focused("canvas"), "and the focus back on the canvas");
    assert_eq!(h.model().chosen, ["Reload"]);
}

#[test]
fn resting_on_a_separator_closes_the_open_submenu_as_another_row_does() {
    let mut h = Harness::<Browser>::new();
    let mut ptr = Pointer::new(&h);
    sort_by_open(&mut h, &mut ptr);
    // down and away from the submenu, in one move
    let separator = row_point(&h, "menu", 4, 0.2);
    ptr.wait(&mut h, 16);
    ptr.move_to(&mut h, separator);
    assert_eq!(h.model().submenus, [0], "not at once");
    ptr.wait(&mut h, 310);
    assert!(h.model().submenus.is_empty());
}
