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

#[test]
fn the_screenshot_consent_opens_on_cancel_and_takes_no_key_before_it_is_armed() {
    let _turn = INBOX.lock().unwrap_or_else(|e| e.into_inner());
    let answer = screenshot_ask(50);
    let (mut driver, window) = start();
    assert_eq!(focus(&driver, window), Some(Key::from("cancel")));

    for keys in [
        &[RETURN][..],
        &[KP_ENTER],
        &[SPACE_KEY],
        &[TAB, TAB, SPACE_KEY, RETURN],
    ] {
        for &key in keys {
            press(&mut driver, window, key);
        }
        assert!(
            answer.try_recv().is_err(),
            "{keys:?} before the window is armed neither grants nor denies"
        );
        assert_eq!(driver.runtime().windows().len(), 1);
    }
    assert_eq!(
        focus(&driver, window),
        Some(Key::from("cancel")),
        "Take Screenshot is no Tab stop until armed, so Tab stays on Cancel"
    );

    armed(&mut driver, window);
    assert_eq!(focus(&driver, window), Some(Key::from("cancel")));
    press(&mut driver, window, RETURN);
    assert_eq!(
        answer.try_recv().unwrap(),
        screenshot::Answer::Cancelled,
        "once armed, Return presses the focused Cancel"
    );
    assert!(driver.runtime().windows().is_empty());
}

#[test]
fn the_screenshot_consent_takes_the_screenshot_on_return_once_armed_and_take_is_focused() {
    let _turn = INBOX.lock().unwrap_or_else(|e| e.into_inner());
    let answer = screenshot_ask(51);
    let (mut driver, window) = start();
    armed(&mut driver, window);
    tab_to(&mut driver, window, "take", 3);
    press(&mut driver, window, RETURN);
    assert_eq!(answer.try_recv().unwrap(), screenshot::Answer::Granted);

    let answer = screenshot_ask(52);
    let window = show(&mut driver);
    armed(&mut driver, window);
    tab_to(&mut driver, window, "take", 3);
    press(&mut driver, window, SPACE_KEY);
    assert_eq!(
        answer.try_recv().unwrap(),
        screenshot::Answer::Granted,
        "Space presses the focused button too"
    );
}

#[test]
fn the_screen_sharing_chooser_opens_on_cancel_and_takes_no_key_before_it_is_armed() {
    let _turn = INBOX.lock().unwrap_or_else(|e| e.into_inner());
    let answer = share_ask(53);
    let (mut driver, window) = start();
    assert_eq!(focus(&driver, window), Some(Key::from("cancel")));

    for keys in [
        &[RETURN][..],
        &[KP_ENTER],
        &[SPACE_KEY],
        &[TAB, TAB, SPACE_KEY, RETURN],
        &[TAB, Keysym(0xff54), RETURN],
    ] {
        for &key in keys {
            press(&mut driver, window, key);
        }
        assert!(
            answer.try_recv().is_err(),
            "{keys:?} before the window is armed picks, shares and denies nothing"
        );
        assert_eq!(driver.runtime().windows().len(), 1);
    }

    armed(&mut driver, window);
    tab_to(&mut driver, window, "cancel", 4);
    press(&mut driver, window, RETURN);
    assert_eq!(
        answer.try_recv().unwrap(),
        screen_cast::Answer::Cancelled,
        "once armed, Return presses the focused Cancel"
    );
    assert!(driver.runtime().windows().is_empty());
}

#[test]
fn the_screen_sharing_chooser_shares_on_return_once_armed_and_share_is_focused() {
    let _turn = INBOX.lock().unwrap_or_else(|e| e.into_inner());
    let answer = share_ask(54);
    let (mut driver, window) = start();
    armed(&mut driver, window);
    assert_eq!(
        focus(&driver, window),
        Some(Key::from("cancel")),
        "arming moves no focus"
    );
    click_at(&mut driver, window, Key::Int(0));
    tab_to(&mut driver, window, "share", 5);
    press(&mut driver, window, RETURN);
    assert_eq!(
        answer.try_recv().unwrap(),
        screen_cast::Answer::Chosen {
            source: display(),
            remember: false,
        }
    );
}

