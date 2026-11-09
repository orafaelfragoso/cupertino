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
    Menu, MenuAnchor, MenuItem, Modal, NavigationStack, PageDots, PopUpButton, Progress,
    RadioGroup, ScrollView, Scroller, SecureField, SegmentedControl, Selection, Separator, Slider,
    SliderStyle, SortOrder, SplitLayout, SplitView, Stack, Stepper, TableView, Text, TextBox,
    TextEditor, TextField, TextRole, Toggle,
};
use cupertino_kit::controls::{BatteryMeter, ModuleLayout, ModuleToggle, Tile};
use cupertino_kit::controls::{Canvas, Grid, Image, ImageFit, ImageSource, RichText, TextLink};
use cupertino_kit::controls::{Popover, TreeItem, TreeView};
use cupertino_kit::controls::{Sidebar, SidebarAccessory, SidebarItem, SidebarSection};
use cupertino_kit::core::{
    AccessibilityNode, Action, Application, Checked, Command, CommandRegistry, Element, Key,
    NodeId, Role, Window, WindowBackground, WindowKey,
};
use cupertino_kit::geometry::{Insets, Point, Size};
use cupertino_kit::layout::{Align, Axis, LayoutDirection, ScrollState, Track};
use cupertino_kit::platform::{
    DecorationMode, Mods, MouseButton, PointerEvent, Timestamp, WindowEvent, WindowSpec,
};
use cupertino_kit::render::{ImageResource, Paint, PaintOp, PixelFormat, ResourceId, Shape};
use cupertino_kit::text::{AttributedText, ContentPurpose, TextStyle};
use cupertino_kit::theme::{Appearance, ColorRole, StateVariant};
use cupertino_ui_test::{
    accessibility_audit, describe, semantic_outline, whole_tree, AssistiveTechnology, Inspector,
};
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
    /// How many messages the application took.
    static HEARD: Cell<usize> = const { Cell::new(0) };
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
        HEARD.set(HEARD.get() + 1);
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
                input_region: None,
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
            menus: None,
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
    let snapshot = run.snapshot();
    let tree = whole_tree(run.window, snapshot.accessibility.clone(), snapshot.focus);
    // a cell is an untitled window around one control
    let problems: Vec<String> = accessibility_audit(&tree)
        .into_iter()
        .filter(|p| p != "Window #0: no name")
        .collect();
    assert_eq!(
        problems,
        Vec::<String>::new(),
        "{} {appearance:?} {direction:?} {state:?} reads:\n{}",
        fixture.name,
        semantic_outline(&tree).join("\n")
    );
    if let (State::Focused, Some(focus)) = (state, snapshot.focus) {
        // the keyboard focus shows in every appearance, high contrast included: a menu item by its highlight, a text
        // editor by its caret, any other control by a ring in the theme's colour
        let theme = repository_theme(appearance);
        let color = |role| theme.colors.get(role, StateVariant::NORMAL).unwrap();
        let role = snapshot
            .accessibility
            .iter()
            .find(|(id, _)| *id == focus)
            .map(|(_, n)| n.role);
        let mut ops = snapshot.display_list.nodes.iter().flat_map(|n| &n.ops);
        let shows = match role {
            Some(Role::MenuItem | Role::MenuItemCheckBox | Role::MenuItemRadio) => {
                let highlight = color(ColorRole::SelectedControlFill);
                ops.any(|op| matches!(op, PaintOp::Fill { paint: Paint::Solid(c), .. } if *c == highlight))
            }
            Some(Role::MultilineTextInput) => {
                let text = color(ColorRole::Text);
                ops.any(|op| {
                    matches!(op, PaintOp::Fill { shape: Shape::Rect(r), paint: Paint::Solid(c) }
                        if *c == text && r.size.width <= 2.0)
                })
            }
            _ => {
                let ring = color(ColorRole::KeyboardFocusIndicator);
                ops.any(|op| {
                    matches!(op, PaintOp::Stroke { width, paint: Paint::Solid(c), .. }
                        if *c == ring && *width == theme.focus.width)
                })
            }
        };
        assert!(
            shows,
            "{} {appearance:?} {direction:?}: the keyboard focus on {role:?} does not show",
            fixture.name
        );
    }
    let dump = Inspector::default().dump(&snapshot);
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

