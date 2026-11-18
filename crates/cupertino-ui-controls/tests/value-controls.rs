//! Controls that choose a value: a slider and a stepper (by pointer and keys, within their range and steps), and
//! the controls that choose one of several (segments, radio buttons, tabs).

#[allow(dead_code)]
mod support;

use cupertino_geometry::{Insets, Point};
use cupertino_ui_controls::{
    RadioGroup, SegmentedControl, Slider, SliderStyle, Spacer, Stack, Stepper, Tabs,
};
use cupertino_ui_core::layout::{Align, Axis};
use cupertino_ui_core::platform::{Keysym, Mods, MouseButton};
use cupertino_ui_core::{AccessibleValue, Element, Role};
use support::{Scene, View, DOWN, END, HOME, LEFT, RIGHT, UP};

const PAGE_UP: Keysym = Keysym(0xff55);

#[derive(Clone, Debug, PartialEq)]
enum Msg {
    Volume(f64),
    VolumeEnd(f64),
    Level(f64),
    Count(f64),
    Range(usize),
    Size(usize),
    Page(usize),
}

#[derive(Default)]
struct State {
    volume: f64,
    level: f64,
    count: f64,
    range: usize,
    size: usize,
    page: usize,
    reports: Vec<Msg>,
}

struct Values;

impl Scene for Values {
    type State = State;
    type Msg = Msg;

    fn state() -> State {
        State {
            volume: 50.0,
            level: 0.5,
            count: 4.0,
            ..State::default()
        }
    }

    fn update(state: &mut State, message: Msg) {
        match &message {
            Msg::Volume(v) => state.volume = *v,
            Msg::Level(v) => state.level = *v,
            Msg::Count(v) => state.count = *v,
            Msg::Range(i) => state.range = *i,
            Msg::Size(i) => state.size = *i,
            Msg::Page(i) => state.page = *i,
            Msg::VolumeEnd(_) => {}
        }
        state.reports.push(message);
    }

    fn view(state: &State) -> Element<Msg> {
        let names = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
        Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::uniform(20.0),
            spacing: 14.0,
            align: Align::Start,
            children: vec![
                Element::new(Slider {
                    label: "Volume".into(),
                    value: state.volume,
                    min: 0.0,
                    max: 100.0,
                    step: Some(5.0),
                    style: SliderStyle::Regular,
                    ticks: 0,
                    enabled: true,
                    on_change: Some(Box::new(Msg::Volume)),
                    on_end: Some(Box::new(Msg::VolumeEnd)),
                }),
                Element::new(Slider {
                    label: "Level".into(),
                    value: state.level,
                    min: 0.0,
                    max: 1.0,
                    step: None,
                    style: SliderStyle::Level,
                    ticks: 0,
                    enabled: true,
                    on_change: Some(Box::new(Msg::Level)),
                    on_end: None,
                }),
                Element::new(Stepper {
                    label: "Count".into(),
                    value: state.count,
                    min: 0.0,
                    max: 10.0,
                    step: 2.0,
                    enabled: true,
                    on_change: Some(Box::new(Msg::Count)),
                }),
                Element::new(SegmentedControl {
                    label: "Range".into(),
                    segments: names(&["Day", "Week", "Month"]),
                    selected: Some(state.range),
                    enabled: true,
                    on_select: Some(Box::new(Msg::Range)),
                }),
                Element::new(RadioGroup {
                    label: "Size".into(),
                    options: names(&["S", "M", "L"]),
                    selected: Some(state.size),
                    axis: Axis::Vertical,
                    spacing: 4.0,
                    enabled: true,
                    on_select: Some(Box::new(Msg::Size)),
                }),
                Element::from(Tabs {
                    label: "Pages".into(),
                    tabs: names(&["One", "Two", "Three"]),
                    selected: state.page,
                    content: Element::new(Spacer { min: 10.0 }),
                    on_select: Some(Box::new(Msg::Page)),
                }),
            ],
        })
    }
}

/// A control that chooses one of three: its name, the role of its choices, their names and what a choice reports.
type Choice = (&'static str, Role, [&'static str; 3], fn(usize) -> Msg);

fn reported(view: &View<Values>) -> Vec<Msg> {
    view.state().reports.clone()
}

fn number(view: &View<Values>, role: Role, name: &str) -> f64 {
    match view.node(role, name).unwrap().value {
        Some(AccessibleValue::Number { value, .. }) => value,
        other => panic!("{name} has {other:?}"),
    }
}

