//! The pull-down button, the colour well, the date picker and the combo box: each opens or steps as its family does
//! and tells assistive technology what it holds.

use crate::support;

use cupertino_geometry::Insets;
use cupertino_ui_controls::{
    CalendarDate, ColorWell, ComboBox, DatePicker, MenuAnchor, MenuItem, PullDownButton, Stack,
    TextChange, TextField,
};
use cupertino_ui_core::{AccessibleValue, Application, Command, Element, Window, WindowKey};
use cupertino_ui_layout::{Align, Axis};
use cupertino_ui_render::{PaintOp, Shape};
use cupertino_ui_text::ContentPurpose;
use cupertino_ui_theme::Color;
use support::{window, Harness, RETURN, TAB};

const UP: u32 = 0xff52;
const DOWN: u32 = 0xff54;
const LEFT: u32 = 0xff51;

#[derive(Clone, Debug, PartialEq)]
enum Message {
    Actions,
    Well,
    Date(CalendarDate),
    Combo,
    Typed(TextChange),
}

#[derive(Default)]
struct Model {
    opened: Vec<Message>,
    date: Option<CalendarDate>,
    typed: String,
}

struct Pickers;

fn combo(model: &Model) -> ComboBox<Message> {
    ComboBox {
        field: TextField {
            label: "Name".into(),
            placeholder: String::new(),
            value: model.typed.clone(),
            revision: 0,
            purpose: ContentPurpose::Normal,
            enabled: true,
            on_change: Some(Box::new(Message::Typed)),
            on_submit: None,
            selection: None,
            initial: false,
        },
        options: vec!["Alpha".into(), "Beta".into()],
        open: false,
        on_open: Some(Message::Combo),
        text_size: 13.0,
    }
}

impl Application for Pickers {
    type Model = Model;
    type Message = Message;

    fn init() -> (Model, Vec<Command<Message>>) {
        (Model::default(), Vec::new())
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        match message {
            Message::Date(date) => model.date = Some(date),
            Message::Typed(change) => model.typed = change.text,
            other => model.opened.push(other),
        }
        Vec::new()
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        window(Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::uniform(8.0),
            spacing: 6.0,
            align: Align::Start,
            children: vec![
                Element::new(PullDownButton {
                    label: "Actions".into(),
                    actions: vec!["Duplicate".into(), "Rename".into()],
                    open: false,
                    enabled: true,
                    on_open: Some(Message::Actions),
                })
                .key("actions"),
                Element::new(ColorWell {
                    label: "Colour".into(),
                    color: Color::from_rgba8([52, 120, 246, 255]),
                    enabled: true,
                    on_open: Some(Message::Well),
                })
                .key("well"),
                Element::new(DatePicker {
                    label: "Date".into(),
                    date: model.date.unwrap_or(CalendarDate {
                        year: 2026,
                        month: 7,
                        day: 11,
                    }),
                    enabled: true,
                    on_change: Some(Box::new(Message::Date)),
                })
                .key("date"),
                Element::from(combo(model)).width(160.0).key("combo"),
            ],
        }))
    }
}

fn harness() -> Harness<Pickers> {
    let mut h = Harness::<Pickers>::new();
    h.resize(cupertino_geometry::Size::new(260.0, 200.0));
    h.settle();
    h
}

#[test]
fn a_pull_down_opens_on_its_press_and_its_menu_hangs_below_with_nothing_marked() {
    let mut h = harness();
    h.click(h.center("actions"));
    assert_eq!(h.model().opened, [Message::Actions]);
    let button = PullDownButton {
        label: "Actions".into(),
        actions: vec!["Duplicate".into(), "Rename".into()],
        open: true,
        enabled: true,
        on_open: Some(()),
    };
    let menu = button.menu(
        WindowKey("w".into()),
        "actions".into(),
        Box::new(|_| ()),
        (),
    );
    assert!(matches!(menu.anchor, MenuAnchor::Below(_)));
    assert_eq!(menu.current, None);
    assert!(menu
        .items
        .iter()
        .all(|item| matches!(item, MenuItem::Item { checked: false, .. })));
    // one chevron, pointing down, where a pop-up button draws two
    let chevrons = h
        .ops("actions")
        .iter()
        .filter(|op| {
            matches!(
                op,
                PaintOp::Stroke {
                    shape: Shape::Path { .. },
                    ..
                }
            )
        })
        .count();
    assert_eq!(chevrons, 1);
}

#[test]
fn a_colour_well_opens_its_picker_on_a_press_or_return_and_reads_as_its_colour() {
    let mut h = harness();
    h.click(h.center("well"));
    assert_eq!(h.model().opened, [Message::Well]);
    let node = h.accessible("well");
    assert!(matches!(&node.value, Some(AccessibleValue::Text { value, .. }) if value == "#3478F6"));
    h.tap(TAB);
    h.tap(TAB);
    assert!(h.focused("well"));
    h.tap(RETURN);
    assert_eq!(h.model().opened, [Message::Well, Message::Well]);
}

#[test]
fn a_date_picker_steps_the_part_in_use_by_the_arrows_and_the_stepper() {
    let mut h = harness();
    // a click on the date takes the focus with the day in use
    let date = h.rect("date");
    h.click(cupertino_geometry::Point::new(
        date.min_x() + 70.0,
        date.min_y() + date.height() / 2.0,
    ));
    assert!(h.focused("date"));
    h.tap(UP);
    assert_eq!(
        h.model().date,
        Some(CalendarDate {
            year: 2026,
            month: 7,
            day: 12
        })
    );
    h.tap(LEFT);
    h.tap(DOWN);
    assert_eq!(
        h.model().date,
        Some(CalendarDate {
            year: 2026,
            month: 6,
            day: 12
        }),
        "Left moves to the month"
    );
    // the stepper's lower half steps the month down again
    let lower = cupertino_geometry::Point::new(date.max_x() - 8.0, date.max_y() - 6.0);
    h.click(lower);
    assert_eq!(
        h.model().date,
        Some(CalendarDate {
            year: 2026,
            month: 5,
            day: 12
        })
    );
    let node = h.accessible("date");
    assert!(
        matches!(&node.value, Some(AccessibleValue::Text { value, .. }) if value == "2026-05-12")
    );
}

#[test]
fn a_combo_box_takes_typed_text_and_its_button_opens_the_choices() {
    let mut h = harness();
    let rect = h.rect("combo");
    h.click(cupertino_geometry::Point::new(
        rect.min_x() + 20.0,
        rect.min_y() + rect.height() / 2.0,
    ));
    h.type_text("Gamma");
    assert_eq!(h.model().typed, "Gamma", "any text, not only a choice");
    h.click(cupertino_geometry::Point::new(
        rect.max_x() - 10.0,
        rect.min_y() + rect.height() / 2.0,
    ));
    assert_eq!(h.model().opened, [Message::Combo]);
    let model = Model {
        typed: "Beta".into(),
        ..Model::default()
    };
    let menu = combo(&model).menu(
        WindowKey("w".into()),
        "combo".into(),
        Box::new(|_| Message::Combo),
        Message::Combo,
    );
    assert_eq!(menu.current, Some(1), "the field's value is marked");
}
