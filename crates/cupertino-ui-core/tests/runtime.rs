//! The production runtime on the headless window system: how it reconciles the application's view with its nodes,
//! routes keys and moves the focus, and keeps the application's windows.

use crate::support;

use std::cell::Cell;
use std::rc::Rc;

use cupertino_ui_core::platform::{Capabilities, Mods, WindowEvent};
use cupertino_ui_core::{Key, Runtime, UiError};
use cupertino_ui_test::{DriverError, HeadlessWindowSystem};
use support::{options, row, turns, Lab, Msg, Row, Shown, ENTER, F12, LEFT_TAB, TAB};

/// The serial in a probe's name: the state its node was made with.
fn serial(name: &str) -> u32 {
    name.split_once('#').unwrap().1.parse().unwrap()
}

fn label(name: &str) -> &str {
    name.split_once('#').unwrap().0
}

fn shows(shown: &mut Shown, rows: Vec<Row>) -> Vec<String> {
    shown.send(Msg::Show(rows));
    shown.names(shown.window)
}

#[test]
fn a_node_keeps_its_state_while_its_key_or_its_place_stays_and_is_made_anew_otherwise() {
    let mut shown = Shown::open();
    let first = shows(
        &mut shown,
        vec![
            row(Some("a"), "a"),
            row(Some("b"), "b"),
            row(Some("c"), "c"),
        ],
    );
    let by_label =
        |names: &[String], l: &str| serial(names.iter().find(|n| label(n) == l).unwrap());
    let (a, b, c) = (
        by_label(&first, "a"),
        by_label(&first, "b"),
        by_label(&first, "c"),
    );
    assert!(a != b && b != c);

    assert!(
        shown
            .driver
            .runtime()
            .find(shown.window, &Key::from("b"))
            .is_some(),
        "a node is found by its key"
    );
    let moved = shows(
        &mut shown,
        vec![
            row(Some("c"), "c"),
            row(Some("a"), "a"),
            row(Some("b"), "b"),
        ],
    );
    assert_eq!(
        moved.iter().map(|n| label(n)).collect::<Vec<_>>(),
        ["c", "a", "b"],
        "in the view's order"
    );
    assert_eq!(
        (
            by_label(&moved, "a"),
            by_label(&moved, "b"),
            by_label(&moved, "c")
        ),
        (a, b, c),
        "reordered keyed nodes keep their state"
    );

    let fewer = shows(&mut shown, vec![row(Some("c"), "c"), row(Some("a"), "a")]);
    assert_eq!(fewer.len(), 2);
    let again = shows(
        &mut shown,
        vec![
            row(Some("c"), "c"),
            row(Some("a"), "a"),
            row(Some("b"), "b"),
        ],
    );
    assert_ne!(
        by_label(&again, "b"),
        b,
        "a key that left comes back as a new node"
    );
    assert_eq!((by_label(&again, "a"), by_label(&again, "c")), (a, c));

    // without keys a node is matched by its place: the second probe takes the state of the second node
    let plain = shows(&mut shown, vec![row(None, "x"), row(None, "y")]);
    let (x, y) = (serial(&plain[0]), serial(&plain[1]));
    let swapped = shows(&mut shown, vec![row(None, "y"), row(None, "x")]);
    assert_eq!(
        swapped
            .iter()
            .map(|n| (label(n), serial(n)))
            .collect::<Vec<_>>(),
        [("y", x), ("x", y)],
        "by position"
    );

    assert!(
        shown
            .driver
            .runtime()
            .find(shown.window, &Key::from("a"))
            .is_none(),
        "the keyed nodes are gone with their keys"
    );
}

#[test]
fn a_view_with_more_nodes_than_the_ceiling_is_an_error_of_the_run() {
    let mut shown = Shown::open_with(6);
    let rows: Vec<Row> = (0..10).map(|_| row(None, "probe")).collect();
    let window = shown.queue(Msg::Show(rows));
    shown.send_key(window, F12, Mods::NONE);
    let result = shown.driver.run_until_idle(support::turns());
    assert!(
        matches!(result, Err(DriverError::Runtime(UiError::NodeLimit(6)))),
        "{result:?}"
    );
}

