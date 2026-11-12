use crate::support;

use cupertino_geometry::{Insets, Point};
use cupertino_ui_controls::{
    Button, ButtonRole, ButtonStyle, Stack, TextChange, TextField, Tooltip, TooltipChange,
    TooltipState,
};
use cupertino_ui_core::{
    Application, Command, CommandRegistry, Element, Role, Shortcut, Window, WindowKey,
};
use cupertino_ui_layout::{Align, Axis};
use cupertino_ui_platform::{
    Keysym, Mods, PointerEvent, PopupEdge, Timestamp, WindowEvent, WindowId, WindowStates,
};
use cupertino_ui_render::PaintOp;
use cupertino_ui_text::{
    AttributedText, ContentPurpose, ImeBatch, ImeEvent, ParagraphStyle, Preedit, TextStyle,
};
use std::cell::Cell;
use support::{text, window, Harness, TAB};

const ESCAPE: u32 = 0xff1b;
const MS: u64 = 1_000_000;

thread_local! {
    static TIP: Cell<&'static str> = const { Cell::new("Move to the bin") };
}

#[derive(Clone, Debug, PartialEq)]
enum Message {
    Tip(TooltipChange),
    /// Logged when the state starts or stops showing.
    Show,
    Hide,
    /// The window's Escape shortcut, which hears Escape only once the focus path let it through.
    Escaped,
    Trash,
    Other,
}

#[derive(Default)]
struct Model {
    tip: TooltipState,
    shown: bool,
    said: Vec<Message>,
}

/// A trash button with the tip "Move to the bin", unless a test sets another, and another button beside it.
struct Page;

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

fn tip(state: TooltipState) -> Tooltip<Message> {
    Tooltip {
        text: TIP.get().into(),
        anchor: button("Trash", Message::Trash),
        state,
        on_change: Box::new(Message::Tip),
    }
}

impl Application for Page {
    type Model = Model;
    type Message = Message;

    fn init() -> (Model, Vec<Command<Message>>) {
        (Model::default(), Vec::new())
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        match message {
            Message::Tip(change) => {
                model.tip.apply(change);
                if model.tip.shown() != model.shown {
                    model.shown = model.tip.shown();
                    model.said.push(if model.shown {
                        Message::Show
                    } else {
                        Message::Hide
                    });
                }
            }
            message => model.said.push(message),
        }
        Vec::new()
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        let mut windows = window(Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::uniform(10.0),
            spacing: 8.0,
            align: Align::Start,
            children: vec![
                Element::from(tip(model.tip)).key("tip"),
                button("Other", Message::Other),
            ],
        }));
        if model.tip.shown() {
            windows.push(tip(model.tip).window(
                WindowKey("tooltip".into()),
                WindowKey("test".into()),
                "tip".into(),
            ));
        }
        windows
    }
}

fn tooltip(h: &Harness<Page>) -> Option<WindowId> {
    h.rt.windows()
        .into_iter()
        .find(|(key, _)| *key == WindowKey("tooltip".into()))
        .map(|(_, id)| id)
}

fn at<A: Application>(h: &mut Harness<A>, ms: u64) {
    h.clock().advance_to(Timestamp(ms * MS)).unwrap();
    h.settle();
}

fn hover<A: Application>(h: &mut Harness<A>, key: &'static str) {
    let position = h.center(key);
    h.pointer(PointerEvent::Motion { position });
}

/// Moves the pointer off the window and onto the middle of the tip.
fn onto_tip(h: &mut Harness<Page>) {
    h.pointer(PointerEvent::Leave);
    let main = h.window;
    h.window = tooltip(h).expect("the tip shows");
    h.pointer(PointerEvent::Motion {
        position: Point::new(20.0, 12.0),
    });
    h.window = main;
}

fn off_tip(h: &mut Harness<Page>) {
    let main = h.window;
    h.window = tooltip(h).expect("the tip shows");
    h.pointer(PointerEvent::Leave);
    h.window = main;
}

fn rested() -> Harness<Page> {
    let mut h = Harness::<Page>::new();
    hover(&mut h, "Trash");
    at(&mut h, 600);
    assert!(h.model().shown);
    h
}

