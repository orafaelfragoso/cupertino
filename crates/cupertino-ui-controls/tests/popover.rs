use crate::support;

use cupertino_geometry::{Insets, Point, Size};
use cupertino_ui_controls::{
    Button, ButtonRole, ButtonStyle, Popover, Spacer, Stack, Text, TextRole,
};
use cupertino_ui_core::{Application, Command, Element, Role, Window, WindowKey};
use cupertino_ui_layout::{Align, Axis, ScrollState};
use cupertino_ui_platform::{
    PointerEvent, PopupEdge, ScrollSource, WindowEvent, WindowId, WindowRequest, WindowStates,
};
use cupertino_ui_render::{Paint, PaintOp, Shape};
use cupertino_ui_text::{AttributedText, TextStyle};
use std::cell::Cell;
use support::{window, Harness, SPACE, TAB};

const ESCAPE: u32 = 0xff1b;

thread_local! {
    static OPEN_AT_START: Cell<bool> = const { Cell::new(false) };
    static POPOVER_FIRST: Cell<bool> = const { Cell::new(false) };
    /// A 1000 pt space below the buttons makes the content taller than the popover's bound.
    static TALL: Cell<bool> = const { Cell::new(false) };
    /// A line of text instead of the buttons, with the space below it when `TALL`.
    static TEXT_ONLY: Cell<bool> = const { Cell::new(false) };
}

#[derive(Clone, Debug, PartialEq)]
enum Message {
    Open,
    Close,
    Shift,
    Choose,
    Scrolled(ScrollState),
}

