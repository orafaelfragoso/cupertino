//! Menus follow a finger as they follow the pointer. Every event of a finger goes to the window it went down in, so a
//! finger that opens a menu on its button and slides onto it still speaks to the button's window: the menu hears it
//! move and lift at its own point. A tap on a pop-up button opens its menu and leaves it open; put down, slid onto an
//! option and lifted, it chooses; lifted off the menus it chooses nothing and closes them, on a gap it keeps them
//! open; a finger that rests on a context menu's region opens its menu, whose submenus open under it too; a second
//! finger opens and chooses nothing; and a cancelled touch ends with nothing chosen.

mod support;

use cupertino_geometry::{Insets, Point, Rect};
use cupertino_menu_model::tracking::{SUBMENU_OPEN_DELAY, TRACKING_INTERVAL};
use cupertino_ui_controls::{
    Button, ButtonRole, ButtonStyle, ContextMenu, Menu, MenuAnchor, MenuItem, PopUpButton, Stack,
};
use cupertino_ui_core::{Application, Command, Element, Key, Window, WindowKey};
use cupertino_ui_layout::{Align, Axis};
use cupertino_ui_platform::{PopupEdge, TouchEvent, WindowEvent, WindowId, WindowStates};
use support::{window, Harness};

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
    /// The canvas button under the context menu's region was clicked.
    Pressed,
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
    pressed: usize,
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