#[test]
fn the_tip_shows_once_the_pointer_has_rested_on_the_anchor_for_the_delay() {
    let mut h = Harness::<Page>::new();
    hover(&mut h, "Trash");
    at(&mut h, 599);
    assert!(!h.model().shown);
    assert!(tooltip(&h).is_none());
    at(&mut h, 600);
    assert_eq!(h.model().said, [Message::Show]);
    let id = tooltip(&h).expect("the tip opens");
    let system = h.rt.window_system();
    assert_eq!(system.parent(id).unwrap(), Some(h.window));
    let placed = system.placement(id).unwrap().expect("a popup").clone();
    assert_eq!(placed.anchor_rect, h.rect("tip"));
    assert_eq!(
        (placed.anchor, placed.gravity),
        (PopupEdge::Bottom, PopupEdge::Bottom)
    );
    assert_eq!(placed.offset, Point::new(0.0, 4.0));
    assert!(!placed.grab, "a tip never takes the keyboard");
}

#[test]
fn leaving_before_the_delay_asks_for_nothing() {
    let mut h = Harness::<Page>::new();
    hover(&mut h, "Trash");
    at(&mut h, 300);
    hover(&mut h, "Other");
    at(&mut h, 900);
    assert!(h.model().said.is_empty());
}

#[test]
fn leaving_the_anchor_hides_the_tip_after_the_grace() {
    let mut h = rested();
    hover(&mut h, "Other");
    at(&mut h, 899);
    assert!(h.model().shown);
    at(&mut h, 900);
    assert!(!h.model().shown);
    assert!(tooltip(&h).is_none());
}

#[test]
fn the_pointer_can_move_onto_the_tip_and_back_without_it_closing() {
    let mut h = rested();
    onto_tip(&mut h);
    at(&mut h, 2000);
    assert!(h.model().shown, "the pointer is on the tip");
    off_tip(&mut h);
    hover(&mut h, "Trash");
    at(&mut h, 4000);
    assert_eq!(h.model().said, [Message::Show]);
}

#[test]
fn leaving_the_tip_hides_it_after_the_grace() {
    let mut h = rested();
    onto_tip(&mut h);
    at(&mut h, 700);
    off_tip(&mut h);
    at(&mut h, 999);
    assert!(h.model().shown);
    at(&mut h, 1000);
    assert_eq!(h.model().said, [Message::Show, Message::Hide]);
    assert!(tooltip(&h).is_none());
}

#[test]
fn a_press_hides_the_tip_and_still_reaches_the_anchor() {
    let mut h = rested();
    h.click(h.center("Trash"));
    assert_eq!(
        h.model().said,
        [Message::Show, Message::Hide, Message::Trash]
    );
    assert!(tooltip(&h).is_none());
}

#[test]
fn escape_hides_the_tip_and_goes_no_further() {
    let mut h = Harness::<Page>::new();
    h.tap(TAB);
    assert!(h.focused("Trash"));
    hover(&mut h, "Trash");
    at(&mut h, 600);
    h.tap(ESCAPE);
    assert_eq!(h.model().said, [Message::Show, Message::Hide]);
    h.tap(ESCAPE);
    assert_eq!(
        h.model().said.len(),
        2,
        "with the tip gone Escape is not its"
    );
}

#[test]
fn the_tip_never_takes_the_focus() {
    let mut h = Harness::<Page>::new();
    h.tap(TAB);
    h.tap(TAB);
    assert!(h.focused("Other"));
    hover(&mut h, "Trash");
    at(&mut h, 600);
    assert!(tooltip(&h).is_some());
    assert!(h.focused("Other"));
    h.tap(TAB);
    assert!(h.focused("Trash"), "Tab goes on as before");
}

#[test]
fn the_text_describes_the_anchor_and_the_tip_is_a_tooltip() {
    let h = Harness::<Page>::new();
    let trash = h.accessible("Trash");
    assert_eq!(trash.role, Role::Button);
    assert_eq!(trash.description.as_deref(), Some("Move to the bin"));
    assert_eq!(h.accessible("Other").description, None);

    let mut h = rested();
    let id = tooltip(&h).unwrap();
    h.window = id;
    h.settle();
    let tips: Vec<_> = h
        .snapshot()
        .accessibility
        .into_iter()
        .map(|(_, n)| n)
        .filter(|n| n.role == Role::Tooltip)
        .collect();
    assert_eq!(tips.len(), 1);
    assert_eq!(tips[0].name.as_deref(), Some("Move to the bin"));
    assert!(!tips[0].states.focusable);
}

