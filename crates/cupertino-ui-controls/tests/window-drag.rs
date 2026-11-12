//! The window's drag region: a navigation stack's toolbar row and a [`WindowDragArea`] start an interactive move on a
//! drag that no control inside takes, and zoom the window on a double click; the controls keep their presses and the
//! rest of the window starts nothing.

use crate::support;

use cupertino_geometry::{Point, Size};
use cupertino_ui_controls::{
    Button, ButtonRole, ButtonStyle, Navigation, NavigationStack, Spacer, Stack, Text, TextChange,
    TextField, TextRole, WindowDragArea,
};
use cupertino_ui_core::{Application, Command, Element, Window};
use cupertino_ui_layout::{Align, Axis};
use cupertino_ui_platform::{
    Mods, MouseButton, PointerEvent, Timestamp, WindowEvent, WindowRequest, WindowStates,
};
use cupertino_ui_text::{AttributedText, ContentPurpose, TextStyle};
use support::{window, Harness};

/// Half a second in nanoseconds: the longest gap of a double click.
const HALF_SECOND: u64 = 500_000_000;

#[derive(Clone, Debug, PartialEq)]
enum Message {
    Navigate(Navigation),
    Search,
    Share,
    Inner,
}

/// The messages the window sent, in order.
type Model = Vec<Message>;

struct Browser;

fn text(key: &'static str, content: &str) -> Element<Message> {
    Element::new(Text {
        content: AttributedText::plain(content, TextStyle::new(13.0)),
        role: TextRole::Body,
        wrap: false,
        secondary: false,
        selected: false,
    })
    .key(key)
}

fn button(key: &'static str, on_press: Message) -> Element<Message> {
    Element::new(Button {
        label: key.into(),
        role: ButtonRole::Normal,
        style: ButtonStyle::Bordered,
        enabled: true,
        busy: false,
        on_press: Some(on_press),
    })
    .key(key)
}

impl Application for Browser {
    type Model = Model;
    type Message = Message;

    fn init() -> (Model, Vec<Command<Message>>) {
        (Vec::new(), Vec::new())
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        model.push(message);
        Vec::new()
    }

    fn view(_: &Model) -> Vec<Window<Message>> {
        // the page: a plain label, and a drag area holding a label and a button
        let page = Element::new(Stack {
            axis: Axis::Vertical,
            padding: Default::default(),
            spacing: 20.0,
            align: Align::Start,
            children: vec![
                text("page-label", "Contents"),
                Element::from(WindowDragArea {
                    content: Element::new(Stack {
                        axis: Axis::Horizontal,
                        padding: Default::default(),
                        spacing: 20.0,
                        align: Align::Center,
                        children: vec![
                            text("area-label", "Status"),
                            button("Inner", Message::Inner),
                        ],
                    }),
                })
                .key("area"),
            ],
        });
        window(Element::from(NavigationStack {
            content: page.key("page"),
            toolbar: vec![
                text("title", "Documents"),
                Element::new(Spacer { min: 40.0 }).key("gap"),
                Element::new(TextField {
                    label: "Search".into(),
                    placeholder: String::new(),
                    value: "notes".into(),
                    revision: 0,
                    purpose: ContentPurpose::Normal,
                    enabled: true,
                    on_change: Some(Box::new(|_: TextChange| Message::Search)),
                    on_submit: None,
                    selection: None,
                    initial: false,
                })
                .width(120.0)
                .key("search"),
                button("Share", Message::Share),
            ],
            can_go_back: false,
            can_go_forward: true,
            on_navigate: Some(Box::new(Message::Navigate)),
            under_toolbar: false,
        }))
    }
}

fn harness() -> Harness<Browser> {
    let mut h = Harness::<Browser>::new();
    h.resize(Size::new(600.0, 300.0));
    h
}

fn at(h: &mut Harness<Browser>, event: PointerEvent, time: u64) {
    h.send(WindowEvent::Pointer(event, Timestamp(time)));
}

fn press(h: &mut Harness<Browser>, position: Point, pressed: bool, time: u64) {
    at(
        h,
        PointerEvent::Button {
            position,
            button: MouseButton::Left,
            pressed,
            serial: 7,
            mods: Mods::NONE,
        },
        time,
    );
}