/// Sort By opens a submenu; a separator is a gap that chooses nothing.
fn items() -> Vec<MenuItem<Message>> {
    vec![
        MenuItem::Submenu {
            label: "Sort By".into(),
            enabled: true,
            items: vec![item("Name"), item("Kind"), item("Date Added")],
        },
        item("Refresh"),
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
            Message::Pressed => model.pressed += 1,
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
                        on_press: Some(Message::Pressed),
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

/// Fingers as the compositor routes them: a finger goes down in the window under it, and every later event of it goes
/// to that window, at points in its coordinates, wherever the finger is.
struct Fingers {
    main: WindowId,
    configured: Vec<WindowId>,
    /// Each finger down: its id, the window it went down in and where it is, in the main window's coordinates.
    down: Vec<(i32, WindowId, Point)>,
}

impl Fingers {
    fn new(h: &Harness<Browser>) -> Self {
        Fingers {
            main: h.window,
            configured: vec![h.window],
            down: Vec::new(),
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
                // the compositor places a popup where it asked and gives it the size it was placed at
                let size = h.rt.window_system().placement(id).unwrap().map(|p| p.size);
                let placed = in_parent(h, id);
                h.rt.window_system_mut()
                    .enqueue_window(id, WindowEvent::PopupPlaced(placed))
                    .unwrap();
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

    /// The window under `p`, in the main window's coordinates.
    fn under(&self, h: &Harness<Browser>, p: Point) -> WindowId {
        let popups: Vec<WindowId> =
            h.rt.windows()
                .into_iter()
                .map(|(_, id)| id)
                .filter(|&id| id != self.main)
                .collect();
        popups
            .into_iter()
            .rev()
            .find(|&id| rect(h, id).contains(p))
            .unwrap_or(self.main)
    }

    /// `p` in the coordinates of window `id`, which the compositor still sends a finger's events to.
    fn local(h: &Harness<Browser>, id: WindowId, p: Point) -> Point {
        let r = rect(h, id);
        Point::new(p.x - r.min_x(), p.y - r.min_y())
    }

    fn put_down(&mut self, h: &mut Harness<Browser>, id: i32, p: Point) {
        let window = self.under(h, p);
        self.down.push((id, window, p));
        h.window = window;
        h.touch(TouchEvent::Down {
            id,
            position: Self::local(h, window, p),
        });
        self.settle(h);
    }

    fn finger(&self, id: i32) -> (WindowId, Point) {
        let &(_, window, at) = self
            .down
            .iter()
            .find(|(f, ..)| *f == id)
            .expect("the finger is down");
        (window, at)
    }

    fn move_to(&mut self, h: &mut Harness<Browser>, id: i32, p: Point) {
        let (window, _) = self.finger(id);
        if let Some(f) = self.down.iter_mut().find(|(f, ..)| *f == id) {
            f.2 = p;
        }
        if !alive(h, window) {
            return;
        }
        h.window = window;
        h.touch(TouchEvent::Motion {
            id,
            position: Self::local(h, window, p),
        });
        self.settle(h);
    }

    /// From where finger `id` is to `to` in `steps` moves 16 ms apart.
    fn glide(&mut self, h: &mut Harness<Browser>, id: i32, to: Point, steps: u32) {
        let (_, from) = self.finger(id);
        for step in 1..=steps {
            let k = f64::from(step) / f64::from(steps);
            let p = Point::new(from.x + (to.x - from.x) * k, from.y + (to.y - from.y) * k);
            self.wait(h, 16);
            self.move_to(h, id, p);
        }
    }

    fn lift(&mut self, h: &mut Harness<Browser>, id: i32) {
        let (window, _) = self.finger(id);
        self.down.retain(|(f, ..)| *f != id);
        if !alive(h, window) {
            return;
        }
        h.window = window;
        h.touch(TouchEvent::Up { id });
        self.settle(h);
    }

    /// The compositor takes every finger away, as for a gesture of its own.
    fn cancel(&mut self, h: &mut Harness<Browser>) {
        let windows: Vec<WindowId> = self.down.drain(..).map(|(_, w, _)| w).collect();
        for window in windows {
            if alive(h, window) {
                h.window = window;
                h.touch(TouchEvent::Cancel);
            }
        }
        self.settle(h);
    }

    /// A finger put down at `p` and lifted 80 ms later, without moving.
    fn tap(&mut self, h: &mut Harness<Browser>, id: i32, p: Point) {
        self.put_down(h, id, p);
        self.wait(h, 80);
        self.lift(h, id);
    }
}

fn alive(h: &Harness<Browser>, id: WindowId) -> bool {
    h.rt.windows().iter().any(|(_, w)| *w == id)
}

/// The rect of the open window `id` in the main window's coordinates, where it asked to be placed.
fn rect(h: &Harness<Browser>, id: WindowId) -> Rect {
    let system = h.rt.window_system();
    let Some(parent) = system.parent(id).unwrap() else {
        return Rect::new(0.0, 0.0, 200.0, 100.0);
    };
    let base = rect(h, parent).origin;
    let spec = system.placement(id).unwrap().unwrap().clone();
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

/// The rect of the open popup `id` in its parent's coordinates.
fn in_parent(h: &Harness<Browser>, id: WindowId) -> Rect {
    let r = rect(h, id);
    let parent = h.rt.window_system().parent(id).unwrap().expect("a popup");
    let base = rect(h, parent).origin;
    Rect::new(
        r.min_x() - base.x,
        r.min_y() - base.y,
        r.width(),
        r.height(),
    )
}

fn popup(h: &Harness<Browser>, key: &str) -> Option<WindowId> {
    h.rt.windows()
        .into_iter()
        .find(|(k, _)| *k == WindowKey(key.to_owned().into()))
        .map(|(_, id)| id)
}

/// The middle of row `index` of the menu `key`, `along` of its width in, in the main window's coordinates.
fn row_point(h: &Harness<Browser>, key: &str, index: u64, along: f64) -> Point {
    let id = popup(h, key).expect("the menu is open");
    let node = h.rt.find(id, &Key::from(index)).unwrap();
    let r =
        h.rt.snapshot(id)
            .unwrap()
            .nodes
            .into_iter()
            .find(|n| n.id == node)
            .unwrap()
            .rect;
    let origin = rect(h, id).origin;
    Point::new(
        origin.x + r.min_x() + r.width() * along,
        origin.y + r.min_y() + r.height() / 2.0,
    )
}

/// The row of the menu `key` with the focus.
fn focused(h: &Harness<Browser>, key: &str) -> Option<u64> {
    let id = popup(h, key)?;
    let focus = h.rt.snapshot(id).unwrap().focus?;
    (0..8).find(|&i| h.rt.find(id, &Key::from(i)) == Some(focus))
}

/// The tracking interval and a little more, in milliseconds: a press held this long is held.
fn held() -> u64 {
    (TRACKING_INTERVAL * 1000.0) as u64 + 50
}

/// A finger rests on the canvas until its context menu opens there.
fn rest_on_canvas(h: &mut Harness<Browser>, f: &mut Fingers, id: i32) {
    let canvas = h.center("canvas");
    f.put_down(h, id, canvas);
    assert_eq!(popup(h, "menu"), None, "not at once");
    f.wait(h, held());
    assert!(popup(h, "menu").is_some(), "the resting finger opens it");
}

#[test]
fn a_tap_on_a_pop_up_button_opens_its_menu_and_leaves_it_open_and_a_tap_on_an_option_chooses_it() {
    let mut h = Harness::<Browser>::new();
    let mut f = Fingers::new(&h);
    let button = h.center("size");
    f.put_down(&mut h, 1, button);
    assert_eq!(
        h.model().menu,
        Some(Open::Sizes),
        "it opens as the finger goes down"
    );
    f.wait(&mut h, 80);
    f.lift(&mut h, 1);
    assert!(
        popup(&h, "sizes").is_some() && h.model().sizes_chosen == 0,
        "a tap leaves it open over the button"
    );
    let large = row_point(&h, "sizes", 2, 0.5);
    f.tap(&mut h, 2, large);
    assert_eq!(
        (h.model().sizes_chosen, h.model().size, popup(&h, "sizes")),
        (1, 2, None),
        "a tap on an option chooses it"
    );
}

#[test]
fn a_finger_slid_from_a_pop_up_button_onto_an_option_chooses_it_and_one_held_still_chooses_the_one_under_it(
) {
    let mut h = Harness::<Browser>::new();
    let mut f = Fingers::new(&h);
    let button = h.center("size");
    f.put_down(&mut h, 1, button);
    let large = row_point(&h, "sizes", 2, 0.5);
    f.glide(&mut h, 1, large, 4);
    assert_eq!(
        focused(&h, "sizes"),
        Some(2),
        "Large is highlighted under the finger"
    );
    f.lift(&mut h, 1);
    assert_eq!(
        (h.model().sizes_chosen, h.model().size, popup(&h, "sizes")),
        (1, 2, None),
        "slid onto Large, the lift chooses it at once"
    );

    // the menu opens over the button with the chosen option under the finger
    f.put_down(&mut h, 2, button);
    f.wait(&mut h, held());
    f.lift(&mut h, 2);
    assert_eq!(
        (h.model().sizes_chosen, h.model().size, popup(&h, "sizes")),
        (2, 2, None),
        "held, the lift chooses the option under it"
    );
}

#[test]
fn a_finger_slid_off_the_menu_and_lifted_chooses_nothing_and_closes_it() {
    let mut h = Harness::<Browser>::new();
    let mut f = Fingers::new(&h);
    let button = h.center("size");
    f.put_down(&mut h, 1, button);
    let large = row_point(&h, "sizes", 2, 0.5);
    f.glide(&mut h, 1, large, 4);
    let menu = rect(&h, popup(&h, "sizes").unwrap());
    let away = Point::new(menu.max_x() + 60.0, menu.max_y() + 40.0);
    f.glide(&mut h, 1, away, 4);
    f.wait(&mut h, held());
    f.lift(&mut h, 1);
    assert_eq!(
        (h.model().sizes_chosen, h.model().size),
        (0, 0),
        "lifted outside, the value stays Small"
    );
    assert_eq!((popup(&h, "sizes"), h.model().menu.clone()), (None, None));
}

#[test]
fn a_finger_resting_on_a_context_region_opens_its_menu_and_one_slid_onto_an_item_chooses_it_without_clicking_under_it(
) {
    let mut h = Harness::<Browser>::new();
    let mut f = Fingers::new(&h);
    rest_on_canvas(&mut h, &mut f, 1);
    let copy = row_point(&h, "menu", 3, 0.5);
    f.glide(&mut h, 1, copy, 6);
    assert_eq!(focused(&h, "menu"), Some(3), "Copy is highlighted");
    f.lift(&mut h, 1);
    assert_eq!(h.model().chosen, ["Copy"], "chosen once");
    assert_eq!((h.model().closes, popup(&h, "menu")), (1, None));
    assert_eq!(
        h.model().pressed,
        0,
        "the canvas button under the finger is not clicked"
    );

    // held and lifted where it went down, the menu stays open and the button is not clicked
    rest_on_canvas(&mut h, &mut f, 2);
    let (_, at) = f.finger(2);
    f.move_to(&mut h, 2, Point::new(at.x - 1.0, at.y - 1.0));
    f.lift(&mut h, 2);
    assert!(
        popup(&h, "menu").is_some(),
        "lifted off the menu without crossing it"
    );
    assert_eq!(h.model().pressed, 0);
}

#[test]
fn a_quick_tap_or_a_finger_that_moves_opens_no_context_menu() {
    let mut h = Harness::<Browser>::new();
    let mut f = Fingers::new(&h);
    let canvas = h.center("canvas");
    f.tap(&mut h, 1, canvas);
    assert_eq!(h.model().pressed, 1, "a tap clicks the button");
    f.wait(&mut h, held());
    assert_eq!(popup(&h, "menu"), None, "and opens nothing");

    f.put_down(&mut h, 2, canvas);
    f.glide(&mut h, 2, Point::new(canvas.x + 20.0, canvas.y), 2);
    f.wait(&mut h, held());
    assert_eq!(
        popup(&h, "menu"),
        None,
        "a finger that moves is not resting"
    );
    f.lift(&mut h, 2);
}

#[test]
fn lifted_on_a_gap_the_menu_stays_open_and_lifted_off_it_after_crossing_it_closes_it() {
    let mut h = Harness::<Browser>::new();
    let mut f = Fingers::new(&h);
    rest_on_canvas(&mut h, &mut f, 1);
    let gap = row_point(&h, "menu", 2, 0.5);
    f.glide(&mut h, 1, gap, 6);
    f.wait(&mut h, held());
    f.lift(&mut h, 1);
    assert!(
        popup(&h, "menu").is_some(),
        "the separator chooses nothing and keeps the menu"
    );
    assert!(h.model().chosen.is_empty());

    h.window = popup(&h, "menu").unwrap();
    h.send(WindowEvent::PopupDone);
    f.settle(&mut h);
    assert_eq!(h.model().menu, None);

    rest_on_canvas(&mut h, &mut f, 2);
    let copy = row_point(&h, "menu", 3, 0.5);
    f.glide(&mut h, 2, copy, 4);
    let canvas = h.center("canvas");
    f.glide(&mut h, 2, canvas, 4);
    f.lift(&mut h, 2);
    assert!(h.model().chosen.is_empty(), "nothing chosen");
    assert_eq!(
        popup(&h, "menu"),
        None,
        "crossed and lifted off it, the menu closes"
    );
    assert_eq!(h.model().pressed, 0);
}

#[test]
fn a_tap_outside_the_menu_dismisses_it_with_nothing_chosen() {
    let mut h = Harness::<Browser>::new();
    let mut f = Fingers::new(&h);
    let button = h.center("size");
    f.put_down(&mut h, 1, button);
    let menu = popup(&h, "sizes").expect("open as the finger goes down");
    f.wait(&mut h, 80);
    f.lift(&mut h, 1);
    assert_eq!(popup(&h, "sizes"), Some(menu), "the tap leaves it open");
    // a tap outside a popup that holds the grab dismisses it: the compositor ends it and keeps the tap
    h.window = menu;
    h.send(WindowEvent::PopupDone);
    f.settle(&mut h);
    assert_eq!((popup(&h, "sizes"), h.model().menu.clone()), (None, None));
    assert_eq!((h.model().sizes_chosen, h.model().size), (0, 0));
}

#[test]
fn a_finger_resting_on_a_submenu_row_opens_it_and_lifted_on_its_item_chooses_it() {
    let mut h = Harness::<Browser>::new();
    let mut f = Fingers::new(&h);
    rest_on_canvas(&mut h, &mut f, 1);
    let sort_by = row_point(&h, "menu", 0, 0.5);
    f.glide(&mut h, 1, sort_by, 4);
    f.wait(&mut h, (SUBMENU_OPEN_DELAY * 1000.0) as u64 + 10);
    assert!(
        popup(&h, "menu/1").is_some(),
        "Sort By's submenu opens under the finger"
    );
    let edge = row_point(&h, "menu", 0, 0.95);
    f.glide(&mut h, 1, edge, 2);
    let kind = row_point(&h, "menu/1", 1, 0.5);
    f.glide(&mut h, 1, kind, 6);
    assert_eq!(focused(&h, "menu/1"), Some(1), "Kind is highlighted");
    f.lift(&mut h, 1);
    assert_eq!(h.model().chosen, ["Kind"], "chosen once");
    assert_eq!((popup(&h, "menu"), popup(&h, "menu/1")), (None, None));
    assert_eq!(h.model().pressed, 0);
}

#[test]
fn a_finger_put_down_in_a_menu_slides_into_its_submenu_and_chooses_there() {
    let mut h = Harness::<Browser>::new();
    let mut f = Fingers::new(&h);
    rest_on_canvas(&mut h, &mut f, 1);
    f.lift(&mut h, 1);
    assert!(popup(&h, "menu").is_some());
    // a second touch, put down on the menu itself: every event of it goes to the menu's window
    let refresh = row_point(&h, "menu", 1, 0.5);
    f.put_down(&mut h, 2, refresh);
    let sort_by = row_point(&h, "menu", 0, 0.5);
    f.glide(&mut h, 2, sort_by, 2);
    f.wait(&mut h, (SUBMENU_OPEN_DELAY * 1000.0) as u64 + 10);
    assert!(popup(&h, "menu/1").is_some());
    let edge = row_point(&h, "menu", 0, 0.95);
    f.glide(&mut h, 2, edge, 2);
    let name = row_point(&h, "menu/1", 0, 0.5);
    f.glide(&mut h, 2, name, 4);
    f.lift(&mut h, 2);
    assert_eq!(h.model().chosen, ["Name"]);
    assert_eq!((popup(&h, "menu"), popup(&h, "menu/1")), (None, None));
}

#[test]
fn a_second_finger_neither_opens_nor_chooses_while_the_first_holds_the_menu() {
    let mut h = Harness::<Browser>::new();
    let mut f = Fingers::new(&h);
    let button = h.center("size");
    f.put_down(&mut h, 1, button);
    assert_eq!(h.model().menu, Some(Open::Sizes));
    // a second finger taps an option while the first still holds the menu it opened
    let medium = row_point(&h, "sizes", 1, 0.5);
    f.tap(&mut h, 2, medium);
    assert_eq!(
        (h.model().sizes_chosen, popup(&h, "sizes").is_some()),
        (0, true),
        "the second finger chooses nothing"
    );
    // and a second finger resting on the canvas opens no other menu
    let canvas = h.center("canvas");
    f.put_down(&mut h, 3, canvas);
    f.wait(&mut h, held());
    assert_eq!(
        h.model().menu,
        Some(Open::Sizes),
        "the first finger's menu stays"
    );
    f.lift(&mut h, 3);
    // the first finger still chooses
    let large = row_point(&h, "sizes", 2, 0.5);
    f.glide(&mut h, 1, large, 4);
    f.lift(&mut h, 1);
    assert_eq!((h.model().sizes_chosen, h.model().size), (1, 2));
}

#[test]
fn a_cancelled_touch_ends_with_nothing_chosen_and_the_menu_answers_the_next() {
    let mut h = Harness::<Browser>::new();
    let mut f = Fingers::new(&h);
    let button = h.center("size");
    f.put_down(&mut h, 1, button);
    let large = row_point(&h, "sizes", 2, 0.5);
    f.glide(&mut h, 1, large, 4);
    f.cancel(&mut h);
    assert_eq!(
        (h.model().sizes_chosen, h.model().size),
        (0, 0),
        "a cancelled touch chooses nothing"
    );
    assert!(
        popup(&h, "sizes").is_some(),
        "the menu stays, for another touch"
    );
    let medium = row_point(&h, "sizes", 1, 0.5);
    f.tap(&mut h, 2, medium);
    assert_eq!(
        (h.model().sizes_chosen, h.model().size, popup(&h, "sizes")),
        (1, 1, None),
        "the next tap chooses"
    );
}