#[test]
fn escape_and_closing_deny_a_consent_window_armed_or_not() {
    let _turn = INBOX.lock().unwrap_or_else(|e| e.into_inner());
    let mut driver = driver();
    for (n, arm, close) in [
        (55, false, false),
        (56, true, false),
        (57, false, true),
        (58, true, true),
    ] {
        let deny = |driver: &mut TestDriver<Dialogs>, window: WindowId| {
            if arm {
                armed(driver, window);
            }
            if close {
                send(driver, window, WindowEvent::CloseRequested);
            } else {
                press(driver, window, ESCAPE);
            }
        };
        let answer = screenshot_ask(n);
        let window = show(&mut driver);
        deny(&mut driver, window);
        assert_eq!(
            answer.try_recv().unwrap(),
            screenshot::Answer::Cancelled,
            "screenshot, armed {arm}, closed {close}"
        );
        assert!(driver.runtime().windows().is_empty());

        let answer = share_ask(n + 10);
        let window = show(&mut driver);
        deny(&mut driver, window);
        assert_eq!(
            answer.try_recv().unwrap(),
            screen_cast::Answer::Cancelled,
            "screen sharing, armed {arm}, closed {close}"
        );
        assert!(driver.runtime().windows().is_empty());
    }
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

#[test]
fn assistive_technology_cannot_consent_before_a_consent_window_is_armed() {
    let _turn = INBOX.lock().unwrap_or_else(|e| e.into_inner());
    let reader = cupertino_ui_test::AssistiveTechnology::new();
    let mut driver = driver_with(Some(reader.adapter()));

    let answer = screenshot_ask(70);
    let window = show(&mut driver);
    ask_click(&mut driver, &reader, window, Key::from("take"));
    assert!(
        answer.try_recv().is_err(),
        "a click asked for on Take Screenshot before the window is armed takes nothing"
    );
    armed(&mut driver, window);
    ask_click(&mut driver, &reader, window, Key::from("take"));
    assert_eq!(
        answer.try_recv().unwrap(),
        screenshot::Answer::Granted,
        "once armed, the reader's click takes the screenshot"
    );

    let answer = share_ask(71);
    let window = show(&mut driver);
    for key in [Key::Int(0), Key::from("remember"), Key::from("share")] {
        ask_click(&mut driver, &reader, window, key);
    }
    assert!(
        answer.try_recv().is_err(),
        "clicks asked for on a row, the check box and Share before the window is armed pick and share nothing"
    );
    armed(&mut driver, window);
    ask_click(&mut driver, &reader, window, Key::from("share"));
    assert!(
        answer.try_recv().is_err(),
        "no row was picked before arming, so Share has nothing to share"
    );
    press(&mut driver, window, ESCAPE);
    assert_eq!(answer.try_recv().unwrap(), screen_cast::Answer::Cancelled);

    // denying needs no arming: the reader's click on Cancel answers at once
    let answer = screenshot_ask(72);
    let window = show(&mut driver);
    ask_click(&mut driver, &reader, window, Key::from("cancel"));
    assert_eq!(answer.try_recv().unwrap(), screenshot::Answer::Cancelled);
    assert!(driver.runtime().windows().is_empty());
}

#[test]
fn each_prompt_is_a_glass_panel_whose_title_bar_is_only_its_buttons_over_its_content() {
    let _turn = INBOX.lock().unwrap_or_else(|e| e.into_inner());
    let _screenshot = screenshot_ask(60);
    let _share = share_ask(61);
    let (driver, window) = start();
    let windows = Dialogs::view(driver.runtime().model());
    assert_eq!(
        windows.len(),
        2,
        "the screenshot consent and the sharing chooser"
    );
    for window in &windows {
        assert_eq!(window.spec.decorations, DecorationMode::Unified);
        assert_eq!(window.background, PANEL_WINDOW_BACKGROUND);
    }
    let prompt = rect_of(&driver, window, &Key::from("prompt"));
    assert!(
        prompt.origin.y >= SIDEBAR_TOP,
        "the prompt lies under the row of the window's buttons: {prompt:?}"
    );
}
