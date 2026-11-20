//! The launcher component's mapping of what the shell host tells it (`Program::event`) and the panel's placement.

use cupertino_kit::core::{Key, WindowKey};
use cupertino_kit_shell::bus::{Message as Published, Panel, Sender};
use cupertino_kit_shell::session::{Activation, Change};
use cupertino_kit_shell::{Event, Program};
use cupertino_launcher::component::{top_margin, Launcher, Shell};
use cupertino_launcher::kit::{Message, Running};
use cupertino_launcher::view::{SHADOW_TOP, TOP_FRACTION};

fn toggle(panel: Panel) -> Option<Message> {
    <Launcher as Program>::event(Event::Bus {
        from: Sender::Host,
        message: Published::TogglePanel {
            panel,
            output: None,
        },
    })
}

#[test]
fn the_panel_sits_a_fifth_down_the_output_less_its_shadow() {
    // 203 points down an output 982 points tall
    assert_eq!(top_margin(982.0), (203.0 - SHADOW_TOP).round() as i32);
    assert_eq!(
        top_margin(2160.0),
        (2160.0 * TOP_FRACTION - SHADOW_TOP).round() as i32
    );
    assert_eq!(top_margin(0.0), 0, "never above the output");
}

#[test]
fn toggle_publications_open_the_search_field_or_the_browser() {
    assert!(matches!(
        toggle(Panel::Launcher),
        Some(Message::Shell(Shell::Toggle(false, None)))
    ));
    assert!(matches!(
        toggle(Panel::Applications),
        Some(Message::Shell(Shell::Toggle(true, None)))
    ));
    assert!(
        toggle(Panel::ControlPanel).is_none(),
        "another panel's toggle"
    );
}

#[test]
fn the_session_lock_and_the_servers_answers_reach_the_model() {
    assert!(matches!(
        <Launcher as Program>::event(Event::Session(Change::Locked(true))),
        Some(Message::Locked)
    ));
    assert!(<Launcher as Program>::event(Event::Session(Change::Locked(false))).is_none());
    for (result, running) in [
        (Activation::Activated, Running::Raised),
        (Activation::Resident, Running::Resident),
        (Activation::NotRunning, Running::NotRunning),
    ] {
        let message = <Launcher as Program>::event(Event::Session(Change::AppActivation {
            serial: 7,
            result,
        }));
        assert!(
            matches!(message, Some(Message::Opened(7, r)) if r == running),
            "{result:?} is {running:?}"
        );
    }
}

#[test]
fn a_window_the_server_closed_is_told_to_the_component() {
    let key = WindowKey(Key::from("panel"));
    assert!(matches!(
        <Launcher as Program>::event(Event::Closed(key.clone())),
        Some(Message::Shell(Shell::Closed(k))) if k == key
    ));
}