#[test]
fn tab_walks_the_focus_through_the_focusable_nodes_in_order_and_round_and_shift_tab_back() {
    let mut shown = Shown::open();
    shows(
        &mut shown,
        vec![
            row(Some("a"), "a"),
            row(Some("b"), "b"),
            row(Some("c"), "c"),
        ],
    );
    let window = shown.window;
    assert_eq!(
        shown.focused(window),
        None,
        "nothing is focused before the first Tab"
    );
    let mut walked = Vec::new();
    for _ in 0..4 {
        shown.press(window, TAB, Mods::NONE);
        walked.push(shown.focused(window).map(|n| label(&n).to_owned()));
    }
    assert_eq!(
        walked,
        [
            Some("a".into()),
            Some("b".into()),
            Some("c".into()),
            Some("a".into())
        ]
    );
    shown.press(window, LEFT_TAB, Mods::SHIFT);
    assert_eq!(
        shown.focused(window).as_deref().map(label),
        Some("c"),
        "Shift+Tab goes back, round the start"
    );

    // a focused node that leaves the view hands the focus on or drops it, never keeps it
    shown.press(window, TAB, Mods::NONE);
    shown.press(window, TAB, Mods::NONE);
    assert_eq!(shown.focused(window).as_deref().map(label), Some("b"));
    shows(&mut shown, vec![row(Some("a"), "a"), row(Some("c"), "c")]);
    assert_ne!(shown.focused(window).as_deref().map(label), Some("b"));
    shown.press(window, TAB, Mods::NONE);
    assert!(shown.focused(window).is_some());
}

#[test]
fn a_key_reaches_the_focused_node_first_and_the_windows_shortcuts_only_if_it_lets_it_pass() {
    let mut shown = Shown::open();
    let taking = Row {
        takes_enter: true,
        ..row(Some("a"), "takes")
    };
    shows(&mut shown, vec![taking, row(Some("b"), "passes")]);
    let window = shown.window;
    let log = |shown: &Shown| -> Vec<Msg> {
        shown
            .model()
            .log
            .iter()
            .filter(|m| !matches!(m, Msg::Show(_)))
            .cloned()
            .collect()
    };

    shown.press(window, ENTER, Mods::NONE);
    assert_eq!(
        log(&shown),
        [Msg::Shortcut("enter")],
        "with nothing focused the window's shortcut has the key"
    );
    shown.press(window, TAB, Mods::NONE);
    shown.press(window, ENTER, Mods::NONE);
    assert_eq!(
        log(&shown)[1..],
        [Msg::Took("takes")],
        "the focused node took it: no shortcut"
    );
    shown.press(window, TAB, Mods::NONE);
    shown.press(window, ENTER, Mods::NONE);
    assert_eq!(
        log(&shown)[2..],
        [Msg::Shortcut("enter")],
        "a node that lets it pass leaves it to the window"
    );
    shown.press(
        window,
        cupertino_ui_core::platform::Keysym(0x73),
        Mods::PRIMARY,
    );
    shown.press(
        window,
        cupertino_ui_core::platform::Keysym(0x73),
        Mods::NONE,
    );
    assert_eq!(
        log(&shown)[3..],
        [Msg::Shortcut("save")],
        "a chord matches its modifiers"
    );
}

#[test]
fn windows_open_and_close_with_the_view_and_a_close_request_goes_to_the_application_or_ends_it() {
    let mut shown = Shown::open();
    let keys = |shown: &Shown| -> Vec<String> {
        shown
            .driver
            .runtime()
            .windows()
            .into_iter()
            .map(|(k, _)| format!("{:?}", k.0))
            .collect()
    };
    assert_eq!(keys(&shown).len(), 1);
    shown.send(Msg::Windows(vec!["main", "other"]));
    assert_eq!(keys(&shown).len(), 2, "a window the view gained opens");
    let other = shown.driver.runtime().windows()[1].1;
    shown.configure(other);

    shown.send(Msg::Windows(vec!["main"]));
    assert_eq!(keys(&shown).len(), 1, "a window the view lost closes");

    let window = shown.window;
    shown
        .driver
        .window_system_mut()
        .enqueue_window(window, WindowEvent::CloseRequested)
        .unwrap();
    shown.settle();
    assert!(
        shown.model().log.contains(&Msg::Closed("main")),
        "the window with an on_close tells the application"
    );
    assert!(!shown.driver.runtime().exited());

    shown.send(Msg::Windows(vec!["last"]));
    let last = shown.driver.runtime().windows()[0].1;
    shown.configure(last);
    shown
        .driver
        .window_system_mut()
        .enqueue_window(last, WindowEvent::CloseRequested)
        .unwrap();
    shown.settle();
    assert!(
        shown.driver.runtime().exited(),
        "a close request of a window without on_close ends the application"
    );

    let mut asked = Shown::open();
    asked.send(Msg::Exit);
    assert!(asked.driver.runtime().exited(), "so does a command to exit");
}

#[test]
fn the_runtime_ends_on_the_turn_its_exit_condition_holds() {
    let asked = Rc::new(Cell::new(false));
    let condition = Rc::clone(&asked);
    let mut runtime: Runtime<Lab, HeadlessWindowSystem> = Runtime::new(
        HeadlessWindowSystem::new(Capabilities::default()),
        options(1000),
    )
    .with_exit_when(Box::new(move || condition.get()));
    runtime.step(turns()).unwrap();
    assert!(
        !runtime.exited(),
        "it runs while the condition does not hold"
    );
    asked.set(true);
    runtime.step(turns()).unwrap();
    assert!(runtime.exited(), "it ends on the next turn once it holds");
}