/// A 64 by 32 pixel picture, a warm left half and a cool right half over a vertical ramp, so a fit or a crop shows.
fn picture() -> ImageSource {
    let (width, height) = (64u32, 32u32);
    let pixels: Vec<u8> = (0..height)
        .flat_map(|y| {
            (0..width).flat_map(move |x| {
                let ramp = (255 * y / (height - 1)) as u8;
                if x < width / 2 {
                    [230, ramp, 40, 255]
                } else {
                    [40, ramp, 230, 255]
                }
            })
        })
        .collect();
    ImageSource {
        id: ResourceId(1),
        image: ImageResource {
            revision: 1,
            width,
            height,
            stride: width * 4,
            format: PixelFormat::Rgba8Premultiplied,
            pixels: pixels.into(),
        },
    }
}

fn image(fit: ImageFit, radius: f64) -> Element<Message> {
    Element::new(Image {
        sources: vec![picture()],
        size: Size::new(48.0, 40.0),
        fit,
        radius,
        label: Some("Picture".into()),
    })
}

const CONTROL: Size = Size::new(180.0, 48.0);
const WIDE: Size = Size::new(240.0, 56.0);
const TALL: Size = Size::new(200.0, 112.0);
const SIDEBAR: Size = Size::new(200.0, 250.0);
const SHEET: Size = Size::new(400.0, 220.0);
/// An alert hangs a tall column of stacked buttons under the toolbar row.
const ALERT: Size = Size::new(400.0, 360.0);
/// A compact module of a panel, 62 pt square inside the cell padding.
const MODULE: Size = Size::new(86.0, 86.0);

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
                check(FIXTURES.iter().find(|f| f.name == $name).unwrap());
            }
        )*

        /// Every fixture, for the checks that go over all of them.
        static FIXTURES: &[Fixture] = &[$(
            Fixture {
                name: $name,
                size: $size,
                states: $states,
                build: $build,
            },
        )*];
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
    module_toggle: "module-toggle", CONTROL, ALL, |look| Element::new(ModuleToggle {
        label: "Wi-Fi".into(),
        caption: "Home Network".into(),
        symbol: "symbol.wifi".into(),
        layout: ModuleLayout::Row,
        on: look.selected,
        enabled: look.enabled,
        on_toggle: Some(Box::new(|_| Message::Nothing)),
    });
    module_tile: "module-tile", MODULE, ALL, |look| Element::from(Tile {
        label: "Stage".into(),
        content: Element::new(ModuleToggle {
            label: "Stage".into(),
            caption: "Off".into(),
            symbol: "symbol.stage".into(),
            layout: ModuleLayout::Compact,
            on: look.selected,
            enabled: look.enabled,
            on_toggle: Some(Box::new(|_| Message::Nothing)),
        }),
    });
    battery: "battery", CONTROL, &[State::Normal], |_| Element::new(BatteryMeter {
        label: "Battery".into(),
        level: 0.8,
        charging: false,
    });
    battery_low: "battery-low", CONTROL, &[State::Normal], |_| Element::new(BatteryMeter {
        label: "Battery".into(),
        level: 0.03,
        charging: false,
    });
    // the tip's popup content, drawn in a cell of its own
    tooltip: "tooltip", CONTROL, &[State::Normal], |_| cupertino_kit::controls::Tooltip {
        text: "Move to the Trash".into(),
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
        on_end: None,
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
        on_end: None,
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
        on_end: None,
    });
    slider_panel_level: "slider-panel-level", WIDE, INTERACTIVE, |look| Element::new(Slider {
        label: "Display".into(),
        value: 0.4,
        min: 0.0,
        max: 1.0,
        step: None,
        style: SliderStyle::PanelLevel("symbol.sun"),
        ticks: 0,
        enabled: look.enabled,
        on_change: Some(Box::new(|_| Message::Nothing)),
        on_end: None,
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
        selection: None,
        initial: false,
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
        selection: None,
        initial: false,
    });
    text_box: "text-box", TALL, FIELD, |look| TextBox {
        editor: Element::new(TextEditor {
            label: "Notes".into(),
            placeholder: String::new(),
            value: "One\nTwo\nThree\nFour\nFive\nSix\nSeven\nEight".into(),
            revision: 0,
            enabled: look.enabled,
            bordered: true,
            on_change: Some(Box::new(|_| Message::Nothing)),
        }),
        enabled: look.enabled,
        state: look.scroll,
        scroller: Scroller::Always,
        on_scroll: Box::new(Message::Scroll),
    }.into();
    secure_field: "secure-field", WIDE, FIELD, |look| Element::new(SecureField {
        label: "Password".into(),
        placeholder: "Required".into(),
        enabled: look.enabled,
        on_change: None,
        on_submit: None,
        ..SecureField::default()
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
            initial: false,
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
    tree: "tree", TALL, &[State::Normal, State::Hover, State::Focused, State::Selected], |look| {
        const ROWS: [(&str, usize, Option<bool>, usize, usize); 5] = [
            ("Documents", 0, Some(true), 0, 2),
            ("Notes", 1, None, 0, 2),
            ("Drafts", 1, Some(false), 1, 2),
            ("Pictures", 0, Some(false), 1, 2),
            ("Music", 0, None, 2, 3),
        ];
        let mut selection = Selection::default();
        if look.selected {
            selection.select_only(1);
        }
        TreeView {
            label: "Folders".into(),
            count: ROWS.len(),
            row_height: 24.0,
            scroll: look.scroll,
            selection,
            multiple: false,
            item: Box::new(|index| {
                let (_, level, expanded, position, siblings) = ROWS[index];
                TreeItem { level, expanded, position, siblings }
            }),
            row: Box::new(|index, selected| Element::new(Text {
                content: label(ROWS[index].0),
                role: TextRole::Body,
                wrap: false,
                secondary: false,
                selected,
            })),
            type_select: None,
            on_scroll: Box::new(Message::Scroll),
            on_select: Some(Box::new(|_| Message::Nothing)),
            on_activate: None,
            on_toggle: Some(Box::new(|_| Message::Nothing)),
        }
        .into()
    };
    text_editor: "text-editor", TALL, FIELD, |look| Element::new(TextEditor {
        label: "Notes".into(),
        placeholder: String::new(),
        value: "First line\nA second line that is long enough to wrap".into(),
        revision: 0,
        enabled: look.enabled,
        bordered: false,
        on_change: None,
    });
    popover: "popover", TALL, &[State::Normal], |look| Popover {
        parent: WindowKey("fixture".into()),
        anchor: Key::from("anchor"),
        label: "Details".into(),
        content: body("Shared with 3 people"),
        scroll: look.scroll,
        on_scroll: Box::new(Message::Scroll),
        on_close: Message::Nothing,
    }
    .window(WindowKey("popover".into()))
    .content;
    sidebar: "sidebar", SIDEBAR, &[State::Normal, State::Hover, State::Focused, State::Selected], |look| {
        let item = |label: &str, icon: &str| SidebarItem {
            key: Key::from(label.to_owned()),
            label: label.into(),
            icon: Some(icon.into()),
            description: None,
            accessory: None,
            busy: false,
        };
        Sidebar {
            label: "Places".into(),
            sections: vec![
                SidebarSection {
                    title: None,
                    items: vec![item("Recents", "clock")],
                },
                SidebarSection {
                    title: Some("Favorites".into()),
                    items: vec![item("Desktop", "menubar.dock.rectangle"), item("Documents", "doc")],
                },
                SidebarSection {
                    title: Some("Locations".into()),
                    items: vec![
                        item("Computer", "desktopcomputer"),
                        SidebarItem {
                            accessory: Some(SidebarAccessory {
                                symbol: "eject".into(),
                                label: "Eject".into(),
                            }),
                            ..item("Backup", "externaldrive")
                        },
                    ],
                },
            ],
            selected: look.selected.then_some(1),
            scroll: look.scroll,
            on_scroll: Box::new(Message::Scroll),
            on_select: Some(Box::new(|_| Message::Nothing)),
            on_accessory: None,
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
            on_row_key: None,
            header_top: 0.0,
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
        toolbar: Vec::new(),
        can_go_back: look.enabled,
        can_go_forward: false,
        on_navigate: Some(Box::new(|_| Message::Nothing)),
        under_toolbar: false,
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
                selection: None,
                initial: false,
            })),
            buttons: vec![choice("Cancel", DialogAction::Cancel), choice("Rename", DialogAction::Default)],
        }),
    }.into();
    alert: "alert", ALERT, &[State::Normal, State::Hover, State::Pressed, State::Focused], |_| Modal {
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
    pop_up_button: "pop-up-button", CONTROL, INTERACTIVE, |look| Element::new(PopUpButton {
        label: "Size".into(),
        options: vec!["Small".into(), "Medium".into(), "Large".into()],
        selected: 1,
        open: false,
        enabled: look.enabled,
        on_open: Some(Message::Nothing),
    });
    menu: "menu", TALL, &[State::Normal, State::Hover, State::Pressed, State::Focused], |_| {
        let item = |label: &str, enabled, checked| MenuItem::Item {
            label: label.into(),
            enabled,
            checked,
            on_select: Message::Nothing,
        };
        let mut windows = Menu {
            parent: WindowKey("fixture".into()),
            anchor: MenuAnchor::At(Point::ZERO),
            label: "Edit".into(),
            items: vec![
                item("Wrap Lines", true, true),
                item("Paste", false, false),
                MenuItem::Separator,
                MenuItem::Submenu {
                    label: "Share".into(),
                    enabled: true,
                    items: Vec::new(),
                },
            ],
            current: None,
            open: Vec::new(),
            on_open: Box::new(|_| Message::Nothing),
            on_close: Message::Nothing,
        }
        .windows(WindowKey("menu".into()));
        windows.remove(0).content
    };
    rich_text: "rich-text", WIDE, &[State::Normal, State::Focused], |_| {
        let text = "Read the guide first.";
        let start = text.find("guide").unwrap();
        Element::new(RichText {
            content: label(text),
            role: TextRole::Body,
            wrap: false,
            secondary: false,
            links: vec![TextLink {
                range: start..start + "guide".len(),
                url: "https://example.org/guide".into(),
                on_open: Some(Message::Nothing),
            }],
        })
    };
    image_contain: "image-contain", CONTROL, &[State::Normal], |_| image(ImageFit::Contain, 0.0);
    image_cover: "image-cover", CONTROL, &[State::Normal], |_| image(ImageFit::Cover, 6.0);
    canvas: "canvas", CONTROL, &[State::Normal], |_| Element::new(Canvas {
        size: Some(Size::new(120.0, 32.0)),
        label: "Levels".into(),
        paint: Box::new(|cx| {
            let colors = cx.env().theme.colors.clone();
            let bar = colors.get(ColorRole::DefaultButtonFill, StateVariant::NORMAL);
            let line = colors.get(ColorRole::Separator, StateVariant::NORMAL);
            for (i, level) in [0.4, 0.9, 0.6, 0.25].into_iter().enumerate() {
                if let Some(bar) = bar {
                    let height = 30.0 * level;
                    cx.push(PaintOp::Fill {
                        shape: Shape::RoundedRect {
                            rect: cupertino_kit::geometry::Rect::new(8.0 + 28.0 * i as f64, 31.0 - height, 20.0, height),
                            radius: 3.0,
                        },
                        paint: Paint::Solid(bar),
                    });
                }
            }
            if let Some(line) = line {
                cx.push(PaintOp::Fill {
                    shape: Shape::Rect(cupertino_kit::geometry::Rect::new(0.0, 31.0, 120.0, 1.0)),
                    paint: Paint::Solid(line),
                });
            }
        }),
        children: Vec::new(),
    });
    grid: "grid", TALL, &[State::Normal], |_| Element::new(Grid {
        columns: vec![Track::Auto, Track::Fraction(1.0)],
        rows: Vec::new(),
        padding: Insets::ZERO,
        spacing: Size::new(8.0, 4.0),
        children: vec![
            body("Name"),
            body("Ada"),
            body("Kind"),
            body("Folder"),
            body("Shared folder").span(2, 1),
            body("Size"),
            body("12 KB"),
        ],
    });
}

