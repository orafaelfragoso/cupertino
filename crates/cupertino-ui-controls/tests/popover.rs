mod support;

use cupertino_geometry::{Insets, Point};
use cupertino_ui_controls::{Button, ButtonRole, ButtonStyle, Popover, Stack};
use cupertino_ui_core::{Application, Command, Element, Role, Window, WindowKey};
use cupertino_ui_layout::{Align, Axis};
use cupertino_ui_platform::{PopupEdge, WindowEvent, WindowId, WindowRequest, WindowStates};
use std::cell::Cell;
use support::{window, Harness, SPACE, TAB};

const ESCAPE: u32 = 0xff1b;

thread_local! {
    static OPEN_AT_START: Cell<bool> = const { Cell::new(false) };
}

#[derive(Clone, Debug, PartialEq)]
enum Message {
    Open,
    Close,
    Shift,
    Choose,
}

#[derive(Default)]
struct Model {
    open: bool,
    shifted: bool,
    closes: usize,
}

struct Options;

fn button(label: &'static str, on_press: Message) -> Element<Message> {
    Element::new(Button {
        label: label.into(),
        role: ButtonRole::Normal,
        style: ButtonStyle::Bordered,
        enabled: true,
        busy: false,
        on_press: Some(on_press),
    })
    .key(label)
}

impl Application for Options {
    type Model = Model;
    type Message = Message;

    fn init() -> (Model, Vec<Command<Message>>) {
        let model = Model {
            open: OPEN_AT_START.get(),
            ..Model::default()
        };
        (model, Vec::new())
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        match message {
            Message::Open => model.open = true,
            Message::Close => {
                model.open = false;
                model.closes += 1;
            }
            Message::Shift => model.shifted = true,
            Message::Choose => {}
        }
        Vec::new()
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        let mut windows = window(Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::new(if model.shifted { 40.0 } else { 10.0 }, 10.0, 0.0, 0.0),
            spacing: 0.0,
            align: Align::Start,
            children: vec![button("Open", Message::Open)],
        }));
        if model.open {
            windows.push(
                Popover {
                    parent: WindowKey("test".into()),
                    anchor: "Open".into(),
                    label: "Options".into(),
                    content: Element::new(Stack {
                        axis: Axis::Vertical,
                        padding: Insets::uniform(0.0),
                        spacing: 6.0,
                        align: Align::Start,
                        children: vec![
                            button("First", Message::Choose),
                            button("Second", Message::Shift),
                        ],
                    }),
                    on_close: Message::Close,
                }
                .window(WindowKey("popover".into())),
            );
        }
        windows
    }
}

fn harness(open_at_start: bool) -> Harness<Options> {
    OPEN_AT_START.set(open_at_start);
    Harness::new()
}

fn popover(h: &Harness<Options>) -> Option<WindowId> {
    h.rt.windows()
        .into_iter()
        .find(|(key, _)| *key == WindowKey("popover".into()))
        .map(|(_, id)| id)
}

/// Moves the harness onto the open popover: input and queries go to it from now on.
fn enter(h: &mut Harness<Options>) -> WindowId {
    let main = h.window;
    let id = popover(h).expect("the popover is open");
    h.rt.window_system_mut()
        .configure(id, None, WindowStates::default())
        .unwrap();
    h.window = id;
    h.settle();
    main
}

#[test]
fn it_opens_below_its_anchor_at_the_size_of_its_content() {
    let mut h = harness(false);
    let anchor = h.rect("Open");
    h.click(h.center("Open"));
    let id = popover(&h).expect("open");
    let system = h.rt.window_system();
    assert_eq!(system.parent(id).unwrap(), Some(h.window));
    let placed = system.placement(id).unwrap().expect("a popup").clone();
    assert_eq!(placed.anchor_rect, anchor);
    assert_eq!(
        (placed.anchor, placed.gravity),
        (PopupEdge::Bottom, PopupEdge::Bottom)
    );
    assert_eq!(placed.offset, Point::new(0.0, 8.0));
    assert!(placed.grab);
    enter(&mut h);
    let first = h.rect("First");
    let second = h.rect("Second");
    assert_eq!(first.origin, Point::new(12.0, 12.0), "12 pt padding");
    assert_eq!(placed.size.height, second.max_y() + 12.0);
    assert_eq!(placed.size.width, first.width().max(second.width()) + 24.0);
}

#[test]
fn the_keyboard_lands_on_the_first_item_stays_inside_and_escape_returns_to_the_anchor() {
    let mut h = harness(false);
    h.tap(TAB);
    assert!(h.focused("Open"));
    h.tap(SPACE);
    let main = enter(&mut h);
    assert!(h.focused("First"));
    h.tap(TAB);
    assert!(h.focused("Second"));
    h.tap(TAB);
    assert!(h.focused("First"), "Tab wraps inside the popover");
    h.key(ESCAPE, true);
    assert_eq!(h.model().closes, 1);
    assert_eq!(popover(&h), None);
    h.window = main;
    assert!(h.focused("Open"));
}

#[test]
fn a_press_outside_dismisses_it() {
    let mut h = harness(false);
    h.click(h.center("Open"));
    enter(&mut h);
    h.send(WindowEvent::PopupDone);
    assert_eq!(h.model().closes, 1);
    assert_eq!(popover(&h), None);
}

#[test]
fn it_is_a_dialog_named_by_its_label() {
    let mut h = harness(false);
    h.click(h.center("Open"));
    enter(&mut h);
    let nodes = h.snapshot().accessibility;
    let dialog = nodes
        .iter()
        .find(|(_, n)| n.role == Role::Dialog)
        .expect("a dialog");
    assert_eq!(dialog.1.name.as_deref(), Some("Options"));
    assert!(!dialog.1.states.focusable);
}

#[test]
fn one_declared_before_its_parent_is_laid_out_opens_once_the_anchor_has_a_place() {
    let h = harness(true);
    let id = popover(&h).expect("open after the first layout");
    let placed = h.rt.window_system().placement(id).unwrap().unwrap();
    assert_eq!(placed.anchor_rect, h.rect("Open"));
}

#[test]
fn it_follows_its_anchor_when_the_anchor_moves() {
    let mut h = harness(false);
    h.click(h.center("Open"));
    let main = enter(&mut h);
    let id = h.window;
    h.tap(TAB);
    h.tap(SPACE);
    h.window = main;
    let moved = h.rect("Open");
    assert_eq!(moved.min_y(), 40.0);
    let requests = h.rt.window_system().requests(id).unwrap();
    assert!(
        matches!(requests.last(), Some(WindowRequest::Reposition(p)) if p.anchor_rect == moved),
        "{requests:?}"
    );
}