#[test]
fn the_arrow_keys_the_page_keys_home_and_end_move_a_slider_by_its_step_within_its_range() {
    // a slider with a step moves by it, one without by a hundredth of its range
    let rows = [
        ("Volume", RIGHT, Msg::Volume(55.0)),
        ("Volume", UP, Msg::Volume(55.0)),
        ("Volume", LEFT, Msg::Volume(45.0)),
        ("Volume", DOWN, Msg::Volume(45.0)),
        ("Volume", PAGE_UP, Msg::Volume(60.0)),
        ("Volume", HOME, Msg::Volume(0.0)),
        ("Volume", END, Msg::Volume(100.0)),
        ("Level", RIGHT, Msg::Level(0.51)),
        ("Level", HOME, Msg::Level(0.0)),
    ];
    for (name, key, expected) in rows {
        let mut view = View::<Values>::open();
        view.focus(Role::Slider, name);
        view.key(key, None, Mods::NONE);
        let got = reported(&view);
        let Some(last) = got
            .iter()
            .find(|m| matches!(m, Msg::Volume(_) | Msg::Level(_)))
        else {
            panic!("{name} {key:?} reported nothing");
        };
        match (last, &expected) {
            (Msg::Level(a), Msg::Level(b)) => assert!((a - b).abs() < 1e-9, "{name} {key:?}: {a}"),
            _ => assert_eq!(last, &expected, "{name} {key:?}"),
        }
    }

    let mut view = View::<Values>::open();
    view.focus(Role::Slider, "Volume");
    view.key(END, None, Mods::NONE);
    view.key(RIGHT, None, Mods::NONE);
    assert_eq!(number(&view, Role::Slider, "Volume"), 100.0);
    assert_eq!(
        reported(&view),
        [Msg::Volume(100.0), Msg::VolumeEnd(100.0)],
        "a change reports once and a key that changes nothing reports nothing"
    );
}

#[test]
fn a_slider_takes_the_value_under_the_pointer_snapped_to_its_step_and_ends_once_when_let_go() {
    let mut view = View::<Values>::open();
    let bounds = view.bounds_of(Role::Slider, "Volume");
    let at = |share: f64| Point::new(bounds.min_x() + bounds.width() * share, bounds.center().y);

    view.press(at(0.31), MouseButton::Left, Mods::NONE);
    let first = number(&view, Role::Slider, "Volume");
    assert!((first - 31.0).abs() <= 5.0 && first % 5.0 == 0.0, "{first}");
    view.move_to(at(0.7));
    view.move_to(at(2.0));
    assert_eq!(number(&view, Role::Slider, "Volume"), 100.0, "past the end");
    view.move_to(at(-1.0));
    assert_eq!(number(&view, Role::Slider, "Volume"), 0.0, "past the start");
    let ended = |view: &View<Values>| {
        reported(view)
            .iter()
            .filter(|m| matches!(m, Msg::VolumeEnd(_)))
            .count()
    };
    assert_eq!(ended(&view), 0, "while held");
    view.release(at(-1.0), MouseButton::Left);
    assert_eq!(ended(&view), 1);
    assert_eq!(reported(&view).last(), Some(&Msg::VolumeEnd(0.0)));
}

#[test]
fn a_stepper_steps_by_its_half_pressed_or_its_arrow_key_and_stops_at_its_limits() {
    let mut view = View::<Values>::open();
    let bounds = view.bounds_of(Role::SpinButton, "Count");
    let up = Point::new(bounds.center().x, bounds.min_y() + bounds.height() * 0.25);
    let down = Point::new(bounds.center().x, bounds.min_y() + bounds.height() * 0.75);

    view.click(up, Mods::NONE);
    assert_eq!(view.state().count, 6.0);
    view.click(down, Mods::NONE);
    view.click(down, Mods::NONE);
    assert_eq!(view.state().count, 2.0);

    view.focus(Role::SpinButton, "Count");
    for (key, expected) in [
        (UP, 4.0),
        (DOWN, 2.0),
        (END, 10.0),
        (HOME, 0.0),
        (DOWN, 0.0),
    ] {
        view.key(key, None, Mods::NONE);
        assert_eq!(view.state().count, expected, "{key:?}");
    }
    assert_eq!(
        reported(&view),
        [6.0, 4.0, 2.0, 4.0, 2.0, 10.0, 0.0].map(Msg::Count),
        "a step that would pass a limit reports nothing"
    );
}

#[test]
fn clicking_a_choice_selects_it_and_the_arrows_move_the_choice_from_the_one_selected() {
    // (control, role of its choices, the choices, what reports)
    let controls: [Choice; 3] = [
        (
            "Range",
            Role::RadioButton,
            ["Day", "Week", "Month"],
            Msg::Range,
        ),
        ("Size", Role::RadioButton, ["S", "M", "L"], Msg::Size),
        ("Pages", Role::Tab, ["One", "Two", "Three"], Msg::Page),
    ];
    for (name, role, choices, report) in controls {
        let mut view = View::<Values>::open();
        let third = view.center_of(role, choices[2]);
        view.click(third, Mods::NONE);
        assert_eq!(reported(&view), [report(2)], "{name}: a click");

        view.focus(role, choices[2]);
        view.key(LEFT, None, Mods::NONE);
        view.key(UP, None, Mods::NONE);
        let got = reported(&view);
        assert!(
            got.contains(&report(1)),
            "{name}: an arrow back from the third: {got:?}"
        );
    }
}