/// Presses at `from`, moves `by` with the button held and releases; then the pointer leaves the window, as it does
/// when the compositor takes it for a move.
fn drag(h: &mut Harness<Browser>, from: Point, by: Point, time: u64) {
    at(h, PointerEvent::Motion { position: from }, time);
    press(h, from, true, time);
    let to = Point::new(from.x + by.x, from.y + by.y);
    at(h, PointerEvent::Motion { position: to }, time + 1);
    press(h, to, false, time + 2);
    at(h, PointerEvent::Leave, time + 3);
}

/// A press and release at `position` at `time`.
fn click(h: &mut Harness<Browser>, position: Point, time: u64) {
    at(h, PointerEvent::Motion { position }, time);
    press(h, position, true, time);
    press(h, position, false, time + 1);
}

fn requests(h: &Harness<Browser>) -> Vec<WindowRequest> {
    h.rt.window_system().requests(h.window).unwrap().to_vec()
}

fn moves(h: &Harness<Browser>) -> usize {
    requests(h)
        .iter()
        .filter(|r| **r == WindowRequest::Move)
        .count()
}

fn zooms(h: &Harness<Browser>) -> Vec<bool> {
    requests(h)
        .iter()
        .filter_map(|r| match r {
            WindowRequest::Maximize(on) => Some(*on),
            _ => None,
        })
        .collect()
}

/// A point in the toolbar row between the title and the search field, where no item lies.
fn empty_toolbar(h: &Harness<Browser>) -> Point {
    h.center("gap")
}

#[test]
fn a_drag_on_the_toolbar_where_no_control_is_moves_the_window() {
    let mut h = harness();
    let gap = empty_toolbar(&h);
    assert!(gap.y < 52.0, "the gap is in the toolbar row");
    drag(&mut h, gap, Point::new(0.0, 30.0), 0);
    assert_eq!(moves(&h), 1);
    // a label in the toolbar lets the press through
    let p = h.center("title");
    drag(&mut h, p, Point::new(-12.0, 0.0), HALF_SECOND * 10);
    assert_eq!(moves(&h), 2);
    assert!(h.model().is_empty(), "the drags sent no message");
}

#[test]
fn a_press_that_does_not_travel_past_the_drag_start_stays_a_click() {
    let mut h = harness();
    let p = empty_toolbar(&h);
    drag(&mut h, p, Point::new(5.0, -5.0), 0);
    assert_eq!(moves(&h), 0, "5 pt on each axis is still a click");
    assert!(zooms(&h).is_empty(), "one click zooms nothing");
}

#[test]
fn the_toolbar_controls_keep_their_presses() {
    let mut h = harness();
    // the button: a drag off it neither presses it nor moves the window; a click presses it
    let share = h.center("Share");
    drag(&mut h, share, Point::new(0.0, 40.0), 0);
    click(&mut h, share, HALF_SECOND * 10);
    assert_eq!(h.model(), &vec![Message::Share]);
    // the search field selects its text instead
    let p = h.center("search");
    drag(&mut h, p, Point::new(-40.0, 0.0), HALF_SECOND * 20);
    // the forward button navigates
    let forward = Point::new(8.0 + 53.0 * 0.75, 26.0);
    click(&mut h, forward, HALF_SECOND * 30);
    assert_eq!(moves(&h), 0, "no control's press moved the window");
    assert_eq!(
        h.model().last(),
        Some(&Message::Navigate(Navigation::Forward))
    );
}

#[test]
fn the_page_under_the_toolbar_starts_no_move() {
    let mut h = harness();
    let p = h.center("page-label");
    drag(&mut h, p, Point::new(0.0, 40.0), 0);
    let page = h.rect("page");
    drag(
        &mut h,
        Point::new(page.max_x() - 10.0, page.max_y() - 10.0),
        Point::new(-40.0, 0.0),
        HALF_SECOND * 10,
    );
    assert_eq!(moves(&h), 0);
}

#[test]
fn a_drag_area_moves_the_window_and_its_controls_keep_their_presses() {
    let mut h = harness();
    let p = h.center("area-label");
    drag(&mut h, p, Point::new(20.0, 20.0), 0);
    assert_eq!(moves(&h), 1);
    let p = h.center("Inner");
    click(&mut h, p, HALF_SECOND * 10);
    let p = h.center("Inner");
    drag(&mut h, p, Point::new(0.0, 40.0), HALF_SECOND * 20);
    assert_eq!(moves(&h), 1, "the button in the area moved nothing");
    assert_eq!(h.model(), &vec![Message::Inner]);
}

