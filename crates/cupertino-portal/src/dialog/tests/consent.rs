//! The keys of the consent windows (screen sharing and screenshots): each opens with the focus on Cancel, takes no key
//! that presses a button until it is armed, and then presses the focused button with Return. Escape and closing the
//! window deny, armed or not.

use super::*;

const TAB: Keysym = Keysym(0xff09);
const SPACE_KEY: Keysym = Keysym(0x20);

/// The key of the node that holds the window's focus.
fn focus(driver: &TestDriver<Dialogs>, window: WindowId) -> Option<Key> {
    let runtime = driver.runtime();
    let focused = runtime.snapshot(window).unwrap().focus?;
    ["cancel", "share", "take", "remember", "sources"]
        .into_iter()
        .map(Key::from)
        .find(|key| runtime.find(window, key) == Some(focused))
        .or_else(|| {
            (0..2)
                .map(Key::Int)
                .find(|key| runtime.find(window, key) == Some(focused))
        })
}

/// A key pressed and let go; a press that closed the window has no release.
fn press(driver: &mut TestDriver<Dialogs>, window: WindowId, keysym: Keysym) {
    let text = (keysym == SPACE_KEY).then_some(' ');
    for pressed in [true, false] {
        if !driver.window_system_mut().windows().contains(&window) {
            return;
        }
        let event = KeyEvent {
            code: 0,
            keysym,
            text,
            pressed,
            repeat: false,
            mods: Mods::NONE,
        };
        send(driver, window, WindowEvent::Key(event, Timestamp(0)));
    }
}

fn screenshot_ask(n: u32) -> async_channel::Receiver<screenshot::Answer> {
    let (sender, answer) = async_channel::bounded(1);
    inbox().push(screenshot::Event::Ask {
        handle: handle(n),
        ask: Box::new(screenshot::Ask {
            app_id: "org.example.Draw".to_owned(),
            app_name: Some("Drawing".to_owned()),
            app_icon: None,
            parent_window: String::new(),
        }),
        reply: screenshot::Reply(sender),
    });
    answer
}

fn display() -> Source {
    Source::Display {
        output: 4,
        name: "DP-1".to_owned(),
        description: "Studio Display".to_owned(),
    }
}

fn share_ask(n: u32) -> async_channel::Receiver<screen_cast::Answer> {
    let (sender, answer) = async_channel::bounded(1);
    inbox().push(screen_cast::Event::Ask {
        handle: handle(n),
        ask: Box::new(screen_cast::Ask {
            app_id: "org.example.Meeting".to_owned(),
            app_name: Some("Meeting".to_owned()),
            app_icon: None,
            parent_window: String::new(),
            sources: vec![
                display(),
                Source::Window {
                    identifier: "a1".to_owned(),
                    title: "Notes".to_owned(),
                    app_id: "org.example.Notes".to_owned(),
                },
            ],
            offer_remember: true,
        }),
        reply: screen_cast::Reply(sender),
    });
    answer
}

/// Tab until `key` holds the focus, at most `steps` times.
fn tab_to(driver: &mut TestDriver<Dialogs>, window: WindowId, key: &'static str, steps: usize) {
    for _ in 0..steps {
        if focus(driver, window) == Some(Key::from(key)) {
            return;
        }
        press(driver, window, TAB);
    }
    assert_eq!(
        focus(driver, window),
        Some(Key::from(key)),
        "Tab reaches {key}"
    );
}

/// Asks the node `key` of `window` for a click, as a screen reader's DoAction does.
fn ask_click(
    driver: &mut TestDriver<Dialogs>,
    reader: &cupertino_ui_test::AssistiveTechnology,
    window: WindowId,
    key: Key,
) {
    let target = driver.runtime().find(window, &key).unwrap();
    driver.ask(reader, target, cupertino_kit::core::Action::Click, None);
    settle(driver, window);
}