struct Model {
    open: bool,
    shifted: bool,
    closes: usize,
    scroll: ScrollState,
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
            shifted: false,
            closes: 0,
            scroll: ScrollState {
                offset: Point::ZERO,
                content: Size::ZERO,
                viewport: Size::ZERO,
            },
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
            Message::Scrolled(state) => model.scroll = state,
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
            windows.insert(
                if POPOVER_FIRST.get() {
                    0
                } else {
                    windows.len()
                },
                Popover {
                    parent: WindowKey("test".into()),
                    anchor: "Open".into(),
                    label: "Options".into(),
                    content: Element::new(Stack {
                        axis: Axis::Vertical,
                        padding: Insets::uniform(0.0),
                        spacing: 6.0,
                        align: Align::Start,
                        children: if TEXT_ONLY.get() {
                            vec![Element::new(Text {
                                content: AttributedText::plain(
                                    "Shared with 3 people",
                                    TextStyle::new(13.0),
                                ),
                                role: TextRole::Body,
                                wrap: false,
                                secondary: false,
                                selected: false,
                            })]
                        } else {
                            vec![
                                button("First", Message::Choose),
                                button("Second", Message::Shift),
                            ]
                        }
                        .into_iter()
                        .chain(TALL.get().then(|| Element::new(Spacer { min: 1000.0 })))
                        .collect(),
                    })
                    .key("content"),
                    scroll: model.scroll,
                    on_scroll: Box::new(Message::Scrolled),
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
    assert_eq!(
        placed.offset,
        Point::new(0.0, 6.0),
        "the pointer's tip 6 pt below"
    );
    assert!(placed.grab);
    enter(&mut h);
    let first = h.rect("First");
    let second = h.rect("Second");
    // the pointer stands 10.5 pt over the panel, whose padding is 2 pt at the top and 5 pt elsewhere
    assert_eq!(first.origin, Point::new(5.0, 12.5));
    assert_eq!(placed.size.height, second.max_y() + 5.0);
    assert_eq!(placed.size.width, first.width().max(second.width()) + 10.0);
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

#[test]
fn one_declared_before_its_parent_opens_and_stays_open() {
    POPOVER_FIRST.set(true);
    let mut h = harness(false);
    h.click(h.center("Open"));
    let id = popover(&h).expect("open");
    enter(&mut h);
    h.tap(TAB);
    assert_eq!(popover(&h), Some(id), "a later view keeps it");
    assert!(h.focused("Second"));
}

#[test]
fn content_taller_than_its_bound_scrolls_inside_the_padding() {
    TALL.set(true);
    let mut h = harness(false);
    h.click(h.center("Open"));
    let id = popover(&h).expect("open");
    let placed = h.rt.window_system().placement(id).unwrap().unwrap().clone();
    assert_eq!(placed.size.height, 600.0, "the bound");
    enter(&mut h);
    let content = h.rect("content");
    let state = h.model().scroll;
    // the harness configures the popup at the window's 400 pt width; the pointer row takes 10.5 pt of its height
    assert_eq!(state.viewport, Size::new(390.0, 582.5));
    assert_eq!(state.content.height, content.height());
    assert!(content.height() > 1000.0);
    assert_eq!(h.rect("First").origin, Point::new(5.0, 12.5));
    h.pointer(PointerEvent::Scroll {
        position: Point::new(30.0, 300.0),
        delta: Point::new(0.0, 30.0),
        value120: None,
        source: ScrollSource::Wheel,
        stop: false,
        inverted: (false, false),
    });
    assert_eq!(h.model().scroll.offset.y, 30.0);
    let thumb = h
        .snapshot()
        .display_list
        .nodes
        .iter()
        .flat_map(|n| n.ops.iter())
        .find_map(|op| match op {
            PaintOp::Fill {
                shape: Shape::RoundedRect { rect, radius },
                paint: Paint::Solid(_),
            } if *radius == 3.0 && rect.size.width == 6.0 => Some(*rect),
            _ => None,
        })
        .expect("the scroller shows while it moves");
    assert_eq!(
        (thumb.origin.x, thumb.size.width),
        (381.0, 6.0),
        "along the trailing edge, 3 pt in"
    );
    assert_eq!(
        h.rect("First").origin,
        Point::new(5.0, -17.5),
        "moved by the offset"
    );
    h.pointer(PointerEvent::Scroll {
        position: Point::new(30.0, 300.0),
        delta: Point::new(0.0, 10_000.0),
        value120: None,
        source: ScrollSource::Wheel,
        stop: false,
        inverted: (false, false),
    });
    assert_eq!(
        h.model().scroll.offset.y,
        content.height() - 582.5,
        "it stops at the end"
    );
}

fn focused_role(h: &Harness<Options>) -> Option<Role> {
    let snapshot = h.snapshot();
    let focus = snapshot.focus?;
    snapshot
        .accessibility
        .iter()
        .find(|(id, _)| *id == focus)
        .map(|(_, n)| n.role)
}

fn scroll_view_is_a_stop(h: &Harness<Options>) -> bool {
    h.snapshot()
        .accessibility
        .iter()
        .any(|(_, n)| n.role == Role::ScrollView && n.states.focusable)
}

#[test]
fn with_no_control_and_content_that_fits_the_keyboard_rests_on_the_dialog_not_its_scroll_view() {
    TEXT_ONLY.set(true);
    let mut h = harness(false);
    h.tap(TAB);
    h.tap(SPACE);
    enter(&mut h);
    assert!(!scroll_view_is_a_stop(&h));
    assert_eq!(focused_role(&h), Some(Role::Dialog));
    h.tap(TAB);
    assert_eq!(
        focused_role(&h),
        Some(Role::Dialog),
        "Tab stays on the dialog"
    );
}

#[test]
fn with_no_control_and_content_past_its_bound_the_scroll_view_takes_the_keyboard() {
    TEXT_ONLY.set(true);
    TALL.set(true);
    let mut h = harness(false);
    h.tap(TAB);
    h.tap(SPACE);
    enter(&mut h);
    assert!(scroll_view_is_a_stop(&h));
    assert_eq!(focused_role(&h), Some(Role::ScrollView));
}

#[test]
fn without_the_glass_it_is_the_opaque_fill_in_a_16_point_corner_with_its_own_edge() {
    let mut h = harness(false);
    h.click(h.center("Open"));
    enter(&mut h);
    let ops: Vec<PaintOp> = h
        .snapshot()
        .display_list
        .nodes
        .into_iter()
        .flat_map(|n| n.ops)
        .collect();
    assert!(
        ops.iter().any(|op| matches!(
            op,
            PaintOp::Fill { shape: Shape::RoundedRect { radius, .. }, .. } if *radius == 16.0
        )),
        "the fill: {ops:?}"
    );
    assert!(
        ops.iter().any(|op| matches!(
            op,
            PaintOp::Stroke { shape: Shape::RoundedRect { radius, .. }, width, .. }
                if *radius == 15.5 && *width == 1.0
        )),
        "the edge: {ops:?}"
    );
}