#[test]
fn a_double_click_on_the_toolbar_zooms_the_window_and_another_restores_it() {
    let mut h = harness();
    let gap = empty_toolbar(&h);
    click(&mut h, gap, 0);
    click(&mut h, gap, HALF_SECOND - 2);
    assert_eq!(zooms(&h), vec![true], "a double click maximizes the window");
    assert_eq!(moves(&h), 0);
    // the compositor maximizes it; the next double click restores it
    let maximized = WindowStates {
        activated: true,
        maximized: true,
        ..WindowStates::default()
    };
    h.rt.window_system_mut()
        .configure(h.window, Some(Size::new(1000.0, 700.0)), maximized)
        .unwrap();
    h.settle();
    click(&mut h, gap, HALF_SECOND * 10);
    click(&mut h, gap, HALF_SECOND * 10 + 1_000_000);
    assert_eq!(zooms(&h), vec![true, false]);
}

#[test]
fn presses_too_far_apart_or_on_a_control_are_no_double_click() {
    let mut h = harness();
    let gap = empty_toolbar(&h);
    click(&mut h, gap, 0);
    click(&mut h, gap, HALF_SECOND + 2);
    assert!(zooms(&h).is_empty(), "more than half a second apart");
    // a third press soon after the second is the double click: the second started a new pair
    click(&mut h, gap, HALF_SECOND * 2);
    assert_eq!(zooms(&h), vec![true]);
    // two quick presses on the button press it twice
    let share = h.center("Share");
    click(&mut h, share, HALF_SECOND * 10);
    click(&mut h, share, HALF_SECOND * 10 + 1_000_000);
    assert_eq!(zooms(&h), vec![true]);
    assert_eq!(h.model(), &vec![Message::Share, Message::Share]);
    // a move between two presses ends the pair
    drag(&mut h, gap, Point::new(0.0, 30.0), HALF_SECOND * 20);
    click(&mut h, gap, HALF_SECOND * 20 + 1_000_000);
    assert_eq!(
        zooms(&h),
        vec![true],
        "the press after a move is a first click"
    );
}

#[test]
fn the_toolbar_keeps_its_one_role_and_adds_no_tab_stop() {
    let h = harness();
    let snapshot = h.snapshot();
    let toolbars = snapshot
        .accessibility
        .iter()
        .filter(|(_, n)| n.role == cupertino_ui_core::Role::Toolbar)
        .count();
    assert_eq!(toolbars, 1);
    let area =
        h.rt.find(h.window, &cupertino_ui_core::Key::from("area"))
            .unwrap();
    assert!(
        snapshot
            .accessibility
            .iter()
            .all(|(id, n)| *id != area || !n.states.focusable),
        "the drag area is no Tab stop"
    );
}

#[test]
fn a_fullscreen_window_neither_moves_nor_zooms_from_its_toolbar() {
    let mut h = harness();
    let fullscreen = WindowStates {
        activated: true,
        fullscreen: true,
        ..WindowStates::default()
    };
    h.rt.window_system_mut()
        .configure(h.window, Some(Size::new(1000.0, 700.0)), fullscreen)
        .unwrap();
    h.settle();
    let gap = empty_toolbar(&h);
    drag(&mut h, gap, Point::new(0.0, 30.0), 0);
    click(&mut h, gap, HALF_SECOND * 10);
    click(&mut h, gap, HALF_SECOND * 10 + 1_000_000);
    assert_eq!(moves(&h), 0, "a fullscreen window does not move");
    assert!(
        zooms(&h).is_empty(),
        "a maximize request would take the window out of fullscreen"
    );
    // out of fullscreen, the toolbar moves and zooms the window again
    h.rt.window_system_mut()
        .configure(
            h.window,
            Some(Size::new(600.0, 300.0)),
            WindowStates {
                activated: true,
                ..WindowStates::default()
            },
        )
        .unwrap();
    h.settle();
    let gap = empty_toolbar(&h);
    drag(&mut h, gap, Point::new(0.0, 30.0), HALF_SECOND * 20);
    click(&mut h, gap, HALF_SECOND * 30);
    click(&mut h, gap, HALF_SECOND * 30 + 1_000_000);
    assert_eq!(moves(&h), 1);
    assert_eq!(zooms(&h), vec![true]);
}
