//! Every control, in each state it has, in each appearance and direction, rendered by the CPU renderer and compared
//! with `tests/fixtures/controls`: `<fixture>.png` is a sheet of the cells (a row per appearance and direction, a
//! column per state) and `<fixture>.txt` holds each cell's focus and accessibility tree. A state a control does not
//! have is an empty cell and an `n/a` block.
//!
//! Each state is reached as a user reaches it: the pointer over the control's centre (hover), a button held down there
//! (pressed), Tab (focused), or the control's own value (disabled, selected). Pixels may differ by at most 2/255 per
//! channel, nowhere else; the pixels are compared on x86_64 only, as the renderer's own baselines are.
//! `CUPERTINO_UPDATE_GOLDENS=1` re-records every fixture it runs and then fails, so the variable can never pass a run.

mod support;

use cupertino_kit::controls::Tabs;
use cupertino_kit::controls::{
    Badge, Button, ButtonRole, ButtonStyle, CheckBox, Column, ColumnWidth, Dialog, DialogAction,
    DialogButton, DialogKind, DisclosureButton, DisclosureStyle, Icon, IconDirection, ListView,
    Modal, NavigationStack, PageDots, Progress, RadioGroup, ScrollView, Scroller, SecureField,
    SegmentedControl, Selection, Separator, Slider, SliderStyle, SortOrder, SplitLayout, SplitView,
    Stack, Stepper, TableView, Text, TextField, TextRole, Toggle,
};
use cupertino_kit::controls::{Sidebar, SidebarItem, SidebarSection};
use cupertino_kit::core::{
    Application, Checked, Command, CommandRegistry, Element, Key, Window, WindowBackground,
    WindowKey,
};
use cupertino_kit::geometry::{Insets, Point, Size};
use cupertino_kit::layout::{Align, Axis, LayoutDirection, ScrollState};
use cupertino_kit::platform::{
    DecorationMode, Mods, MouseButton, PointerEvent, Timestamp, WindowEvent, WindowSpec,
};
use cupertino_kit::text::{AttributedText, ContentPurpose, TextStyle};
use cupertino_kit::theme::Appearance;
use cupertino_ui_test::Inspector;
use std::cell::Cell;
use support::{repository_theme, Run, TAB};
use tiny_skia::Pixmap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Normal,
    Hover,
    Pressed,
    Disabled,
    Focused,
    Selected,
}

const STATES: [State; 6] = [
    State::Normal,
    State::Hover,
    State::Pressed,
    State::Disabled,
    State::Focused,
    State::Selected,
];

const ALL: &[State] = &STATES;
const INTERACTIVE: &[State] = &[
    State::Normal,
    State::Hover,
    State::Pressed,
    State::Disabled,
    State::Focused,
];
const FIELD: &[State] = &[State::Normal, State::Hover, State::Disabled, State::Focused];

/// What a fixture builds its control from.
#[derive(Clone, Copy)]
struct Look {
    enabled: bool,
    selected: bool,
    scroll: ScrollState,
}

struct Fixture {
    name: &'static str,
    size: Size,
    states: &'static [State],
    build: fn(&Look) -> Element<Message>,
}

#[derive(Clone, Debug)]
enum Message {
    Nothing,
    Scroll(ScrollState),
}