/// The actions that operate a control, which a screen reader offers its user to perform.
const OPERATIONS: [Action; 6] = [
    Action::Click,
    Action::Increment,
    Action::Decrement,
    Action::Expand,
    Action::Collapse,
    Action::ShowMenu,
];

/// A cell of `fixture` in its normal state with a screen reader listening, and the nodes of its tree in order.
fn heard(
    fixture: &'static Fixture,
) -> (
    Run<Cells>,
    AssistiveTechnology,
    Vec<(NodeId, AccessibilityNode)>,
) {
    CELL.set(Some((fixture, State::Normal)));
    let reader = AssistiveTechnology::new();
    let run = Run::<Cells>::with_accessibility(Some(reader.adapter()));
    let mut nodes = run.snapshot().accessibility;
    nodes.sort_by_key(|(id, _)| *id);
    (run, reader, nodes)
}

/// What the screen reader can tell of the cell: its outline, and how many windows are open.
fn seen(run: &Run<Cells>) -> (Vec<String>, usize) {
    let snapshot = run.snapshot();
    let tree = whole_tree(run.window, snapshot.accessibility, snapshot.focus);
    (
        semantic_outline(&tree),
        run.driver.runtime().windows().len(),
    )
}

#[test]
fn every_action_a_control_offers_reaches_the_application_or_changes_what_it_reads() {
    let mut ignored = Vec::new();
    for fixture in FIXTURES
        .iter()
        .filter(|f| f.states.contains(&State::Normal))
    {
        let (_, _, nodes) = heard(fixture);
        for (index, (_, node)) in nodes.iter().enumerate() {
            // choosing the tab that is already chosen changes nothing
            let chosen = node.role == Role::Tab && node.states.selected;
            for &action in node
                .actions
                .iter()
                .filter(|a| OPERATIONS.contains(a) && !(chosen && **a == Action::Click))
            {
                // each action on a cell of its own, so one action's effect does not hide another's
                let (mut run, reader, nodes) = heard(fixture);
                let before = (HEARD.get(), seen(&run));
                run.driver.ask(&reader, nodes[index].0, action, None);
                run.settle();
                if (HEARD.get(), seen(&run)) == before {
                    ignored.push(format!(
                        "{}: {} ignores {action:?}",
                        fixture.name,
                        describe(node)
                    ));
                }
            }
            if node.states.focusable && node.actions.contains(&Action::Focus) {
                let (mut run, reader, nodes) = heard(fixture);
                run.driver.ask(&reader, nodes[index].0, Action::Focus, None);
                run.settle();
                if run.snapshot().focus != Some(nodes[index].0) {
                    ignored.push(format!(
                        "{}: {} ignores Focus",
                        fixture.name,
                        describe(node)
                    ));
                }
            }
        }
    }
    assert_eq!(ignored, Vec::<String>::new());
}
