//! Buttons, switches and check boxes as a person uses them: what a press, a key and a drag activate, once and only
//! while the control takes input, and what each reports.

use crate::support;

use cupertino_geometry::{Insets, Point};
use cupertino_ui_controls::{Button, ButtonRole, ButtonStyle, CheckBox, Stack, Toggle};
use cupertino_ui_core::layout::{Align, Axis};
use cupertino_ui_core::platform::{Mods, MouseButton};
use cupertino_ui_core::{Checked, Element, Role};
use support::{Scene, View, ESCAPE, RETURN, SPACE, TAB};

#[derive(Clone, Debug, PartialEq)]
enum Msg {
    Pressed(&'static str),
    Wifi(bool),
    Agree(bool),
    Quiet(bool),
}

struct State {
    presses: Vec<&'static str>,
    wifi: bool,
    agree: Checked,
    reports: Vec<Msg>,
}

struct Controls;

impl Scene for Controls {
    type State = State;
    type Msg = Msg;

    fn state() -> State {
        State {
            presses: Vec::new(),
            wifi: false,
            agree: Checked::Mixed,
            reports: Vec::new(),
        }
    }

    fn update(state: &mut State, message: Msg) {
        match &message {
            Msg::Pressed(name) => state.presses.push(name),
            Msg::Wifi(on) => state.wifi = *on,
            Msg::Agree(on) => {
                state.agree = if *on { Checked::True } else { Checked::False };
            }
            Msg::Quiet(_) => {}
        }
        state.reports.push(message);
    }

    fn view(state: &State) -> Element<Msg> {
        let button = |label: &str, name: &'static str, enabled: bool, busy: bool| {
            Element::new(Button {
                label: label.into(),
                role: ButtonRole::Normal,
                style: ButtonStyle::Bordered,
                enabled,
                busy,
                on_press: Some(Msg::Pressed(name)),
            })
        };
        Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::uniform(20.0),
            spacing: 14.0,
            align: Align::Start,
            children: vec![
                button("Go", "go", true, false),
                button("Off", "off", false, false),
                button("Wait", "wait", true, true),
                Element::new(Toggle {
                    label: "Wi-Fi".into(),
                    on: state.wifi,
                    enabled: true,
                    on_toggle: Some(Box::new(Msg::Wifi)),
                }),
                Element::new(Toggle {
                    label: "Quiet".into(),
                    on: false,
                    enabled: false,
                    on_toggle: Some(Box::new(Msg::Quiet)),
                }),
                Element::new(CheckBox {
                    label: "Agree".into(),
                    checked: state.agree,
                    enabled: true,
                    on_toggle: Some(Box::new(Msg::Agree)),
                }),
            ],
        })
    }
}

fn pressed(view: &View<Controls>) -> Vec<&'static str> {
    view.state().presses.clone()
}

#[derive(Clone, Copy)]
enum Gesture {
    Click,
    DoubleClick,
    LetGoAway,
    OffAndBack,
}

#[test]
fn a_button_activates_once_when_the_press_is_let_go_over_it_and_never_when_it_takes_no_input() {
    let rows: [(&str, &str, Gesture, &[&str]); 6] = [
        ("a click", "Go", Gesture::Click, &["go"]),
        ("a click on a disabled button", "Off", Gesture::Click, &[]),
        ("a click on a busy button", "Wait", Gesture::Click, &[]),
        ("a press let go elsewhere", "Go", Gesture::LetGoAway, &[]),
        (
            "a press dragged off and back",
            "Go",
            Gesture::OffAndBack,
            &["go"],
        ),
        ("two clicks", "Go", Gesture::DoubleClick, &["go", "go"]),
    ];
    for (what, name, gesture, expected) in rows {
        let mut view = View::<Controls>::open();
        let at = view.center_of(Role::Button, name);
        let away = Point::new(at.x + 200.0, at.y);
        match gesture {
            Gesture::Click => view.click(at, Mods::NONE),
            Gesture::DoubleClick => {
                view.click(at, Mods::NONE);
                view.click(at, Mods::NONE);
            }
            Gesture::LetGoAway => {
                view.press(at, MouseButton::Left, Mods::NONE);
                view.move_to(away);
                view.release(away, MouseButton::Left);
            }
            Gesture::OffAndBack => {
                view.press(at, MouseButton::Left, Mods::NONE);
                view.move_to(away);
                view.move_to(at);
                view.release(at, MouseButton::Left);
            }
        }
        assert_eq!(pressed(&view), expected, "{what}");
    }
}

#[test]
fn return_activates_a_focused_button_on_press_and_space_on_release_unless_escape_comes_between() {
    let mut view = View::<Controls>::open();
    view.key(TAB, None, Mods::NONE);
    assert!(view.node(Role::Button, "Go").unwrap().states.focused);

    view.key_down(RETURN, None, Mods::NONE);
    assert_eq!(pressed(&view), ["go"], "Return acts on press");
    view.key_up(RETURN, Mods::NONE);
    assert_eq!(pressed(&view), ["go"]);

    view.key_down(SPACE, Some(' '), Mods::NONE);
    assert_eq!(pressed(&view), ["go"], "Space arms");
    view.key_up(SPACE, Mods::NONE);
    assert_eq!(pressed(&view), ["go", "go"], "and acts on release");

    view.key_down(SPACE, Some(' '), Mods::NONE);
    view.key(ESCAPE, None, Mods::NONE);
    view.key_up(SPACE, Mods::NONE);
    assert_eq!(pressed(&view), ["go", "go"], "Escape disarms it");
}

#[test]
fn a_switch_and_a_check_box_report_the_state_they_would_take_and_not_while_disabled() {
    let mut view = View::<Controls>::open();
    let wifi = view.center_of(Role::Switch, "Wi-Fi");
    view.click(wifi, Mods::NONE);
    assert!(view.state().wifi);
    view.click(wifi, Mods::NONE);
    assert!(!view.state().wifi);

    let quiet = view.center_of(Role::Switch, "Quiet");
    view.click(quiet, Mods::NONE);

    let agree = view.center_of(Role::CheckBox, "Agree");
    view.click(agree, Mods::NONE);
    assert_eq!(view.state().agree, Checked::True, "a mixed box turns on");
    view.click(agree, Mods::NONE);
    assert_eq!(view.state().agree, Checked::False);

    assert_eq!(
        view.state().reports,
        [
            Msg::Wifi(true),
            Msg::Wifi(false),
            Msg::Agree(true),
            Msg::Agree(false)
        ]
    );
}

#[test]
fn a_switch_takes_the_side_its_thumb_is_dragged_to_and_a_drag_let_go_away_changes_nothing() {
    let rows = [
        ("dragged along its track", 24.0, 0.0, true),
        ("dragged toward its off end", -10.0, 0.0, false),
        ("dragged and let go away from it", 24.0, 200.0, false),
        ("moved less than a drag, which is a click", 1.0, 0.0, true),
    ];
    for (what, along, off, on) in rows {
        let mut view = View::<Controls>::open();
        let bounds = view.bounds_of(Role::Switch, "Wi-Fi");
        let start = Point::new(bounds.min_x() + 14.0, bounds.center().y);
        let end = Point::new(start.x + along, start.y + off);
        view.press(start, MouseButton::Left, Mods::NONE);
        view.move_to(Point::new((start.x + end.x) / 2.0, end.y));
        view.move_to(end);
        view.release(end, MouseButton::Left);
        assert_eq!(view.state().wifi, on, "{what}");
    }
}