thread_local! {
    static CELL: Cell<Option<(&'static Fixture, State)>> = const { Cell::new(None) };
}

struct Cells;

impl Application for Cells {
    type Model = (&'static Fixture, Look);
    type Message = Message;

    fn init() -> (Self::Model, Vec<Command<Message>>) {
        let (fixture, state) = CELL.get().expect("a cell is being rendered");
        let look = Look {
            enabled: state != State::Disabled,
            selected: state == State::Selected,
            scroll: ScrollState {
                offset: Point::ZERO,
                content: Size::ZERO,
                viewport: Size::ZERO,
            },
        };
        ((fixture, look), Vec::new())
    }

    fn update((_, look): &mut Self::Model, message: Message) -> Vec<Command<Message>> {
        if let Message::Scroll(scroll) = message {
            look.scroll = scroll;
        }
        Vec::new()
    }

    fn view((fixture, look): &Self::Model) -> Vec<Window<Message>> {
        vec![Window {
            key: WindowKey("fixture".into()),
            spec: WindowSpec {
                title: fixture.name.into(),
                app_id: "test.fixtures".into(),
                size: fixture.size,
                min_size: None,
                max_size: None,
                decorations: DecorationMode::Preferred,
                foreign_parent: None,
            },
            shortcuts: CommandRegistry::new(),
            on_close: None,
            background: WindowBackground::Theme,
            popup: None,
            content: Element::new(Stack {
                axis: Axis::Vertical,
                padding: Insets::uniform(12.0),
                spacing: 0.0,
                align: Align::Start,
                children: vec![(fixture.build)(look).key("control")],
            }),
        }]
    }
}

/// One cell: its frame's premultiplied pixels and its focus and accessibility lines.
fn render(
    fixture: &'static Fixture,
    appearance: Appearance,
    direction: LayoutDirection,
    state: State,
) -> (Vec<u8>, String) {
    CELL.set(Some((fixture, state)));
    let mut run = Run::<Cells>::with(repository_theme(appearance), direction);
    // the first part of the control that acts (a group's first option), or the control itself
    let centre = {
        let runtime = run.driver.runtime();
        let id = runtime.find(run.window, &"control".into()).unwrap();
        let snapshot = runtime.snapshot(run.window).unwrap();
        let control = snapshot.nodes.iter().find(|n| n.id == id).unwrap().rect;
        let rect = snapshot
            .accessibility
            .iter()
            .map(|(_, n)| n)
            .find(|n| !n.actions.is_empty() && control.contains(n.bounds.center()))
            .map_or(control, |n| n.bounds);
        Point::new(
            rect.origin.x + rect.size.width / 2.0,
            rect.origin.y + rect.size.height / 2.0,
        )
    };
    let pointer = |event| WindowEvent::Pointer(event, Timestamp(0));
    match state {
        State::Hover => run.send(pointer(PointerEvent::Motion { position: centre })),
        State::Pressed => {
            run.send(pointer(PointerEvent::Motion { position: centre }));
            run.send(pointer(PointerEvent::Button {
                position: centre,
                button: MouseButton::Left,
                pressed: true,
                serial: 1,
                mods: Mods::NONE,
            }));
        }
        State::Focused => run.key(TAB, None),
        State::Normal | State::Disabled | State::Selected => {}
    }
    run.driver.advance_to(Timestamp(2_000_000_000)).unwrap();
    run.settle();
    let system = run.driver.window_system_mut();
    let frame = system.frames(run.window).unwrap().last().unwrap();
    assert_eq!(
        (frame.width, frame.height),
        (fixture.size.width as u32, fixture.size.height as u32),
        "{}: a cell is drawn at 1x",
        fixture.name
    );
    let pixels = frame.pixels.clone();
    let dump = Inspector::default().dump(&run.snapshot());
    let focus = dump.lines().find(|l| l.starts_with("focus ")).unwrap();
    let semantics = dump
        .lines()
        .skip_while(|l| *l != "semantics")
        .skip(1)
        .take_while(|l| *l != "display");
    let mut text = format!("{focus}\n");
    for line in semantics {
        text.push_str(line);
        text.push('\n');
    }
    (pixels, text)
}

const ROWS: [(Appearance, LayoutDirection); 8] = [
    (Appearance::Light, LayoutDirection::Ltr),
    (Appearance::Light, LayoutDirection::Rtl),
    (Appearance::Dark, LayoutDirection::Ltr),
    (Appearance::Dark, LayoutDirection::Rtl),
    (Appearance::LightHighContrast, LayoutDirection::Ltr),
    (Appearance::LightHighContrast, LayoutDirection::Rtl),
    (Appearance::DarkHighContrast, LayoutDirection::Ltr),
    (Appearance::DarkHighContrast, LayoutDirection::Rtl),
];

/// Every cell of `fixture`: the sheet and the semantics text.
fn sheet(fixture: &'static Fixture) -> (Pixmap, String) {
    let (w, h) = (fixture.size.width as u32, fixture.size.height as u32);
    let mut sheet = Pixmap::new(w * STATES.len() as u32, h * ROWS.len() as u32).unwrap();
    let stride = sheet.width() as usize * 4;
    let mut text = String::new();
    for (row, &(appearance, direction)) in ROWS.iter().enumerate() {
        for (column, &state) in STATES.iter().enumerate() {
            text.push_str(&format!("{appearance:?} {direction:?} {state:?}\n"));
            if !fixture.states.contains(&state) {
                text.push_str("  n/a\n");
                continue;
            }
            let (pixels, semantics) = render(fixture, appearance, direction, state);
            text.push_str(&semantics);
            let data = sheet.data_mut();
            for y in 0..h as usize {
                let at = (row * h as usize + y) * stride + column * w as usize * 4;
                data[at..at + w as usize * 4]
                    .copy_from_slice(&pixels[y * w as usize * 4..(y + 1) * w as usize * 4]);
            }
        }
    }
    (sheet, text)
}

fn check(fixture: &'static Fixture) {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/controls");
    let (png, txt) = (
        format!("{dir}/{}.png", fixture.name),
        format!("{dir}/{}.txt", fixture.name),
    );
    let (actual, text) = sheet(fixture);
    if std::env::var("CUPERTINO_UPDATE_GOLDENS").as_deref() == Ok("1") {
        std::fs::create_dir_all(dir).unwrap();
        actual.save_png(&png).unwrap();
        std::fs::write(&txt, &text).unwrap();
        panic!(
            "{}: re-recorded; review the diff and run again without CUPERTINO_UPDATE_GOLDENS",
            fixture.name
        );
    }
    let expected_text = std::fs::read_to_string(&txt).unwrap();
    if let Some(difference) = Inspector::first_difference(&expected_text, &text) {
        panic!("{}: the semantics differ: {difference}", fixture.name);
    }
    if cfg!(target_arch = "x86_64") {
        let expected = Pixmap::load_png(&png).unwrap();
        assert_eq!(
            (expected.width(), expected.height()),
            (actual.width(), actual.height()),
            "{}: the sheet's size",
            fixture.name
        );
        let (w, h) = (fixture.size.width as usize, fixture.size.height as usize);
        let off = expected
            .data()
            .as_chunks::<4>()
            .0
            .iter()
            .zip(actual.data().as_chunks::<4>().0)
            .enumerate()
            .find(|(_, (e, a))| e.iter().zip(a.iter()).any(|(x, y)| x.abs_diff(*y) > 2));
        if let Some((i, (e, a))) = off {
            let (x, y) = (i % actual.width() as usize, i / actual.width() as usize);
            let (appearance, direction) = ROWS[y / h];
            panic!(
                "{}: {appearance:?} {direction:?} {:?} differs at ({}, {}): {e:?} recorded, {a:?} now",
                fixture.name,
                STATES[x / w],
                x % w,
                y % h
            );
        }
    } else {
        eprintln!(
            "SKIPPED: {} pixels are measured on x86_64 only",
            fixture.name
        );
    }
}

fn label(text: &str) -> AttributedText {
    AttributedText::plain(text, TextStyle::new(13.0))
}

fn body(text: &str) -> Element<Message> {
    Element::new(Text {
        content: label(text),
        role: TextRole::Body,
        wrap: false,
        secondary: false,
        selected: false,
    })
}

fn button(style: ButtonStyle, role: ButtonRole, look: &Look) -> Element<Message> {
    Element::new(Button {
        label: if style == ButtonStyle::RoundIcon {
            "+"
        } else {
            "Continue"
        }
        .into(),
        role,
        style,
        enabled: look.enabled,
        busy: false,
        on_press: Some(Message::Nothing),
    })
}

const CONTROL: Size = Size::new(180.0, 48.0);
const WIDE: Size = Size::new(240.0, 56.0);
const TALL: Size = Size::new(200.0, 112.0);
const SIDEBAR: Size = Size::new(200.0, 220.0);
const SHEET: Size = Size::new(400.0, 220.0);

fn choice(label: &str, action: DialogAction) -> DialogButton<Message> {
    DialogButton {
        label: label.into(),
        action,
        on_press: Message::Nothing,
    }
}

macro_rules! fixtures {
    ($($test:ident: $name:literal, $size:expr, $states:expr, $build:expr;)*) => {
        $(
            #[test]
            fn $test() {
                static FIXTURE: Fixture = Fixture {
                    name: $name,
                    size: $size,
                    states: $states,
                    build: $build,
                };
                check(&FIXTURE);
            }
        )*
    };
}

fixtures! {
    text: "text", CONTROL, &[State::Normal, State::Selected], |look| Element::new(Text {
        content: label("Wi-Fi network"),
        role: TextRole::Body,
        wrap: false,
        secondary: false,
        selected: look.selected,
    });
    text_secondary: "text-secondary", CONTROL, &[State::Normal], |look| Element::new(Text {
        content: label("Connected"),
        role: TextRole::Caption,
        wrap: false,
        secondary: true,
        selected: look.selected,
    });
    icon: "icon", CONTROL, &[State::Normal], |_| Element::new(Icon {
        name: "chevron.forward".into(),
        size: 16.0,
        direction: IconDirection::Semantic,
        label: Some("Next".into()),
        tint: None,
    });
    button_bordered: "button-bordered", CONTROL, INTERACTIVE,
        |look| button(ButtonStyle::Bordered, ButtonRole::Normal, look);
    button_default: "button-default", CONTROL, INTERACTIVE,
        |look| button(ButtonStyle::Bordered, ButtonRole::Default, look);
    button_destructive: "button-destructive", CONTROL, INTERACTIVE,
        |look| button(ButtonStyle::Bordered, ButtonRole::Destructive, look);
    button_borderless: "button-borderless", CONTROL, INTERACTIVE,
        |look| button(ButtonStyle::Borderless, ButtonRole::Normal, look);
    button_pill: "button-pill", CONTROL, INTERACTIVE,
        |look| button(ButtonStyle::Pill, ButtonRole::Normal, look);
    button_round_icon: "button-round-icon", CONTROL, INTERACTIVE,
        |look| button(ButtonStyle::RoundIcon, ButtonRole::Normal, look);
    check_box: "check-box", CONTROL, ALL, |look| Element::new(CheckBox {
        label: "Remember me".into(),
        checked: if look.selected { Checked::True } else { Checked::False },
        enabled: look.enabled,
        on_toggle: Some(Box::new(|_| Message::Nothing)),
    });
    toggle: "toggle", CONTROL, ALL, |look| Element::new(Toggle {
        label: "Wi-Fi".into(),
        on: look.selected,
        enabled: look.enabled,
        on_toggle: Some(Box::new(|_| Message::Nothing)),
    });
    // the tip's popup content, drawn in a cell of its own
    tooltip: "tooltip", CONTROL, &[State::Normal], |_| cupertino_kit::controls::Tooltip {
        text: "Move to the bin".into(),
        anchor: body("Trash"),
        state: cupertino_kit::controls::TooltipState::default(),
        on_change: Box::new(|_| Message::Nothing),
    }
    .window(WindowKey("tip".into()), WindowKey("fixture".into()), "control".into())
    .content;
    slider: "slider", WIDE, INTERACTIVE, |look| Element::new(Slider {
        label: "Volume".into(),
        value: 0.4,
        min: 0.0,
        max: 1.0,
        step: None,
        style: SliderStyle::Regular,
        ticks: 0,
        enabled: look.enabled,
        on_change: Some(Box::new(|_| Message::Nothing)),
    });
    slider_ticks: "slider-ticks", WIDE, INTERACTIVE, |look| Element::new(Slider {
        label: "Speed".into(),
        value: 0.5,
        min: 0.0,
        max: 1.0,
        step: Some(0.25),
        style: SliderStyle::Regular,
        ticks: 5,
        enabled: look.enabled,
        on_change: Some(Box::new(|_| Message::Nothing)),
    });
    slider_level: "slider-level", WIDE, INTERACTIVE, |look| Element::new(Slider {
        label: "Brightness".into(),
        value: 0.4,
        min: 0.0,
        max: 1.0,
        step: None,
        style: SliderStyle::Level,
        ticks: 0,
        enabled: look.enabled,
        on_change: Some(Box::new(|_| Message::Nothing)),
    });
    stepper: "stepper", CONTROL, INTERACTIVE, |look| Element::new(Stepper {
        label: "Copies".into(),
        value: 2.0,
        min: 1.0,
        max: 9.0,
        step: 1.0,
        enabled: look.enabled,
        on_change: Some(Box::new(|_| Message::Nothing)),
    });
    disclosure: "disclosure", CONTROL, ALL, |look| Element::new(DisclosureButton {
        label: "Details".into(),
        expanded: look.selected,
        style: DisclosureStyle::Triangle,
        enabled: look.enabled,
        on_toggle: Some(Box::new(|_| Message::Nothing)),
    });
    disclosure_bordered: "disclosure-bordered", CONTROL, ALL, |look| Element::new(DisclosureButton {
        label: "Details".into(),
        expanded: look.selected,
        style: DisclosureStyle::Bordered,
        enabled: look.enabled,
        on_toggle: Some(Box::new(|_| Message::Nothing)),
    });
    badge: "badge", CONTROL, &[State::Normal], |_| Element::new(Badge {
        text: Some("3".into()),
        description: "3 unread".into(),
    });
    page_dots: "page-dots", CONTROL, INTERACTIVE, |look| Element::new(PageDots {
        label: "Page".into(),
        count: 4,
        current: 1,
        enabled: look.enabled,
        on_select: Some(Box::new(|_| Message::Nothing)),
    });
    progress: "progress", CONTROL, &[State::Normal], |_| Element::new(Progress {
        label: "Downloading".into(),
        value: Some(0.4),
    });
    radio_group: "radio-group", TALL, ALL, |look| Element::new(RadioGroup {
        label: "Size".into(),
        options: vec!["Small".into(), "Large".into()],
        selected: look.selected.then_some(0),
        axis: Axis::Vertical,
        spacing: 6.0,
        enabled: look.enabled,
        on_select: Some(Box::new(|_| Message::Nothing)),
    });
    segmented: "segmented", WIDE, ALL, |look| Element::new(SegmentedControl {
        label: "View".into(),
        segments: vec!["Day".into(), "Week".into(), "Month".into()],
        selected: look.selected.then_some(1),
        enabled: look.enabled,
        on_select: Some(Box::new(|_| Message::Nothing)),
    });
    tabs: "tabs", Size::new(280.0, 120.0), &[State::Normal, State::Hover, State::Pressed, State::Focused], |_| Tabs {
        label: "Settings".into(),
        tabs: vec!["General".into(), "Sharing".into(), "Advanced".into()],
        selected: 0,
        content: body("Start up in the last folder"),
        on_select: Some(Box::new(|_| Message::Nothing)),
    }.into();
    text_field: "text-field", WIDE, FIELD, |look| Element::new(TextField {
        label: "Name".into(),
        placeholder: "Your name".into(),
        value: "Ada".into(),
        revision: 0,
        purpose: ContentPurpose::Normal,
        enabled: look.enabled,
        on_change: Some(Box::new(|_| Message::Nothing)),
        on_submit: None,
    });
    text_field_long: "text-field-long", WIDE, FIELD, |look| Element::new(TextField {
        label: "Name".into(),
        placeholder: "Your name".into(),
        value: "Augusta Ada King, Countess of Lovelace, mathematician".into(),
        revision: 0,
        purpose: ContentPurpose::Normal,
        enabled: look.enabled,
        on_change: Some(Box::new(|_| Message::Nothing)),
        on_submit: None,
    });
    secure_field: "secure-field", WIDE, FIELD, |look| Element::new(SecureField {
        label: "Password".into(),
        placeholder: "Required".into(),
        enabled: look.enabled,
        on_submit: None,
    });
    scroll_view: "scroll-view", TALL, &[State::Normal, State::Hover, State::Focused], |look| ScrollView {
        content: Element::new(Text {
            content: label("One\nTwo\nThree\nFour\nFive\nSix\nSeven\nEight"),
            role: TextRole::Body,
            wrap: true,
            secondary: false,
            selected: false,
        }),
        state: look.scroll,
        horizontal: false,
        vertical: true,
        scroller: Scroller::Always,
        on_scroll: Box::new(Message::Scroll),
    }.into();
    list: "list", TALL, &[State::Normal, State::Hover, State::Focused, State::Selected], |look| {
        let mut selection = Selection::default();
        if look.selected {
            selection.select_only(1);
        }
        ListView {
            label: "Fruit".into(),
            count: 20,
            row_height: 24.0,
            scroll: look.scroll,
            selection,
            multiple: false,
            row: Box::new(|index, selected| Element::new(Text {
                content: label(&format!("Row {index}")),
                role: TextRole::Body,
                wrap: false,
                secondary: false,
                selected,
            })),
            key: None,
            type_select: None,
            on_scroll: Box::new(Message::Scroll),
            on_select: Some(Box::new(|_| Message::Nothing)),
            on_activate: None,
        }
        .into()
    };
    sidebar: "sidebar", SIDEBAR, &[State::Normal, State::Hover, State::Focused, State::Selected], |look| {
        let item = |label: &str, icon: &str| SidebarItem {
            key: Key::from(label.to_owned()),
            label: label.into(),
            icon: Some(icon.into()),
        };
        Sidebar {
            label: "Places".into(),
            sections: vec![
                SidebarSection {
                    title: None,
                    items: vec![item("Recents", "clock")],
                },
                SidebarSection {
                    title: Some("Favourites".into()),
                    items: vec![item("Desktop", "menubar.dock.rectangle"), item("Documents", "doc")],
                },
                SidebarSection {
                    title: Some("Locations".into()),
                    items: vec![item("Computer", "desktopcomputer")],
                },
            ],
            selected: look.selected.then_some(1),
            scroll: look.scroll,
            on_scroll: Box::new(Message::Scroll),
            on_select: Some(Box::new(|_| Message::Nothing)),
        }
        .into()
    };
    table: "table", TALL, &[State::Normal, State::Hover, State::Pressed, State::Focused, State::Selected], |look| {
        let mut selection = Selection::default();
        if look.selected {
            selection.select_only(1);
        }
        TableView {
            label: "Files".into(),
            columns: vec![
                Column { title: "Name".into(), width: ColumnWidth::Fill },
                Column { title: "Size".into(), width: ColumnWidth::Fixed(56.0) },
            ],
            sort: Some((0, SortOrder::Ascending)),
            on_sort: Some(Box::new(|_| Message::Nothing)),
            count: 20,
            row_height: 24.0,
            scroll: look.scroll,
            selection,
            multiple: false,
            cell: Box::new(|index, column, selected| Element::new(Text {
                content: label(&if column == 0 { format!("File {index}") } else { format!("{index} KB") }),
                role: TextRole::Body,
                wrap: false,
                secondary: column == 1,
                selected,
            })),
            type_select: None,
            on_scroll: Box::new(Message::Scroll),
            on_select: Some(Box::new(|_| Message::Nothing)),
            on_activate: None,
        }
        .into()
    };
    separator: "separator", CONTROL, &[State::Normal], |_| Element::new(Stack {
        axis: Axis::Horizontal,
        padding: Insets::ZERO,
        spacing: 8.0,
        align: Align::Start,
        children: vec![
            Element::new(Separator { axis: Axis::Vertical }),
            Element::new(Stack {
                axis: Axis::Vertical,
                padding: Insets::ZERO,
                spacing: 4.0,
                align: Align::Start,
                children: vec![
                    Element::new(Text {
                        content: label("Above"),
                        role: TextRole::Body,
                        wrap: false,
                        secondary: false,
                        selected: false,
                    }),
                    Element::new(Separator { axis: Axis::Horizontal }),
                ],
            }),
        ],
    });
    split: "split", TALL, &[State::Normal, State::Hover, State::Pressed, State::Focused], |_| SplitView {
        label: "Sidebar".into(),
        axis: Axis::Horizontal,
        leading: body("Mail"),
        trailing: body("Inbox"),
        layout: SplitLayout { position: 72.0, collapsed: None },
        min_leading: 48.0,
        min_trailing: 48.0,
        on_change: Some(Box::new(|_| Message::Nothing)),
    }.into();
    navigation: "navigation", TALL, INTERACTIVE, |look| NavigationStack {
        content: body("Page"),
        can_go_back: look.enabled,
        can_go_forward: false,
        on_navigate: Some(Box::new(|_| Message::Nothing)),
    }.into();
    dialog: "dialog", SHEET, &[State::Normal, State::Hover, State::Pressed, State::Focused], |_| Modal {
        content: body("Document"),
        dialog: Some(Dialog {
            kind: DialogKind::Dialog,
            title: "Rename".into(),
            message: String::new(),
            body: Some(Element::new(TextField {
                label: "Name".into(),
                placeholder: String::new(),
                value: "Report".into(),
                revision: 0,
                purpose: ContentPurpose::Normal,
                enabled: true,
                on_change: None,
                on_submit: None,
            })),
            buttons: vec![choice("Cancel", DialogAction::Cancel), choice("Rename", DialogAction::Default)],
        }),
    }.into();
    alert: "alert", SHEET, &[State::Normal, State::Hover, State::Pressed, State::Focused], |_| Modal {
        content: body("Document"),
        dialog: Some(Dialog {
            kind: DialogKind::Alert,
            title: "Save changes?".into(),
            message: "Your changes are lost if you don't save them.".into(),
            body: None,
            buttons: vec![
                choice("Don't Save", DialogAction::Destructive),
                choice("Cancel", DialogAction::Cancel),
                choice("Save", DialogAction::Default),
            ],
        }),
    }.into();
}