#[test]
fn keyboard_focus_shows_the_tip_after_the_delay_and_moving_on_hides_it() {
    let mut h = Harness::<Page>::new();
    h.tap(TAB);
    at(&mut h, 599);
    assert!(tooltip(&h).is_none());
    at(&mut h, 600);
    assert_eq!(h.model().said, [Message::Show]);
    assert!(tooltip(&h).is_some());
    h.tap(TAB);
    assert!(h.focused("Other"));
    assert_eq!(h.model().said, [Message::Show, Message::Hide]);
    assert!(tooltip(&h).is_none());
}

#[test]
fn a_tip_shown_for_the_keyboard_focus_stays_when_the_pointer_passes() {
    let mut h = Harness::<Page>::new();
    h.tap(TAB);
    hover(&mut h, "Trash");
    at(&mut h, 600);
    hover(&mut h, "Other");
    at(&mut h, 2000);
    assert!(h.model().shown, "the focus is still on the anchor");
    h.tap(ESCAPE);
    assert!(!h.model().shown);
}

#[test]
fn a_tip_shown_for_the_keyboard_focus_hides_when_the_window_deactivates() {
    let mut h = Harness::<Page>::new();
    h.tap(TAB);
    at(&mut h, 600);
    h.send(WindowEvent::Focus(false));
    assert_eq!(h.model().said, [Message::Show, Message::Hide]);
    assert!(tooltip(&h).is_none());
}

#[test]
fn deactivating_the_window_or_escape_during_the_delay_cancels_the_tip() {
    let mut h = Harness::<Page>::new();
    h.tap(TAB);
    at(&mut h, 300);
    h.send(WindowEvent::Focus(false));
    at(&mut h, 900);
    assert!(h.model().said.is_empty());

    // the window's colours cross back to active over the frames that follow
    h.send(WindowEvent::Focus(true));
    at(&mut h, 1150);
    h.tap(ESCAPE);
    at(&mut h, 1750);
    assert!(h.model().said.is_empty(), "{:?}", h.model().said);
}

/// A field with a tip, beside a button: a click focuses the field.
struct FieldPage;

impl Application for FieldPage {
    type Model = Model;
    type Message = Message;

    fn init() -> (Model, Vec<Command<Message>>) {
        (Model::default(), Vec::new())
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        Page::update(model, message)
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        window(Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::uniform(10.0),
            spacing: 8.0,
            align: Align::Start,
            children: vec![
                Element::from(Tooltip {
                    text: "Your full name".into(),
                    anchor: Element::new(TextField {
                        label: "Name".into(),
                        placeholder: String::new(),
                        value: String::new(),
                        revision: 0,
                        purpose: ContentPurpose::Normal,
                        enabled: true,
                        on_change: None,
                        on_submit: None,
                        selection: None,
                        initial: false,
                    })
                    .key("Name"),
                    state: model.tip,
                    on_change: Box::new(Message::Tip),
                }),
                button("Other", Message::Other),
            ],
        }))
    }
}

#[test]
fn a_click_that_focuses_the_anchor_shows_no_tip() {
    let mut h = Harness::<FieldPage>::new();
    h.click(h.center("Name"));
    assert!(h.focused("Name"));
    hover(&mut h, "Other");
    at(&mut h, 900);
    assert!(h.model().said.is_empty());
}

/// A field with the focus and, beside it, the trash button with its tip; the window has an Escape shortcut.
struct Form;

impl Application for Form {
    type Model = Model;
    type Message = Message;

    fn init() -> (Model, Vec<Command<Message>>) {
        (Model::default(), Vec::new())
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        Page::update(model, message)
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        let mut windows = window(Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::uniform(10.0),
            spacing: 8.0,
            align: Align::Start,
            children: vec![
                Element::new(TextField {
                    label: "Name".into(),
                    placeholder: String::new(),
                    value: String::new(),
                    revision: 0,
                    purpose: ContentPurpose::Normal,
                    enabled: true,
                    on_change: Some(Box::new(|_: TextChange| Message::Other)),
                    on_submit: None,
                    selection: None,
                    initial: false,
                })
                .key("Name"),
                Element::from(tip(model.tip)).key("tip"),
            ],
        }));
        windows[0].shortcuts = CommandRegistry::new().with(
            Shortcut {
                mods: Mods::default(),
                keysym: Keysym(ESCAPE),
            },
            Message::Escaped,
        );
        windows
    }
}

fn escaped(h: &Harness<Form>) -> usize {
    h.model()
        .said
        .iter()
        .filter(|m| **m == Message::Escaped)
        .count()
}

/// The focus in the field and the trash button's tip shown by the pointer.
fn hovered_while_typing() -> Harness<Form> {
    let mut h = Harness::<Form>::new();
    h.click(h.center("Name"));
    assert!(h.focused("Name"));
    hover(&mut h, "Trash");
    at(&mut h, 600);
    assert!(h.model().tip.shown());
    h
}

#[test]
fn escape_dismisses_a_tip_shown_by_the_pointer_while_the_focus_is_elsewhere() {
    let mut h = hovered_while_typing();
    h.tap(ESCAPE);
    assert!(!h.model().tip.shown());
    assert_eq!(escaped(&h), 0, "the focus path never heard it");
    h.tap(ESCAPE);
    assert_eq!(escaped(&h), 1, "with the tip gone Escape goes on as before");
}

#[test]
fn escape_first_cancels_what_the_field_is_composing() {
    let mut h = hovered_while_typing();
    h.send(WindowEvent::Ime(ImeEvent::Done(ImeBatch {
        preedit: Some(Preedit {
            text: "\u{b4}".into(),
            cursor: None,
        }),
        commit: None,
        delete: (0, 0),
        current: true,
    })));
    h.tap(ESCAPE);
    assert!(h.model().tip.shown(), "one Escape cancels the accent");
    assert_eq!(escaped(&h), 0);
    h.tap(ESCAPE);
    assert!(!h.model().tip.shown());
}

#[test]
fn an_overlong_tip_ends_in_an_ellipsis_within_the_text_width() {
    TIP.set("Move the selected items to the bin, where they stay until it is emptied or they are put back");
    let mut h = rested();
    let id = tooltip(&h).unwrap();
    h.rt.window_system_mut()
        .configure(id, None, WindowStates::default())
        .unwrap();
    h.window = id;
    h.settle();
    let runs: Vec<_> = h
        .snapshot()
        .display_list
        .nodes
        .into_iter()
        .flat_map(|node| node.ops)
        .filter_map(|op| match op {
            PaintOp::Glyphs { origin, run, .. } => Some((origin, run)),
            _ => None,
        })
        .collect();
    let mark = text()
        .paragraph(
            &AttributedText::plain("\u{2026}", TextStyle::new(11.0)),
            &ParagraphStyle::default(),
        )
        .unwrap()
        .layout(None);
    let (origin, ellipsis) = runs.last().unwrap();
    assert_eq!(runs.len(), 2, "the text, then the ellipsis");
    assert_eq!(ellipsis.glyphs.len(), 1);
    assert_eq!(
        ellipsis.glyphs[0].id,
        mark.lines()[0].runs[0].run.glyphs[0].id
    );
    let padding = 10.0;
    assert!(
        origin.x + mark.size().width <= padding + 300.0 + 1e-6,
        "{origin:?}"
    );
    assert!(origin.x + mark.size().width > padding + 280.0, "{origin:?}");
}

#[test]
fn escape_during_the_delay_cancels_the_tip_wherever_the_focus_is() {
    let mut h = Harness::<Form>::new();
    h.click(h.center("Name"));
    hover(&mut h, "Trash");
    at(&mut h, 300);
    h.tap(ESCAPE);
    assert_eq!(escaped(&h), 1, "nothing showed, so the key goes on");
    at(&mut h, 900);
    assert!(!h.model().tip.shown());
}
